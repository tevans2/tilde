//! Saving the index between launches.
//!
//! A flat binary file: header (magic, format version, config fingerprint, the FSEvents
//! ID it's current to, item count), then one record per item, then what folders hold
//! that isn't an item (for folder totals): a count, then per folder its key, the bytes
//! of its hidden files and the folders inside it the index doesn't enter, with their
//! measured sizes. Written to a temporary
//! file and renamed into place, so a crash mid-save leaves the old file intact.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use tilde_types::EntryKind;

use crate::ItemMeta;
use crate::live::{SavedFolder, Skipped};

const MAGIC: &[u8; 8] = b"TILDEIX\0";
const VERSION: u32 = 2;
/// A skipped folder whose size isn't known.
const UNKNOWN: u64 = u64::MAX;

pub(crate) struct Saved {
    pub last_event_id: u64,
    pub items: Vec<(String, ItemMeta)>,
    pub folders: Vec<SavedFolder>,
}

pub(crate) fn save(
    path: &Path,
    fingerprint: u64,
    last_event_id: u64,
    items: &[(String, ItemMeta)],
    folders: &[SavedFolder],
) -> io::Result<u64> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut w = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    w.write_all(&fingerprint.to_le_bytes())?;
    w.write_all(&last_event_id.to_le_bytes())?;
    w.write_all(&(items.len() as u64).to_le_bytes())?;
    for (key, meta) in items {
        write_str(&mut w, key)?;
        w.write_all(&[kind_code(meta.kind)])?;
        w.write_all(&meta.size.to_le_bytes())?;
        w.write_all(&meta.modified.to_le_bytes())?;
        w.write_all(&meta.child_count.unwrap_or(u32::MAX).to_le_bytes())?;
    }
    w.write_all(&(folders.len() as u64).to_le_bytes())?;
    for folder in folders {
        write_str(&mut w, &folder.key)?;
        w.write_all(&folder.unindexed_files.to_le_bytes())?;
        w.write_all(&(folder.skipped.len() as u32).to_le_bytes())?;
        for s in &folder.skipped {
            write_str(&mut w, &s.name)?;
            w.write_all(&s.size.unwrap_or(UNKNOWN).to_le_bytes())?;
            w.write_all(&[s.stale as u8])?;
        }
    }
    w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(std::fs::metadata(path)?.len())
}

/// Reads a saved index. `None` if it's missing, from another format version, made with a
/// different configuration, or damaged.
pub(crate) fn load(path: &Path, fingerprint: u64) -> Option<Saved> {
    let mut r = BufReader::with_capacity(1 << 20, File::open(path).ok()?);
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic).ok()?;
    if &magic != MAGIC || read_u32(&mut r)? != VERSION || read_u64(&mut r)? != fingerprint {
        return None;
    }
    let last_event_id = read_u64(&mut r)?;
    let count = read_u64(&mut r)? as usize;
    let mut items = Vec::with_capacity(count.min(10_000_000));
    for _ in 0..count {
        let key = read_str(&mut r)?;
        let mut kind = [0u8; 1];
        r.read_exact(&mut kind).ok()?;
        let size = read_u64(&mut r)?;
        let modified = read_u64(&mut r)? as i64;
        let child_count = read_u32(&mut r)?;
        let meta = ItemMeta {
            id: 0,
            kind: kind_from(kind[0])?,
            size,
            modified,
            child_count: (child_count != u32::MAX).then_some(child_count),
            extra: false,
        };
        items.push((key, meta));
    }
    let count = read_u64(&mut r)? as usize;
    let mut folders = Vec::with_capacity(count.min(1_000_000));
    for _ in 0..count {
        let key = read_str(&mut r)?;
        let unindexed_files = read_u64(&mut r)?;
        let skipped_count = read_u32(&mut r)? as usize;
        let mut skipped = Vec::with_capacity(skipped_count.min(100_000));
        for _ in 0..skipped_count {
            let name = read_str(&mut r)?.into_boxed_str();
            let size = read_u64(&mut r)?;
            let mut stale = [0u8; 1];
            r.read_exact(&mut stale).ok()?;
            skipped.push(Skipped { name, size: (size != UNKNOWN).then_some(size), stale: stale[0] != 0 });
        }
        folders.push(SavedFolder { key, unindexed_files, skipped });
    }
    Some(Saved { last_event_id, items, folders })
}

fn write_str(w: &mut impl Write, s: &str) -> io::Result<()> {
    let len = u16::try_from(s.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "path too long"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(s.as_bytes())
}

fn read_str(r: &mut impl Read) -> Option<String> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len).ok()?;
    let mut bytes = vec![0u8; u16::from_le_bytes(len) as usize];
    r.read_exact(&mut bytes).ok()?;
    String::from_utf8(bytes).ok()
}

fn kind_code(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::File => 0,
        EntryKind::Folder => 1,
        EntryKind::Package => 2,
        EntryKind::Symlink => 3,
        EntryKind::Other => 4,
    }
}

fn kind_from(code: u8) -> Option<EntryKind> {
    Some(match code {
        0 => EntryKind::File,
        1 => EntryKind::Folder,
        2 => EntryKind::Package,
        3 => EntryKind::Symlink,
        4 => EntryKind::Other,
        _ => return None,
    })
}

fn read_u32(r: &mut impl Read) -> Option<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

fn read_u64(r: &mut impl Read) -> Option<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).ok()?;
    Some(u64::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(kind: EntryKind, size: u64, child_count: Option<u32>) -> ItemMeta {
        ItemMeta { id: 0, kind, size, modified: 1_700_000_000, child_count, extra: false }
    }

    #[test]
    fn round_trips_and_rejects_mismatches() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cache/index.bin");
        let items = vec![
            ("Documents".to_string(), meta(EntryKind::Folder, 0, Some(3))),
            ("Documents/café notes.md".to_string(), meta(EntryKind::File, 1234, None)),
            ("/opt/homebrew".to_string(), meta(EntryKind::Folder, 0, None)),
        ];
        let folders = vec![SavedFolder {
            key: "Documents".into(),
            unindexed_files: 6_148,
            skipped: vec![
                Skipped { name: ".git".into(), size: Some(52_000), stale: true },
                Skipped { name: "node_modules".into(), size: None, stale: false },
            ],
        }];
        let size = save(&p, 42, 9_000, &items, &folders).unwrap();
        assert!(size > 0);

        let loaded = load(&p, 42).unwrap();
        assert_eq!(loaded.last_event_id, 9_000);
        assert_eq!(loaded.items, items);
        assert_eq!(loaded.folders, folders);

        assert!(load(&p, 43).is_none(), "different config");
        std::fs::write(&p, b"TILDEIX\0garbage").unwrap();
        assert!(load(&p, 42).is_none(), "damaged");
        assert!(load(&dir.path().join("missing"), 42).is_none());
    }
}
