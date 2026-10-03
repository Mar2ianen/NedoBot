use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};

use crate::{
    config::Config,
    features::new_user_audit::{
        prompt::PROMPT_VERSION,
        repo::{
            NewUserAuditJobParams, enqueue_new_user_audit_job_in_transaction,
            record_new_user_audit_snapshot_in_transaction,
        },
    },
};
use teloxide_antispam::signals::{
    NewUserAnalysisConfig, NewUserFeatures, RiskAnalysis, SpamClass, TextTexture, analyze_risk,
    char_count_i32, has_profile_photo, id_bucket, looks_like_feminine_first_name,
    max_pairwise_message_similarity, message_style, message_style_persona,
    only_channel_post_comments, only_replies_or_comments, personal_channel_has_external_link,
    personal_channel_has_invite_link, telegram_id_risk_coefficient, telegram_id_spam_probability,
    username_stats,
};

fn analysis_config_from_runtime(config: &Config) -> NewUserAnalysisConfig {
    let Some(profile) = config.moderation_risk_profile() else {
        return NewUserAnalysisConfig::default();
    };
    let profile_name = config.community.moderation.risk_profile.trim();
    NewUserAnalysisConfig {
        old_user_message_threshold: profile.old_user_message_threshold,
        review_threshold: profile.review_threshold,
        risk_profile: profile_name.to_string(),
        risk_profile_version: if profile.version.trim().is_empty() {
            profile_name.to_string()
        } else {
            profile.version.clone()
        },
        telegram_id_model_version: profile
            .telegram_id
            .as_ref()
            .map(|model| model.version.clone()),
        telegram_id_model: profile.telegram_id.clone(),
        ..NewUserAnalysisConfig::default()
    }
}




















/// Сохраняет baseline, ревизию снимка и unified-audit job одной транзакцией.
/// Между baseline и job нет окна, в котором materializer может увидеть
/// несогласованное состояние.
pub(crate) async fn enqueue_new_user_audit_for_profile_refresh(
    pool: &PgPool,
    runtime_config: &Config,
    chat_id: i64,
    telegram_user_id: i64,
) -> anyhow::Result<()> {
    let Some(features) = load_features(
        pool,
        chat_id,
        telegram_user_id,
        &runtime_config.community.instance.id,
    )
    .await?
    else {
        return Ok(());
    };
    let analysis_config = analysis_config_from_runtime(runtime_config);
    let is_old_active_user = features.message_count >= analysis_config.old_user_message_threshold;
    let risk = analyze_risk(&features, &analysis_config, is_old_active_user);
    let input_json = project_unified_user_audit_snapshot(&features, &risk, &analysis_config);
    let material_revision =
        project_unified_user_audit_material_revision(&features, &analysis_config);
    let snapshot_hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&material_revision)?)
    );

    let params = NewUserAuditJobParams {
        chat_id,
        telegram_user_id,
        snapshot_hash: &snapshot_hash,
        prompt_version: PROMPT_VERSION,
        input_json: &input_json,
        avatar_file_id: features.profile_photo_file_id.as_deref(),
        avatar_file_unique_id: features.profile_photo_file_unique_id.as_deref(),
        review_threshold: analysis_config.review_threshold,
    };
    let mut tx = pool.begin().await?;
    // Authoritative transactions consistently lock job, then audit, then review.
    enqueue_new_user_audit_job_in_transaction(&mut tx, params).await?;
    save_audit_in_transaction(&mut tx, &features, &risk, &analysis_config).await?;
    record_new_user_audit_snapshot_in_transaction(&mut tx, params).await?;
    tx.commit().await?;

    tracing::info!(
        chat_id,
        telegram_user_id,
        risk_score = risk.score,
        risk_level = %risk.level,
        "unified new user audit baseline and job saved"
    );
    Ok(())
}

const UNIFIED_AUDIT_TEXT_LIMIT: usize = 280;
const UNIFIED_AUDIT_RECENT_MESSAGES_LIMIT: usize = 5;

fn project_unified_user_audit_material_revision(
    features: &NewUserFeatures,
    config: &NewUserAnalysisConfig,
) -> Value {
    // Изменение текстовых сообщений должно создавать новую генерацию, а не
    // только перезаписывать input_json у уже завершённой оценки.
    json!({
        "schema_version": 3,
        "risk_profile_version": config.risk_profile_version,
        "telegram_id_model_version": config.telegram_id_model_version,
        "review_threshold": config.review_threshold,
        "subject": {
            "chat_id": features.chat_id,
            "telegram_user_id": features.telegram_user_id,
        },
        "profile": {
            "username": bounded_audit_text(features.username.as_deref()),
            "display_name": bounded_audit_text(features.display_name.as_deref()),
            "bio_preview": bounded_audit_text(features.bio.as_deref()),
            "profile_photo_unique_id": features.profile_photo_file_unique_id,
            "shared_spammer_identity": features.shared_spammer_identity,
        },
        "first_message": bounded_audit_text(features.first_message_text.as_deref()),
        "first_message_reply_context": bounded_audit_text(features.first_message_reply_context.as_deref()),
        "recent_messages": features.recent_message_texts.iter()
            .take(UNIFIED_AUDIT_RECENT_MESSAGES_LIMIT)
            .map(|text| bounded_audit_text(Some(text)))
            .collect::<Vec<_>>(),
        "personal_channel": {
            "title_preview": bounded_audit_text(features.personal_channel_title.as_deref()),
            "username": bounded_audit_text(features.personal_channel_username.as_deref()),
            "recent_content_preview": bounded_audit_text(features.personal_channel_last_text.as_deref()),
            "message_count": features.personal_channel_message_count,
            "has_adult_links": features.personal_channel_has_adult_links,
            "has_invite_link": personal_channel_has_invite_link(features),
            "has_external_link": personal_channel_has_external_link(features),
        },
        "membership": {
            "status": bounded_audit_text(features.member_status.as_deref()),
            "is_admin": features.member_is_admin,
        },
    })
}

