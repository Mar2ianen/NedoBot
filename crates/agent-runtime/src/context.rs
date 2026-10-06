//! Учёт контекста: оценка токенов, бюджет, давление, окно наблюдений.
//!
//! Три уровня по спеке v0.1 (§10):
//!
//! - L0 — локальная оценка через [`Tokenizer`] (дешёво, кэшируется);
//! - L1 — оценка сериализованного запроса (делает planner снаружи);
//! - L2 — правда провайдера (usage после запроса, хранит владелец сессии).
//!
//! Этот модуль даёт L0 и планирование бюджета без LLM.

use serde::{Deserialize, Serialize};

/// Уверенность локальной оценки токенов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EstimateConfidence {
    Exact,
    Heuristic,
    UpperBound,
}

/// Локальная оценка размера одной записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenEstimate {
    pub text: u32,
    pub framing: u32,
    pub confidence: EstimateConfidence,
}

impl TokenEstimate {
    /// Суммарная оценка (текст + оформление).
    pub fn total(&self) -> u32 {
        self.text.saturating_add(self.framing)
    }
}

/// Счётчик токенов. Реализация не определяет архитектуру:
/// точный (tiktoken), HF, эвристика или remote — взаимозаменяемы.
pub trait Tokenizer: Send + Sync {
    /// Посчитать токены фрагмента текста.
    fn count_text(&self, text: &str) -> u32;
}

/// Дешёвая эвристика: ~4 символа на токен. Для кириллицы и кода может
/// занижать размер; это L0-оценка, а не гарантия физического лимита провайдера.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApproxTokenizer;

impl Tokenizer for ApproxTokenizer {
    fn count_text(&self, text: &str) -> u32 {
        let chars = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
        // Минимум 1 токен на непустой текст, иначе 0.
        if chars == 0 {
            0
        } else {
            chars.div_ceil(4).max(1)
        }
    }
}

/// Класс записи контекста (что можно ронять первым).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContextClass {
    Instruction,
    RetainedFact,
    Summary,
    UserMessage,
    AssistantMessage,
    Reasoning,
    ToolCall,
    ToolResult,
    Artifact,
}

/// Одна управляемая единица контекста.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    pub id: String,
    pub class: ContextClass,
    /// Оценка L0, считается один раз при добавлении.
    pub estimate: TokenEstimate,
    /// Чем ниже число, тем раньше роняем при давлении.
    pub priority: u8,
    pub generation: u32,
    /// Общий id причинной пары (tool call + tool result).
    /// Записи с одинаковым `pair` роняются только вместе
    /// либо заменяются одной surrogate-записью (§9.2 спеки).
    pub pair: Option<String>,
}

impl ContextEntry {
    /// Суммарная оценка записи в токенах.
    pub fn tokens(&self) -> u32 {
        self.estimate.total()
    }
}

/// Снимок контекста для planner'а: только данные, без LLM.
#[derive(Debug, Clone)]
pub struct ContextSnapshot<'a> {
    pub entries: &'a [ContextEntry],
    pub budget: ContextBudget,
    pub used_tokens: u32,
    pub economic_threshold: Option<u32>,
}

/// Бюджет витка по спеке §11: отдельно физический лимит,
/// экономический порог и желаемый рабочий набор.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ContextBudget {
    /// Жёсткий лимит входа модели.
    pub hard_input_limit: u32,
    /// Желаемый рабочий набор (обычно ниже триггера компактификации).
    pub target_input_limit: u32,
    /// Резерв под ответ модели.
    pub output_reserve: u32,
    /// Резерв под схемы инструментов.
    pub tool_reserve: u32,
    /// Страховочная погрешность оценки.
    pub safety_margin: u32,
}

impl ContextBudget {
    /// Доступно под историю и наблюдение после всех резервов.
    pub fn usable_for_history(&self) -> u32 {
        self.target_input_limit
            .saturating_sub(self.output_reserve)
            .saturating_sub(self.tool_reserve)
            .saturating_sub(self.safety_margin)
    }
}

/// Состояние давления контекста (§26): не bool, а шкала.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextPressure {
    Normal,
    ApproachingTarget { used: u32, target: u32 },
    CompactionRequired,
    EconomicCliff { used: u32, threshold: u32 },
    HardLimit { used: u32, limit: u32 },
}

