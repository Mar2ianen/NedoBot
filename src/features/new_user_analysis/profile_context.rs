//! Общий локальный контекст профиля для baseline и закрытого Decision.
use crate::config_file::{RiskProfile, TelegramIdRiskModel};
use chrono::NaiveDate;
use std::borrow::Cow;
use teloxide_antispam::{preprocess::prepare_text, signals::NewUserFeatures};

pub(crate) const PROFILE_CONTEXT_VERSION: &str = "profile-context-v2";

pub(crate) fn telegram_id_model_for_date(
    profile: &RiskProfile,
    date: NaiveDate,
) -> Option<TelegramIdRiskModel> {
    let mut model = profile.telegram_id.clone()?;
    if let Some(time) = &profile.telegram_id_time {
        let days = date.signed_duration_since(time.reference_date).num_days();
        model.midpoint_billion = (model.midpoint_billion
            + time.midpoint_billion_per_day * days as f64)
            .max(f64::EPSILON);
        model.version = format!("{}/{}/{}", model.version, time.version, date);
    }
    Some(model)
}

/// Дополнение словаря зависимости распространёнными разговорными формами.
/// Только явные алиасы: окончания слова не считаются признаком имени.
fn feminine_alias(name: Option<&str>) -> Option<&'static str> {
    let prepared = prepare_text(name?);
    let token = prepared
        .canonical
        .split_whitespace()
        .next()?
        .trim_matches(|c: char| !c.is_alphabetic());
    match token {
        "кать" | "катюш" | "катюша" => Some("Катя"),
        "анюта" | "анют" => Some("Анна"),
        "настя" | "настюш" | "настюша" => Some("Анастасия"),
        "наташа" | "наташ" => Some("Наталья"),
        _ => None,
    }
}

pub(crate) fn looks_like_feminine_first_name(name: Option<&str>) -> bool {
    teloxide_antispam::signals::looks_like_feminine_first_name(name)
        || feminine_alias(name).is_some()
}

/// Сохраняем исходные профильные строки; чистому scorer даём канонический alias.
pub(super) fn features_for_baseline(features: &NewUserFeatures) -> Cow<'_, NewUserFeatures> {
    match feminine_alias(features.first_name.as_deref()) {
        Some(alias) => {
            let mut normalized = features.clone();
            normalized.first_name = Some(alias.into());
            Cow::Owned(normalized)
        }
        None => Cow::Borrowed(features),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_file::TelegramIdTimeAdjustment;
    use teloxide_antispam::signals::{
        NewUserAnalysisConfig, analyze_risk, telegram_id_spam_probability,
    };

    #[test]
    fn temporal_frontier_moves_forward_without_changing_static_models() {
        let mut profile = RiskProfile {
            version: "synthetic".into(),
            old_user_message_threshold: 5,
            review_threshold: 70,
            telegram_id: Some(TelegramIdRiskModel {
                floor: 0.0,
                ceil: 1.0,
                k: 2.0,
                midpoint_billion: 8.0,
                version: "synthetic-id".into(),
            }),
            telegram_id_time: None,
        };
        let start = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let later = NaiveDate::from_ymd_opt(2026, 4, 11).unwrap();
        let fixed = telegram_id_model_for_date(&profile, later).unwrap();
        assert_eq!(fixed.midpoint_billion, 8.0);
        assert_eq!(fixed.version, "synthetic-id");
        profile.telegram_id_time = Some(TelegramIdTimeAdjustment {
            reference_date: start,
            midpoint_billion_per_day: 0.001,
            version: "synthetic-time".into(),
        });
        let initial = telegram_id_model_for_date(&profile, start).unwrap();
        let shifted = telegram_id_model_for_date(&profile, later).unwrap();
        assert!((shifted.midpoint_billion - 8.1).abs() < 1e-12);
        assert_eq!(telegram_id_spam_probability(8_000_000_000, &initial), 0.5);
        assert!(telegram_id_spam_probability(8_000_000_000, &shifted) < 0.5);
        assert!((telegram_id_spam_probability(8_100_000_000, &shifted) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn nickname_aliases_reach_baseline_without_rewriting_profile_strings() {
        for alias in ["Катюша", "Кать", "Анюта", "Настя", "Наташа"] {
            let features = NewUserFeatures {
                first_name: Some(alias.into()),
                message_count: 1,
                ..Default::default()
            };
            let risk = analyze_risk(
                &features_for_baseline(&features),
                &NewUserAnalysisConfig::default(),
                false,
            );
            assert!(
                risk.signals
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["label"] == "atypical_feminine_first_name"),
                "{alias}"
            );
            assert_eq!(features.first_name.as_deref(), Some(alias));
            assert!(looks_like_feminine_first_name(Some(alias)));
        }
        for name in ["Никита", "Дима", "Каталог", "Анютин сервер", "Чат"]
        {
            assert!(!looks_like_feminine_first_name(Some(name)), "{name}");
        }
    }
}
