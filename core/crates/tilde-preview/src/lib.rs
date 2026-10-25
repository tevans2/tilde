//! Text previews: code and other text files rendered by `bat` with syntax highlighting.

mod ansi;

pub use ansi::{Span, StyledText, parse};

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Default)]
pub struct BatOptions {
    /// `--theme`; empty uses bat's own config (or its default).
    pub theme: String,
    /// Only the first this many lines are rendered.
    pub max_lines: u32,
}

/// Finds `bat` on `PATH` or in common install locations (apps started from the Dock don't
/// inherit the shell's `PATH`).
pub fn find_bat() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let path_dirs = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    let fallbacks = [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".cargo/bin"),
        home.join(".local/bin"),
        home.join(".nix-profile/bin"),
        PathBuf::from("/run/current-system/sw/bin"),
    ];
    path_dirs.into_iter().chain(fallbacks).map(|d| d.join("bat")).find(|p| p.is_file())
}

/// Whether a file looks like text: no NUL bytes in its first 8 KB.
pub fn looks_like_text(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut buf = [0u8; 8192];
    let Ok(n) = f.read(&mut buf) else { return false };
    !buf[..n].contains(&0)
}

/// Renders `path` with bat. `None` if bat isn't installed, the file isn't text, or bat
/// fails.
pub fn bat_preview(path: &Path, options: &BatOptions) -> Option<StyledText> {
    let bat = find_bat()?;
    if !looks_like_text(path) {
        return None;
    }
    let mut cmd = Command::new(bat);
    cmd.args(["--color=always", "--decorations=always", "--style=numbers", "--paging=never", "--wrap=never"])
        .arg(format!("--line-range=:{}", options.max_lines.max(1)))
        // Exact theme colours rather than the nearest 256-colour match.
        .env("COLORTERM", "truecolor")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if !options.theme.trim().is_empty() {
        cmd.arg(format!("--theme={}", options.theme.trim()));
    }
    let output = cmd.arg("--").arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        std::fs::write(dir.path().join("b.bin"), [0u8, 1, 2, 3]).unwrap();
        assert!(looks_like_text(&dir.path().join("a.txt")));
        assert!(!looks_like_text(&dir.path().join("b.bin")));
        assert!(!looks_like_text(&dir.path().join("missing")));
    }

    #[test]
    fn renders_with_bat_when_installed() {
        if find_bat().is_none() {
            return; // bat isn't required to build or test Tilde
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
        let out = bat_preview(&file, &BatOptions { theme: "Monokai Extended".into(), max_lines: 2 }).unwrap();
        assert!(out.text.contains("fn main()"), "{:?}", out.text);
        assert!(!out.text.contains('}'), "limited to 2 lines: {:?}", out.text);
        assert!(!out.spans.is_empty(), "highlighted");
        assert!(bat_preview(&dir.path().join("missing.rs"), &BatOptions::default()).is_none());
    }
}
