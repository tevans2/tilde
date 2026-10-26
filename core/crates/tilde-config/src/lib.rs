//! User configuration from `~/.config/tilde/config.toml` (or `$XDG_CONFIG_HOME/tilde/`).
//!
//! A missing file means defaults. A broken file also means defaults, plus an error
//! message for the UI, so a typo never stops Tilde from starting.

mod keys;
mod theme;

pub use keys::{
    ActionSpec, Binding, Chord, GO_ACTIONS, Hotkey, KeyList, KeyMode, Keymap, KeysConfig, NAMED_KEYS, SELECT_ACTIONS,
    actions, parse_chord, parse_hotkey,
};
pub use theme::{Appearance, COLOR_KEYS, PRESETS, ResolvedTheme, ThemeConfig, parse_hex, preset_bat_themes};

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_FONT_FAMILY: &str = "JetBrains Mono";
pub const DEFAULT_FONT_SIZE: f64 = 13.0;
const FONT_SIZE_RANGE: std::ops::RangeInclusive<f64> = 8.0..=32.0;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub font: FontConfig,
    pub theme: ThemeConfig,
    pub preview: PreviewConfig,
    pub index: IndexConfig,
    pub frecency: FrecencyConfig,
    /// Before Tilde kept its own frecency. Still read: `add_on_visit` means `tell_zoxide`.
    pub zoxide: ZoxideConfig,
    pub activity: ActivityConfig,
    pub edit: EditConfig,
    pub keys: KeysConfig,
    pub sort: SortConfig,
    pub dialog: DialogConfig,
    pub cleanup: CleanupConfig,
}

/// The Clean Up page (`C`): what it suggests.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CleanupConfig {
    /// "Big files": at least this big ("500M", "2G")…
    pub big_file: String,
    /// …and not opened or changed for this long ("6mo", "1y", "90d").
    pub big_file_age: String,
    /// "Old downloads": in an inbox folder (Downloads, Desktop) this long.
    pub download_age: String,
    /// "Build output": in projects where nothing has changed for this long.
    pub project_idle: String,
    /// Duplicates smaller than this are ignored.
    pub duplicates_min: String,
    /// Folder names inside projects that hold build output or downloaded dependencies.
    pub build_folders: Vec<String>,
    /// Paths (starting with `~/` or `/`), names, or `*` patterns never suggested.
    pub ignore: Vec<String>,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        CleanupConfig {
            big_file: "500M".into(),
            big_file_age: "6mo".into(),
            download_age: "60d".into(),
            project_idle: "90d".into(),
            duplicates_min: "1M".into(),
            build_folders: [
                "node_modules",
                "target",
                "DerivedData",
                ".venv",
                "venv",
                "build",
                ".gradle",
                "Pods",
                ".next",
                "dist",
            ]
            .map(String::from)
            .to_vec(),
            ignore: Vec::new(),
        }
    }
}

/// `CleanupConfig` in bytes and seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupRules {
    pub big_file: u64,
    pub big_file_age: i64,
    pub download_age: i64,
    pub project_idle: i64,
    pub duplicates_min: u64,
    pub build_folders: Vec<String>,
    pub ignore: Vec<String>,
}

impl CleanupConfig {
    /// The rules, with anything that doesn't parse replaced by its default (and named in
    /// the returned problems).
    pub fn rules(&self) -> (CleanupRules, Vec<String>) {
        let d = CleanupConfig::default();
        let mut problems = Vec::new();
        let mut size = |key: &str, v: &str, default: &str| {
            parse_size(v).unwrap_or_else(|| {
                problems.push(format!("cleanup.{key} “{v}” isn't a size like 500M or 2G"));
                parse_size(default).unwrap()
            })
        };
        let big_file = size("big_file", &self.big_file, &d.big_file);
        let duplicates_min = size("duplicates_min", &self.duplicates_min, &d.duplicates_min);
        let mut age = |key: &str, v: &str, default: &str| {
            parse_age(v).unwrap_or_else(|| {
                problems.push(format!("cleanup.{key} “{v}” isn't an age like 60d, 6mo or 1y"));
                parse_age(default).unwrap()
            })
        };
        let rules = CleanupRules {
            big_file,
            big_file_age: age("big_file_age", &self.big_file_age, &d.big_file_age),
            download_age: age("download_age", &self.download_age, &d.download_age),
            project_idle: age("project_idle", &self.project_idle, &d.project_idle),
            duplicates_min,
            build_folders: self.build_folders.clone(),
            ignore: self.ignore.clone(),
        };
        (rules, problems)
    }
}

