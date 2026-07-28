//! RMCP tool contracts and transport-neutral tool implementations.

pub mod catalog;
pub mod chat;
pub mod db;
pub mod semantic;

use rmcp::ErrorData;

pub(crate) fn invalid_arguments(message: impl Into<String>) -> ErrorData {
    ErrorData::invalid_params(message.into(), None)
}

pub(crate) fn read_error(message: &'static str) -> ErrorData {
    ErrorData::internal_error(message, None)
}