fn project_unified_user_audit_snapshot(
    features: &NewUserFeatures,
    risk: &RiskAnalysis,
    config: &NewUserAnalysisConfig,
) -> Value {
    json!({
        "schema_version": 1,
        "subject": {
            "chat_id": features.chat_id,
            "telegram_user_id": features.telegram_user_id,
        },
        "profile": {
            "username": bounded_audit_text(features.username.as_deref()),
            "display_name": bounded_audit_text(features.display_name.as_deref()),
            "is_bot": features.is_bot,
            "is_premium": features.is_premium,
            "language_code": bounded_audit_text(features.language_code.as_deref()),
            "bio_preview": bounded_audit_text(features.bio.as_deref()),
            "has_profile_photo": has_profile_photo(features),
            "avatar_image_available": false,
            "profile_photo_reuse_count": features.profile_photo_reuse_count,
        },
        "activity": {
            "first_seen_at": features.first_seen_at.map(|value| value.to_rfc3339()),
            "last_seen_at": features.last_seen_at.map(|value| value.to_rfc3339()),
            "account_seen_age_sec": features.account_seen_age_sec,
            "chat_age_sec": features.chat_age_sec,
            "message_count": features.message_count,
            "reply_count": features.reply_count,
            "link_count": features.link_count,
            "media_count": features.media_count,
            "voice_count": features.voice_count,
            "message_count_24h": features.message_count_24h,
            "link_count_24h": features.link_count_24h,
            "burst_messages_per_min": features.burst_messages_per_min,
            "only_replies_or_comments": only_replies_or_comments(features),
            "only_channel_post_comments": only_channel_post_comments(features),
        },
        "text": {
            "first_message_preview": bounded_audit_text(features.first_message_text.as_deref()),
            "first_message_reply_context_preview": bounded_audit_text(features.first_message_reply_context.as_deref()),
            "last_message_preview": bounded_audit_text(features.last_message_text.as_deref()),
            "recent_message_previews": features.recent_message_texts.iter()
                .take(UNIFIED_AUDIT_RECENT_MESSAGES_LIMIT)
                .map(|text| bounded_audit_text(Some(text)))
                .collect::<Vec<_>>(),
            "repetitive_pattern": features.text_texture.repetitive_pattern,
            "max_pairwise_similarity": features.text_texture.max_pairwise_similarity,
        },
        "personal_channel": {
            "present": features.personal_channel_chat_id.is_some(),
            "title_preview": bounded_audit_text(features.personal_channel_title.as_deref()),
            "username": bounded_audit_text(features.personal_channel_username.as_deref()),
            "recent_content_preview": bounded_audit_text(features.personal_channel_last_text.as_deref()),
            "message_count": features.personal_channel_message_count,
            "has_adult_links": features.personal_channel_has_adult_links,
            "has_invite_link": personal_channel_has_invite_link(features),
            "has_external_link": personal_channel_has_external_link(features),
        },
        "membership": {
            "status": bounded_audit_text(features.member_status.as_deref()),
            "is_present": features.member_is_present,
            "is_admin": features.member_is_admin,
            "join_event_seen": features.join_event_seen,
            "via_chat_folder_invite_link": features.via_chat_folder_invite_link,
        },
        "risk": {
            "risk_profile": config.risk_profile,
            "risk_profile_version": config.risk_profile_version,
            "telegram_id_model_version": config.telegram_id_model_version,
            "review_threshold": config.review_threshold,
            "telegram_id_spam_probability": config
                .telegram_id_model
                .as_ref()
                .map(|model| telegram_id_spam_probability(features.telegram_user_id, model)),
            "telegram_id_risk_coefficient": config.telegram_id_model.as_ref().map(|model| {
                telegram_id_risk_coefficient(telegram_id_spam_probability(
                    features.telegram_user_id,
                    model,
                ))
            }),
            "shared_spammer_identity": features.shared_spammer_identity,
            "score": risk.score,
            "level": risk.level,
            "primary_class": risk.primary_class,
            "class_scores": risk.class_scores,
            "labels": risk.labels,
            "reasons": risk.reasons,
            "signals": risk.signals,
        },
    })
}

fn bounded_audit_text(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }

    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let redacted = redact_urls(&normalized);
    let mut preview = redacted
        .chars()
        .take(UNIFIED_AUDIT_TEXT_LIMIT)
        .collect::<String>();
    if redacted.chars().count() > UNIFIED_AUDIT_TEXT_LIMIT {
        preview.push('…');
    }
    Some(preview)
}

