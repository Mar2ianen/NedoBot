use super::Handler;

pub(super) fn append(handler: Handler, chat_member: Handler) -> Handler {
    handler.branch(chat_member)
}
