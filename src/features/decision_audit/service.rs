use super::{
    client::{DecisionClient, Failure, POLICY_VERSION, QUESTIONS},
    repo::{self, DecisionJob},
};
use crate::{
    config::Config,
    config_file::LocalCascadeMode,
    features::{
        jobs::{
            policy::EXTERNAL_ANALYSIS_POLL,
            worker::{PollOutcome, WorkerId, WorkerSpec, WorkerSupervisor, run_poll_worker},
        },
        new_user_analysis::profile_context::{
            PROFILE_CONTEXT_VERSION, looks_like_feminine_first_name, telegram_id_model_for_date,
        },
        new_user_audit::{deterministic, local_cascade::hash_bytes, structure},
        user_profiles::avatar::cache_profile_avatar,
    },
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{FromRow, PgPool};
use teloxide::Bot;
use teloxide_antispam::{
    preprocess::{PREPROCESSING_VERSION, prepare_text},
    signals::telegram_id_spam_probability,
};

#[derive(FromRow)]
struct Source {
    user_id: i64,
    text: String,
    edited_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    message_count: i64,
    text_ordinal: i64,
    entities: serde_json::Value,
    age_seconds: i64,
    label_id: i64,
    normal_veto: bool,
    trusted: bool,
    first_name: Option<String>,
    avatar_file_id: Option<String>,
    avatar_unique_id: Option<String>,
}

/// Короткие завуалированные приглашения не доказывают спам.
/// Это лишь повод запросить Decision с доступными сигналами профиля.
fn short_adult_contact_bait(text: &str) -> bool {
    // Общий versioned Unicode view. Не обрезаем длинный текст до bait-префикса.
    let bounded: String = text.chars().take(structure::MAX_INPUT_CHARS + 1).collect();
    if bounded.chars().count() > structure::MAX_INPUT_CHARS {
        return false;
    }
    let prepared = prepare_text(&bounded);
    let normalized = prepared
        .canonical
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.chars().count() > 80 {
        return false;
    }
    matches!(
        normalized.as_str(),
        "приму"
            | "приму к себе"
            | "приму мужчину"
            | "есть время сегодня"
            | "свободна сегодня"
            | "жду в гости"
            | "встречусь"
            | "встречусь свободна"
            | "свободна для встреч"
            | "свободна для встреч есть выезд"
            | "свободна сейчас встречусь пиши"
    ) || normalized == "кто в гости"
        || normalized.ends_with(" кто в гости")
}

fn short_bait_for_decision(
    text: &str,
    feminine_name_pattern: bool,
    avatar_observable: bool,
) -> bool {
    (feminine_name_pattern || avatar_observable) && short_adult_contact_bait(text)
}

/// SQL-only дешёвая стадия после ingest; не ждёт profile API, LLM или embedding.
/// Родитель, биография и текст чужих сообщений в author_message не попадают.
pub async fn observe_message(
    pool: &PgPool,
    config: &Config,
    chat_id: i64,
    message_id: i32,
) -> anyhow::Result<()> {
    if !config.community.moderation.enabled
        || !config.chat_allows(chat_id, |chat| chat.ingest && chat.moderation)
    {
        return Ok(());
    }
    let local_enabled =
        config.community.moderation.local_cascade.mode != LocalCascadeMode::Disabled;
    if !local_enabled && !repo::enabled_for_chat(config, chat_id) {
        return Ok(());
    }
    let source: Option<Source> = sqlx::query_as(r#"
        select m.user_id,coalesce(m.text,'') as text,m.edited_at,m.created_at,u.message_count,
          (select count(*)+1 from (select 1 from telegram_messages earlier
            where earlier.chat_id=m.chat_id and earlier.user_id=m.user_id
              and earlier.message_id<m.message_id and earlier.source_channel_id is null
              and earlier.sender_chat_id is null and nullif(trim(earlier.text),'') is not null limit 10
          ) prior_texts) as text_ordinal,
          coalesce(m.raw_json->'entities',m.raw_json->'caption_entities','[]'::jsonb) as entities,
          extract(epoch from (now()-u.first_seen_at))::bigint as age_seconds,
          coalesce(l.id,0) as label_id,coalesce(l.label='not_spam',false) as normal_veto,
          (coalesce(p.is_bot,false) or coalesce(s.is_admin,false)) as trusted,
          p.first_name,
          p.profile_photo_file_id as avatar_file_id,p.profile_photo_file_unique_id as avatar_unique_id
        from telegram_messages m join telegram_chat_users u on u.chat_id=m.chat_id and u.telegram_user_id=m.user_id
        left join telegram_user_profiles p on p.telegram_user_id=m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id=m.chat_id and s.telegram_user_id=m.user_id
        left join lateral (select id,label from spam_label_events where chat_id=m.chat_id and telegram_user_id=m.user_id
          order by created_at desc,id desc limit 1) l on true
        where m.chat_id=$1 and m.message_id=$2 and m.source_channel_id is null and m.sender_chat_id is null and u.first_seen_at is not null"#)
        .bind(chat_id).bind(message_id).fetch_optional(pool).await?;
    let Some(source) = source else {
        return Ok(());
    };
    if crate::telegram::identity::is_channel_sender_proxy(source.user_id) {
        return Ok(());
    }
    let eligible = deterministic::text_stream_eligible(source.message_count, source.age_seconds)
        && !source.normal_veto
        && !source.trusted;
    let campaign = eligible
        .then(|| {
            deterministic::classify(
                Some(&source.text),
                source.message_count,
                Some(source.age_seconds),
                false,
            )
        })
        .flatten();
    let contacts = if eligible {
        super::contacts::extract(
            pool,
            chat_id,
            source.user_id,
            &source.text,
            &source.entities,
        )
        .await?
    } else {
        Default::default()
    };
    // Местный hint не является диагнозом или санкцией: он только возвращает
    // в очередь Decision короткий текст ПОСЛЕ первых initial_texts сообщений.
    // ФИО и username не отправляем внешней модели; передаём только boolean.
    let feminine_name_pattern = looks_like_feminine_first_name(source.first_name.as_deref());
    let avatar_observable = config.community.moderation.decision.avatar_enabled
        && source.avatar_file_id.is_some()
        && source.avatar_unique_id.is_some();
    let short_bait_candidate =
        short_bait_for_decision(&source.text, feminine_name_pattern, avatar_observable);
    let local_candidate = campaign.is_some() || short_bait_candidate;
    let reason = if source.normal_veto {
        "manual_not_spam"
    } else if source.trusted {
        "trusted"
    } else if !eligible {
        "not_eligible"
    } else if contacts.external_candidate() {
        "external_contact"
    } else if local_candidate {
        "local_candidate"
    } else {
        "no_local_signal"
    };
    // Установившиеся участники не создают записи/внешнюю нагрузку на каждом сообщении.
    if !eligible {
        tracing::debug!(chat_id, message_id, reason, "cheap moderation skipped");
        return Ok(());
    }
    let features = structure::extract(&source.text);
    let source_hash = hash_bytes(source.text.as_bytes());
    let observation = json!({"structure":features,"normalization":prepare_text(&source.text.chars().take(structure::MAX_INPUT_CHARS).collect::<String>()).flags,
        "external_contacts":contacts,"normalization_version":PREPROCESSING_VERSION,"rule":campaign.as_ref().map(|c|&c.rule),"rule_version":deterministic::RULE_VERSION,
        "short_bait_candidate":short_bait_candidate});
    sqlx::query("insert into moderation_text_observations (instance_id,chat_id,message_id,user_id,source_hash,eligible,reason,local_candidate,features) values ($1,$2,$3,$4,$5,$6,$7,$8,$9) on conflict (instance_id,chat_id,message_id) do update set source_hash=excluded.source_hash,eligible=excluded.eligible,reason=excluded.reason,local_candidate=excluded.local_candidate,features=excluded.features,observed_at=now()")
        .bind(&config.community.instance.id).bind(chat_id).bind(message_id).bind(source.user_id).bind(&source_hash)
        .bind(eligible).bind(reason).bind(local_candidate).bind(&observation).execute(pool).await?;
    // Первые initial_texts авторских текстов проверяются без словарного сигнала;
    // service/media updates не расходуют слоты. Дальше нужен дешёвый сигнал.
    if !repo::enabled_for_chat(config, chat_id)
        || source.text.trim().is_empty()
        || (source.text_ordinal > config.community.moderation.decision.initial_texts
            && !local_candidate
            && !contacts.external_candidate()
            && features.obfuscation_score < 40)
    {
        return Ok(());
    }
    let settings = &config.community.moderation.decision;
    let post_topic = super::topics::load_post_topic(pool, config, chat_id, message_id).await?;
    let id_model = config
        .moderation_risk_profile()
        .and_then(|profile| telegram_id_model_for_date(profile, source.created_at.date_naive()));
    let id_probability = id_model
        .as_ref()
        .map(|model| telegram_id_spam_probability(source.user_id, model));
    let input = json!({"author_message":crate::features::new_user_analysis::bounded_audit_text_with_limit(Some(&source.text.chars().take(structure::MAX_INPUT_CHARS).collect::<String>()), settings.max_input_chars),
        "chat_policy":settings.chat_policy,"structural_features":features,"external_contacts":contacts,
        "profile_signals":{"version":PROFILE_CONTEXT_VERSION,
            "feminine_first_name_pattern":feminine_name_pattern,"avatar_observable":avatar_observable,
            "telegram_id_spam_probability":id_probability,"telegram_id_model_version":id_model.as_ref().map(|model|&model.version),
            "evaluated_on_utc":source.created_at.date_naive(),"observed_message_count":source.message_count},
        "text_signals":{"short_invitation":short_adult_contact_bait(&source.text)},
        "topic_context":{"chat_topic":settings.chat_topic,"post_topic":post_topic.as_ref().map(|(text,_)|text)},
        "_post_context":post_topic.as_ref().map(|(_,snapshot)|snapshot),
        "score_captcha":settings.mode==crate::config_file::DecisionMode::ScoreCaptcha,
        "configuration_hash":configuration_hash(config)});
    let (avatar_file_id, avatar_unique_id) = match (
        settings.avatar_enabled,
        source.avatar_file_id,
        source.avatar_unique_id,
    ) {
        (true, Some(file), Some(unique)) => (Some(file), Some(unique)),
        _ => (None, None),
    };
    let question_version = hash_bytes(QUESTIONS.as_bytes());
    let snapshot_hash = hash_bytes(&serde_json::to_vec(
        &json!({"source_hash":source_hash,"edited_at":source.edited_at,
        "input":input,"label_id":source.label_id,"avatar_unique_id":avatar_unique_id,
        "model":settings.model,"questions":question_version,"normalization":PREPROCESSING_VERSION,"policy":POLICY_VERSION}),
    )?);
    let queued = repo::enqueue(
        pool,
        config,
        &DecisionJob {
            id: 0,
            instance_id: config.community.instance.id.clone(),
            chat_id,
            message_id,
            user_id: source.user_id,
            snapshot_hash,
            source_hash,
            source_edited_at: source.edited_at,
            label_event_id: source.label_id,
            question_version,
            normalization_version: PREPROCESSING_VERSION.into(),
            policy_version: POLICY_VERSION.into(),
            model: settings.model.clone(),
            status: repo::DecisionJobStatus::Pending,
            input_json: input,
            avatar_file_id,
            avatar_unique_id,
            attempts: 0,
            fence: 0,
        },
    )
    .await?;
    sqlx::query("update moderation_text_observations set features=jsonb_set(features,'{decision_enqueue}',$4) where instance_id=$1 and chat_id=$2 and message_id=$3")
        .bind(&config.community.instance.id).bind(chat_id).bind(message_id).bind(serde_json::to_value(queued)?).execute(pool).await?;
    tracing::info!(
        chat_id,
        message_id,
        local_candidate,
        decision_enqueue = ?queued,
        "cheap moderation observed"
    );
    Ok(())
}

/// Уже существующий bounded profile worker может добавить доступный аватар к
/// тому же тексту. Обновление профиля не превращается в внешний запрос на каждый пост.
pub async fn observe_latest_author_message(
    pool: &PgPool,
    config: &Config,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<()> {
    if !repo::enabled_for_chat(config, chat_id)
        || !config.community.moderation.decision.avatar_enabled
    {
        return Ok(());
    }
    let message: Option<i32> = sqlx::query_scalar("select message_id from telegram_messages where chat_id=$1 and user_id=$2 and source_channel_id is null order by created_at desc,message_id desc limit 1")
        .bind(chat_id).bind(user_id).fetch_optional(pool).await?;
    if let Some(message_id) = message {
        observe_message(pool, config, chat_id, message_id).await?;
    }
    Ok(())
}

pub fn configuration_hash(config: &Config) -> String {
    let moderation = &config.community.moderation;
    hash_bytes(&serde_json::to_vec(&json!({"decision":moderation.decision,
        "profile_context_version":PROFILE_CONTEXT_VERSION,
        "topic_sources":{"primary_chat":config.discussion_chat_id,"primary_channel":config.source_channel_id,
            "routes":config.community.first_comment.routes.iter().map(|r|(r.source_channel_id,&r.discussion_chat)).collect::<Vec<_>>()},
        "id_risk_profile":config.moderation_risk_profile().map(|p|json!({"version":p.version,"telegram_id":p.telegram_id.as_ref().map(|m|json!({"floor":m.floor,"ceil":m.ceil,"k":m.k,"midpoint_billion":m.midpoint_billion,"version":m.version})),"telegram_id_time":p.telegram_id_time})),
        "captcha_enabled":moderation.captcha_enabled,"captcha_dry_run":moderation.captcha_dry_run,
        "captcha_threshold":moderation.captcha_threshold,"captcha_ttl_sec":moderation.captcha_ttl_sec}))
        .expect("validated serializable decision configuration"))
}

pub fn spawn_worker(
    supervisor: &mut WorkerSupervisor,
    bot: Bot,
    pool: PgPool,
    config: std::sync::Arc<Config>,
) -> anyhow::Result<()> {
    if !config.community.moderation.enabled || !config.community.moderation.decision.enabled {
        return Ok(());
    }
    let client = DecisionClient::new(&config.community.moderation.decision)?;
    supervisor.spawn(WorkerId::DecisionAudit, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::DecisionAudit, EXTERNAL_ANALYSIS_POLL),
            shutdown,
            || async {
                let action_failed = match super::actions::process_next(&bot, &pool, &config).await {
                    Ok(true) => return PollOutcome::Processed,
                    Ok(false) => false,
                    Err(_) => true,
                };
                match process_next(&bot, &pool, &config, &client).await {
                    Ok(true) => PollOutcome::Processed,
                    Ok(false) if action_failed => PollOutcome::Failed("action_operation_failed"),
                    Ok(false) => PollOutcome::Idle,
                    Err(_) => PollOutcome::Failed("decision_operation_failed"),
                }
            },
        )
        .await;
    });
    Ok(())
}

