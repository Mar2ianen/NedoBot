use teloxide::{dispatching::UpdateFilterExt, types::Update};

use super::Handler;

/// Preserve the user-visible command precedence over ordinary message ingress.
pub(super) fn command_first(command_handler: Handler, message_handler: Handler) -> Handler {
    teloxide::dptree::entry().branch(
        Update::filter_message()
            .branch(command_handler)
            .branch(message_handler),
    )
}
