//! Archives as folders: a zip, tar, 7z or rar opens like a folder, read-only.
//!
//! A path inside an archive is the archive's path followed by the path within it
//! (`~/Downloads/assets.zip/sprites/ship.png`). [`split`] tells such a path apart from a
//! real one, [`list`] lists a folder inside an archive, and [`extract`] writes members
//! out (to preview, open or copy them). Reading goes through macOS's own libarchive, so
//! every format `bsdtar` reads works. Listings are cached while the archive is unchanged.

mod ffi;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use tilde_types::{Entry, EntryKind};

/// Archives kept listed in memory.
const CACHED_LISTINGS: usize = 4;

/// Extensions of archives Tilde opens as folders. Documents that are zips inside (`.docx`,
/// `.key`, `.epub`) aren't on the list: they open as documents.
const EXTENSIONS: &[&str] =
    &["zip", "jar", "cbz", "tar", "tgz", "tbz", "tbz2", "txz", "tlz", "7z", "rar", "cbr", "xar", "cpio", "iso"];
/// Compressed tars, by their last two extensions.
const TAR_EXTENSIONS: &[&str] = &["gz", "bz2", "xz", "lz", "lzma", "z", "zst"];

/// An archive Tilde can open as a folder, judged by name.
pub fn is_archive_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let mut parts = lower.rsplit('.');
    let (Some(ext), Some(stem)) = (parts.next(), parts.next()) else { return false };
    if stem.is_empty() && parts.next().is_none() {
        return false; // a dotfile like `.zip`
    }
    EXTENSIONS.contains(&ext) || (TAR_EXTENSIONS.contains(&ext) && stem.rsplit('.').next() == Some("tar"))
}

/// For a path inside an archive: the archive file and the path within it (`""` for the
/// archive itself). `None` for real files and folders, and paths that don't exist.
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    let mut archive = path;
    loop {
        match fs::metadata(archive) {
            Ok(m) if m.is_file() => {
                if !is_archive_name(&archive.file_name()?.to_string_lossy()) {
                    return None;
                }
                let inner = path.strip_prefix(archive).ok()?.to_string_lossy().into_owned();
                return Some((archive.to_path_buf(), inner));
            }
            Ok(_) => return None,
            Err(_) => archive = archive.parent()?,
        }
    }
}

/// The items directly inside `inner` (`""` for the top) in `archive`. Folder sizes are
/// the total of everything in them. `cancel` is checked while the archive is read.
pub fn list(archive: &Path, inner: &str, cancel: &dyn Fn() -> bool) -> io::Result<Vec<Entry>> {
    let tree = tree(archive, cancel)?;
    match tree.folders.get(inner.trim_matches('/')) {
        Some(items) => Ok(items.clone()),
        None => Err(io::Error::new(io::ErrorKind::NotFound, format!("“{inner}” isn't a folder in the archive"))),
    }
}

/// Writes the members `inner` (files, or folders with everything in them) of `archive`
/// under `dest`, at their paths within the archive, and returns where each one is.
/// Files already there with the right size are left alone.
pub fn extract(archive: &Path, inner: &[String], dest: &Path) -> io::Result<Vec<PathBuf>> {
    let wanted: Vec<String> = inner.iter().map(|p| p.trim_matches('/').to_string()).collect();
    let out: Vec<PathBuf> = wanted.iter().map(|w| dest.join(w)).collect();
    let tree = tree(archive, &|| false)?;
    // Everything wanted that isn't already there.
    let missing = |p: &str| match tree.files.get(p) {
        Some(&size) => fs::metadata(dest.join(p)).map(|m| m.len() != size).unwrap_or(true),
        None => false,
    };
    let under =
        |p: &str, w: &str| w.is_empty() || p == w || (p.starts_with(w) && p.as_bytes().get(w.len()) == Some(&b'/'));
    let needed = tree.files.keys().any(|p| wanted.iter().any(|w| under(p, w)) && missing(p));
    for w in &wanted {
        if !tree.files.contains_key(w) && !tree.folders.contains_key(w) {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("“{w}” isn't in the archive")));
        }
        if tree.folders.contains_key(w) {
            fs::create_dir_all(dest.join(w))?;
        }
    }
    if !needed {
        return Ok(out);
    }

    let mut reader = ffi::Reader::open(archive)?;
    let mut buf = vec![0u8; 256 * 1024];
    while let Some(h) = reader.next()? {
        let Some(path) = clean(&h.path) else { continue };
        if !wanted.iter().any(|w| under(&path, w)) {
            continue;
        }
        let target = dest.join(&path);
        match h.filetype {
            ffi::AE_IFDIR => fs::create_dir_all(&target)?,
            ffi::AE_IFREG if missing(&path) => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                // Written beside the target and renamed, so a half-written file is never used.
                let partial =
                    target.with_file_name(format!(".{}.partial", target.file_name().unwrap().to_string_lossy()));
                let mode = if h.perm & 0o111 != 0 { 0o755 } else { 0o644 };
                let mut file =
                    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&partial)?;
                loop {
                    let n = reader.read(&mut buf).inspect_err(|_| {
                        let _ = fs::remove_file(&partial);
                    })?;
                    if n == 0 {
                        break;
                    }
                    file.write_all(&buf[..n])?;
                }
                drop(file);
                set_modified(&partial, h.mtime);
                fs::rename(&partial, &target)?;
            }
            // Links aren't written: one could point anywhere.
            _ => {}
        }
    }
    Ok(out)
}