pub async fn process_next(
    bot: &Bot,
    pool: &PgPool,
    config: &Config,
    client: &DecisionClient,
) -> anyhow::Result<bool> {
    let Some(job) = repo::claim(pool, config).await? else {
        return Ok(false);
    };
    let avatar = if job.avatar_file_id.is_some() {
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            cache_profile_avatar(
                bot,
                &config.static_files_dir,
                job.user_id,
                job.avatar_file_id.as_deref(),
                job.avatar_unique_id.as_deref(),
            ),
        )
        .await
        {
            Ok(Ok(Some(image))) => image.base64().await.ok(),
            _ => None,
        }
    } else {
        None
    };
    if job.avatar_file_id.is_some() && avatar.is_none() {
        repo::fail(pool, &job, Failure::ImageUnavailable).await?;
        return Ok(true);
    }
    match client.assess(&job.input_json, avatar.as_deref()).await {
        Ok((assessment, latency)) => {
            let current = repo::complete(pool, &job, &assessment, latency).await?;
            tracing::info!(job_id=job.id,current,latency_ms=latency,verdict=?assessment.verdict(),"decision shadow completed");
        }
        Err(error) => {
            repo::fail(pool, &job, error).await?;
            tracing::warn!(
                job_id = job.id,
                error_kind = error.kind(),
                "decision shadow unavailable; local moderation unaffected"
            );
        }
    }
    Ok(true)
}

