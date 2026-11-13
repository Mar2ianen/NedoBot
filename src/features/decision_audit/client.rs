//! Нативный Decisions API. Ошибки не содержат body, ключей или URL.
use crate::config_file::DecisionConfig;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

pub const QUESTIONS: &str = include_str!("questions.json");
pub const POLICY_VERSION: &str = "decision-score-v2";
const MAX_RESPONSE_BYTES: usize = 256 * 1_024;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    Noul {
        noul: f64,
        confidence: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        probabilities: BTreeMap<String, f64>,
        legend: BTreeMap<String, String>,
        confidence: f64,
    },
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DecisionAssessment {
    pub answers: BTreeMap<String, Answer>,
    #[serde(default)]
    pub usage: Usage,
}
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Usage {
    #[serde(default)]
    pub total_input_tokens: u64,
    #[serde(default)]
    pub total_output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowVerdict {
    ReviewCandidate,
    NoSignal,
    Abstain,
}

impl DecisionAssessment {
    pub fn validate(&self) -> Result<(), Failure> {
        let questions: BTreeMap<String, Value> =
            serde_json::from_str(QUESTIONS).map_err(|_| Failure::InvalidResponse)?;
        if self.answers.keys().collect::<BTreeSet<_>>() != questions.keys().collect() {
            return Err(Failure::InvalidResponse);
        }
        for (id, answer) in &self.answers {
            let q = &questions[id];
            let valid = match answer {
                Answer::Noul { noul, confidence } => {
                    q["type"] == "noul" && probability(*noul) && probability(*confidence)
                }
                Answer::Choice {
                    choice,
                    probabilities,
                    confidence,
                } => {
                    q["type"] == "choice"
                        && probability(*confidence)
                        && q["criteria"].as_object().is_some_and(|criteria| {
                            probabilities.keys().collect::<BTreeSet<_>>()
                                == criteria.keys().collect::<BTreeSet<_>>()
                                && probabilities.get(choice).is_some_and(|chosen| {
                                    probabilities.values().all(|p| p <= &(chosen + 1e-6))
                                })
                        })
                        && distribution(probabilities)
                }
                Answer::Score {
                    score,
                    probabilities,
                    legend,
                    confidence,
                } => {
                    let criteria = q["criteria"].as_array();
                    q["type"] == "score"
                        && probability(*confidence)
                        && score.is_finite()
                        && criteria.is_some_and(|levels| {
                            let expected: BTreeSet<String> =
                                (0..levels.len()).map(|n| n.to_string()).collect();
                            probabilities.keys().cloned().collect::<BTreeSet<_>>() == expected
                                && legend.keys().cloned().collect::<BTreeSet<_>>() == expected
                                && levels.iter().enumerate().all(|(i, v)| {
                                    v.as_str() == Some(legend[&i.to_string()].as_str())
                                })
                                && *score >= 0.0
                                && *score <= levels.len().saturating_sub(1) as f64
                                && (*score
                                    - probabilities
                                        .iter()
                                        .map(|(k, p)| k.parse::<f64>().unwrap_or_default() * p)
                                        .sum::<f64>())
                                .abs()
                                    <= 0.05
                        })
                        && distribution(probabilities)
                }
            };
            if !valid {
                return Err(Failure::InvalidResponse);
            }
        }
        Ok(())
    }
    /// Для общей метрики учитываем суммарную массу рекламных классов:
    /// неопределённость между двумя типами рекламы не равна вероятности «не спам».
    pub fn captcha_verdict(&self) -> ShadowVerdict {
        if self.validate().is_err() {
            return ShadowVerdict::Abstain;
        }
        let probability = |id: &str| match &self.answers[id] {
            Answer::Noul { noul, .. } => *noul,
            _ => 0.0,
        };
        if probability("quoted_or_discussed_spam") >= 0.2 {
            return ShadowVerdict::Abstain;
        }
        let promotional_mass = match &self.answers["spam_campaign_type"] {
            Answer::Choice { probabilities, .. } => 1.0 - probabilities["none"],
            _ => return ShadowVerdict::Abstain,
        };
        let relevance = match &self.answers["contextual_relevance"] {
            Answer::Score { score, .. } => *score,
            _ => return ShadowVerdict::Abstain,
        };
        if promotional_mass >= 0.8
            && relevance <= 0.8
            && (probability("unsolicited_promotion") >= 0.8
                || (probability("work_solicitation") >= 0.8
                    && probability("private_contact_funnel") >= 0.8))
        {
            ShadowVerdict::ReviewCandidate
        } else {
            ShadowVerdict::NoSignal
        }
    }
    /// Независимые marginal probabilities не перемножаются. Сам по себе verdict не разрешает санкцию.
    pub fn verdict(&self) -> ShadowVerdict {
        if self.validate().is_err() {
            return ShadowVerdict::Abstain;
        }
        let noul = |id: &str| match self.answers.get(id) {
            Some(Answer::Noul { noul, .. }) => *noul,
            _ => 0.0,
        };
        if noul("quoted_or_discussed_spam") >= 0.2 {
            return ShadowVerdict::Abstain;
        }
        let (category, certainty) = match &self.answers["spam_campaign_type"] {
            Answer::Choice {
                choice, confidence, ..
            } => (choice.as_str(), *confidence),
            _ => return ShadowVerdict::Abstain,
        };
        let relevance = match &self.answers["contextual_relevance"] {
            Answer::Score { score, .. } => *score,
            _ => return ShadowVerdict::Abstain,
        };
        if category != "none"
            && certainty >= 0.5
            && relevance <= 0.8
            && (noul("unsolicited_promotion") >= 0.9
                || (noul("work_solicitation") >= 0.8 && noul("private_contact_funnel") >= 0.8))
        {
            ShadowVerdict::ReviewCandidate
        } else {
            ShadowVerdict::NoSignal
        }
    }
}
fn probability(p: f64) -> bool {
    p.is_finite() && (0.0..=1.0).contains(&p)
}
fn distribution(probs: &BTreeMap<String, f64>) -> bool {
    !probs.is_empty()
        && probs.values().all(|p| probability(*p))
        && (probs.values().sum::<f64>() - 1.0).abs() <= 0.025
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Timeout,
    Transport,
    Http(u16),
    InvalidResponse,
    Configuration,
    ImageUnavailable,
}
impl Failure {
    pub fn kind(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::Http(429) => "http_429",
            Self::Http(400) => "http_400",
            Self::Http(401 | 403) => "authentication",
            Self::Http(_) => "http_status",
            Self::InvalidResponse => "invalid_response",
            Self::Configuration => "configuration",
            Self::ImageUnavailable => "image_unavailable",
        }
    }
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Transport | Self::ImageUnavailable | Self::Http(429 | 500..=599)
        )
    }
}

