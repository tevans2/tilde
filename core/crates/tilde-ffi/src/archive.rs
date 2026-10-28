//! Archives as folders, for the app: telling paths inside archives apart, and writing
//! their members out to preview, open or copy them.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::index::home_dir;
use crate::ops::OpError;

/// Members written out for previews are kept this long after their last use.
const KEEP_EXTRACTED: Duration = Duration::from_secs(3 * 86_400);

/// The archive holding `path`, when `path` is inside one (not the archive itself).
#[uniffi::export]
pub fn archive_containing(path: String) -> Option<String> {
    match tilde_archive::split(Path::new(&path)) {
        Some((archive, inner)) if !inner.is_empty() => Some(archive.to_string_lossy().into_owned()),
        _ => None,
    }
}

/// An archive Tilde opens as a folder (zip, tar, 7z, rar…), by name.
#[uniffi::export]
pub fn is_archive(name: String) -> bool {
    tilde_archive::is_archive_name(&name)
}

/// Real files for `paths`: those inside archives are written out (once) under
/// `~/Library/Caches/Tilde/Archives`, the rest are returned as they are. Can take a
/// while for big members; call it off the main thread.
#[uniffi::export]
pub fn extract_from_archives(paths: Vec<String>) -> Result<Vec<String>, OpError> {
    let mut out = paths.clone();
    let mut by_archive: HashMap<PathBuf, Vec<(usize, String)>> = HashMap::new();
    for (i, path) in paths.iter().enumerate() {
        if let Some((archive, inner)) = tilde_archive::split(Path::new(path))
            && !inner.is_empty()
        {
            by_archive.entry(archive).or_default().push((i, inner));
        }
    }
    if by_archive.is_empty() {
        return Ok(out);
    }
    let root = home_dir().join("Library/Caches/Tilde/Archives");
    prune(&root);
    for (archive, members) in by_archive {
        let dest = root.join(cache_name(&archive));
        let inner: Vec<String> = members.iter().map(|(_, m)| m.clone()).collect();
        let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let written = tilde_archive::extract(&archive, &inner, &dest)
            .map_err(|e| OpError::Invalid { message: format!("Couldn't read “{name}”: {e}") })?;
        let _ = mark_used(&dest);
        for ((i, _), path) in members.iter().zip(written) {
            out[*i] = path.to_string_lossy().into_owned();
        }
    }
    Ok(out)
}

/// A folder name for an archive's members: changes when the archive does.
fn cache_name(archive: &Path) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    archive.hash(&mut h);
    if let Ok(m) = std::fs::metadata(archive) {
        (m.len(), m.mtime(), m.mtime_nsec()).hash(&mut h);
    }
    let stem = archive.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    format!("{stem}-{:016x}", h.finish())
}

/// Notes when an archive's members were last wanted, for [`prune`].
fn mark_used(dir: &Path) -> std::io::Result<()> {
    std::fs::write(dir.join(".used"), b"")
}

/// Deletes members written out more than `KEEP_EXTRACTED` ago.
fn prune(root: &Path) {
    let Ok(dirs) = std::fs::read_dir(root) else { return };
    for dir in dirs.flatten() {
        let used = std::fs::metadata(dir.path().join(".used")).and_then(|m| m.modified());
        if used.is_ok_and(|t| t.elapsed().unwrap_or_default() > KEEP_EXTRACTED) {
            let _ = std::fs::remove_dir_all(dir.path());
        }
    }
}
