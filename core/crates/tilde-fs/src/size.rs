//! Folder sizes: the total of everything inside, for showing and sorting by size.
//!
//! A folder is walked in parallel (fork-join: list it, then measure its subfolders at
//! the same time) on one small pool shared by every measurement, at utility priority.
//! Reading folders mostly waits on the file system, so several reads in flight finish a
//! big tree several times sooner than one at a time, for about the same CPU.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rayon::prelude::*;
use tilde_types::EntryKind;

use crate::{ListOptions, list_dir};

/// Threads measuring folders, shared by every measurement.
const THREADS: usize = 4;

fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(THREADS)
            .thread_name(|i| format!("tilde-size-{i}"))
            // Deep trees recurse a level per folder.
            .stack_size(8 << 20)
            .start_handler(|_| {
                // SAFETY: only changes the calling thread's scheduling class.
                unsafe {
                    libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
                }
            })
            .build()
            .expect("folder size thread pool")
    })
}

/// The total size of the files under `dir` (their lengths, as the list shows for files),
/// hidden ones and package contents included. Links aren't followed and other volumes
/// aren't entered; folders that can't be read count as empty. Calls `on_folder` (from any
/// of the pool's threads) with the total of each folder down to `keep` levels below
/// `dir` (`dir` itself is level 0), so those can be remembered too. Returns
/// `ErrorKind::Interrupted` once `cancel` says so; it's checked before each folder.
pub fn folder_size(
    dir: &Path,
    keep: usize,
    cancel: &(dyn Fn() -> bool + Sync),
    on_folder: &(dyn Fn(&Path, u64) + Sync),
) -> io::Result<u64> {
    let opts = ListOptions { include_hidden: true, buffer_size: 64 * 1024 };
    pool().install(|| measure(dir, 0, keep, &opts, cancel, on_folder))
}

fn measure(
    dir: &Path,
    depth: usize,
    keep: usize,
    opts: &ListOptions,
    cancel: &(dyn Fn() -> bool + Sync),
    on_folder: &(dyn Fn(&Path, u64) + Sync),
) -> io::Result<u64> {
    if cancel() {
        return Err(io::Error::from(io::ErrorKind::Interrupted));
    }
    let entries = match list_dir(dir, opts, cancel) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
        Err(_) => Vec::new(),
    };
    let mut files = 0;
    let mut folders: Vec<PathBuf> = Vec::new();
    for e in entries {
        match e.kind {
            EntryKind::Folder | EntryKind::Package if !e.mount_point => folders.push(dir.join(&e.name)),
            EntryKind::File => files += e.size,
            _ => {}
        }
    }
    let inside = folders
        .par_iter()
        .map(|f| measure(f, depth + 1, keep, opts, cancel, on_folder))
        .try_reduce(|| 0, |a, b| Ok(a + b))?;
    let total = files + inside;
    if depth <= keep {
        on_folder(dir, total);
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

        let seen = Mutex::new(Vec::new());
        let total = folder_size(root, 1, &|| false, &|p, n| {
            seen.lock().unwrap().push((p.strip_prefix(root).unwrap().to_string_lossy().into_owned(), n))
        })
        .unwrap();
        assert_eq!(total, 1115, "links aren't followed");
        let mut seen = seen.into_inner().unwrap();
        seen.sort();
        assert_eq!(seen, [("".into(), 1115), ("a".into(), 1015)], "kept to one level down");

        let cancelled = folder_size(root, 0, &|| true, &|_, _| {});
        assert_eq!(cancelled.unwrap_err().kind(), io::ErrorKind::Interrupted);
    }

    /// Sums file lengths the simple way, for comparison.
    fn reference(dir: &Path) -> u64 {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| {
                let t = e.file_type().unwrap();
                if t.is_dir() {
                    reference(&e.path())
                } else if t.is_file() {
                    e.metadata().unwrap().len()
                } else {
                    0
                }
            })
            .sum()
    }

    #[test]
    fn a_wide_deep_tree_totals_the_same_as_a_simple_walk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut n = 0u64;
        for a in 0..12 {
            for b in 0..(a % 5 + 1) {
                let deep = root.join(format!("a{a}/b{b}/c/d/e{}", a % 3));
                fs::create_dir_all(&deep).unwrap();
                for f in 0..(a + b) {
                    n += 1;
                    fs::write(deep.join(format!("f{f}")), vec![0u8; (n * 37 % 4000) as usize]).unwrap();
                    fs::write(root.join(format!("a{a}/top{b}-{f}")), vec![0u8; (n % 300) as usize]).unwrap();
                }
            }
        }
        let kept = Mutex::new(Vec::new());
        let total =
            folder_size(root, 2, &|| false, &|p, size| kept.lock().unwrap().push((p.to_path_buf(), size))).unwrap();
        assert_eq!(total, reference(root));
        let kept = kept.into_inner().unwrap();
        for (p, size) in &kept {
            assert_eq!(*size, reference(p), "{p:?}");
        }
        let levels =
            |d: usize| kept.iter().filter(|(p, _)| p.strip_prefix(root).unwrap().components().count() == d).count();
        assert_eq!((levels(0), levels(1), levels(3)), (1, 12, 0), "kept down to two levels");
    }

    #[test]
    fn cancelling_part_way_stops_the_walk() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..200 {
            fs::create_dir_all(dir.path().join(format!("{i}/x"))).unwrap();
        }
        let checks = AtomicUsize::new(0);
        let result = folder_size(dir.path(), 0, &|| checks.fetch_add(1, Ordering::SeqCst) > 20, &|_, _| {});
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(checks.load(Ordering::SeqCst) < 200, "stopped early, not after every folder");
    }
}
