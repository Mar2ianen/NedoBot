//! Осторожные связи для уже разобранных ссылок Telegram.
//! Проверка членства по сети и разрешение username выполняются вне модуля.

use std::collections::HashSet;

use teloxide_url::{TelegramLink, TelegramPeer};

use crate::{config::Config, db::membership::MembershipContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRelation {
    CurrentMember,
    FormerMember,
    BannedMember,
    AffiliatedChannel,
    KnownExternal,
    Unresolved,
}

pub struct RelationContext<'a> {
    pub chat_id: i64,
    /// ID из настроенных маршрутов первых комментариев и отдельно
    /// подтверждённых через Bot API значений `linked_chat_id`.
    pub affiliated_channels: &'a HashSet<i64>,
    /// Членство учитывается только для совпадающего явного числового ID.
    pub user_membership: Option<(u64, MembershipContext)>,
}

/// Настроенные пары чата обсуждений и канала известны без вызова Bot API.
/// Связанные чаты вызывающая сторона добавляет только после проверки API.
pub fn configured_affiliated_channels(config: &Config, chat_id: i64) -> HashSet<i64> {
    config
        .first_comment_route_for_chat(chat_id)
        .map(|route| route.source_channel_id)
        .collect()
}

pub fn relation(link: &TelegramLink, context: &RelationContext<'_>) -> LinkRelation {
    match link {
        TelegramLink::UserId(id) => {
            let Some((known_id, membership)) = context.user_membership else {
                return LinkRelation::Unresolved;
            };
            if *id != known_id {
                return LinkRelation::Unresolved;
            }
            match membership {
                MembershipContext::CurrentMember => LinkRelation::CurrentMember,
                MembershipContext::FormerMember => LinkRelation::FormerMember,
                MembershipContext::Banned => LinkRelation::BannedMember,
                MembershipContext::Unknown | MembershipContext::NonMemberHistoryUnknown => {
                    LinkRelation::Unresolved
                }
            }
        }
        TelegramLink::Message {
            peer: TelegramPeer::ChatId(id),
            ..
        } => {
            if *id == context.chat_id || context.affiliated_channels.contains(id) {
                LinkRelation::AffiliatedChannel
            } else {
                LinkRelation::KnownExternal
            }
        }
        _ => LinkRelation::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_and_invites_never_gain_guessed_identity() {
        let affiliated = HashSet::new();
        let context = RelationContext {
            chat_id: -1001,
            affiliated_channels: &affiliated,
            user_membership: None,
        };
        assert_eq!(
            relation(
                &TelegramLink::Profile {
                    username: "alice".into()
                },
                &context
            ),
            LinkRelation::Unresolved
        );
        assert_eq!(
            relation(
                &TelegramLink::Invite {
                    hash: "SYNTHETIC".into()
                },
                &context
            ),
            LinkRelation::Unresolved
        );
    }

    #[test]
    fn only_configured_numeric_channel_ids_are_affiliated() {
        let affiliated = HashSet::from([-1002]);
        let context = RelationContext {
            chat_id: -1001,
            affiliated_channels: &affiliated,
            user_membership: None,
        };
        assert_eq!(
            relation(
                &TelegramLink::Message {
                    peer: TelegramPeer::ChatId(-1002),
                    message_id: 1,
                    topic_id: None,
                },
                &context
            ),
            LinkRelation::AffiliatedChannel
        );
        assert_eq!(
            relation(
                &TelegramLink::Message {
                    peer: TelegramPeer::ChatId(-1003),
                    message_id: 1,
                    topic_id: None,
                },
                &context
            ),
            LinkRelation::KnownExternal
        );
    }

    #[test]
    fn numeric_user_relation_requires_matching_known_membership() {
        let affiliated = HashSet::new();
        let context = RelationContext {
            chat_id: -1001,
            affiliated_channels: &affiliated,
            user_membership: Some((42, MembershipContext::FormerMember)),
        };
        assert_eq!(
            relation(&TelegramLink::UserId(42), &context),
            LinkRelation::FormerMember
        );
        assert_eq!(
            relation(&TelegramLink::UserId(43), &context),
            LinkRelation::Unresolved
        );
    }
}
