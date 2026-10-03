//! Политика автоматической модерации по score.
//!
//! Лестница из двух ступеней: удаление сообщений сохраняет review-карточку
//! (улики уже зафиксированы в аудите), бан применяется только выше высокого
//! порога. Чистая функция для unit-тестов и переиспользования между
//! инстансами; исполнение (Bot API, идемпотентность) остаётся в боте.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoPolicy {
    /// Score review-карточки (обычно `review_threshold` риск-профиля).
    pub review_threshold: i32,
    /// Score бана с удалением сообщений.
    pub ban_threshold: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoAction {
    /// Ниже review-порога: ничего не делать.
    None,
    /// Review-порог и выше, но ниже бана: удалить первое сообщение,
    /// review-карточка идёт обычным путём.
    DeleteMessages,
    /// Порог бана и выше: бан + удаление недавних сообщений + System-метка.
    Ban,
}

pub fn decide_auto_action(score: i32, policy: &AutoPolicy) -> AutoAction {
    if score >= policy.ban_threshold {
        AutoAction::Ban
    } else if score >= policy.review_threshold {
        AutoAction::DeleteMessages
    } else {
        AutoAction::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> AutoPolicy {
        AutoPolicy {
            review_threshold: 70,
            ban_threshold: 90,
        }
    }

    #[test]
    fn low_score_takes_no_action() {
        assert_eq!(decide_auto_action(69, &policy()), AutoAction::None);
    }

    #[test]
    fn review_band_deletes_messages() {
        assert_eq!(
            decide_auto_action(70, &policy()),
            AutoAction::DeleteMessages
        );
        assert_eq!(
            decide_auto_action(89, &policy()),
            AutoAction::DeleteMessages
        );
    }

    #[test]
    fn ban_threshold_bans() {
        assert_eq!(decide_auto_action(90, &policy()), AutoAction::Ban);
        assert_eq!(decide_auto_action(100, &policy()), AutoAction::Ban);
    }
}
