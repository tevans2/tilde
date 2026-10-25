//! The only crate the Swift app sees. Exposes `Engine` through UniFFI.
//!
//! Rules (see docs/SPEC.md §4): Swift sends coarse commands tagged with a generation
//! number and fetches visible rows in windows. Work happens on a background worker and
//! completion is reported through `EngineListener`. Results for stale generations are
//! dropped.

mod config;
mod edit;
mod engine;
mod index;
mod nvim;
mod ops;
mod preview;
mod views;

pub use config::{AppConfig, ConfigError, ConfigLoad, ThemeAppearance, ensure_config_file, load_config};
pub use edit::{EditLineError, EditPreview, EditSession, PendingChange};
pub use engine::{ActivityFilter, ActivityInfo, Engine, EngineListener, OriginFilter, ResultsInfo, RowGroup};
pub use index::{Destination, FolderInfo, IndexInfo, IndexService, IndexState, IndexStatus};
pub use nvim::{NvimEditor, NvimLaunch, NvimListener, NvimOpenMode};
pub use ops::{FileOps, OpError, OpKind, OpOutcome, OpPhase, OpRequest, OpsListener};
pub use preview::{TextPreview, TextSpan, bat_installed, bat_preview};
pub use views::{ActivityOrigin, Category, HighlightRange, RowKind, RowView, Section};

uniffi::setup_scaffolding!();
