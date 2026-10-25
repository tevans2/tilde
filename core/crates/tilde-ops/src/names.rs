//! File name rules: what's allowed, and Finder-style "keep both" names.

use std::path::{Path, PathBuf};

/// Why a name can't be used, or `Ok`.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("A name can't be empty.".into());
    }
    if name == "." || name == ".." {
        return Err(format!("“{name}” can't be used as a name."));
    }
    if name.contains('/') || name.contains(':') || name.contains('\0') {
        return Err("Names can't contain “/” or “:”.".into());
    }
    if name.len() > 255 {
        return Err("That name is too long (255 bytes at most).".into());
    }
    Ok(())
}

/// A free name in `dir` for `name`, the way the Finder does it: `report.pdf` becomes
/// `report 2.pdf`, then `report 3.pdf`. Folders and dotfiles keep their whole name as the
/// stem. `taken` says whether a candidate path is already used.
pub fn keep_both(dir: &Path, name: &str, is_folder: bool, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let candidate = dir.join(name);
    if !taken(&candidate) {
        return candidate;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if !is_folder && i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    // Continue an existing " N" suffix instead of stacking "a 2 2".
    let (base, mut n) = match stem.rsplit_once(' ') {
        Some((b, num)) if !b.is_empty() && num.parse::<u32>().is_ok_and(|n| n >= 2) => (b, num.parse().unwrap()),
        _ => (stem, 1),
    };
    loop {
        n += 1;
        let candidate = dir.join(format!("{base} {n}{ext}"));
        if !taken(&candidate) {
            return candidate;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn rejects_bad_names() {
        assert!(validate_name("report.pdf").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("  ").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a:b").is_err());
        assert!(validate_name(&"x".repeat(256)).is_err());
    }

    #[test]
    fn keeps_both_like_the_finder() {
        let dir = Path::new("/d");
        let taken: HashSet<PathBuf> =
            ["report.pdf", "report 2.pdf", "Folder", ".env", "a 2.txt"].iter().map(|n| dir.join(n)).collect();
        let t = |p: &Path| taken.contains(p);
        assert_eq!(keep_both(dir, "new.txt", false, t), dir.join("new.txt"));
        assert_eq!(keep_both(dir, "report.pdf", false, t), dir.join("report 3.pdf"));
        assert_eq!(keep_both(dir, "Folder", true, t), dir.join("Folder 2"));
        assert_eq!(keep_both(dir, ".env", false, t), dir.join(".env 2"));
        assert_eq!(keep_both(dir, "a 2.txt", false, t), dir.join("a 3.txt"));
    }
}