fn redact_urls(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            let normalized = token
                .trim_start_matches(['(', '[', '{', '<', '"', '\''])
                .to_ascii_lowercase();
            if ["http://", "https://", "t.me/", "telegram.me/"]
                .iter()
                .any(|prefix| normalized.contains(prefix))
            {
                "[link]"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn load_features(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
    instance_id: &str,
) -> anyhow::Result<Option<NewUserFeatures>> {
    let row = sqlx::query(
        r#"
        with user_messages as (
            select *
            from telegram_messages
            where chat_id = $1
              and user_id = $2
              and source_channel_id is null
        ), first_msg as (
            select message_id, text, reply_to_message_id
            from user_messages
            where nullif(btrim(text), '') is not null
            order by created_at asc
            limit 1
        ), last_msg as (
            select message_id, text
            from user_messages
            order by created_at desc
            limit 1
        ), normalized_messages as (
            select
                message_id,
                text,
                created_at,
                nullif(regexp_replace(lower(coalesce(text, '')), '\\s+', ' ', 'g'), '') as normalized_text
            from user_messages
        ), normalized_counts as (
            select normalized_text, count(*)::bigint as reuse_count
            from normalized_messages
            where normalized_text is not null
            group by normalized_text
        ), msg_stats as (
            select
                count(*)::bigint as message_count,
                count(*) filter (where reply_to_message_id is not null)::bigint as reply_count,
                count(*) filter (where has_links)::bigint as link_count,
                count(*) filter (where has_photo or has_video or has_document or has_audio or has_voice or has_sticker or has_animation)::bigint as media_count,
                count(*) filter (where has_voice)::bigint as voice_count,
                count(*) filter (where reply_to_message_id in (select discussion_message_id from post_comment_jobs where discussion_chat_id = $1))::bigint as reply_to_channel_post_count,
                count(*) filter (where reply_to_message_id in (select bot_comment_message_id from post_comment_jobs where discussion_chat_id = $1))::bigint as reply_to_bot_count,
                count(*) filter (where reply_to_message_id is null)::bigint as top_level_message_count,
                count(*) filter (
                    where reply_to_message_id is not null
                      and reply_to_message_id not in (select discussion_message_id from post_comment_jobs where discussion_chat_id = $1)
                      and reply_to_message_id not in (select bot_comment_message_id from post_comment_jobs where discussion_chat_id = $1)
                )::bigint as reply_to_comment_count,
                count(*) filter (where created_at >= now() - interval '24 hours')::bigint as message_count_24h,
                count(*) filter (where has_links and created_at >= now() - interval '24 hours')::bigint as link_count_24h,
                avg(char_length(coalesce(text, '')))::double precision as avg_message_len,
                array_remove(array_agg(text order by created_at desc, message_id desc), null) as recent_message_texts
            from user_messages
        ), texture_stats as (
            select
                coalesce(sum(reuse_count), 0)::bigint as normalized_message_count,
                count(normalized_text)::bigint as distinct_normalized_message_count,
                coalesce(sum(greatest(reuse_count - 1, 0)), 0)::bigint as duplicate_normalized_message_count,
                coalesce(max(reuse_count), 0)::bigint as max_normalized_message_reuse_count
            from normalized_counts
        ), id_rank as (
            select
                case
                    when max(telegram_user_id) = min(telegram_user_id) then 1.0::double precision
                    else (($2::double precision - min(telegram_user_id)::double precision)
                        / nullif(max(telegram_user_id)::double precision - min(telegram_user_id)::double precision, 0))
                end as ratio
            from telegram_user_profiles
            where telegram_user_id > 0 and not coalesce(is_bot, false)
        ), latest_join_event as (
            select invite_link, via_chat_folder_invite_link
            from telegram_chat_member_events
            where chat_id = $1 and telegram_user_id = $2
            order by event_at desc
            limit 1
        )
        select
            cu.chat_id,
            cu.telegram_user_id,
            cu.first_seen_at,
            cu.last_seen_at,
            extract(epoch from (coalesce(cu.last_seen_at, now()) - cu.first_seen_at))::bigint as account_seen_age_sec,
            extract(epoch from (now() - cu.first_seen_at))::bigint as chat_age_sec,
            cu.first_message_id,
            cu.last_message_id,
            coalesce(ms.message_count, cu.message_count, 0)::bigint as message_count,
            coalesce(ms.reply_count, cu.reply_count, 0)::bigint as reply_count,
            coalesce(ms.link_count, cu.link_count, 0)::bigint as link_count,
            coalesce(ms.media_count, cu.media_count, 0)::bigint as media_count,
            coalesce(ms.voice_count, 0)::bigint as voice_count,
            coalesce(ms.reply_to_channel_post_count, cu.reply_to_channel_post_count, 0)::bigint as reply_to_channel_post_count,
            coalesce(ms.reply_to_bot_count, cu.reply_to_bot_count, 0)::bigint as reply_to_bot_count,
            coalesce(ms.top_level_message_count, 0)::bigint as top_level_message_count,
            coalesce(ms.reply_to_comment_count, 0)::bigint as reply_to_comment_count,
            coalesce(ms.message_count_24h, 0)::bigint as message_count_24h,
            coalesce(ms.link_count_24h, 0)::bigint as link_count_24h,
            case
                when cu.first_seen_at is null or cu.last_seen_at is null then null
                when extract(epoch from (cu.last_seen_at - cu.first_seen_at)) <= 0 then null
                else (coalesce(ms.message_count, cu.message_count, 0)::double precision / greatest(extract(epoch from (cu.last_seen_at - cu.first_seen_at)) / 60.0, 1.0))
            end as burst_messages_per_min,
            fm.message_id as first_message_id_from_messages,
            fm.text as first_message_text,
            reply_parent.text as first_message_reply_context,
            lm.message_id as last_message_id_from_messages,
            lm.text as last_message_text,
            coalesce(ms.recent_message_texts, array[]::text[]) as recent_message_texts,
            coalesce(ts.normalized_message_count, 0)::bigint as normalized_message_count,
            coalesce(ts.distinct_normalized_message_count, 0)::bigint as distinct_normalized_message_count,
            coalesce(ts.duplicate_normalized_message_count, 0)::bigint as duplicate_normalized_message_count,
            coalesce(ts.max_normalized_message_reuse_count, 0)::bigint as max_normalized_message_reuse_count,
            ms.avg_message_len,
            ir.ratio as id_rank_ratio,
            p.username,
            coalesce(uns.reuse_count, 0)::bigint as username_reuse_count,
            coalesce(uns.reuse_spammer_count, 0)::bigint as username_reuse_spammer_count,
            exists (
                select 1
                from shared_spam_reputation r
                where r.telegram_user_id = cu.telegram_user_id
                  and r.source_instance_id <> $3
            ) as shared_spammer_identity,
            p.first_name,
            p.last_name,
            nullif(trim(concat_ws(' ', p.first_name, p.last_name)), '') as display_name,
            coalesce(dns.reuse_count, 0)::bigint as display_name_reuse_count,
            coalesce(dns.reuse_spammer_count, 0)::bigint as display_name_reuse_spammer_count,
            coalesce(p.is_bot, false) as is_bot,
            p.is_premium,
            p.language_code,
            p.bio,
            p.profile_photo_file_id,
            p.profile_photo_file_unique_id,
            p.profile_photo_count,
            coalesce(pr.reuse_count, 0)::bigint as profile_photo_reuse_count,
            p.profile_photo_width,
            p.profile_photo_height,
            p.emoji_status_custom_emoji_id,
            p.profile_accent_color_id,
            p.personal_channel_chat_id,
            p.personal_channel_title,
            coalesce(pctr.reuse_count, 0)::bigint as personal_channel_title_reuse_count,
            coalesce(pctr.reuse_spammer_count, 0)::bigint
                as personal_channel_title_reuse_spammer_count,
            p.personal_channel_username,
            p.personal_channel_message_count,
            p.personal_channel_last_message_id,
            p.personal_channel_last_message_at,
            p.personal_channel_last_text,
            coalesce(p.personal_channel_has_adult_links, false) as personal_channel_has_adult_links,
            p.personal_channel_refreshed_at,
            p.personal_channel_fetch_error,
            s.status as member_status,
            s.is_present as member_is_present,
            s.is_admin as member_is_admin,
            exists (
                select 1
                from telegram_chat_member_events e
                where e.chat_id = $1 and e.telegram_user_id = $2
            ) as join_event_seen,
            lje.invite_link,
            coalesce(lje.via_chat_folder_invite_link, false) as via_chat_folder_invite_link
        from telegram_chat_users cu
        left join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
        left join telegram_chat_member_snapshots s on s.chat_id = cu.chat_id and s.telegram_user_id = cu.telegram_user_id
        left join msg_stats ms on true
        left join first_msg fm on true
        left join telegram_messages reply_parent
          on reply_parent.chat_id = cu.chat_id
         and reply_parent.message_id = fm.reply_to_message_id
        left join last_msg lm on true
        left join texture_stats ts on true
        left join id_rank ir on true
        left join latest_join_event lje on true
        left join lateral (
            select
                count(*)::bigint as reuse_count,
                count(*) filter (
                    where coalesce(cu2.is_spammer, false)
                       or exists (
                            select 1 from shared_spam_reputation r
                            where r.telegram_user_id = p2.telegram_user_id
                       )
                )::bigint as reuse_spammer_count
            from telegram_user_profiles p2
            left join telegram_chat_users cu2
              on cu2.chat_id = $1 and cu2.telegram_user_id = p2.telegram_user_id
            where nullif(lower(trim(p.username)), '') is not null
              and lower(trim(p2.username)) = lower(trim(p.username))
              and p2.telegram_user_id <> p.telegram_user_id
        ) uns on true
        left join lateral (
            select
                count(*)::bigint as reuse_count,
                count(*) filter (
                    where coalesce(cu2.is_spammer, false)
                       or exists (
                            select 1 from shared_spam_reputation r
                            where r.telegram_user_id = p2.telegram_user_id
                       )
                )::bigint as reuse_spammer_count
            from telegram_user_profiles p2
            left join telegram_chat_users cu2
              on cu2.chat_id = $1 and cu2.telegram_user_id = p2.telegram_user_id
            where nullif(trim(concat_ws(' ', p.first_name, p.last_name)), '') is not null
              and lower(nullif(trim(concat_ws(' ', p2.first_name, p2.last_name)), '')) = lower(nullif(trim(concat_ws(' ', p.first_name, p.last_name)), ''))
              and p2.telegram_user_id <> p.telegram_user_id
        ) dns on true
        left join lateral (
            select count(*)::bigint as reuse_count
            from telegram_user_profiles p2
            where p.profile_photo_file_unique_id is not null
              and p2.profile_photo_file_unique_id = p.profile_photo_file_unique_id
              and p2.telegram_user_id <> p.telegram_user_id
        ) pr on true
        left join lateral (
            select
                count(*)::bigint as reuse_count,
                count(*) filter (
                    where coalesce(cu2.is_spammer, false)
                       or exists (
                            select 1 from shared_spam_reputation r
                            where r.telegram_user_id = p2.telegram_user_id
                       )
                )::bigint as reuse_spammer_count
            from telegram_user_profiles p2
            left join telegram_chat_users cu2
              on cu2.chat_id = $1 and cu2.telegram_user_id = p2.telegram_user_id
            where nullif(lower(trim(p.personal_channel_title)), '') is not null
              and char_length(trim(p.personal_channel_title)) >= 4
              and lower(trim(p2.personal_channel_title)) = lower(trim(p.personal_channel_title))
              and p2.telegram_user_id <> p.telegram_user_id
        ) pctr on true
        where cu.chat_id = $1 and cu.telegram_user_id = $2
        "#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .bind(instance_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| {
        let recent_message_texts = row.get::<Vec<String>, _>("recent_message_texts");
        let message_style = message_style(&recent_message_texts);
        let max_pairwise_similarity = max_pairwise_message_similarity(&recent_message_texts);
        let max_reuse_count = row.get("max_normalized_message_reuse_count");
        let duplicate_normalized_message_count = row.get("duplicate_normalized_message_count");
        let repetitive_pattern = duplicate_normalized_message_count > 0
            || max_reuse_count > 1
            || max_pairwise_similarity.is_some_and(|similarity| similarity >= 0.86);

        NewUserFeatures {
            chat_id: row.get("chat_id"),
            telegram_user_id: row.get("telegram_user_id"),
            first_seen_at: row.get("first_seen_at"),
            last_seen_at: row.get("last_seen_at"),
            account_seen_age_sec: row.get("account_seen_age_sec"),
            chat_age_sec: row.get("chat_age_sec"),
            first_message_id: row
                .try_get("first_message_id_from_messages")
                .ok()
                .or_else(|| row.try_get("first_message_id").ok()),
            last_message_id: row
                .try_get("last_message_id_from_messages")
                .ok()
                .or_else(|| row.try_get("last_message_id").ok()),
            message_count: row.get("message_count"),
            reply_count: row.get("reply_count"),
            link_count: row.get("link_count"),
            media_count: row.get("media_count"),
            voice_count: row.get("voice_count"),
            reply_to_channel_post_count: row.get("reply_to_channel_post_count"),
            reply_to_bot_count: row.get("reply_to_bot_count"),
            top_level_message_count: row.get("top_level_message_count"),
            reply_to_comment_count: row.get("reply_to_comment_count"),
            message_count_24h: row.get("message_count_24h"),
            link_count_24h: row.get("link_count_24h"),
            burst_messages_per_min: row.get("burst_messages_per_min"),
            first_message_text: row.get("first_message_text"),
            first_message_reply_context: row.get("first_message_reply_context"),
            last_message_text: row.get("last_message_text"),
            recent_message_texts,
            text_texture: TextTexture {
                normalized_count: row.get("normalized_message_count"),
                distinct_normalized_count: row.get("distinct_normalized_message_count"),
                duplicate_normalized_count: duplicate_normalized_message_count,
                max_reuse_count,
                max_pairwise_similarity,
                avg_message_len: row.get("avg_message_len"),
                repetitive_pattern,
            },
            message_style,
            id_rank_ratio: row.get("id_rank_ratio"),
            username: row.get("username"),
            username_reuse_count: row.get("username_reuse_count"),
            username_reuse_spammer_count: row.get("username_reuse_spammer_count"),
            shared_spammer_identity: row.get("shared_spammer_identity"),
            first_name: row.get("first_name"),
            last_name: row.get("last_name"),
            display_name: row.get("display_name"),
            display_name_reuse_count: row.get("display_name_reuse_count"),
            display_name_reuse_spammer_count: row.get("display_name_reuse_spammer_count"),
            is_bot: row.get("is_bot"),
            is_premium: row.get("is_premium"),
            language_code: row.get("language_code"),
            bio: row.get("bio"),
            profile_photo_file_id: row.get("profile_photo_file_id"),
            profile_photo_file_unique_id: row.get("profile_photo_file_unique_id"),
            profile_photo_count: row.get("profile_photo_count"),
            profile_photo_reuse_count: row.get("profile_photo_reuse_count"),
            profile_photo_width: row.get("profile_photo_width"),
            profile_photo_height: row.get("profile_photo_height"),
            emoji_status_custom_emoji_id: row.get("emoji_status_custom_emoji_id"),
            profile_accent_color_id: row.get("profile_accent_color_id"),
            personal_channel_chat_id: row.get("personal_channel_chat_id"),
            personal_channel_title: row.get("personal_channel_title"),
            personal_channel_title_reuse_count: row.get("personal_channel_title_reuse_count"),
            personal_channel_title_reuse_spammer_count: row
                .get("personal_channel_title_reuse_spammer_count"),
            personal_channel_username: row.get("personal_channel_username"),
            personal_channel_message_count: row.get("personal_channel_message_count"),
            personal_channel_last_message_id: row.get("personal_channel_last_message_id"),
            personal_channel_last_message_at: row.get("personal_channel_last_message_at"),
            personal_channel_last_text: row.get("personal_channel_last_text"),
            personal_channel_has_adult_links: row.get("personal_channel_has_adult_links"),
            personal_channel_refreshed_at: row.get("personal_channel_refreshed_at"),
            personal_channel_fetch_error: row.get("personal_channel_fetch_error"),
            member_status: row.get("member_status"),
            member_is_present: row.get("member_is_present"),
            member_is_admin: row.get("member_is_admin"),
            join_event_seen: row.get("join_event_seen"),
            invite_link: row.get("invite_link"),
            via_chat_folder_invite_link: row.get("via_chat_folder_invite_link"),
        }
    }))
}










