pub struct DecisionClient {
    client: reqwest::Client,
    endpoint: String,
    model: String,
}
impl DecisionClient {
    pub fn new(config: &DecisionConfig) -> anyhow::Result<Self> {
        config.validate()?;
        anyhow::ensure!(config.enabled, "decision client is disabled");
        let key = std::env::var(&config.api_key_env)
            .map_err(|_| anyhow::anyhow!("decision key missing"))?;
        let mut header =
            HeaderValue::from_str(&key).map_err(|_| anyhow::anyhow!("invalid decision key"))?;
        header.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert("x-goog-api-key", header);
        Ok(Self {
            client: reqwest::Client::builder()
                .default_headers(headers)
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(config.timeout_sec))
                .build()?,
            endpoint: format!("{}/decisions", config.base_url.trim_end_matches('/')),
            model: config.model.clone(),
        })
    }
    pub async fn assess(
        &self,
        input: &Value,
        avatar: Option<&str>,
    ) -> Result<(DecisionAssessment, u64), Failure> {
        let questions: Value =
            serde_json::from_str(QUESTIONS).map_err(|_| Failure::Configuration)?;
        let text = serde_json::to_string(input).map_err(|_| Failure::Configuration)?;
        let input = if let Some(data) = avatar {
            json!([{"type":"text","text":format!("{text}\n")},{"type":"image","data":data,"mime_type":"image/jpeg","resolution":"low"}])
        } else {
            Value::String(text)
        };
        let start = Instant::now();
        let mut response = self
            .client
            .post(&self.endpoint)
            .json(&json!({"model":self.model,"input":input,"questions":questions}))
            .send()
            .await
            .map_err(transport_error)?;
        if !response.status().is_success() {
            return Err(Failure::Http(response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(Failure::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        let assessment: DecisionAssessment =
            serde_json::from_slice(&bytes).map_err(|_| Failure::InvalidResponse)?;
        assessment.validate()?;
        Ok((assessment, start.elapsed().as_millis() as u64))
    }
}
fn transport_error(e: reqwest::Error) -> Failure {
    if e.is_timeout() {
        Failure::Timeout
    } else {
        Failure::Transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, response::IntoResponse, routing::post};
    fn sample() -> Value {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/decision_response.json"
        ))
        .unwrap()
    }
    #[test]
    fn validates_types_distributions_and_atomic_targets() {
        let base = sample();
        let valid: DecisionAssessment = serde_json::from_value(base.clone()).unwrap();
        assert!(valid.validate().is_ok());
        assert_eq!(valid.verdict(), ShadowVerdict::NoSignal);
        let mut invalid = base.clone();
        invalid["answers"]
            .as_object_mut()
            .unwrap()
            .remove("work_solicitation");
        assert!(
            serde_json::from_value::<DecisionAssessment>(invalid)
                .unwrap()
                .validate()
                .is_err()
        );
        for (path, value) in [("noul", json!(1.1)), ("confidence", json!(-1))] {
            let mut invalid = base.clone();
            invalid["answers"]["work_solicitation"][path] = value;
            assert!(
                serde_json::from_value::<DecisionAssessment>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        let mut wrong = base.clone();
        wrong["answers"]["contextual_relevance"]["probabilities"]["0"] = json!(0.5);
        assert!(
            serde_json::from_value::<DecisionAssessment>(wrong)
                .unwrap()
                .validate()
                .is_err()
        );
        let mut wrong = base.clone();
        wrong["answers"]["spam_campaign_type"]["choice"] = json!("unknown");
        assert!(
            serde_json::from_value::<DecisionAssessment>(wrong)
                .unwrap()
                .validate()
                .is_err()
        );
        // confidence=0.99 при noul=0.01 не является вероятностью спама.
        assert!(valid.answers.values().all(|a| match a {
            Answer::Noul { noul, confidence } => *noul == 0.01 && *confidence == 0.99,
            _ => true,
        }));
        let mut uncertain = base;
        uncertain["answers"]["quoted_or_discussed_spam"]["noul"] = json!(0.8);
        assert_eq!(
            serde_json::from_value::<DecisionAssessment>(uncertain)
                .unwrap()
                .verdict(),
            ShadowVerdict::Abstain
        );
    }
    async fn mock(
        status: u16,
        body: Value,
        slow: bool,
    ) -> (DecisionClient, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/decisions", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/decisions",
            post(move |Json(request): Json<Value>| {
                let response = body.clone();
                async move {
                    assert_eq!(request["model"], "test-decision");
                    assert_eq!(
                        request["questions"],
                        serde_json::from_str::<Value>(QUESTIONS).unwrap()
                    );
                    assert!(request["input"].is_string() || request["input"].is_array());
                    if slow {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        Json(response),
                    )
                        .into_response()
                }
            }),
        );
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            DecisionClient {
                client: reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_millis(75))
                    .build()
                    .unwrap(),
                endpoint,
                model: "test-decision".into(),
            },
            handle,
        )
    }
    #[tokio::test]
    async fn native_wire_and_failures_are_not_ham() {
        let (client, server) = mock(200, sample(), false).await;
        assert!(
            client
                .assess(&json!({"author_message":"fixture"}), None)
                .await
                .is_ok()
        );
        assert!(
            client
                .assess(&json!({"author_message":"fixture"}), Some("aW1hZ2U="))
                .await
                .is_ok()
        );
        server.abort();
        for status in [400, 401, 429, 503, 302] {
            let (client, server) =
                mock(status, json!({"error":"do not log this body"}), false).await;
            assert_eq!(
                client.assess(&json!({}), None).await.unwrap_err(),
                Failure::Http(status)
            );
            assert_eq!(
                Failure::Http(status).retryable(),
                matches!(status, 429 | 503)
            );
            server.abort();
        }
        let (client, server) = mock(200, json!({"answers":{}}), false).await;
        assert_eq!(
            client.assess(&json!({}), None).await.unwrap_err(),
            Failure::InvalidResponse
        );
        server.abort();
        let (client, server) = mock(200, sample(), true).await;
        assert_eq!(
            client.assess(&json!({}), None).await.unwrap_err(),
            Failure::Timeout
        );
        server.abort();
    }
}
