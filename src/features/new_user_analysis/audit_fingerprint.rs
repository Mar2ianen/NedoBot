use super::*;

pub(super) fn hex_digest(material: &[u8]) -> String {
    Sha256::digest(material)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) struct FingerprintCalculator;

impl FingerprintCalculator {
    pub(super) fn digest(material: &[u8]) -> String {
        hex_digest(material)
    }

    pub(super) fn trigger(
        features: &NewUserFeatures,
        latest_link_evidence: Option<&(i32, Option<String>)>,
        latest_long_message_evidence: Option<&(i32, Option<String>)>,
    ) -> Option<String> {
        audit_trigger_fingerprint(features, latest_link_evidence, latest_long_message_evidence)
    }

    pub(super) fn snapshot(
        features: &NewUserFeatures,
        risk: &RiskAnalysis,
        config: &NewUserAnalysisConfig,
    ) -> Value {
        project_unified_user_audit_snapshot(features, risk, config)
    }

    pub(super) fn material_revision(
        features: &NewUserFeatures,
        config: &NewUserAnalysisConfig,
    ) -> Value {
        project_unified_user_audit_material_revision(features, config)
    }
}

pub(super) async fn load_latest_link_evidence(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
) -> anyhow::Result<Option<(i32, Option<String>)>> {
    const MAX_RECENT_MESSAGES_TO_SCAN: i64 = 64;
    let rows: Vec<(i32, Option<String>, Vec<String>)> = sqlx::query_as(
        r#"select message.message_id, message.text,
                  array_agg(evidence.target_sha256::text order by evidence.source, evidence.item_index)
           from public.telegram_messages message
           join public.telegram_message_url_evidence evidence
             on evidence.chat_id = message.chat_id and evidence.message_id = message.message_id
           where message.chat_id = $1 and message.user_id = $2 and message.source_channel_id is null
             and message.created_at >= now() - interval '24 hours'
             and evidence.parser_version = $4 and evidence.kind = 'url'
           group by message.message_id, message.text, message.created_at
           order by message.created_at desc, message.message_id desc
           limit $3"#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .bind(MAX_RECENT_MESSAGES_TO_SCAN)
    .bind(crate::db::url_evidence::URL_EVIDENCE_PARSER_VERSION)
    .fetch_all(pool)
    .await?;
    for (message_id, text, target_hashes) in rows {
        if target_hashes.is_empty() {
            continue;
        }
        let fingerprint = link_target_fingerprint(text.as_deref(), target_hashes);
        return Ok(Some((message_id, Some(fingerprint))));
    }
    Ok(None)
}

pub(super) fn link_target_fingerprint(
    text: Option<&str>,
    mut target_hashes: Vec<String>,
) -> String {
    target_hashes.sort();
    target_hashes.dedup();
    // В отпечаток попадают только digest-ы уже materialized evidence;
    // исходные URL могут содержать invite-токены и deep-link.
    let evidence =
        serde_json::to_vec(&("url-evidence-v2", text, target_hashes)).unwrap_or_default();
    let digest = Sha256::digest(evidence)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("url-evidence-v2:{digest}")
}

pub(super) async fn load_latest_long_message_evidence(
    pool: &PgPool,
    chat_id: i64,
    telegram_user_id: i64,
) -> anyhow::Result<Option<(i32, Option<String>)>> {
    Ok(sqlx::query_as(
        r#"select message_id, text
           from telegram_messages
           where chat_id = $1 and user_id = $2 and source_channel_id is null
             and text is not null and char_length(text) >= $3
             and created_at >= now() - interval '24 hours'
           order by created_at desc, message_id desc limit 1"#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .bind(LONG_FIRST_COMMENT_MIN_CHARS as i32)
    .fetch_optional(pool)
    .await?)
}

pub(super) fn audit_trigger_fingerprint(
    features: &NewUserFeatures,
    latest_link_evidence: Option<&(i32, Option<String>)>,
    latest_long_message_evidence: Option<&(i32, Option<String>)>,
) -> Option<String> {
    let channel_has_promotional_links = features.personal_channel_has_adult_links
        || personal_channel_has_invite_link(features)
        || personal_channel_has_external_link(features);
    let channel_evidence = channel_has_promotional_links.then(|| {
        json!({
            "chat_id": features.personal_channel_chat_id,
            "last_message_id": features.personal_channel_last_message_id,
            "title": bounded_audit_text(features.personal_channel_title.as_deref()),
            "username": features.personal_channel_username,
            "last_text": bounded_audit_text(features.personal_channel_last_text.as_deref()),
            "adult_links": features.personal_channel_has_adult_links,
            "invite_link": personal_channel_has_invite_link(features),
            "external_link": personal_channel_has_external_link(features),
        })
    });
    if latest_long_message_evidence.is_none()
        && channel_evidence.is_none()
        && latest_link_evidence.is_none()
    {
        return None;
    }
    let evidence = json!({
        "avatar": features.profile_photo_file_unique_id,
        "latest_long_message": latest_long_message_evidence,
        "latest_link_message": latest_link_evidence,
        "channel": channel_evidence,
    });
    Some(
        Sha256::digest(serde_json::to_vec(&evidence).unwrap_or_default())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

pub(super) fn has_long_first_comment(features: &NewUserFeatures) -> bool {
    features
        .first_message_text
        .as_deref()
        .is_some_and(|text| text.chars().count() >= LONG_FIRST_COMMENT_MIN_CHARS)
}

pub(super) fn project_unified_user_audit_material_revision(
    features: &NewUserFeatures,
    config: &NewUserAnalysisConfig,
) -> Value {
    // Изменение текстовых сообщений должно создавать новую генерацию, а не
    // только перезаписывать input_json у уже завершённой оценки.
    json!({
        "schema_version": 5,
        "deterministic_feature_config_version": deterministic::RULE_VERSION,
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
            "local_spammer_identity": features.local_spammer_identity,
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

pub(super) fn project_unified_user_audit_snapshot(
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
            "local_spammer_identity": features.local_spammer_identity,
            "level": risk.level,
            "primary_class": risk.primary_class,
            "class_scores": risk.class_scores,
            "labels": risk.labels,
            "reasons": risk.reasons,
            "signals": risk.signals,
        },
    })
}

pub(crate) fn bounded_audit_text(value: Option<&str>) -> Option<String> {
    bounded_audit_text_with_limit(value, UNIFIED_AUDIT_TEXT_LIMIT)
}

pub(crate) fn bounded_audit_text_with_limit(value: Option<&str>, limit: usize) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }

    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let redacted = redact_urls(&normalized);
    let mut preview = redacted.chars().take(limit).collect::<String>();
    if redacted.chars().count() > limit {
        preview.push('…');
    }
    Some(preview)
}

pub(super) fn redact_urls(text: &str) -> String {
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