fn audit_insert_columns() -> &'static [&'static str] {
    &[
        "chat_id",
        "telegram_user_id",
        "first_seen_at",
        "last_seen_at",
        "account_seen_age_sec",
        "chat_age_sec",
        "first_message_id",
        "last_message_id",
        "message_count",
        "reply_count",
        "link_count",
        "media_count",
        "voice_count",
        "reply_to_channel_post_count",
        "reply_to_bot_count",
        "top_level_message_count",
        "reply_to_comment_count",
        "only_replies_or_comments",
        "only_channel_post_comments",
        "message_count_24h",
        "link_count_24h",
        "burst_messages_per_min",
        "first_message_text",
        "first_message_len",
        "last_message_text",
        "last_message_len",
        "normalized_message_count",
        "distinct_normalized_message_count",
        "duplicate_normalized_message_count",
        "max_normalized_message_reuse_count",
        "max_pairwise_message_similarity",
        "avg_message_len",
        "repetitive_message_pattern",
        "telegram_user_id_bucket",
        "telegram_user_id_rank_ratio",
        "telegram_user_id_is_recent",
        "username",
        "username_len",
        "username_has_digits",
        "username_digit_count",
        "username_has_random_suffix",
        "username_pattern",
        "first_name",
        "last_name",
        "display_name",
        "first_name_feminine_pattern",
        "display_name_reuse_count",
        "display_name_reuse_spammer_count",
        "display_name_reused_by_spammers",
        "is_bot",
        "is_premium",
        "language_code",
        "bio",
        "bio_len",
        "has_profile_photo",
        "profile_photo_count",
        "profile_photo_reuse_count",
        "profile_photo_file_unique_id",
        "profile_photo_dc_id",
        "profile_photo_dc_source",
        "profile_photo_width",
        "profile_photo_height",
        "has_emoji_status",
        "profile_accent_color_id",
        "personal_channel_chat_id",
        "personal_channel_title",
        "personal_channel_title_reuse_count",
        "personal_channel_title_reuse_spammer_count",
        "personal_channel_title_reused_by_spammers",
        "personal_channel_username",
        "personal_channel_message_count",
        "personal_channel_last_message_id",
        "personal_channel_last_message_at",
        "personal_channel_last_text",
        "personal_channel_has_adult_links",
        "personal_channel_has_invite_link",
        "personal_channel_has_external_link",
        "personal_channel_title_len",
        "personal_channel_last_text_len",
        "personal_channel_refreshed_at",
        "personal_channel_fetch_error",
        "member_status",
        "member_is_present",
        "member_is_admin",
        "join_event_seen",
        "invite_link",
        "via_chat_folder_invite_link",
        "risk_baseline_score",
        "risk_baseline_signals",
        "risk_first_message_score",
        "risk_first_message_signals",
        "risk_avatar_score",
        "risk_avatar_signals",
        "risk_personal_channel_score",
        "risk_personal_channel_signals",
        "risk_score",
        "risk_level",
        "primary_risk_class",
        "risk_class_scores",
        "risk_labels",
        "risk_reasons",
        "risk_signal_breakdown",
        "risk_profile",
        "risk_profile_version",
        "telegram_id_model_version",
        "raw_features",
    ]
}