/// "500M", "2G", "1.5T", "800K", "1000" (bytes), in decimal units as the Finder shows them.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_uppercase();
    let s = s.strip_suffix('B').unwrap_or(&s);
    let (num, mult) = match s.chars().last()? {
        'K' => (&s[..s.len() - 1], 1e3),
        'M' => (&s[..s.len() - 1], 1e6),
        'G' => (&s[..s.len() - 1], 1e9),
        'T' => (&s[..s.len() - 1], 1e12),
        _ => (s, 1.0),
    };
    let n: f64 = num.trim().parse().ok()?;
    (n >= 0.0).then_some((n * mult) as u64)
}

/// "60d", "12w", "6mo", "1y", "36h" in seconds (a month is 30 days, a year 365).
pub fn parse_age(s: &str) -> Option<i64> {
    let s = s.trim().to_lowercase();
    let split = s.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let unit_secs = match unit.trim() {
        "h" => 3_600.0,
        "d" => 86_400.0,
        "w" => 7.0 * 86_400.0,
        "mo" | "m" => 30.0 * 86_400.0,
        "y" => 365.0 * 86_400.0,
        _ => return None,
    };
    (n >= 0.0).then_some((n * unit_secs) as i64)
}

/// The file-dialog helper: a global hotkey that opens Tilde's search over another app's
/// Open or Save dialog and takes the dialog to what's picked.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DialogConfig {
    /// Needs the Accessibility permission (to see the dialog and type into it).
    pub enabled: bool,
    pub hotkey: String,
    /// After picking a file in an Open dialog, press Open too.
    pub confirm: bool,
}

impl Default for DialogConfig {
    fn default() -> Self {
        DialogConfig { enabled: false, hotkey: "ctrl+alt+space".into(), confirm: false }
    }
}

/// How folders are sorted unless one has its own order (`s` in SELECT).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SortConfig {
    /// `name`, `modified`, `added`, `size` or `kind`.
    pub by: String,
    /// Against the natural direction (Z→A, oldest first, smallest first).
    pub reverse: bool,
    pub folders_first: bool,
    /// Remember the order chosen for each folder.
    pub remember: bool,
}

impl Default for SortConfig {
    fn default() -> Self {
        SortConfig { by: "name".into(), reverse: false, folders_first: true, remember: true }
    }
}

impl SortConfig {
    /// The default order (an unknown `by` is reported by `load` and means name).
    pub fn order(&self) -> tilde_types::SortOrder {
        tilde_types::SortOrder {
            key: tilde_types::SortKey::parse(&self.by).unwrap_or_default(),
            reverse: self.reverse,
            folders_first: self.folders_first,
        }
    }
}

/// The preview pane on the right.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PreviewConfig {
    pub visible: bool,
    /// Share of the window's width (0.15–0.85).
    pub width: f64,
    /// Share of the window's width in PREVIEW (`p`), 0.3–0.95. `f` there takes the whole
    /// window.
    pub mode_width: f64,
    /// Render code and other text files with `bat` (if installed). Markdown and
    /// everything else use Quick Look.
    pub bat: bool,
    /// bat themes (`bat --list-themes`) for light and dark appearance. Empty picks one to
    /// match the theme preset, else (dark) your bat config or (light) "GitHub".
    pub bat_theme_light: String,
    pub bat_theme_dark: String,
    /// Lines rendered by bat.
    pub max_lines: u32,
}

const PREVIEW_WIDTH_RANGE: std::ops::RangeInclusive<f64> = 0.15..=0.85;
const PREVIEW_MODE_WIDTH_RANGE: std::ops::RangeInclusive<f64> = 0.3..=0.95;

impl Config {
    /// The bat themes to use: (light, dark). An empty dark theme means "bat's own config".
    pub fn bat_themes(&self) -> (String, String) {
        let preset = preset_bat_themes(&self.theme.preset);
        let pick = |set: &str, from_preset: Option<&str>, fallback: &str| {
            if !set.trim().is_empty() { set.trim().to_string() } else { from_preset.unwrap_or(fallback).to_string() }
        };
        (
            pick(&self.preview.bat_theme_light, preset.map(|p| p.0), "GitHub"),
            pick(&self.preview.bat_theme_dark, preset.map(|p| p.1), ""),
        )
    }
}

