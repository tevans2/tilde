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
mod sorts;
mod views;

pub use config::{
    ActionKeys, AppConfig, ConfigError, ConfigLoad, HotkeyInfo, KeyChord, KeymapInfo, SettingValue, Settings,
    ThemeAppearance, ensure_config_file, load_config, load_keymap, load_settings, parse_hotkey, reset_setting,
    set_setting,
};
pub use edit::{EditLineError, EditPreview, EditSession, PendingChange};
pub use engine::{
    ActivityFilter, ActivityInfo, Engine, EngineListener, FolderSort, OriginFilter, ResultsInfo, RowGroup, SortBy,
};
pub use index::{Destination, FolderInfo, IndexInfo, IndexService, IndexState, IndexStatus};
pub use nvim::{NvimEditor, NvimLaunch, NvimListener, NvimOpenMode, NvimStatus, find_nvim};
pub use ops::{
    ClashPolicy, FileOps, HistoryEntry, HistoryPreview, HistoryState, OpError, OpKind, OpOutcome, OpPhase, OpRequest,
    OpsListener, common_tags,
};
pub use preview::{TextPreview, TextSpan, bat_installed, bat_preview};
pub use views::{ActivityOrigin, Category, HighlightRange, RowKind, RowView, Section};

uniffi::setup_scaffolding!();
