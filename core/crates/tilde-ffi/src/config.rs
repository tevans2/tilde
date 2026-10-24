#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AppConfig {
    pub font_family: String,
    pub font_size: f64,
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
    ConfigLoad {
        config: AppConfig { font_family: loaded.config.font.family, font_size: loaded.config.font.size },
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
