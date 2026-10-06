//! Учёт использования и экономики (§25 спеки).
//!
//! Токены — не всегда деньги: для subscription-профиля стоимость
//! отсутствует, но влияние на квоту фиксируется отдельно.

use serde::{Deserialize, Serialize};

/// Источник цифр использования.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UsageSource {
    /// Сообщил провайдер.
    ProviderReported,
    /// Посчитал локальный estimator.
    Estimated,
    /// Частично провайдер, частично оценка.
    Mixed,
}

/// Использование одного model-запроса. Максимальная детализация,
/// адаптер не сводит всё к `total_tokens`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input: Option<u64>,
    pub cached_input: Option<u64>,
    pub cache_write: Option<u64>,
    pub output: Option<u64>,
    pub reasoning: Option<u64>,
    pub source: Option<UsageSource>,
}

impl Usage {
    /// Суммирует два отчёта (например, стриминговые чанки).
    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            input: add_opt(self.input, other.input),
            cached_input: add_opt(self.cached_input, other.cached_input),
            cache_write: add_opt(self.cache_write, other.cache_write),
            output: add_opt(self.output, other.output),
            reasoning: add_opt(self.reasoning, other.reasoning),
            source: match (self.source, other.source) {
                (None, None) => None,
                (Some(source), None) | (None, Some(source)) => Some(source),
                (Some(a), Some(b)) if a == b => Some(a),
                _ => Some(UsageSource::Mixed),
            },
        }
    }
}

fn add_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.saturating_add(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// Денежная оценка запроса. Для subscription всегда `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Минимальная денежная единица (например, копейки/центы).
    pub amount_minor: u64,
    pub currency: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_adds_saturating() {
        let total = Usage {
            input: Some(100),
            output: Some(50),
            source: Some(UsageSource::ProviderReported),
            ..Default::default()
        }
        .saturating_add(Usage {
            input: Some(30),
            reasoning: Some(10),
            source: Some(UsageSource::ProviderReported),
            ..Default::default()
        });
        assert_eq!(total.input, Some(130));
        assert_eq!(total.output, Some(50));
        assert_eq!(total.reasoning, Some(10));
        assert_eq!(total.source, Some(UsageSource::ProviderReported));
    }

    #[test]
    fn empty_usage_is_an_identity_for_provenance() {
        let provider = Usage {
            source: Some(UsageSource::ProviderReported),
            ..Default::default()
        };
        assert_eq!(
            Usage::default().saturating_add(provider).source,
            provider.source
        );
        assert_eq!(
            provider.saturating_add(Usage::default()).source,
            provider.source
        );
        assert_eq!(
            Usage::default().saturating_add(Usage::default()).source,
            None
        );
    }
}
