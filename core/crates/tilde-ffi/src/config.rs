use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ThemeAppearance {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AppConfig {
    pub appearance: ThemeAppearance,
    /// Colour key → `0xRRGGBBAA` for light appearance. Missing keys use system colours.
    pub light_colors: HashMap<String, u32>,
    pub dark_colors: HashMap<String, u32>,
    pub font_family: String,
    pub font_size: f64,
    pub preview_visible: bool,
    pub preview_width: f64,
    /// Width in PREVIEW (`p`).
    pub preview_mode_width: f64,
    pub preview_bat: bool,
    /// bat theme for light appearance.
    pub bat_theme_light: String,
    /// bat theme for dark appearance; empty uses bat's own config.
    pub bat_theme_dark: String,
    pub bat_max_lines: u32,
    /// How PREVIEW draws Markdown: "web", "native" or "quicklook".
    pub preview_markdown: String,
    /// EDIT uses Neovim instead of the built-in editor.
    pub editor_nvim: bool,
    /// The file-dialog helper's hotkey, when it's on (and the hotkey is valid).
    pub dialog_hotkey: Option<HotkeyInfo>,
    pub dialog_confirm: bool,
}

/// A global hotkey, for registering with the system.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HotkeyInfo {
    /// One character, or a name: space, return, tab, esc, down, …
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
    /// `⌃⌥Space`.
    pub display: String,
}

/// Parses a hotkey as written in the config (`ctrl+alt+space`).
#[uniffi::export]
pub fn parse_hotkey(spec: String) -> Result<HotkeyInfo, ConfigError> {
    tilde_config::parse_hotkey(&spec).map(hotkey_info).map_err(|message| ConfigError::Io { message })
}

