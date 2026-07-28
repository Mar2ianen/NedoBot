//! Legacy MCP transport adapters.
//!
//! Protocol lifecycle and listener setup live here; `features::chat_read_api`
//! remains transport-neutral.

pub mod http;
pub mod server;
pub mod stdio;
pub mod tools;