impl Default for PreviewConfig {
    fn default() -> Self {
        PreviewConfig {
            visible: true,
            width: 0.5,
            mode_width: 0.75,
            bat: true,
            bat_theme_light: String::new(),
            bat_theme_dark: String::new(),
            max_lines: 1000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    /// Font family name. The UI falls back to the closest installed family, then to the
    /// system monospaced font.
    pub family: String,
    /// Base size in points for the file list. Other text is sized relative to it.
    pub size: f64,
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig { family: DEFAULT_FONT_FAMILY.into(), size: DEFAULT_FONT_SIZE }
    }
}

/// What the Everywhere search indexes.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexConfig {
    pub enabled: bool,
    /// Folders to index. `~` is the home folder.
    pub roots: Vec<String>,
    /// Folder names (any depth) or paths (starting with `~/` or `/`) whose contents are
    /// skipped. The folders themselves are still indexed.
    pub exclude: Vec<String>,
    /// Names, `*`/`?` patterns, or paths of files and folders left out entirely.
    pub hide: Vec<String>,
    pub include_hidden: bool,
    /// Indexing stops at this many items.
    pub max_items: u64,
}

pub const DEFAULT_MAX_ITEMS: u64 = 1_000_000;

impl Default for IndexConfig {
    fn default() -> Self {
        IndexConfig {
            enabled: true,
            roots: vec!["~".into()],
            exclude: [
                "node_modules",
                ".git",
                "target",
                "build",
                "DerivedData",
                "Pods",
                "__pycache__",
                ".venv",
                "venv",
                "~/Library",
                "~/.Trash",
            ]
            .map(String::from)
            .to_vec(),
            hide: Vec::new(),
            include_hidden: false,
            max_items: DEFAULT_MAX_ITEMS,
        }
    }
}

/// `roots` and `exclude` with `~` expanded and exclusions split into names and paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIndex {
    pub roots: Vec<PathBuf>,
    pub exclude_names: Vec<String>,
    pub exclude_paths: Vec<PathBuf>,
    /// Exact names and `*`/`?` patterns.
    pub hide_names: Vec<String>,
    pub hide_paths: Vec<PathBuf>,
}

impl IndexConfig {
    pub fn resolve(&self, home: &Path) -> ResolvedIndex {
        let expand = |s: &str| expand_path(home, s);
        let is_path = |e: &&String| e.starts_with('/') || e.starts_with("~/");
        let (paths, names): (Vec<&String>, Vec<&String>) = self.exclude.iter().partition(is_path);
        let (hide_paths, hide_names): (Vec<&String>, Vec<&String>) = self.hide.iter().partition(is_path);
        ResolvedIndex {
            roots: self.roots.iter().map(|r| expand(r)).collect(),
            exclude_names: names.into_iter().cloned().collect(),
            exclude_paths: paths.into_iter().map(|p| expand(p)).collect(),
            hide_names: hide_names.into_iter().cloned().collect(),
            hide_paths: hide_paths.into_iter().map(|p| expand(p)).collect(),
        }
    }
}

/// `~` and `~/…` against `home`; anything else as written.
fn expand_path(home: &Path, s: &str) -> PathBuf {
    match s.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(s),
    }
}

/// Ranking by use: how often and how recently folders and files are used in Tilde.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FrecencyConfig {
    /// Rank search results and the folder picker by use.
    pub enabled: bool,
    /// Add zoxide's scores (folders used in the terminal), if it's installed.
    pub zoxide: bool,
    /// Also record folder visits with zoxide, so `z` in the shell learns from Tilde.
    pub tell_zoxide: bool,
}

impl Default for FrecencyConfig {
    fn default() -> Self {
        FrecencyConfig { enabled: true, zoxide: true, tell_zoxide: false }
    }
}

/// The old `[zoxide]` section, still accepted.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ZoxideConfig {
    /// Now `[frecency] zoxide`.
    pub enabled: Option<bool>,
    pub add_on_visit: Option<bool>,
}

impl Config {
    /// Whether zoxide's scores count in the ranking.
    pub fn use_zoxide(&self) -> bool {
        self.zoxide.enabled.unwrap_or(self.frecency.zoxide)
    }

    /// Whether folder visits are also recorded with zoxide.
    pub fn tell_zoxide(&self) -> bool {
        self.zoxide.add_on_visit.unwrap_or(self.frecency.tell_zoxide)
    }
}

/// The Activity view (`@`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ActivityConfig {
    /// How far back the feed goes.
    pub days: u32,
    /// Folders where new things land until they're filed. The screenshot folder is
    /// always included.
    pub inbox: Vec<String>,
}

impl Default for ActivityConfig {
    fn default() -> Self {
        ActivityConfig { days: 30, inbox: vec!["~/Downloads".into(), "~/Desktop".into()] }
    }
}

impl ActivityConfig {
    pub fn inbox_paths(&self, home: &Path) -> Vec<PathBuf> {
        self.inbox.iter().map(|p| expand_path(home, p)).collect()
    }
}

