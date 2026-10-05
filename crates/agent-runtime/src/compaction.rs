//! Политика компактификации (§27 спеки): планировщик без LLM.
//!
//! Planner получает [`ContextSnapshot`] и возвращает [`CompactionPlan`].
//! Всё детерминировано и покрывается unit-тестами без модели:
//! LLM summary — отдельная Stage 3, сюда приходит только её результат.

use serde::{Deserialize, Serialize};

use crate::context::{ContextClass, ContextEntry, ContextPressure, ContextSnapshot, pressure};
use crate::error::{AgentError, ContextErrorKind};

/// Стратегия усечения крупного tool result (Stage 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TruncateStrategy {
    /// Оставить начало и конец, середину заменить дайджестом.
    HeadTail { head_tokens: u32, tail_tokens: u32 },
    /// Оставить только начало.
    HeadOnly { head_tokens: u32 },
}

/// Одно действие плана компактификации.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompactionAction {
    /// Убрать запись из проекции (источник в journal остаётся).
    Drop(String),
    /// Заменить группу записей одной surrogate-записью.
    Replace { old: Vec<String>, new: ContextEntry },
    /// Усечь крупный tool result по стратегии.
    TruncateToolResult {
        id: String,
        strategy: TruncateStrategy,
    },
    /// Суммировать диапазон записей (выполняет LLM, planner лишь помечает).
    Summarize { from_id: String, to_id: String },
}

/// План структурной компактификации.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompactionPlan {
    pub actions: Vec<CompactionAction>,
    /// Сколько токенов план ориентировочно освобождает.
    pub freed_estimate: u32,
}

impl CompactionPlan {
    /// True, если план сводит использование к цели бюджета.
    pub fn meets_target(&self, used: u32, target: u32) -> bool {
        used.saturating_sub(self.freed_estimate) <= target
    }
}

/// Политика компактификации: чистая функция над снимком.
pub trait CompactionPolicy: Send + Sync {
    /// Построить план. Ошибка — только при противоречивом снимке,
    /// нехватка места планом не считается (следующая ступень — LLM summary).
    fn plan(&self, snapshot: &ContextSnapshot) -> Result<CompactionPlan, AgentError>;

    /// Проверка инварианта §9.2: пара роняется только целиком.
    fn check_pairs(plan: &CompactionPlan, snapshot: &ContextSnapshot) -> bool {
        let dropped: std::collections::HashSet<&str> = plan
            .actions
            .iter()
            .flat_map(|action| match action {
                CompactionAction::Drop(id) => vec![id.as_str()],
                CompactionAction::Replace { old, .. } => {
                    old.iter().map(String::as_str).collect::<Vec<_>>()
                }
                _ => Vec::new(),
            })
            .collect();
        let mut pairs: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for entry in snapshot.entries {
            if let Some(pair) = entry.pair.as_deref() {
                pairs.entry(pair).or_default().push(entry.id.as_str());
            }
        }
        pairs.values().all(|ids| {
            let dropped_count = ids.iter().filter(|id| dropped.contains(*id)).count();
            dropped_count == 0 || dropped_count == ids.len()
        })
    }
}

/// Жадная структурная политика: сначала усечь крупные tool results,
/// затем ронять дешёвое и старое. Обязательный префикс не трогает.
#[derive(Debug, Clone)]
pub struct GreedyCompactionPolicy {
    /// Tool result крупнее порога сначала усекается, а не роняется.
    pub truncate_above_tokens: u32,
}

impl Default for GreedyCompactionPolicy {
    fn default() -> Self {
        Self {
            truncate_above_tokens: 2000,
        }
    }
}

/// Классы обязательного префикса: planner их никогда не роняет.
fn is_mandatory(class: ContextClass) -> bool {
    matches!(
        class,
        ContextClass::Instruction | ContextClass::RetainedFact | ContextClass::Summary
    )
}