fn hotkey_info(h: tilde_config::Hotkey) -> HotkeyInfo {
    HotkeyInfo { display: h.display(), key: h.key, ctrl: h.ctrl, alt: h.alt, shift: h.shift, cmd: h.cmd }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ConfigLoad {
    pub config: AppConfig,
    /// Where the config file lives (it may not exist yet).
    pub path: String,
    /// Why some or all of the file was ignored, for showing to the user.
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ConfigError {
    #[error("{message}")]
    Io { message: String },
}

/// Reads the user's config file, falling back to defaults.
#[uniffi::export]
pub fn load_config() -> ConfigLoad {
    let path = tilde_config::config_path();
    let loaded = tilde_config::load(&path);
    let (theme, _) = loaded.config.theme.resolve();
    let (bat_theme_light, bat_theme_dark) = loaded.config.bat_themes();
    ConfigLoad {
        config: AppConfig {
            appearance: match theme.appearance {
                tilde_config::Appearance::System => ThemeAppearance::System,
                tilde_config::Appearance::Light => ThemeAppearance::Light,
                tilde_config::Appearance::Dark => ThemeAppearance::Dark,
            },
            light_colors: theme.light.into_iter().collect(),
            dark_colors: theme.dark.into_iter().collect(),
            font_family: loaded.config.font.family,
            font_size: loaded.config.font.size,
            preview_visible: loaded.config.preview.visible,
            preview_width: loaded.config.preview.width,
            preview_mode_width: loaded.config.preview.mode_width,
            preview_bat: loaded.config.preview.bat,
            bat_theme_light,
            bat_theme_dark,
            bat_max_lines: loaded.config.preview.max_lines,
            preview_markdown: match loaded.config.preview.markdown {
                tilde_config::MarkdownView::Web => "web",
                tilde_config::MarkdownView::Native => "native",
                tilde_config::MarkdownView::Quicklook => "quicklook",
            }
            .into(),
            editor_nvim: loaded.config.edit.editor == tilde_config::EditorKind::Nvim,
            dialog_hotkey: loaded
                .config
                .dialog
                .enabled
                .then(|| tilde_config::parse_hotkey(&loaded.config.dialog.hotkey).ok().map(hotkey_info))
                .flatten(),
            dialog_confirm: loaded.config.dialog.confirm,
        },
        path: path.to_string_lossy().into_owned(),
        error: loaded.error,
    }
}

/// Creates the config file from the commented template if it's missing. Returns its path.
#[uniffi::export]
pub fn ensure_config_file() -> Result<String, ConfigError> {
    let path = tilde_config::config_path();
    tilde_config::ensure_file(&path).map_err(|e| ConfigError::Io { message: e.to_string() })?;
    Ok(path.to_string_lossy().into_owned())
}

/// Everything the Settings window edits, as currently in the config file.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Settings {
    pub appearance: ThemeAppearance,
    pub theme_preset: String,
    /// Presets to choose from.
    pub presets: Vec<String>,
    pub font_family: String,
    pub font_size: f64,
    pub preview_visible: bool,
    pub preview_width: f64,
    pub preview_mode_width: f64,
    pub preview_bat: bool,
    pub editor_nvim: bool,
    pub nvim_path: String,
    /// `nvim_config = "clean"`.
    pub nvim_clean: bool,
    pub frecency_enabled: bool,
    pub frecency_zoxide: bool,
    /// zoxide is installed.
    pub zoxide_found: bool,
    pub frecency_tell_zoxide: bool,
    pub activity_days: u32,
    pub index_enabled: bool,
    pub index_include_hidden: bool,
    pub index_max_items: u64,
    pub dialog_enabled: bool,
    /// As written in the config: `ctrl+alt+space`.
    pub dialog_hotkey: String,
    pub dialog_confirm: bool,
    /// `[cleanup]`, as written: "500M", "6mo".
    pub cleanup_big_file: String,
    pub cleanup_big_file_age: String,
    pub cleanup_download_age: String,
    pub cleanup_project_idle: String,
    pub cleanup_duplicates_min: String,
    pub cleanup_build_folders: Vec<String>,
    pub cleanup_ignore: Vec<String>,
    /// `[search]`: `text:` searches inside files, and Spotlight helps beyond the folder.
    pub search_contents: bool,
    pub search_spotlight: bool,
    /// Spotlight is indexing the home folder's disk.
    pub spotlight_available: bool,
    /// `[sort] folder_sizes`: folders are measured in the background.
    pub folder_sizes: bool,
    /// `[sort] same_everywhere`: a sort chosen applies to every folder.
    pub sort_same_everywhere: bool,
}

#[uniffi::export]
pub fn load_settings() -> Settings {
    let c = tilde_config::load(&tilde_config::config_path()).config;
    let tell_zoxide = c.tell_zoxide();
    let use_zoxide = c.use_zoxide();
    Settings {
        appearance: match c.theme.appearance.as_str() {
            "light" => ThemeAppearance::Light,
            "dark" => ThemeAppearance::Dark,
            _ => ThemeAppearance::System,
        },
        theme_preset: c.theme.preset,
        presets: tilde_config::PRESETS.iter().map(|p| p.to_string()).collect(),
        font_family: c.font.family,
        font_size: c.font.size,
        preview_visible: c.preview.visible,
        preview_width: c.preview.width,
        preview_mode_width: c.preview.mode_width,
        preview_bat: c.preview.bat,
        editor_nvim: c.edit.editor == tilde_config::EditorKind::Nvim,
        nvim_path: c.edit.nvim_path,
        nvim_clean: c.edit.nvim_config == tilde_config::NvimConfigMode::Clean,
        frecency_enabled: c.frecency.enabled,
        frecency_zoxide: use_zoxide,
        zoxide_found: tilde_index::Zoxide::find().is_some(),
        frecency_tell_zoxide: tell_zoxide,
        activity_days: c.activity.days,
        index_enabled: c.index.enabled,
        index_include_hidden: c.index.include_hidden,
        index_max_items: c.index.max_items,
        dialog_enabled: c.dialog.enabled,
        dialog_hotkey: c.dialog.hotkey,
        dialog_confirm: c.dialog.confirm,
        cleanup_big_file: c.cleanup.big_file,
        cleanup_big_file_age: c.cleanup.big_file_age,
        cleanup_download_age: c.cleanup.download_age,
        cleanup_project_idle: c.cleanup.project_idle,
        cleanup_duplicates_min: c.cleanup.duplicates_min,
        cleanup_build_folders: c.cleanup.build_folders,
        cleanup_ignore: c.cleanup.ignore,
        search_contents: c.search.contents,
        search_spotlight: c.search.spotlight,
        spotlight_available: tilde_contents::spotlight_available(&crate::index::home_dir()),
        folder_sizes: c.sort.folder_sizes,
        sort_same_everywhere: c.sort.same_everywhere,
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum SettingValue {
    Bool { value: bool },
    Int { value: i64 },
    Float { value: f64 },
    Text { value: String },
    List { value: Vec<String> },
}

/// Writes one setting to the config file, keeping its comments. `key` is dotted:
/// `font.size`, `keys.select.move`.
#[uniffi::export]
pub fn set_setting(key: String, value: SettingValue) -> Result<(), ConfigError> {
    let path: Vec<&str> = key.split('.').collect();
    let value = match value {
        SettingValue::Bool { value } => tilde_config::Value::Bool(value),
        SettingValue::Int { value } => tilde_config::Value::Int(value),
        SettingValue::Float { value } => tilde_config::Value::Float(value),
        SettingValue::Text { value } => tilde_config::Value::Text(value),
        SettingValue::List { value } => tilde_config::Value::List(value),
    };
    tilde_config::set_value(&tilde_config::config_path(), &path, value)
        .map_err(|e| ConfigError::Io { message: e.to_string() })
}

/// Removes a setting from the config file, so its default applies (`keys.select.move`).
#[uniffi::export]
pub fn reset_setting(key: String) -> Result<(), ConfigError> {
    let path: Vec<&str> = key.split('.').collect();
    tilde_config::remove_value(&tilde_config::config_path(), &path)
        .map_err(|e| ConfigError::Io { message: e.to_string() })
}

/// A key with modifiers, as the app matches it against key presses.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeyChord {
    /// One character (`j`, `G`, `@`) or a name: down, up, left, right, return, tab, esc,
    /// space, backspace, delete, home, end, pageup, pagedown.
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    /// Only with named keys.
    pub shift: bool,
    /// `⌃J`, `⏎`.
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ActionKeys {
    /// `move`, `down`: the name used in `[keys.go]` / `[keys.select]`.
    pub action: String,
    pub description: String,
    pub keys: Vec<KeyChord>,
    /// The default keys, for display.
    pub defaults: Vec<String>,
    /// Set in the config file.
    pub customized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeymapInfo {
    pub go: Vec<ActionKeys>,
    pub select: Vec<ActionKeys>,
}

/// The key bindings from the config (warnings arrive with `load_config`).
#[uniffi::export]
pub fn load_keymap() -> KeymapInfo {
    let keys = tilde_config::load(&tilde_config::config_path()).config.keys;
    let map = keys.resolve();
    let convert = |bindings: Vec<tilde_config::Binding>,
                   user: &std::collections::BTreeMap<String, tilde_config::KeyList>| {
        bindings
            .into_iter()
            .map(|b| ActionKeys {
                action: b.action.name.to_string(),
                description: b.action.description.to_string(),
                keys: b
                    .keys
                    .iter()
                    .map(|c| KeyChord {
                        key: c.key.clone(),
                        ctrl: c.ctrl,
                        alt: c.alt,
                        shift: c.shift,
                        display: c.display(),
                    })
                    .collect(),
                defaults: b
                    .action
                    .defaults
                    .iter()
                    .filter_map(|d| tilde_config::parse_chord(d).ok())
                    .map(|c| c.display())
                    .collect(),
                customized: user.contains_key(b.action.name),
            })
            .collect()
    };
    KeymapInfo { go: convert(map.go, &keys.go), select: convert(map.select, &keys.select) }
}
