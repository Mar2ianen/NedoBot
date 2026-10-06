//! Планировщик tool-вызовов (§16 спеки).
//!
//! Параллелить можно только независимые вызовы. Планировщик строит
//! пакеты: внутри пакета вызовы идут параллельно, пакеты — строго
//! последовательно. Исполнение остаётся на владельце runtime.

use serde::{Deserialize, Serialize};

/// Допустимая конкурентность вызова.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolConcurrency {
    /// Безопасен параллельно с чем угодно.
    ParallelSafe,
    /// Требует отдельного пакета.
    Exclusive,
    /// Сериализуется только внутри своего ресурса.
    ResourceScoped(String),
}

/// Задача на исполнение.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTask {
    pub id: String,
    pub concurrency: ToolConcurrency,
}

/// Разбивает задачи на последовательные пакеты параллельного исполнения.
///
/// Правила:
/// - `Exclusive` всегда один в пакете;
/// - два `ResourceScoped` с тем же ключом никогда в одном пакете;
/// - в пакете не больше `max_parallel` задач.
pub fn schedule_batches(tasks: &[ToolTask], max_parallel: usize) -> Vec<Vec<usize>> {
    let width = max_parallel.max(1);
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut current_resources: std::collections::HashSet<&str> = std::collections::HashSet::new();

    let flush = |batches: &mut Vec<Vec<usize>>,
                 current: &mut Vec<usize>,
                 resources: &mut std::collections::HashSet<&str>| {
        if !current.is_empty() {
            batches.push(std::mem::take(current));
            resources.clear();
        }
    };

    for (index, task) in tasks.iter().enumerate() {
        match &task.concurrency {
            ToolConcurrency::Exclusive => {
                flush(&mut batches, &mut current, &mut current_resources);
                batches.push(vec![index]);
            }
            ToolConcurrency::ResourceScoped(key) => {
                if current.len() >= width || current_resources.contains(key.as_str()) {
                    flush(&mut batches, &mut current, &mut current_resources);
                }
                current.push(index);
                current_resources.insert(key.as_str());
            }
            ToolConcurrency::ParallelSafe => {
                if current.len() >= width {
                    flush(&mut batches, &mut current, &mut current_resources);
                }
                current.push(index);
            }
        }
    }
    flush(&mut batches, &mut current, &mut current_resources);
    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, concurrency: ToolConcurrency) -> ToolTask {
        ToolTask {
            id: id.to_owned(),
            concurrency,
        }
    }

    #[test]
    fn parallel_reads_share_batch() {
        let tasks = vec![
            task("a", ToolConcurrency::ParallelSafe),
            task("b", ToolConcurrency::ParallelSafe),
        ];
        assert_eq!(schedule_batches(&tasks, 8), vec![vec![0, 1]]);
    }

    #[test]
    fn exclusive_splits_batches() {
        let tasks = vec![
            task("a", ToolConcurrency::ParallelSafe),
            task("fmt", ToolConcurrency::Exclusive),
            task("b", ToolConcurrency::ParallelSafe),
        ];
        assert_eq!(schedule_batches(&tasks, 8), vec![vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn same_resource_never_parallel() {
        let tasks = vec![
            task("w1", ToolConcurrency::ResourceScoped("file-a".to_owned())),
            task("w2", ToolConcurrency::ResourceScoped("file-a".to_owned())),
            task("r", ToolConcurrency::ResourceScoped("file-b".to_owned())),
        ];
        let batches = schedule_batches(&tasks, 8);
        // w1 и w2 обязаны оказаться в разных пакетах.
        let pos = |id: usize| {
            batches
                .iter()
                .position(|batch| batch.contains(&id))
                .unwrap()
        };
        assert_ne!(pos(0), pos(1));
        // А w2 и r могут идти вместе.
        assert_eq!(pos(1), pos(2));
    }

    #[test]
    fn width_is_respected() {
        let tasks = vec![
            task("a", ToolConcurrency::ParallelSafe),
            task("b", ToolConcurrency::ParallelSafe),
            task("c", ToolConcurrency::ParallelSafe),
        ];
        assert_eq!(schedule_batches(&tasks, 2), vec![vec![0, 1], vec![2]]);
    }
}
