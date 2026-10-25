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
    pub index: IndexConfig,
    pub zoxide: ZoxideConfig,
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
        let expand = |s: &str| -> PathBuf {
            match s.strip_prefix('~') {
                Some("") => home.to_path_buf(),
                Some(rest) if rest.starts_with('/') => home.join(&rest[1..]),
                _ => PathBuf::from(s),
            }
        };
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ZoxideConfig {
    /// Rank folders by zoxide's frecency and include folders it knows about.
    pub enabled: bool,
    /// Tell zoxide about folders opened in Tilde, so `z` in the shell learns from Tilde.
    pub add_on_visit: bool,
}

impl Default for ZoxideConfig {
    fn default() -> Self {
        ZoxideConfig { enabled: true, add_on_visit: true }
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

[zoxide]
# Rank folders you visit often (per zoxide) higher, and include every folder it knows.
enabled = true
# Tell zoxide about folders you open in Tilde, so `z` in your shell learns from Tilde.
add_on_visit = true
"#;

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
