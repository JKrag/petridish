pub mod cli;
pub mod config;
pub mod discovery;
pub mod events;
pub mod git;
pub mod scan;
pub mod sensors;

// `schema` moved to `petridish-core` (ADR-0002) so `petri` can share the wire
// types without depending on `swab`. Re-exported under the old path so every
// existing `crate::schema::...` reference in this crate keeps working unchanged.
pub use petridish_core::schema;
