use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};

pub(crate) struct LabelEvent<'a> {
    pub chat_id: i64,
    pub telegram_user_id: i64,
    pub label: &'a str,
    pub subtype: &'a str,
    pub source: &'a str,
    pub reason: &'a str,
    pub evidence: &'a Value,
    pub operator_telegram_user_id: Option<i64>,
}

pub(crate) async fn record_label_event_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    event: LabelEvent<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into spam_label_events
            (chat_id, telegram_user_id, label, subtype, source, reason, evidence,
             operator_telegram_user_id)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(event.chat_id)
    .bind(event.telegram_user_id)
    .bind(event.label)
    .bind(event.subtype)
    .bind(event.source)
    .bind(event.reason)
    .bind(event.evidence)
    .bind(event.operator_telegram_user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) fn owner_review_spam_subtype(signals: &Value) -> &'static str {
    let labels = signals
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|signal| signal.get("label").and_then(Value::as_str))
        .collect::<Vec<_>>();

    if labels.iter().any(|label| {
        matches!(
            *label,
            "explicit_adult_promo_bio" | "personal_channel_adult_links"
        )
    }) {
        return "adult_personal_channel_promo";
    }
    if labels.iter().any(|label| {
        matches!(
            *label,
            "foreign_invite_link_message" | "invite_link_from_new_user"
        )
    }) {
        return "foreign_invite_link_spam";
    }
    if labels.iter().any(|label| {
        matches!(
            *label,
            "profile_bio_subscription_invite_offer"
                | "personal_channel_invite_link"
                | "personal_channel_external_link"
        )
    }) {
        return "profile_channel_bait";
    }

    let has_unified_campaign = signals
        .as_array()
        .into_iter()
        .flatten()
        .filter(|signal| {
            signal.get("label").and_then(Value::as_str) == Some("unified_first_message_analysis")
        })
        .any(|signal| {
            signal
                .get("assessment")
                .and_then(|assessment| assessment.get("template_campaign"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || signal
                    .get("assessment")
                    .and_then(|assessment| assessment.get("direct_dm_offer"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        });
    if has_unified_campaign {
        return "promo_dm_bait";
    }

    "llm_generic_comment"
}

pub(crate) fn normal_label_evidence(request_id: i64) -> Value {
    json!({"review_request_id": request_id})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_reviewed_spam_from_strong_signals() {
        assert_eq!(
            owner_review_spam_subtype(&json!([
                {"label": "personal_channel_adult_links"},
                {"label": "missing_profile_photo"}
            ])),
            "adult_personal_channel_promo"
        );
        assert_eq!(
            owner_review_spam_subtype(&json!([
                {"label": "unified_first_message_analysis", "assessment": {"template_campaign": true}}
            ])),
            "promo_dm_bait"
        );
        assert_eq!(
            owner_review_spam_subtype(&json!([{"label": "missing_profile_photo"}])),
            "llm_generic_comment"
        );
    }
}