async fn save_audit_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    features: &NewUserFeatures,
    risk: &RiskAnalysis,
    config: &NewUserAnalysisConfig,
) -> anyhow::Result<()> {
    let username_stats = username_stats(features.username.as_deref());
    let profile_photo_dc = best_effort_profile_photo_dc(features.profile_photo_file_id.as_deref());
    let first_message_len = features.first_message_text.as_deref().map(char_count_i32);
    let last_message_len = features.last_message_text.as_deref().map(char_count_i32);
    let bio_len = features.bio.as_deref().map(char_count_i32);
    let channel_title_len = features
        .personal_channel_title
        .as_deref()
        .map(char_count_i32);
    let channel_last_text_len = features
        .personal_channel_last_text
        .as_deref()
        .map(char_count_i32);
    let telegram_id_spam_probability = config
        .telegram_id_model
        .as_ref()
        .map(|model| telegram_id_spam_probability(features.telegram_user_id, model));
    let telegram_id_risk_coefficient_value =
        telegram_id_spam_probability.map(telegram_id_risk_coefficient);
    let telegram_user_id_is_recent = features
        .id_rank_ratio
        .is_some_and(|ratio| ratio >= config.recent_id_ratio_threshold);
    let raw_features = json!({
        "dc": {
            "available": profile_photo_dc.dc_id.is_some(),
            "source": profile_photo_dc.source,
            "note": profile_photo_dc.note,
        },
        "thresholds": {
            "recent_id_ratio": config.recent_id_ratio_threshold,
            "old_user_message_threshold": config.old_user_message_threshold,
            "review_threshold": config.review_threshold,
        },
        "risk_profile": config.risk_profile,
        "risk_profile_version": config.risk_profile_version,
        "telegram_id_model_version": config.telegram_id_model_version,
        "telegram_id_model": config.telegram_id_model.as_ref().map(|model| json!({
            "floor": model.floor,
            "ceil": model.ceil,
            "k": model.k,
            "midpoint_billion": model.midpoint_billion,
            "version": model.version,
        })),
        "telegram_id_spam_probability": telegram_id_spam_probability,
        "telegram_id_risk_coefficient": telegram_id_risk_coefficient_value,
        "known_risk_classes": SpamClass::all().map(SpamClass::as_str),
        "profile_photo_file_id_present": features.profile_photo_file_id.is_some(),
        "profile_photo_file_unique_id_present": features.profile_photo_file_unique_id.is_some(),
        "profile_photo_reuse_count": features.profile_photo_reuse_count,
        "username_reuse_count": features.username_reuse_count,
        "username_reuse_spammer_count": features.username_reuse_spammer_count,
        "shared_spammer_identity": features.shared_spammer_identity,
        "first_name_feminine_pattern": looks_like_feminine_first_name(features.first_name.as_deref()),
        "chat_context": {
            "only_replies_or_comments": only_replies_or_comments(features),
            "only_channel_post_comments": only_channel_post_comments(features),
            "reply_to_channel_post_count": features.reply_to_channel_post_count,
            "reply_to_bot_count": features.reply_to_bot_count,
            "reply_to_comment_count": features.reply_to_comment_count,
            "top_level_message_count": features.top_level_message_count,
        },
        "text_texture": {
            "recent_message_text_count": features.recent_message_texts.len(),
            "normalized_message_count": features.text_texture.normalized_count,
            "distinct_normalized_message_count": features.text_texture.distinct_normalized_count,
            "duplicate_normalized_message_count": features.text_texture.duplicate_normalized_count,
            "max_normalized_message_reuse_count": features.text_texture.max_reuse_count,
            "max_pairwise_message_similarity": features.text_texture.max_pairwise_similarity,
            "repetitive_message_pattern": features.text_texture.repetitive_pattern,
        },
        "message_style": {
            "persona": message_style_persona(features).as_str(),
            "text_message_count": features.message_style.text_message_count,
            "single_exclamation_ending_count": features.message_style.single_exclamation_ending_count,
            "repeated_exclamation_ending_count": features.message_style.repeated_exclamation_ending_count,
            "period_ending_count": features.message_style.period_ending_count,
            "emoji_message_count": features.message_style.emoji_message_count,
            "emoji_ending_count": features.message_style.emoji_ending_count,
            "single_emoji_message_count": features.message_style.single_emoji_message_count,
            "single_emoji_ending_count": features.message_style.single_emoji_ending_count,
            "adjacent_emoji_message_count": features.message_style.adjacent_emoji_message_count,
            "other_non_text_ending_count": features.message_style.other_non_text_ending_count,
            "unmatched_closing_parenthesis_ending_count": features.message_style.unmatched_closing_parenthesis_ending_count,
        },
    });

    let columns = audit_insert_columns();
    let mut query = QueryBuilder::<Postgres>::new("insert into telegram_new_user_profile_audits (");

    {
        let mut separated = query.separated(", ");
        for column in columns {
            separated.push(*column);
        }
    }

    query.push(") values (");
    {
        let mut values = query.separated(", ");
        values.push_bind(features.chat_id);
        values.push_bind(features.telegram_user_id);
        values.push_bind(features.first_seen_at);
        values.push_bind(features.last_seen_at);
        values.push_bind(features.account_seen_age_sec);
        values.push_bind(features.chat_age_sec);
        values.push_bind(features.first_message_id);
        values.push_bind(features.last_message_id);
        values.push_bind(features.message_count);
        values.push_bind(features.reply_count);
        values.push_bind(features.link_count);
        values.push_bind(features.media_count);
        values.push_bind(features.voice_count);
        values.push_bind(features.reply_to_channel_post_count);
        values.push_bind(features.reply_to_bot_count);
        values.push_bind(features.top_level_message_count);
        values.push_bind(features.reply_to_comment_count);
        values.push_bind(only_replies_or_comments(features));
        values.push_bind(only_channel_post_comments(features));
        values.push_bind(features.message_count_24h);
        values.push_bind(features.link_count_24h);
        values.push_bind(features.burst_messages_per_min);
        values.push_bind(&features.first_message_text);
        values.push_bind(first_message_len);
        values.push_bind(&features.last_message_text);
        values.push_bind(last_message_len);
        values.push_bind(features.text_texture.normalized_count);
        values.push_bind(features.text_texture.distinct_normalized_count);
        values.push_bind(features.text_texture.duplicate_normalized_count);
        values.push_bind(features.text_texture.max_reuse_count);
        values.push_bind(features.text_texture.max_pairwise_similarity);
        values.push_bind(features.text_texture.avg_message_len);
        values.push_bind(features.text_texture.repetitive_pattern);
        values.push_bind(id_bucket(features.telegram_user_id));
        values.push_bind(features.id_rank_ratio);
        values.push_bind(telegram_user_id_is_recent);
        values.push_bind(&features.username);
        values.push_bind(features.username.as_deref().map(char_count_i32));
        values.push_bind(username_stats.has_digits);
        values.push_bind(username_stats.digit_count);
        values.push_bind(username_stats.has_random_suffix);
        values.push_bind(&username_stats.pattern);
        values.push_bind(&features.first_name);
        values.push_bind(&features.last_name);
        values.push_bind(&features.display_name);
        values.push_bind(looks_like_feminine_first_name(
            features.first_name.as_deref(),
        ));
        values.push_bind(features.display_name_reuse_count);
        values.push_bind(features.display_name_reuse_spammer_count);
        values.push_bind(features.display_name_reuse_spammer_count > 0);
        values.push_bind(features.is_bot);
        values.push_bind(features.is_premium);
        values.push_bind(&features.language_code);
        values.push_bind(&features.bio);
        values.push_bind(bio_len);
        values.push_bind(has_profile_photo(features));
        values.push_bind(features.profile_photo_count);
        values.push_bind(features.profile_photo_reuse_count);
        values.push_bind(&features.profile_photo_file_unique_id);
        values.push_bind(profile_photo_dc.dc_id);
        values.push_bind(&profile_photo_dc.source);
        values.push_bind(features.profile_photo_width);
        values.push_bind(features.profile_photo_height);
        values.push_bind(features.emoji_status_custom_emoji_id.is_some());
        values.push_bind(features.profile_accent_color_id);
        values.push_bind(features.personal_channel_chat_id);
        values.push_bind(&features.personal_channel_title);
        values.push_bind(features.personal_channel_title_reuse_count);
        values.push_bind(features.personal_channel_title_reuse_spammer_count);
        values.push_bind(features.personal_channel_title_reuse_spammer_count > 0);
        values.push_bind(&features.personal_channel_username);
        values.push_bind(features.personal_channel_message_count);
        values.push_bind(features.personal_channel_last_message_id);
        values.push_bind(features.personal_channel_last_message_at);
        values.push_bind(&features.personal_channel_last_text);
        values.push_bind(features.personal_channel_has_adult_links);
        values.push_bind(personal_channel_has_invite_link(features));
        values.push_bind(personal_channel_has_external_link(features));
        values.push_bind(channel_title_len);
        values.push_bind(channel_last_text_len);
        values.push_bind(features.personal_channel_refreshed_at);
        values.push_bind(&features.personal_channel_fetch_error);
        values.push_bind(&features.member_status);
        values.push_bind(features.member_is_present);
        values.push_bind(features.member_is_admin);
        values.push_bind(features.join_event_seen);
        values.push_bind(&features.invite_link);
        values.push_bind(features.via_chat_folder_invite_link);
        values.push_bind(risk.score);
        values.push_bind(&risk.signals);
        values.push_bind(0_i32);
        values.push_bind(json!([]));
        values.push_bind(0_i32);
        values.push_bind(json!([]));
        values.push_bind(0_i32);
        values.push_bind(json!([]));
        values.push_bind(risk.score);
        values.push_bind(&risk.level);
        values.push_bind(&risk.primary_class);
        values.push_bind(&risk.class_scores);
        values.push_bind(json!(risk.labels));
        values.push_bind(json!(risk.reasons));
        values.push_bind(&risk.signals);
        values.push_bind(&config.risk_profile);
        values.push_bind(&config.risk_profile_version);
        values.push_bind(&config.telegram_id_model_version);
        values.push_bind(raw_features);
    }

    query.push(") on conflict (chat_id, telegram_user_id) do update set analyzed_at = now(), ");
    {
        let mut updates = query.separated(", ");
        for column in columns.iter().copied().filter(|column| {
            !matches!(
                *column,
                "chat_id"
                    | "telegram_user_id"
                    | "risk_baseline_score"
                    | "risk_baseline_signals"
                    | "risk_first_message_score"
                    | "risk_first_message_signals"
                    | "risk_avatar_score"
                    | "risk_avatar_signals"
                    | "risk_personal_channel_score"
                    | "risk_personal_channel_signals"
                    | "risk_score"
                    | "risk_level"
                    | "risk_signal_breakdown"
            )
        }) {
            updates.push(format_args!("{column} = excluded.{column}"));
        }
        updates.push("risk_baseline_score = excluded.risk_baseline_score");
        updates.push("risk_baseline_signals = excluded.risk_baseline_signals");
        updates.push("risk_personal_channel_score = 0");
        updates.push("risk_personal_channel_signals = '[]'::jsonb");
        updates.push("risk_score = least(100, excluded.risk_baseline_score + telegram_new_user_profile_audits.risk_first_message_score + telegram_new_user_profile_audits.risk_avatar_score)");
        updates.push("risk_level = case when least(100, excluded.risk_baseline_score + telegram_new_user_profile_audits.risk_first_message_score + telegram_new_user_profile_audits.risk_avatar_score) >= 70 then 'high' when least(100, excluded.risk_baseline_score + telegram_new_user_profile_audits.risk_first_message_score + telegram_new_user_profile_audits.risk_avatar_score) >= 40 then 'medium' else 'low' end");
        updates.push("risk_signal_breakdown = excluded.risk_baseline_signals || telegram_new_user_profile_audits.risk_first_message_signals || telegram_new_user_profile_audits.risk_avatar_signals");
    }

    query.build().execute(&mut **tx).await?;

    Ok(())
}





