impl CompactionPolicy for GreedyCompactionPolicy {
    fn plan(&self, snapshot: &ContextSnapshot) -> Result<CompactionPlan, AgentError> {
        let state = pressure(
            snapshot.used_tokens,
            &snapshot.budget,
            snapshot.economic_threshold,
        );
        let needs_work = matches!(
            state,
            ContextPressure::CompactionRequired
                | ContextPressure::EconomicCliff { .. }
                | ContextPressure::HardLimit { .. }
        );
        if !needs_work {
            return Ok(CompactionPlan::default());
        }
        let target = snapshot.budget.usable_for_history();
        let mut need = snapshot.used_tokens.saturating_sub(target);
        if need == 0 {
            return Ok(CompactionPlan::default());
        }
        let mut plan = CompactionPlan::default();
        // Кандидаты: немандаторные записи, сначала низкий приоритет и крупные.
        let mut candidates: Vec<&ContextEntry> = snapshot
            .entries
            .iter()
            .filter(|entry| !is_mandatory(entry.class))
            .filter(|entry| {
                !entry.pair.as_ref().is_some_and(|pair| {
                    snapshot
                        .entries
                        .iter()
                        .any(|other| other.pair.as_ref() == Some(pair) && is_mandatory(other.class))
                })
            })
            .collect();
        candidates.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then(b.tokens().cmp(&a.tokens()))
        });
        // Группируем по парам, чтобы не рвать причинность.
        let mut groups: Vec<Vec<&ContextEntry>> = Vec::new();
        let mut pair_index: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for entry in candidates {
            if let Some(pair) = entry.pair.as_deref() {
                if let Some(slot) = pair_index.get(pair) {
                    groups[*slot].push(entry);
                } else {
                    pair_index.insert(pair, groups.len());
                    groups.push(vec![entry]);
                }
            } else {
                groups.push(vec![entry]);
            }
        }
        for group in groups {
            if need == 0 {
                break;
            }
            let group_tokens: u32 = group.iter().map(|entry| entry.tokens()).sum();
            // Один крупный tool result — усечь вместо удаления.
            if group.len() == 1
                && group[0].class == ContextClass::ToolResult
                && group[0].tokens() > self.truncate_above_tokens
            {
                let keep = self.truncate_above_tokens;
                plan.actions.push(CompactionAction::TruncateToolResult {
                    id: group[0].id.clone(),
                    strategy: TruncateStrategy::HeadTail {
                        head_tokens: keep / 2,
                        tail_tokens: keep / 2,
                    },
                });
                let freed = group_tokens.saturating_sub(keep);
                plan.freed_estimate = plan.freed_estimate.saturating_add(freed);
                need = need.saturating_sub(freed);
                continue;
            }
            for entry in &group {
                plan.actions.push(CompactionAction::Drop(entry.id.clone()));
                plan.freed_estimate = plan.freed_estimate.saturating_add(entry.tokens());
                need = need.saturating_sub(entry.tokens());
            }
        }
        if need > 0 {
            return Err(AgentError::Context(crate::error::ContextError {
                kind: ContextErrorKind::BudgetExceeded,
                message: "нет записей, доступных для структурной компактификации".to_owned(),
            }));
        }
        debug_assert!(Self::check_pairs(&plan, snapshot));
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ContextBudget, ContextEntry, EstimateConfidence, TokenEstimate};

    fn entry(
        id: &str,
        class: ContextClass,
        tokens: u32,
        priority: u8,
        pair: Option<&str>,
    ) -> ContextEntry {
        ContextEntry {
            id: id.to_owned(),
            class,
            estimate: TokenEstimate {
                text: tokens,
                framing: 0,
                confidence: EstimateConfidence::Exact,
            },
            priority,
            generation: 0,
            pair: pair.map(str::to_owned),
        }
    }

    fn budget() -> ContextBudget {
        ContextBudget {
            hard_input_limit: 10_000,
            target_input_limit: 6_000,
            output_reserve: 500,
            tool_reserve: 500,
            safety_margin: 500,
        }
    }

    #[test]
    fn empty_plan_when_no_pressure() {
        let entries = vec![entry("a", ContextClass::UserMessage, 100, 5, None)];
        let snapshot = ContextSnapshot {
            entries: &entries,
            budget: budget(),
            used_tokens: 100,
            economic_threshold: None,
        };
        let plan = GreedyCompactionPolicy::default().plan(&snapshot).unwrap();
        assert!(plan.actions.is_empty());
    }

    #[test]
    fn mandatory_prefix_survives() {
        let entries = vec![
            entry("sys", ContextClass::Instruction, 3_000, 9, None),
            entry("old", ContextClass::UserMessage, 4_000, 1, None),
        ];
        let snapshot = ContextSnapshot {
            entries: &entries,
            budget: budget(),
            used_tokens: 7_000,
            economic_threshold: None,
        };
        let plan = GreedyCompactionPolicy::default().plan(&snapshot).unwrap();
        assert!(
            plan.actions
                .iter()
                .all(|action| !matches!(action, CompactionAction::Drop(id) if id == "sys"))
        );
        assert!(GreedyCompactionPolicy::check_pairs(&plan, &snapshot));
    }

    #[test]
    fn large_tool_result_is_truncated_not_dropped() {
        let entries = vec![
            entry("sys", ContextClass::Instruction, 1_000, 9, None),
            entry("big", ContextClass::ToolResult, 5_000, 1, None),
        ];
        let snapshot = ContextSnapshot {
            entries: &entries,
            budget: budget(),
            used_tokens: 6_000,
            economic_threshold: None,
        };
        let plan = GreedyCompactionPolicy::default().plan(&snapshot).unwrap();
        assert!(plan.actions.iter().any(|action| matches!(
            action,
            CompactionAction::TruncateToolResult { id, .. } if id == "big"
        )));
    }

    #[test]
    fn causal_pair_drops_together() {
        let entries = vec![
            entry("sys", ContextClass::Instruction, 500, 9, None),
            entry("call", ContextClass::ToolCall, 2_500, 1, Some("p1")),
            entry("res", ContextClass::ToolResult, 2_500, 1, Some("p1")),
        ];
        let snapshot = ContextSnapshot {
            entries: &entries,
            budget: budget(),
            used_tokens: 5_500,
            economic_threshold: None,
        };
        let plan = GreedyCompactionPolicy::default().plan(&snapshot).unwrap();
        assert!(GreedyCompactionPolicy::check_pairs(&plan, &snapshot));
        let dropped: Vec<&str> = plan
            .actions
            .iter()
            .filter_map(|action| match action {
                CompactionAction::Drop(id) => Some(id.as_str()),
                _ => None,
            })
            .collect();
        // Пара либо целиком в плане, либо целиком вне его.
        assert!(
            dropped.contains(&"call") == dropped.contains(&"res"),
            "pair broken: {dropped:?}"
        );
    }

    #[test]
    fn pair_is_dropped_entirely_even_when_first_half_frees_enough() {
        let entries = vec![
            entry("call", ContextClass::ToolCall, 5000, 1, Some("p")),
            entry("result", ContextClass::ToolResult, 1000, 1, Some("p")),
        ];
        let snapshot = ContextSnapshot {
            entries: &entries,
            budget: budget(),
            used_tokens: 6000,
            economic_threshold: None,
        };
        let plan = GreedyCompactionPolicy::default().plan(&snapshot).unwrap();
        assert_eq!(plan.actions.len(), 2);
        assert!(GreedyCompactionPolicy::check_pairs(&plan, &snapshot));
    }

    #[test]
    fn mandatory_pair_and_insufficient_compaction_return_a_budget_error() {
        let entries = vec![
            entry("sys", ContextClass::Instruction, 5500, 9, Some("p")),
            entry("res", ContextClass::ToolResult, 500, 1, Some("p")),
        ];
        let snapshot = ContextSnapshot {
            entries: &entries,
            budget: budget(),
            used_tokens: 6000,
            economic_threshold: None,
        };
        assert!(GreedyCompactionPolicy::default().plan(&snapshot).is_err());
    }
}