/// EDIT mode (`e`, `r`, `o`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditConfig {
    pub editor: EditorKind,
    /// Path to nvim. Empty looks on the login shell's PATH, then in Homebrew's folders.
    pub nvim_path: String,
    pub nvim_config: NvimConfigMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditorKind {
    #[default]
    Builtin,
    Nvim,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NvimConfigMode {
    /// The user's own nvim config.
    #[default]
    User,
    /// `nvim --clean`, plus `nvim_init_path()` if it exists.
    Clean,
}

impl Default for EditConfig {
    fn default() -> Self {
        EditConfig { editor: EditorKind::Builtin, nvim_path: String::new(), nvim_config: NvimConfigMode::User }
    }
}

/// Tilde's own nvim config for `nvim_config = "clean"`: `init.lua` next to config.toml.
pub fn nvim_init_path() -> PathBuf {
    config_path().with_file_name("init.lua")
}

/// The result of reading the config file.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub config: Config,
    /// Why the file (or part of it) was ignored, if it was.
    pub error: Option<String>,
}

/// `$XDG_CONFIG_HOME/tilde/config.toml`, else `~/.config/tilde/config.toml`.
pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("tilde").join("config.toml")
}

pub fn load(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded { config: Config::default(), error: None };
        }
        Err(e) => return Loaded { config: Config::default(), error: Some(format!("couldn't read config: {e}")) },
    };
    match toml::from_str::<Config>(&text) {
        Ok(mut config) => {
            let mut error = None;
            if !FONT_SIZE_RANGE.contains(&config.font.size) {
                error = Some(format!(
                    "font.size {} is outside {}–{}; using {}",
                    config.font.size,
                    FONT_SIZE_RANGE.start(),
                    FONT_SIZE_RANGE.end(),
                    DEFAULT_FONT_SIZE
                ));
                config.font.size = DEFAULT_FONT_SIZE;
            }
            if config.font.family.trim().is_empty() {
                config.font.family = DEFAULT_FONT_FAMILY.into();
            }
            if !PREVIEW_WIDTH_RANGE.contains(&config.preview.width) {
                let msg = format!(
                    "preview.width {} is outside {}–{}; using {}",
                    config.preview.width,
                    PREVIEW_WIDTH_RANGE.start(),
                    PREVIEW_WIDTH_RANGE.end(),
                    PreviewConfig::default().width
                );
                error = Some(error.map_or(msg.clone(), |e| format!("{e}; {msg}")));
                config.preview.width = PreviewConfig::default().width;
            }
            if !PREVIEW_MODE_WIDTH_RANGE.contains(&config.preview.mode_width) {
                let msg = format!(
                    "preview.mode_width {} is outside {}–{}; using {}",
                    config.preview.mode_width,
                    PREVIEW_MODE_WIDTH_RANGE.start(),
                    PREVIEW_MODE_WIDTH_RANGE.end(),
                    PreviewConfig::default().mode_width
                );
                error = Some(error.map_or(msg.clone(), |e| format!("{e}; {msg}")));
                config.preview.mode_width = PreviewConfig::default().mode_width;
            }
            config.preview.max_lines = config.preview.max_lines.clamp(1, 100_000);
            config.activity.days = config.activity.days.clamp(1, 3650);
            let (_, mut problems) = config.theme.resolve();
            problems.extend(config.cleanup.rules().1);
            if let Err(e) = parse_hotkey(&config.dialog.hotkey) {
                problems.push(format!("dialog.hotkey: {e}"));
            }
            if tilde_types::SortKey::parse(&config.sort.by).is_none() {
                problems.push(format!(
                    "sort.by “{}” isn't one of name, modified, added, size, kind; using name",
                    config.sort.by
                ));
            }
            problems.extend(config.keys.resolve().warnings);
            if !problems.is_empty() {
                let msg = problems.join("; ");
                error = Some(error.map_or(msg.clone(), |e| format!("{e}; {msg}")));
            }
            Loaded { config, error }
        }
        Err(e) => Loaded {
            config: Config::default(),
            error: Some(format!("config.toml has an error, using defaults: {}", e.message())),
        },
    }
}

/// Written when the user asks to open a config file that doesn't exist yet.
pub const TEMPLATE: &str = r##"# Tilde configuration. Reload with ⌘R after saving.

[font]
# Any installed font family. If there's no exact match, the closest installed family
# is used (so "JetBrains Mono" also finds "JetBrainsMono Nerd Font").
family = "JetBrains Mono"
# Base size in points for the file list (8–32). Other text is sized relative to it.
size = 13

[theme]
# system, light or dark.
appearance = "system"
# default (macOS colours), catppuccin, gruvbox, nord or solarized. Each has light and
# dark variants that follow the appearance.
preset = "default"

