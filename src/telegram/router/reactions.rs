use super::Handler;

pub(super) fn append(handler: Handler, reaction: Handler, reaction_count: Handler) -> Handler {
    handler.branch(reaction).branch(reaction_count)
}
