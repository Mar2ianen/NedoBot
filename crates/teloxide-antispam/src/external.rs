use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

/// Жёсткий cap внешнего репутационного сигнала (CAS): слабый сигнал,
/// положительный вердикт не должен сам выводить пользователя в high.
pub const EXTERNAL_SCORE_CAP: i32 = 12;

/// Слабый внешний репутационный сигнал через Combot Anti-Spam.
///
/// CAS — глобальный shared banlist с бесплатным публичным API. Сигнал
/// намеренно слабый: CAS может врать (чужие баны необжалуемы) и может не
/// знать наших спамеров, поэтому положительный вердикт даёт не более
/// `EXTERNAL_SCORE_CAP` очков и никогда сам не выводит пользователя в high.
/// Отрицательный ответ («Record not found») и любые транспортные ошибки
/// трактуются как unknown/clean и ничего не добавляют.
const CAS_CHECK_URL: &str = "https://api.cas.chat/check";
const CAS_NOT_FOUND_DESCRIPTION: &str = "Record not found.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CasVerdict {
    /// Пользователь есть в CAS banlist.
    Banned,
    /// CAS явно ответил отсутствием записи.
    Clean,
    /// Транспортная ошибка, неожиданный ответ или чужой ok=false.
    /// Не evidence ни в какую сторону.
    Unknown,
}

#[derive(Debug, Deserialize)]
struct CasResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    description: Option<String>,
}

pub fn verdict_from_response(body: &str) -> CasVerdict {
    let response: CasResponse = match serde_json::from_str(body) {
        Ok(response) => response,
        Err(_) => return CasVerdict::Unknown,
    };
    if response.ok && response.result.is_some() {
        return CasVerdict::Banned;
    }
    if !response.ok
        && response
            .description
            .as_deref()
            .is_some_and(|description| description == CAS_NOT_FOUND_DESCRIPTION)
    {
        return CasVerdict::Clean;
    }
    CasVerdict::Unknown
}

/// Проверяет пользователя в CAS. Любая ошибка — `Unknown`, не clean.
pub async fn check_cas(user_id: i64, timeout: Duration) -> CasVerdict {
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(client) => client,
        Err(_) => return CasVerdict::Unknown,
    };
    let response = client
        .get(CAS_CHECK_URL)
        .query(&[("user_id", user_id.to_string())])
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            tracing::info!(user_id, %error, "CAS lookup failed; treating as unknown");
            return CasVerdict::Unknown;
        }
    };
    if !response.status().is_success() {
        tracing::info!(
            user_id,
            status = %response.status(),
            "CAS lookup returned non-success status; treating as unknown"
        );
        return CasVerdict::Unknown;
    }
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            tracing::info!(user_id, %error, "CAS lookup body unreadable; treating as unknown");
            return CasVerdict::Unknown;
        }
    };
    verdict_from_response(&body)
}

/// Превращает вердикт в capped-компонент скора и наблюдаемые сигналы.
/// Clean ничего не добавляет (не шумим в breakdown), Unknown фиксируется
/// меткой без очков, чтобы отличать «проверено, чисто» от «не проверено».
pub fn external_component(verdict: CasVerdict) -> (i32, Value) {
    match verdict {
        CasVerdict::Banned => (
            EXTERNAL_SCORE_CAP,
            json!([{
                "class": "external_reputation",
                "label": "cas_banned",
                "coefficient": EXTERNAL_SCORE_CAP,
                "warning_strength": "supporting",
            }]),
        ),
        CasVerdict::Unknown => (
            0,
            json!([{
                "class": "external_reputation",
                "label": "cas_unknown",
                "coefficient": 0,
                "warning_strength": "supporting",
            }]),
        ),
        CasVerdict::Clean => (0, Value::Array(Vec::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banned_verdict_requires_ok_with_result() {
        assert_eq!(
            verdict_from_response(r#"{"ok":true,"result":{"offenses":3}}"#),
            CasVerdict::Banned
        );
    }

    #[test]
    fn record_not_found_is_clean() {
        assert_eq!(
            verdict_from_response(r#"{"ok":false,"description":"Record not found."}"#),
            CasVerdict::Clean
        );
    }

    #[test]
    fn other_failures_are_unknown() {
        assert_eq!(
            verdict_from_response(r#"{"ok":false,"description":"Too many requests."}"#),
            CasVerdict::Unknown
        );
        assert_eq!(verdict_from_response(r#"{"ok":true}"#), CasVerdict::Unknown);
        assert_eq!(verdict_from_response("not json"), CasVerdict::Unknown);
    }

    #[test]
    fn banned_component_is_capped() {
        let (score, signals) = external_component(CasVerdict::Banned);
        assert_eq!(score, EXTERNAL_SCORE_CAP);
        assert!(signals.as_array().is_some_and(|items| !items.is_empty()));
    }

    #[test]
    fn clean_adds_nothing() {
        let (score, signals) = external_component(CasVerdict::Clean);
        assert_eq!(score, 0);
        assert!(signals.as_array().is_some_and(|items| items.is_empty()));
    }
}
