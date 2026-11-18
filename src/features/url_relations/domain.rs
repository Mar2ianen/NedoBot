//! Pure, fail-closed classification of parsed URL relations.
use super::*;

pub(super) fn classify_qr_url(
    target: &str,
    chat_id: i64,
    affiliated_channels: &HashSet<i64>,
) -> QrUrlRelation {
    match parse_telegram_link(target) {
        Some(TelegramLink::Message {
            peer: TelegramPeer::ChatId(id),
            ..
        }) if id == chat_id || affiliated_channels.contains(&id) => QrUrlRelation::Affiliated,
        Some(TelegramLink::Message {
            peer: TelegramPeer::ChatId(_),
            ..
        }) => QrUrlRelation::External,
        Some(_) => QrUrlRelation::Unresolved,
        None if teloxide_url::parse_url(target)
            .is_some_and(|url| matches!(url.scheme(), "http" | "https")) =>
        {
            QrUrlRelation::External
        }
        None => QrUrlRelation::Unresolved,
    }
}

pub(super) fn relation(link: &TelegramLink, context: &RelationContext<'_>) -> LinkRelation {
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
