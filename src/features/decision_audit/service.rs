use super::{
    client::{DecisionClient, Failure, POLICY_VERSION, QUESTIONS},
    repo::{self, DecisionJob},
};
use crate::{
    config::Config,
    config_file::LocalCascadeMode,
    features::{
        jobs::policy::EXTERNAL_ANALYSIS_POLL,
        new_user_audit::{deterministic, local_cascade::hash_bytes, structure},
        user_profiles::avatar::cache_profile_avatar,
    },
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{FromRow, PgPool};
use teloxide::Bot;
use teloxide_antispam::preprocess::{PREPROCESSING_VERSION, prepare_text};

#[derive(FromRow)]
struct Source {
    user_id: i64,
    text: String,
    edited_at: Option<DateTime<Utc>>,
    message_count: i64,
    first_text_message: bool,
    age_seconds: i64,
    label_id: i64,
    normal_veto: bool,
    trusted: bool,
    avatar_file_id: Option<String>,
    avatar_unique_id: Option<String>,
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
        select m.user_id,coalesce(m.text,'') as text,m.edited_at,u.message_count,
          not exists (
            select 1 from telegram_messages earlier
            where earlier.chat_id=m.chat_id and earlier.user_id=m.user_id
              and earlier.message_id<m.message_id and earlier.source_channel_id is null
              and earlier.sender_chat_id is null and nullif(trim(earlier.text),'') is not null
          ) as first_text_message,
          extract(epoch from (now()-u.first_seen_at))::bigint as age_seconds,
          coalesce(l.id,0) as label_id,coalesce(l.label='not_spam',false) as normal_veto,
          (coalesce(p.is_bot,false) or coalesce(s.is_admin,false)) as trusted,
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
    let local_candidate = campaign.is_some();
    let reason = if source.normal_veto {
        "manual_not_spam"
    } else if source.trusted {
        "trusted"
    } else if !eligible {
        "not_eligible"
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
        "normalization_version":PREPROCESSING_VERSION,"rule":campaign.as_ref().map(|c|&c.rule),"rule_version":deterministic::RULE_VERSION});
    sqlx::query("insert into moderation_text_observations (instance_id,chat_id,message_id,user_id,source_hash,eligible,reason,local_candidate,features) values ($1,$2,$3,$4,$5,$6,$7,$8,$9) on conflict (instance_id,chat_id,message_id) do update set source_hash=excluded.source_hash,eligible=excluded.eligible,reason=excluded.reason,local_candidate=excluded.local_candidate,features=excluded.features,observed_at=now()")
        .bind(&config.community.instance.id).bind(chat_id).bind(message_id).bind(source.user_id).bind(&source_hash)
        .bind(eligible).bind(reason).bind(local_candidate).bind(&observation).execute(pool).await?;
    // Первый авторский текст — контрольная выборка: service/media updates
    // не расходуют её слот. Следующие тексты — только с конкретным сигналом.
    if !repo::enabled_for_chat(config, chat_id)
        || source.text.trim().is_empty()
        || (!source.first_text_message && !local_candidate && features.obfuscation_score < 40)
    {
        return Ok(());
    }
    let settings = &config.community.moderation.decision;
    let input = json!({"author_message":crate::features::new_user_analysis::bounded_audit_text_with_limit(Some(&source.text.chars().take(structure::MAX_INPUT_CHARS).collect::<String>()), settings.max_input_chars),
        "chat_policy":settings.chat_policy,"structural_features":features,
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
    hash_bytes(
        &serde_json::to_vec(&config.community.moderation.decision)
            .expect("validated serializable decision configuration"),
    )
}

pub fn spawn_worker(bot: Bot, pool: PgPool, config: Config) -> anyhow::Result<()> {
    if !config.community.moderation.enabled || !config.community.moderation.decision.enabled {
        return Ok(());
    }
    let client = DecisionClient::new(&config.community.moderation.decision)?;
    tokio::spawn(async move {
        loop {
            let seconds = match process_next(&bot, &pool, &config, &client).await {
                Ok(true) => continue,
                Ok(false) => EXTERNAL_ANALYSIS_POLL.idle_seconds(),
                Err(_) => {
                    tracing::warn!("decision shadow database operation failed");
                    EXTERNAL_ANALYSIS_POLL.error_seconds()
                }
            };
            tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
        }
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