# Override any colour for both appearances (#RRGGBB or #RRGGBBAA):
#   background bar border text text_dim selection selection_text match match_selected
#   go select shelf badge_label badge_folder badge_app badge_image badge_video
#   badge_audio badge_pdf badge_document badge_code badge_text badge_archive badge_other
[theme.colors]
# match = "#ff9e64"

# Or for one appearance only.
[theme.light]
[theme.dark]
# background = "#101014"

[preview]
visible = true
# Share of the window the preview takes (0.15–0.85).
width = 0.5
# Share it takes in preview mode (p), 0.3–0.95; f there takes the whole window.
mode_width = 0.75
# Syntax-highlight code and other text files with bat (if installed). Markdown and
# everything else use Quick Look.
bat = true
# bat themes (from `bat --list-themes`) for light and dark appearance. Empty matches
# the theme preset, else uses your bat config (dark) or "GitHub" (light).
bat_theme_light = ""
bat_theme_dark = ""
# Lines rendered by bat.
max_lines = 1000

# What searching Everywhere covers. Apply changes with Rebuild in the Index window (⌘⇧I).
[index]
enabled = true
roots = ["~"]
# Folder names (at any depth) or paths (starting with ~/ or /) whose contents are
# skipped. The folders themselves still show up in results.
exclude = [
  "node_modules", ".git", "target", "build", "DerivedData", "Pods",
  "__pycache__", ".venv", "venv", "~/Library", "~/.Trash",
]
# Files and folders left out entirely: names, patterns with * and ?, or paths.
# For example: ["*.log", "*.pyc", "~/Downloads/huge-dataset"]
hide = []
include_hidden = false
# Indexing stops at this many items; the Index window shows what to exclude.
max_items = 1000000

[frecency]
# Rank search results and the folder picker by how often and how recently you use
# folders and files in Tilde (scores halve every 10 days without use).
enabled = true
# Also count zoxide's scores (folders you use in the terminal), if it's installed.
zoxide = true
# Also tell zoxide about folders you open in Tilde, so `z` in your shell learns from Tilde.
tell_zoxide = false

# The Activity view (@): recent downloads, screenshots, AirDrops and edits.
[activity]
# How far back the feed goes.
days = 30
# Folders where new things land until you file them (the inbox, i). The screenshot
# folder is always included.
inbox = ["~/Downloads", "~/Desktop"]

# Editing a folder as text (e, r, o).
[edit]
# "builtin" or "nvim". Neovim runs inside Tilde; :w applies, :q discards.
editor = "builtin"
# Path to nvim. Empty looks on your login shell's PATH, then in Homebrew's folders.
nvim_path = ""
# "user" loads your nvim config. "clean" skips it and loads init.lua next to this file,
# if there is one.
nvim_config = "user"

# How folders are sorted. `s` in SELECT picks another order for the folder you're in,
# which is remembered for that folder.
[sort]
# name, modified, added (when it arrived in the folder), size or kind (by extension).
by = "name"
# Against the natural direction: Z→A, oldest first, smallest first.
reverse = false
folders_first = true
# Remember the order chosen for each folder.
remember = true

# The file-dialog helper: in any app's Open or Save dialog, press the hotkey to search
# with Tilde, and the dialog goes to what you pick. Needs the Accessibility permission
# (Settings › General shows it).
[dialog]
enabled = false
# Works while other apps are in front, so it needs ctrl, alt or cmd.
hotkey = "ctrl+alt+space"
# After picking a file in an Open dialog, press Open too.
confirm = false

# The Clean Up page (C): suggestions for freeing space. Everything it moves goes to the
# Trash and can be undone (History); emptying the Trash asks first.
[cleanup]
# Big files: at least this big, and not opened or changed for this long.
big_file = "500M"
big_file_age = "6mo"
# Old downloads: still in Downloads or on the Desktop after this long.
download_age = "60d"
# Build output (node_modules, target, …) in projects untouched for this long.
project_idle = "90d"
# Duplicates smaller than this are left out.
duplicates_min = "1M"
build_folders = [
  "node_modules", "target", "DerivedData", ".venv", "venv", "build", ".gradle",
  "Pods", ".next", "dist",
]
# Never suggested: paths (~/… or /…), names, or patterns with *. X on the page adds here.
ignore = []

# Keys for GO (typing in the bar) and SELECT (the list). Give an action a key or a list
# of keys; [] unbinds it. A key is the character it types (j, G, @) or a name (down, up,
# left, right, return, tab, esc, space, backspace, delete, home, end, pageup, pagedown),
# with ctrl+, alt+ or shift+ (names only) in front. GO keys need ctrl+ or alt+ unless
# they're named keys, since plain characters are typed into the bar.
# Settings (⌘,) › Keys lists every action and its current keys.
[keys.go]
# down = ["down", "ctrl+j"]
# enter = ["tab", "ctrl+l"]