#[derive(Debug, Clone)]
struct DcParseResult {
    dc_id: Option<i32>,
    source: Option<String>,
    note: String,
}

fn best_effort_profile_photo_dc(file_id: Option<&str>) -> DcParseResult {
    let Some(file_id) = file_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return DcParseResult {
            dc_id: None,
            source: None,
            note: "no_profile_photo_file_id".to_string(),
        };
    };

    // Telegram Bot API does not expose DC directly. Desktop clients such as AyuGram
    // can show it because they decode MTProto file locations. Bot API file_id is an
    // opaque, versioned identifier; guessing a DC from random bytes would create bad
    // training labels. Keep decoded metadata only as a future hook for a verified
    // Pyrogram/AyuGram-compatible decoder.
    let normalized = file_id.replace('-', "+").replace('_', "/");
    let decode_attempt = URL_SAFE_NO_PAD
        .decode(file_id.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(normalized.as_bytes()));
    let note = match decode_attempt {
        Ok(bytes) => format!(
            "dc_unavailable_from_bot_api; file_id_decoded_bytes={} but no verified decoder is installed",
            bytes.len()
        ),
        Err(_) => "dc_unavailable_from_bot_api; file_id_decode_failed".to_string(),
    };

    DcParseResult {
        dc_id: None,
        source: Some("bot_api_file_id_unverified".to_string()),
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use teloxide_antispam::signals::analyze_new_or_low_activity_user;

    #[test]
    fn latest_personal_channel_content_is_a_redacted_material_audit_input() {
        let features = NewUserFeatures {
            personal_channel_chat_id: Some(-100_000_000_001),
            personal_channel_last_text: Some(
                "Пишите в личку за VPN: https://example.org/promo".to_string(),
            ),
            recent_message_texts: vec!["Пишите в личку за VPN".to_string()],
            ..Default::default()
        };
        let config = NewUserAnalysisConfig::default();
        let risk = analyze_new_or_low_activity_user(&features, &config);
        let snapshot = project_unified_user_audit_snapshot(&features, &risk, &config);
        let revision = project_unified_user_audit_material_revision(&features, &config);

        assert_eq!(
            snapshot["personal_channel"]["recent_content_preview"],
            "Пишите в личку за VPN: [link]"
        );
        assert_eq!(
            revision["personal_channel"]["recent_content_preview"],
            snapshot["personal_channel"]["recent_content_preview"]
        );

        let changed_features = NewUserFeatures {
            personal_channel_last_text: Some("Обычный пост о книгах".to_string()),
            ..features.clone()
        };
        assert_ne!(
            revision,
            project_unified_user_audit_material_revision(&changed_features, &config)
        );

        let changed_message = NewUserFeatures {
            recent_message_texts: vec!["Есть вариант заработать, пиши в лс".to_string()],
            ..features.clone()
        };
        assert_ne!(
            revision,
            project_unified_user_audit_material_revision(&changed_message, &config)
        );
    }

    #[test]
    fn dc_parser_does_not_guess_without_verified_decoder() {
        let result = best_effort_profile_photo_dc(Some("AQADBAADb6sxG4x8AAEC"));
        assert!(result.dc_id.is_none());
        assert!(result.note.contains("dc_unavailable_from_bot_api"));
    }

    #[test]
    fn bounded_audit_text_redacts_links_and_normalizes_whitespace() {
        assert_eq!(
            bounded_audit_text(Some("  promo https://t.me/+secret\nnext  ")),
            Some("promo [link] next".to_string())
        );
        assert_eq!(
            bounded_audit_text(Some("(telegram.me/+secret)")),
            Some("[link]".to_string())
        );
    }

    #[test]
    fn bounded_audit_text_truncates_by_characters() {
        let input = "я".repeat(UNIFIED_AUDIT_TEXT_LIMIT + 1);
        let preview = bounded_audit_text(Some(&input)).expect("non-empty preview");

        assert_eq!(preview.chars().count(), UNIFIED_AUDIT_TEXT_LIMIT + 1);
        assert!(preview.ends_with('…'));
    }
}
