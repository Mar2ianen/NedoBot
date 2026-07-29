pub mod claim;
// The production binary compiles its own module tree; this read-model is used by
// the standalone report binary through the library crate.
#[allow(dead_code)]
pub mod observability;
pub mod policy;