/// Оценивает давление по факту использования.
pub fn pressure(
    used: u32,
    budget: &ContextBudget,
    economic_threshold: Option<u32>,
) -> ContextPressure {
    if used >= budget.hard_input_limit {
        return ContextPressure::HardLimit {
            used,
            limit: budget.hard_input_limit,
        };
    }
    if let Some(threshold) = economic_threshold
        && used >= threshold
    {
        return ContextPressure::EconomicCliff { used, threshold };
    }
    if used >= budget.target_input_limit {
        return ContextPressure::CompactionRequired;
    }
    // Порог «подходим»: 80% от target.
    let warn_at = budget.target_input_limit.saturating_mul(8) / 10;
    if used >= warn_at {
        return ContextPressure::ApproachingTarget {
            used,
            target: budget.target_input_limit,
        };
    }
    ContextPressure::Normal
}

/// Обрезает строку по символам с маркером `…`, как раньше делал /ask.
/// Вынесено сюда для dogfood без смены поведения.
pub fn truncate_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        head.chars()
            .take(limit.saturating_sub(1))
            .collect::<String>()
            + "…"
    } else {
        head
    }
}

/// Окно наблюдений с суммарным лимитом: старые записи вытесняются.
/// Совместимо по поведению со старым `push_observation` в /ask.
#[derive(Debug, Clone, Default)]
pub struct ObservationWindow {
    entries: Vec<String>,
    max_total_chars: usize,
    max_entry_chars: usize,
}

impl ObservationWindow {
    /// Создаёт окно. `max_entry_chars` — обрезка одной записи,
    /// `max_total_chars` — суммарный лимит окна.
    pub fn new(max_entry_chars: usize, max_total_chars: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_total_chars,
            max_entry_chars,
        }
    }

    /// Добавляет наблюдение с обрезкой и вытеснением старых.
    pub fn push(&mut self, observation: String) {
        self.entries
            .push(truncate_chars(&observation, self.max_entry_chars));
        while self.total_chars() > self.max_total_chars && !self.entries.is_empty() {
            self.entries.remove(0);
        }
    }

    /// Снимок окна (старые первые).
    pub fn snapshot(&self) -> &[String] {
        &self.entries
    }

    /// Суммарный размер окна в символах.
    pub fn total_chars(&self) -> usize {
        self.entries.iter().map(|e| e.chars().count()).sum()
    }

    /// Оценка окна в токенах через переданный токенизатор.
    pub fn estimated_tokens(&self, tokenizer: &dyn Tokenizer) -> u32 {
        self.entries
            .iter()
            .map(|e| tokenizer.count_text(e))
            .fold(0, u32::saturating_add)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approx_tokenizer_counts_empty_as_zero() {
        assert_eq!(ApproxTokenizer.count_text(""), 0);
        assert_eq!(ApproxTokenizer.count_text("a"), 1);
        assert_eq!(ApproxTokenizer.count_text("abcd"), 1);
        assert_eq!(ApproxTokenizer.count_text("abcde"), 2);
    }

    #[test]
    fn truncate_chars_marks_cut() {
        assert_eq!(truncate_chars("abcdef", 100), "abcdef");
        assert_eq!(truncate_chars("abcdef", 5), "abcd…");
    }

    #[test]
    fn observation_window_evicts_oldest() {
        let mut window = ObservationWindow::new(10, 12);
        window.push("aaa".to_owned());
        window.push("bbb".to_owned());
        window.push("cccccccc".to_owned());
        assert!(window.total_chars() <= 12);
        assert!(window.snapshot().len() <= 2);
        assert!(window.snapshot().last().unwrap().contains('c'));
    }

    #[test]
    fn pressure_scale_orders_correctly() {
        let budget = ContextBudget {
            hard_input_limit: 1000,
            target_input_limit: 800,
            output_reserve: 50,
            tool_reserve: 50,
            safety_margin: 50,
        };
        assert_eq!(pressure(10, &budget, None), ContextPressure::Normal);
        assert!(matches!(
            pressure(700, &budget, None),
            ContextPressure::ApproachingTarget { .. }
        ));
        assert_eq!(
            pressure(850, &budget, None),
            ContextPressure::CompactionRequired
        );
        assert_eq!(
            pressure(1000, &budget, None),
            ContextPressure::HardLimit {
                used: 1000,
                limit: 1000
            }
        );
        assert!(matches!(
            pressure(900, &budget, Some(850)),
            ContextPressure::EconomicCliff { .. }
        ));
    }
}
