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
    pub preview_bat: bool,
    /// bat theme for light appearance.
    pub bat_theme_light: String,
    /// bat theme for dark appearance; empty uses bat's own config.
    pub bat_theme_dark: String,
    pub bat_max_lines: u32,
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
            preview_bat: loaded.config.preview.bat,
            bat_theme_light,
            bat_theme_dark,
            bat_max_lines: loaded.config.preview.max_lines,
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
