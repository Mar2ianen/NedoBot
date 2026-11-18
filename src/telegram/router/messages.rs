use super::Handler;

/// Edited-message updates are routed independently from ordinary messages.
pub(super) fn append_edited(handler: Handler, edited_message: Handler) -> Handler {
    handler.branch(edited_message)
}
