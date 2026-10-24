//! The only crate the Swift app sees. Exposes `Engine` through UniFFI.
//!
//! Rules (see docs/SPEC.md §4): Swift sends coarse commands tagged with a generation
//! number and fetches visible rows in windows. Work happens on a background worker and
//! completion is reported through `EngineListener`. Results for stale generations are
//! dropped.

mod engine;
mod views;

pub use engine::{Engine, EngineListener};
pub use views::{Category, HighlightRange, RowKind, RowView};

uniffi::setup_scaffolding!();