/// A path within an archive made safe to use under a folder: no `..`, nothing absolute,
/// no `./`. `None` for the archive's own top (`./`).
fn clean(raw: &str) -> Option<String> {
    let mut parts = Vec::new();
    for c in Path::new(raw).components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn set_modified(path: &Path, mtime: i64) {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else { return };
    let t = libc::timeval { tv_sec: mtime as libc::time_t, tv_usec: 0 };
    // SAFETY: a valid C path and two timevals.
    unsafe {
        libc::utimes(c.as_ptr(), [t, t].as_ptr());
    }
}

/// An archive's contents, as folders of entries.
struct Tree {
    /// Each folder's items by its path within the archive (`""` is the top).
    folders: HashMap<String, Vec<Entry>>,
    /// Every file's size, by path.
    files: HashMap<String, u64>,
}

/// Listings by archive, newest last, with the size and modification time they were read at.
type Cache = Vec<(PathBuf, u64, i64, Arc<Tree>)>;
static CACHE: Mutex<Cache> = Mutex::new(Vec::new());

fn tree(archive: &Path, cancel: &dyn Fn() -> bool) -> io::Result<Arc<Tree>> {
    let meta = fs::metadata(archive)?;
    let (size, mtime) = (meta.len(), meta.mtime());
    {
        let mut cache = CACHE.lock().unwrap();
        if let Some(i) = cache.iter().position(|(p, s, m, _)| p == archive && *s == size && *m == mtime) {
            let hit = cache.remove(i);
            let tree = hit.3.clone();
            cache.push(hit);
            return Ok(tree);
        }
    }
    let tree = Arc::new(read_tree(archive, cancel)?);
    let mut cache = CACHE.lock().unwrap();
    cache.retain(|(p, ..)| p != archive);
    cache.push((archive.to_path_buf(), size, mtime, tree.clone()));
    if cache.len() > CACHED_LISTINGS {
        cache.remove(0);
    }
    Ok(tree)
}

/// A file, folder or link while the tree is put together.
struct Node {
    kind: EntryKind,
    size: u64,
    modified: i64,
}

fn read_tree(archive: &Path, cancel: &dyn Fn() -> bool) -> io::Result<Tree> {
    let mut reader = ffi::Reader::open(archive)?;
    // Items by folder, then name. Folders are added for every path seen, as many archives
    // only list files.
    let mut folders: HashMap<String, BTreeMap<String, Node>> = HashMap::new();
    folders.insert(String::new(), BTreeMap::new());
    let mut files = HashMap::new();
    let mut count = 0usize;
    while let Some(h) = reader.next()? {
        count += 1;
        if count.is_multiple_of(256) && cancel() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        let Some(path) = clean(&h.path) else { continue };
        let kind = match h.filetype {
            ffi::AE_IFDIR => EntryKind::Folder,
            ffi::AE_IFREG => EntryKind::File,
            ffi::AE_IFLNK => EntryKind::Symlink,
            _ => EntryKind::Other,
        };
        // Its folders, top down.
        let mut parent = String::new();
        for name in path.split('/').collect::<Vec<_>>().split_last().unwrap().1 {
            let folder = if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") };
            folders.get_mut(&parent).unwrap().entry(name.to_string()).or_insert(Node {
                kind: EntryKind::Folder,
                size: 0,
                modified: h.mtime,
            });
            folders.entry(folder.clone()).or_default();
            parent = folder;
        }
        let name = path.rsplit('/').next().unwrap().to_string();
        if kind == EntryKind::Folder {
            folders.entry(path.clone()).or_default();
        } else if kind == EntryKind::File {
            files.insert(path.clone(), h.size);
        }
        let size = if kind == EntryKind::File { h.size } else { 0 };
        folders.get_mut(&parent).unwrap().insert(name, Node { kind, size, modified: h.mtime });
        reader.skip()?;
    }

    // Folder sizes: everything below, deepest folders first.
    let mut paths: Vec<&String> = folders.keys().collect();
    paths.sort_by_key(|p| std::cmp::Reverse(if p.is_empty() { 0 } else { p.matches('/').count() + 1 }));
    let mut totals: HashMap<String, u64> = HashMap::new();
    for path in paths {
        let total = folders[path]
            .iter()
            .map(|(name, node)| match node.kind {
                EntryKind::Folder => totals.get(&join(path, name)).copied().unwrap_or(0),
                _ => node.size,
            })
            .sum();
        totals.insert(path.clone(), total);
    }

    let folders = folders
        .iter()
        .map(|(path, items)| {
            let entries = items
                .iter()
                .map(|(name, node)| {
                    let inner = join(path, name);
                    let folder = node.kind == EntryKind::Folder;
                    Entry {
                        name: name.clone(),
                        kind: node.kind,
                        size: if folder { totals.get(&inner).copied().unwrap_or(0) } else { node.size },
                        modified: node.modified,
                        file_id: 0,
                        hidden: name.starts_with('.') || name == "__MACOSX",
                        child_count: folder.then(|| folders.get(&inner).map_or(0, |c| c.len() as u32)),
                        mount_point: false,
                        added: 0,
                    }
                })
                .collect();
            (path.clone(), entries)
        })
        .collect();
    Ok(Tree { folders, files })
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() { name.to_string() } else { format!("{folder}/{name}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// A zip and a tar.gz with the same files, made with the system's tools.
    fn archives() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("sprites/ships")).unwrap();
        fs::write(src.join("README.md"), "# Assets\n").unwrap();
        fs::write(src.join("sprites/ships/scout.png"), vec![7u8; 3000]).unwrap();
        fs::write(src.join("sprites/sheet.json"), "{}").unwrap();
        fs::write(src.join(".hidden"), "x").unwrap();
        let zip = dir.path().join("assets.zip");
        let tgz = dir.path().join("assets.tar.gz");
        assert!(
            Command::new("/usr/bin/zip")
                .args(["-qr", zip.to_str().unwrap(), "."])
                .current_dir(&src)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("/usr/bin/tar")
                .args(["-czf", tgz.to_str().unwrap(), "-C", src.to_str().unwrap(), "."])
                .status()
                .unwrap()
                .success()
        );
        (dir, zip, tgz)
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        let mut v: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        v.sort();
        v
    }

    #[test]
    fn recognises_archives_by_name() {
        for yes in ["a.zip", "B.ZIP", "x.tar.gz", "x.tgz", "y.tar.bz2", "z.7z", "r.rar", "t.tar"] {
            assert!(is_archive_name(yes), "{yes}");
        }
        for no in ["notes.gz", "report.docx", "deck.key", ".zip", "zip", "a.zipx"] {
            assert!(!is_archive_name(no), "{no}");
        }
    }

    #[test]
    fn lists_folders_inside_zips_and_tars() {
        let (_dir, zip, tgz) = archives();
        for archive in [&zip, &tgz] {
            let top = list(archive, "", &|| false).unwrap();
            assert_eq!(names(&top), [".hidden", "README.md", "sprites"], "{archive:?}");
            let sprites = top.iter().find(|e| e.name == "sprites").unwrap();
            assert_eq!(sprites.kind, EntryKind::Folder);
            assert_eq!(sprites.size, 3002, "a folder's size is everything in it");
            assert_eq!(sprites.child_count, Some(2));
            assert!(top.iter().find(|e| e.name == ".hidden").unwrap().hidden);

            let ships = list(archive, "sprites/ships", &|| false).unwrap();
            assert_eq!(names(&ships), ["scout.png"]);
            assert_eq!(ships[0].size, 3000);
            assert!(list(archive, "nope", &|| false).is_err());
        }
    }

    #[test]
    fn splits_paths_inside_archives() {
        let (dir, zip, _) = archives();
        assert_eq!(split(&zip), Some((zip.clone(), String::new())));
        assert_eq!(split(&zip.join("sprites/ships")), Some((zip.clone(), "sprites/ships".into())));
        assert_eq!(split(dir.path()), None);
        assert_eq!(split(&dir.path().join("src/README.md")), None, "a real file");
        assert_eq!(split(&dir.path().join("src/README.md/x")), None, "not an archive");
    }

    #[test]
    fn extracts_files_and_folders() {
        let (dir, zip, tgz) = archives();
        for archive in [&zip, &tgz] {
            let out = dir.path().join(format!("out-{}", archive.file_name().unwrap().to_string_lossy()));
            let paths = extract(archive, &["README.md".into(), "sprites".into()], &out).unwrap();
            assert_eq!(fs::read_to_string(&paths[0]).unwrap(), "# Assets\n");
            assert_eq!(fs::read(paths[1].join("ships/scout.png")).unwrap().len(), 3000);
            assert!(!out.join(".hidden").exists(), "only what was asked for");
            // Again: nothing to do, and it's still there.
            assert_eq!(extract(archive, &["README.md".into()], &out).unwrap()[0], out.join("README.md"));
            assert!(extract(archive, &["missing.txt".into()], &out).is_err());
        }
    }

    #[test]
    fn unsafe_paths_are_cleaned_or_dropped() {
        assert_eq!(clean("./a/b/"), Some("a/b".into()));
        assert_eq!(clean("/etc/passwd"), Some("etc/passwd".into()));
        assert_eq!(clean("../../evil"), None);
        assert_eq!(clean("./"), None);
    }
}
