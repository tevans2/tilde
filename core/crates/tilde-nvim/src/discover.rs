//! Finding Neovim: the configured path, then the login shell's `PATH` (apps started from
//! the Dock get a minimal one), then the Homebrew locations.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Oldest supported version (for `vim.uv`, extmark conceal and `vim.keymap`).
pub const MIN_VERSION: (u32, u32) = (0, 10);
const FALLBACKS: &[&str] = &["/opt/homebrew/bin/nvim", "/usr/local/bin/nvim"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    /// As printed: `0.11.2`.
    pub version: String,
}

/// Finds a usable nvim. `configured` (may be empty) wins when set.
pub fn find(configured: &str) -> Result<Found, String> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let path = expand_home(configured);
        return check(&path).map_err(|e| format!("{e} (edit.nvim_path in config.toml)"));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = login_path() {
        candidates.extend(std::env::split_paths(path).map(|d| d.join("nvim")));
    }
    candidates.extend(FALLBACKS.iter().map(PathBuf::from));
    let mut last_error = None;
    for c in candidates.iter().filter(|c| c.is_file()) {
        match check(c) {
            Ok(found) => return Ok(found),
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| "Neovim isn't installed (brew install neovim)".into()))
}

/// The `PATH` an interactive login shell sets up, read once. nvim and the tools a user's
/// config starts (language servers, formatters) expect it.
pub fn login_path() -> Option<&'static str> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
        let out = Command::new(shell).args(["-l", "-c", "printf %s \"$PATH\""]).output().ok()?;
        let path = String::from_utf8(out.stdout).ok()?;
        // Login scripts can print; the PATH is what comes last.
        let path = path.lines().last()?.trim().to_string();
        (!path.is_empty()).then_some(path)
    })
    .as_deref()
}

fn check(path: &Path) -> Result<Found, String> {
    let out = Command::new(path).arg("--version").output().map_err(|e| format!("can't run {}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let version = parse_version(&text).ok_or_else(|| format!("{} doesn't look like Neovim", path.display()))?;
    let (major, minor) = numbers(&version);
    if (major, minor) < MIN_VERSION {
        return Err(format!(
            "Neovim {version} at {} is too old; Tilde needs {}.{} or later",
            path.display(),
            MIN_VERSION.0,
            MIN_VERSION.1
        ));
    }
    Ok(Found { path: path.to_path_buf(), version })
}

/// `NVIM v0.11.2-dev+…` → `0.11.2-dev+…`.
fn parse_version(text: &str) -> Option<String> {
    let first = text.lines().next()?;
    Some(first.strip_prefix("NVIM v")?.trim().to_string())
}

fn numbers(version: &str) -> (u32, u32) {
    let mut parts = version.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty());
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor)
}

pub(crate) fn expand_home(s: &str) -> PathBuf {
    match s.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("NVIM v0.11.2\nBuild type: Release").as_deref(), Some("0.11.2"));
        assert_eq!(parse_version("vim 9"), None);
        assert_eq!(numbers("0.12.0-dev+123"), (0, 12));
        assert!(numbers("0.9.5") < MIN_VERSION);
    }

    #[test]
    fn rejects_a_configured_path_that_is_not_nvim() {
        let e = find("/bin/echo").unwrap_err();
        assert!(e.contains("doesn't look like Neovim"), "{e}");
        assert!(find("/nonexistent/nvim").unwrap_err().contains("can't run"));
    }
}
