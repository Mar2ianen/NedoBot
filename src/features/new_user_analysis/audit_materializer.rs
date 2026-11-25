use super::*;

pub(super) fn audit_insert_columns() -> &'static [&'static str] {
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
        "risk_external_score",
        "risk_external_signals",
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
        "telegram_user_id_spam_probability",
        "raw_features",
    ]
}

pub(super) async fn save_audit_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    features: &NewUserFeatures,
    risk: &RiskAnalysis,
    config: &NewUserAnalysisConfig,
    reset_avatar_risk: bool,
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
        "profile_context_version": profile_context::PROFILE_CONTEXT_VERSION,
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
        values.push_bind(telegram_id_spam_probability);
        values.push_bind(raw_features);
    }

    query.push(") on conflict (chat_id, telegram_user_id) do update set analyzed_at = now(), ");
    {
        let mut updates = query.separated(", ");
        let previous_avatar_score = if reset_avatar_risk {
            "0"
        } else {
            "telegram_new_user_profile_audits.risk_avatar_score"
        };
        let previous_avatar_signals = if reset_avatar_risk {
            "'[]'::jsonb"
        } else {
            "telegram_new_user_profile_audits.risk_avatar_signals"
        };
        let previous_personal_channel_score =
            "telegram_new_user_profile_audits.risk_personal_channel_score";
        let previous_personal_channel_signals =
            "telegram_new_user_profile_audits.risk_personal_channel_signals";
        let previous_external_score = "telegram_new_user_profile_audits.risk_external_score";
        let previous_external_signals = "telegram_new_user_profile_audits.risk_external_signals";
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
                    | "risk_external_score"
                    | "risk_external_signals"
                    | "risk_score"
                    | "risk_level"
                    | "risk_signal_breakdown"
            )
        }) {
            updates.push(format_args!("{column} = excluded.{column}"));
        }
        updates.push("risk_baseline_score = excluded.risk_baseline_score");
        updates.push("risk_baseline_signals = excluded.risk_baseline_signals");
        updates.push(format_args!("risk_avatar_score = {previous_avatar_score}"));
        updates.push(format_args!(
            "risk_avatar_signals = {previous_avatar_signals}"
        ));
        updates.push(format_args!(
            "risk_personal_channel_score = {previous_personal_channel_score}"
        ));
        updates.push(format_args!(
            "risk_personal_channel_signals = {previous_personal_channel_signals}"
        ));
        updates.push(format_args!(
            "risk_external_score = {previous_external_score}"
        ));
        updates.push(format_args!(
            "risk_external_signals = {previous_external_signals}"
        ));
        let combined = format_args!(
            "least(100, excluded.risk_baseline_score + telegram_new_user_profile_audits.risk_first_message_score + {previous_avatar_score} + {previous_personal_channel_score} + {previous_external_score})"
        );
        updates.push(format_args!("risk_score = {combined}"));
        updates.push(format_args!("risk_level = case when {combined} >= 70 then 'high' when {combined} >= 40 then 'medium' else 'low' end"));
        updates.push(format_args!("risk_signal_breakdown = excluded.risk_baseline_signals || telegram_new_user_profile_audits.risk_first_message_signals || {previous_avatar_signals} || {previous_personal_channel_signals} || {previous_external_signals}"));
    }

    query.build().execute(&mut **tx).await?;

    Ok(())
}

#[derive(Debug, Clone)]
pub(super) struct DcParseResult {
    pub(super) dc_id: Option<i32>,
    pub(super) source: Option<String>,
    pub(super) note: String,
}

pub(super) fn best_effort_profile_photo_dc(file_id: Option<&str>) -> DcParseResult {
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
