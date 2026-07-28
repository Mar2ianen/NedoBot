//! MCP transport adapters.
//!
//! Protocol lifecycle and listener setup live here; `features::chat_read_api`
//! remains transport-neutral.

pub mod bootstrap;
pub mod http;
pub mod rmcp_stdio;
pub mod server;
pub mod stdio;
pub mod tools;
