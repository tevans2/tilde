//! Folder sizes: the total of everything inside, for showing and sorting by size.

use std::io;
use std::path::Path;

use tilde_types::EntryKind;

use crate::{ListOptions, list_dir};

/// The total size of the files under `dir` (their lengths, as the list shows for files),
/// hidden ones and package contents included. Links aren't followed and other volumes
/// aren't entered; folders that can't be read count as empty. Calls `on_folder` with the
/// total of each folder down to `keep` levels below `dir` (`dir` itself is level 0), so
/// those can be remembered too. Returns `ErrorKind::Interrupted` once `cancel` says so.
pub fn folder_size(
    dir: &Path,
    keep: usize,
    cancel: &dyn Fn() -> bool,
    on_folder: &mut dyn FnMut(&Path, u64),
) -> io::Result<u64> {
    let opts = ListOptions { include_hidden: true, buffer_size: 64 * 1024 };
    measure(dir, 0, keep, &opts, cancel, on_folder)
}

fn measure(
    dir: &Path,
    depth: usize,
    keep: usize,
    opts: &ListOptions,
    cancel: &dyn Fn() -> bool,
    on_folder: &mut dyn FnMut(&Path, u64),
) -> io::Result<u64> {
    let entries = match list_dir(dir, opts, cancel) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
        Err(_) => Vec::new(),
    };
    let mut total = 0;
    for e in entries {
        match e.kind {
            EntryKind::Folder | EntryKind::Package if !e.mount_point => {
                total += measure(&dir.join(&e.name), depth + 1, keep, opts, cancel, on_folder)?;
            }
            EntryKind::File => total += e.size,
            _ => {}
        }
    }
    if depth <= keep {
        on_folder(dir, total);
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn adds_up_everything_inside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        fs::write(root.join("top.txt"), vec![0u8; 100]).unwrap();
        fs::write(root.join("a/one.bin"), vec![0u8; 1000]).unwrap();
        fs::write(root.join("a/b/.hidden"), vec![0u8; 10]).unwrap();
        fs::write(root.join("a/b/c/deep"), vec![0u8; 5]).unwrap();
        std::os::unix::fs::symlink(root.join("a"), root.join("link")).unwrap();

        let mut seen = Vec::new();
        let total = folder_size(root, 1, &|| false, &mut |p, n| {
            seen.push((p.strip_prefix(root).unwrap().to_string_lossy().into_owned(), n))
        })
        .unwrap();
        assert_eq!(total, 1115, "links aren't followed");
        seen.sort();
        assert_eq!(seen, [("".into(), 1115), ("a".into(), 1015)], "kept to one level down");

        let cancelled = folder_size(root, 0, &|| true, &mut |_, _| {});
        assert_eq!(cancelled.unwrap_err().kind(), io::ErrorKind::Interrupted);
    }
}
