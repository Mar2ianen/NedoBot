use super::Handler;

pub(super) fn append(handler: Handler, callback: Handler) -> Handler {
    handler.branch(callback)
}
