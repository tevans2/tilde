//! User configuration from `~/.config/tilde/config.toml` (or `$XDG_CONFIG_HOME/tilde/`).
//!
//! A missing file means defaults. A broken file also means defaults, plus an error
//! message for the UI, so a typo never stops Tilde from starting.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_FONT_FAMILY: &str = "JetBrains Mono";
pub const DEFAULT_FONT_SIZE: f64 = 13.0;
const FONT_SIZE_RANGE: std::ops::RangeInclusive<f64> = 8.0..=32.0;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub font: FontConfig,
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
            Loaded { config, error }
        }
        Err(e) => Loaded {
            config: Config::default(),
            error: Some(format!("config.toml has an error, using defaults: {}", e.message())),
        },
    }
}

/// Written when the user asks to open a config file that doesn't exist yet.
pub const TEMPLATE: &str = r#"# Tilde configuration. Reload with ⌘R after saving.

[font]
# Any installed font family. If there's no exact match, the closest installed family
# is used (so "JetBrains Mono" also finds "JetBrainsMono Nerd Font").
family = "JetBrains Mono"
# Base size in points for the file list (8–32). Other text is sized relative to it.
size = 13
"#;

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
