//! zoxide: its scores (folders used in the terminal) add to Tilde's frecency, and
//! (optionally) it hears about folders visited in Tilde so the shell's `z` learns too.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct Zoxide {
    bin: PathBuf,
}

impl Zoxide {
    /// Finds the `zoxide` binary on `PATH` or in common install locations. Apps started
    /// from the Dock don't inherit the shell's `PATH`, hence the fallbacks.
    pub fn find() -> Option<Zoxide> {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let path_dirs =
            std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
        let fallbacks = [
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
            home.join(".cargo/bin"),
            home.join(".local/bin"),
            home.join(".nix-profile/bin"),
            PathBuf::from("/run/current-system/sw/bin"),
        ];
        path_dirs.into_iter().chain(fallbacks).map(|d| d.join("zoxide")).find(|p| p.is_file()).map(|bin| Zoxide { bin })
    }

    /// Folder paths and their frecency scores.
    pub fn scores(&self) -> Vec<(PathBuf, f64)> {
        let output = Command::new(&self.bin).args(["query", "--list", "--score"]).stderr(Stdio::null()).output();
        match output {
            Ok(o) if o.status.success() => parse_scores(&String::from_utf8_lossy(&o.stdout)),
            _ => Vec::new(),
        }
    }

    /// Records a visit in the background.
    pub fn add(&self, path: &Path) {
        let bin = self.bin.clone();
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let _ =
                Command::new(bin).arg("add").arg("--").arg(path).stdout(Stdio::null()).stderr(Stdio::null()).status();
        });
    }
}

/// Parses `zoxide query --list --score` output: `  12.5 /some/path` per line.
pub fn parse_scores(output: &str) -> Vec<(PathBuf, f64)> {
    output
        .lines()
        .filter_map(|line| {
            let (score, path) = line.trim_start().split_once(' ')?;
            Some((PathBuf::from(path), score.parse().ok()?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scores_including_paths_with_spaces() {
        let out = "  90.0 /Users/me/Developer/projects\n  66.2 /Users/me/ntombi ai\n   0.5 /tmp\n\nbad line\n";
        assert_eq!(
            parse_scores(out),
            [
                (PathBuf::from("/Users/me/Developer/projects"), 90.0),
                (PathBuf::from("/Users/me/ntombi ai"), 66.2),
                (PathBuf::from("/tmp"), 0.5),
            ]
        );
    }
}