[keys.select]
# move = "m"
# tag = "t"
# redo = ["U", "ctrl+r"]
"##;

/// A value for `set_value`.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    List(Vec<String>),
}

/// Sets `section.….key` in the config file to `value`, keeping comments and formatting
/// (including a comment after the old value). Creates the file from the template, and
/// any missing tables, as needed.
pub fn set_value(path: &Path, key_path: &[&str], value: Value) -> std::io::Result<()> {
    let invalid = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidData, m);
    let (key, tables) = key_path.split_last().ok_or_else(|| invalid("empty key".into()))?;
    ensure_file(path)?;
    let text = std::fs::read_to_string(path)?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| invalid(format!("{e}")))?;
    let mut table = doc.as_table_mut();
    for name in tables {
        table = table
            .entry(name)
            .or_insert_with(toml_edit::table)
            .as_table_mut()
            .ok_or_else(|| invalid(format!("{name} is not a table")))?;
    }
    let mut new = match value {
        Value::Bool(b) => toml_edit::Value::from(b),
        Value::Int(i) => toml_edit::Value::from(i),
        Value::Float(f) => toml_edit::Value::from(f),
        Value::Text(t) => toml_edit::Value::from(t),
        Value::List(items) => toml_edit::Value::Array(items.iter().map(String::as_str).collect()),
    };
    match table.get_mut(key) {
        // Replaced in place: the key keeps the comment above it.
        Some(old) => {
            if let Some(v) = old.as_value() {
                *new.decor_mut() = v.decor().clone();
            }
            *old = toml_edit::Item::Value(new);
        }
        None => {
            table.insert(key, toml_edit::Item::Value(new));
        }
    }
    std::fs::write(path, doc.to_string())
}

/// Removes `section.….key` from the config file (so the default applies again), keeping
/// everything else. Missing keys and files are fine.
pub fn remove_value(path: &Path, key_path: &[&str]) -> std::io::Result<()> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(()) };
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    let Some((key, tables)) = key_path.split_last() else { return Ok(()) };
    let mut table = doc.as_table_mut();
    for name in tables {
        match table.get_mut(name).and_then(|t| t.as_table_mut()) {
            Some(t) => table = t,
            None => return Ok(()),
        }
    }
    if table.remove(key).is_some() {
        std::fs::write(path, doc.to_string())?;
    }
    Ok(())
}

/// Adds `entry` to the list at `section.key` (like `cleanup.ignore`), keeping comments.
/// Creates the file, table and list as needed; doesn't add it twice.
pub fn add_to_list(path: &Path, section: &str, key: &str, entry: &str) -> std::io::Result<()> {
    let invalid = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidData, m);
    ensure_file(path)?;
    let text = std::fs::read_to_string(path)?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| invalid(format!("{e}")))?;
    let table = doc
        .entry(section)
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .ok_or_else(|| invalid(format!("[{section}] is not a table")))?;
    if !table.contains_key(key) {
        table.insert(key, toml_edit::value(toml_edit::Array::new()));
    }
    let array = table[key].as_array_mut().ok_or_else(|| invalid(format!("{section}.{key} is not a list")))?;
    if !array.iter().any(|v| v.as_str() == Some(entry)) {
        array.push(entry);
    }
    std::fs::write(path, doc.to_string())
}

/// The two `[index]` lists an entry can be added to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexList {
    Exclude,
    Hide,
}

/// Adds `entry` to `[index].exclude` or `[index].hide` in the config file, keeping its
/// comments and formatting. Creates the file from the template if needed. If the list
/// isn't in the file yet, it starts from the default so the defaults aren't lost.
pub fn add_index_entry(path: &Path, list: IndexList, entry: &str) -> std::io::Result<()> {
    ensure_file(path)?;
    let text = std::fs::read_to_string(path)?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    let index = doc
        .entry("index")
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "[index] is not a table"))?;
    let (key, defaults) = match list {
        IndexList::Exclude => ("exclude", IndexConfig::default().exclude),
        IndexList::Hide => ("hide", IndexConfig::default().hide),
    };
    if !index.contains_key(key) {
        let mut array = toml_edit::Array::new();
        for d in &defaults {
            array.push(d.as_str());
        }
        index.insert(key, toml_edit::value(array));
    }
    let array = index[key]
        .as_array_mut()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("index.{key} is not a list")))?;
    if !array.iter().any(|v| v.as_str() == Some(entry)) {
        array.push(entry);
    }
    std::fs::write(path, doc.to_string())
}