#[cfg(test)]
mod short_bait_tests {
    use super::{short_adult_contact_bait, short_bait_for_decision};

    #[test]
    fn short_invitations_reach_decision_candidate_gate() {
        for sample in [
            "Приму",
            "Приму к себе!!!",
            "Пpиму\u{200b} к себе!!!",
            "Есть время сегодня?",
            "Котики, мне скучно, кто в гости?",
            "\u{200b}Котики, мне скучно, кто в гости? 😘",
            "Есть время сегодня 💖",
            "Приму мужчину ❤️",
            "Встречусь,свободна ❤️🥰",
            "Свободна для встреч\nЕсть выезд ❤️",
        ] {
            assert!(short_adult_contact_bait(sample), "{sample}");
        }
    }

    #[test]
    fn profile_context_is_required_for_additional_decision_enqueue() {
        assert!(!short_bait_for_decision("Приму", false, false));
        assert!(short_bait_for_decision("Приму", true, false));
        assert!(short_bait_for_decision("Приму", false, true));
        assert!(!short_bait_for_decision(
            "Приму PR после тестов",
            true,
            true
        ));
    }

    #[test]
    fn unrelated_mentions_do_not_match_short_invitation() {
        for sample in [
            "Приём ведётся по записи",
            "Приму PR после тестов",
            "Есть время сегодня посмотреть логи?",
            "Встречусь с командой после работы",
            "Свободна для встреч с партнёрами по проекту",
            "Кто в гости к нам приехал на фестиваль? Тут будет длинное обсуждение, а не краткое приглашение.",
        ] {
            assert!(!short_adult_contact_bait(sample), "{sample}");
        }
        assert!(!short_adult_contact_bait(&format!(
            "Приму{}",
            " ".repeat(20_000)
        )));
    }
}