/// Creates the config file from `TEMPLATE` if it doesn't exist.
pub fn ensure_file(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, TEMPLATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_str(s: &str) -> Loaded {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, s).unwrap();
        load(&p)
    }

    #[test]
    fn missing_file_is_defaults_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let l = load(&dir.path().join("nope.toml"));
        assert_eq!(l, Loaded { config: Config::default(), error: None });
        assert_eq!(l.config.font.family, "JetBrains Mono");
    }

    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(load_str(TEMPLATE), Loaded { config: Config::default(), error: None });
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let l = load_str("[font]\nsize = 15\n");
        assert_eq!(l.config.font.size, 15.0);
        assert_eq!(l.config.font.family, DEFAULT_FONT_FAMILY);
        assert!(l.error.is_none());
    }

    #[test]
    fn edit_section_picks_the_editor() {
        let l = load_str("[edit]\neditor = \"nvim\"\nnvim_config = \"clean\"\n");
        assert_eq!(l.config.edit.editor, EditorKind::Nvim);
        assert_eq!(l.config.edit.nvim_config, NvimConfigMode::Clean);
        assert!(load_str("[edit]\neditor = \"emacs\"\n").error.unwrap().contains("emacs"));
    }

    #[test]
    fn errors_fall_back_to_defaults_with_a_message() {
        let l = load_str("[font]\nfamliy = \"Menlo\"\n");
        assert_eq!(l.config, Config::default());
        assert!(l.error.unwrap().contains("famliy"));

        let l = load_str("[font]\nsize = 99\nfamily = \"Menlo\"\n");
        assert_eq!(l.config.font.family, "Menlo");
        assert_eq!(l.config.font.size, DEFAULT_FONT_SIZE);
        assert!(l.error.unwrap().contains("outside"));
    }

    #[test]
    fn theme_problems_are_reported_but_the_rest_loads() {
        let l = load_str("[font]\nsize = 15\n[theme]\npreset = \"nord\"\n[theme.colors]\nmatch = \"orange\"\n");
        assert_eq!(l.config.font.size, 15.0);
        assert_eq!(l.config.theme.preset, "nord");
        assert!(l.error.unwrap().contains("theme.colors.match"));
    }

    #[test]
    fn preview_width_is_checked() {
        let l = load_str("[preview]\nwidth = 0.95\nbat_theme_dark = \"Nord\"\n");
        assert_eq!(l.config.preview.width, 0.5);
        assert_eq!(l.config.bat_themes(), ("GitHub".to_string(), "Nord".to_string()));
        assert!(l.error.unwrap().contains("preview.width"));
    }

    #[test]
    fn cleanup_rules_parse_sizes_and_ages() {
        assert_eq!(parse_size("500M"), Some(500_000_000));
        assert_eq!(parse_size("1.5g"), Some(1_500_000_000));
        assert_eq!(parse_size("800KB"), Some(800_000));
        assert_eq!(parse_size("1000"), Some(1000));
        assert_eq!(parse_size("lots"), None);
        assert_eq!(parse_age("60d"), Some(60 * 86_400));
        assert_eq!(parse_age("6mo"), Some(180 * 86_400));
        assert_eq!(parse_age("1y"), Some(365 * 86_400));
        assert_eq!(parse_age("soon"), None);
        let (rules, problems) = CleanupConfig::default().rules();
        assert!(problems.is_empty());
        assert_eq!((rules.big_file, rules.download_age), (500_000_000, 60 * 86_400));
        let l = load_str("[cleanup]\nbig_file = \"huge\"\n");
        assert!(l.error.unwrap().contains("cleanup.big_file"));
        assert_eq!(l.config.cleanup.rules().0.big_file, 500_000_000);

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        add_to_list(&p, "cleanup", "ignore", "~/Movies/keep").unwrap();
        add_to_list(&p, "cleanup", "ignore", "~/Movies/keep").unwrap();
        assert_eq!(load(&p).config.cleanup.ignore, ["~/Movies/keep"]);
    }

    #[test]
    fn preview_mode_width_is_checked() {
        assert_eq!(load_str("[preview]\nmode_width = 0.9\n").config.preview.mode_width, 0.9);
        let l = load_str("[preview]\nmode_width = 1.5\n");
        assert_eq!(l.config.preview.mode_width, 0.75);
        assert!(l.error.unwrap().contains("preview.mode_width"));
    }

    #[test]
    fn bat_themes_follow_the_preset_unless_set() {
        let l = load_str("[theme]\npreset = \"gruvbox\"\n");
        assert_eq!(l.config.bat_themes(), ("gruvbox-light".to_string(), "gruvbox-dark".to_string()));
        let l = load_str("[theme]\npreset = \"gruvbox\"\n[preview]\nbat_theme_light = \"OneHalfLight\"\n");
        assert_eq!(l.config.bat_themes().0, "OneHalfLight");
        assert_eq!(Config::default().bat_themes(), ("GitHub".to_string(), String::new()));
    }

    #[test]
    fn index_paths_expand_and_split() {
        let cfg = IndexConfig {
            roots: vec!["~".into(), "/Volumes/Work".into()],
            exclude: vec!["node_modules".into(), "~/Library".into(), "/tmp".into()],
            hide: vec!["*.log".into(), "~/Downloads/big.iso".into()],
            ..Default::default()
        };
        let r = cfg.resolve(Path::new("/Users/me"));
        assert_eq!(r.roots, [PathBuf::from("/Users/me"), PathBuf::from("/Volumes/Work")]);
        assert_eq!(r.exclude_names, ["node_modules"]);
        assert_eq!(r.exclude_paths, [PathBuf::from("/Users/me/Library"), PathBuf::from("/tmp")]);
        assert_eq!(r.hide_names, ["*.log"]);
        assert_eq!(r.hide_paths, [PathBuf::from("/Users/me/Downloads/big.iso")]);
    }

    #[test]
    fn adding_index_entries_keeps_comments_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");

        // Missing file: created from the template, entry appended.
        add_index_entry(&p, IndexList::Hide, "*.log").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("# Tilde configuration"));
        assert_eq!(load(&p).config.index.hide, ["*.log"]);

        // Adding twice doesn't duplicate.
        add_index_entry(&p, IndexList::Hide, "*.log").unwrap();
        assert_eq!(load(&p).config.index.hide, ["*.log"]);

        // A file without the list starts from the defaults.
        std::fs::write(&p, "# mine\n[font]\nsize = 14\n").unwrap();
        add_index_entry(&p, IndexList::Exclude, "~/Movies").unwrap();
        let l = load(&p);
        assert!(l.error.is_none(), "{:?}", l.error);
        assert!(l.config.index.exclude.contains(&"node_modules".to_string()));
        assert!(l.config.index.exclude.contains(&"~/Movies".to_string()));
        assert_eq!(l.config.font.size, 14.0);
        assert!(std::fs::read_to_string(&p).unwrap().starts_with("# mine"));
    }

    #[test]
    fn set_value_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        set_value(&p, &["font", "size"], Value::Float(15.0)).unwrap();
        set_value(&p, &["theme", "preset"], Value::Text("nord".into())).unwrap();
        set_value(&p, &["keys", "select", "move"], Value::List(vec!["M".into()])).unwrap();
        set_value(&p, &["preview", "visible"], Value::Bool(false)).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("# Base size in points"), "comments kept");
        let l = load(&p);
        assert!(l.error.is_none(), "{:?}", l.error);
        assert_eq!(l.config.font.size, 15.0);
        assert_eq!(l.config.theme.preset, "nord");
        assert!(!l.config.preview.visible);
        assert_eq!(l.config.keys.select["move"], KeyList::Many(vec!["M".into()]));

        std::fs::write(&p, "[edit]\neditor = \"builtin\" # or nvim\n").unwrap();
        set_value(&p, &["edit", "editor"], Value::Text("nvim".into())).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "[edit]\neditor = \"nvim\" # or nvim\n");
        set_value(&p, &["frecency", "enabled"], Value::Bool(false)).unwrap();
        assert!(!load(&p).config.frecency.enabled);
        remove_value(&p, &["frecency", "enabled"]).unwrap();
        assert!(load(&p).config.frecency.enabled, "back to the default");
        remove_value(&p, &["nope", "nothing"]).unwrap();
    }

    #[test]
    fn sort_section() {
        let l = load_str("[sort]\nby = \"Added\"\nreverse = true\n");
        assert!(l.error.is_none());
        let o = l.config.sort.order();
        assert_eq!((o.key, o.reverse, o.folders_first), (tilde_types::SortKey::Added, true, true));
        let l = load_str("[sort]\nby = \"colour\"\n");
        assert!(l.error.unwrap().contains("sort.by"));
        assert_eq!(l.config.sort.order().key, tilde_types::SortKey::Name);
    }

    #[test]
    fn key_warnings_are_reported_but_the_rest_loads() {
        let l = load_str("[font]\nsize = 15\n[keys.select]\nmove = \"cmd+m\"\n");
        assert_eq!(l.config.font.size, 15.0);
        assert!(l.error.unwrap().contains("keys.select.move"));
    }

    #[test]
    fn ensure_file_creates_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tilde/config.toml");
        ensure_file(&p).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), TEMPLATE);
        std::fs::write(&p, "[font]\nsize = 14\n").unwrap();
        ensure_file(&p).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "[font]\nsize = 14\n");
    }
}
