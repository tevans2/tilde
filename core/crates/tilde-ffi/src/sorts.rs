//! Sort orders chosen for particular folders (`s` in SELECT), kept in
//! `~/Library/Application Support/Tilde/sort.json` as `{ "/path": "added" }`, with a
//! leading `-` for reversed. The file is re-read on every lookup, so all windows share it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tilde_types::{SortKey, SortOrder};

use crate::index::home_dir;

/// Most folders remembered; beyond this, entries are dropped (alphabetically first).
const MAX_FOLDERS: usize = 2000;

pub(crate) struct SortMemory {
    /// `None` keeps everything in memory (tests).
    file: Option<PathBuf>,
    memory: BTreeMap<String, String>,
}

impl SortMemory {
    pub(crate) fn on_disk() -> SortMemory {
        SortMemory {
            file: Some(home_dir().join("Library/Application Support/Tilde/sort.json")),
            memory: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> SortMemory {
        SortMemory { file: None, memory: BTreeMap::new() }
    }

    fn load(&self) -> BTreeMap<String, String> {
        match &self.file {
            Some(f) => std::fs::read_to_string(f).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default(),
            None => self.memory.clone(),
        }
    }

    /// The order remembered for `folder`, if any (`folders_first` comes from `default`).
    pub(crate) fn get(&self, folder: &Path, default: SortOrder) -> Option<SortOrder> {
        let saved = self.load().remove(folder.to_string_lossy().as_ref())?;
        let (reverse, name) = match saved.strip_prefix('-') {
            Some(name) => (true, name),
            None => (false, saved.as_str()),
        };
        Some(SortOrder { key: SortKey::parse(name)?, reverse, ..default })
    }

    /// Remembers `order` for `folder`; the default order is forgotten instead.
    pub(crate) fn set(&mut self, folder: &Path, order: SortOrder, default: SortOrder) {
        let mut map = self.load();
        let key = folder.to_string_lossy().into_owned();
        if order.key == default.key && order.reverse == default.reverse {
            map.remove(&key);
        } else {
            map.insert(key, format!("{}{}", if order.reverse { "-" } else { "" }, order.key.as_str()));
        }
        while map.len() > MAX_FOLDERS {
            let first = map.keys().next().cloned().unwrap();
            map.remove(&first);
        }
        match &self.file {
            Some(f) => {
                if let Some(dir) = f.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let tmp = f.with_extension("tmp");
                if let Ok(text) = serde_json::to_string_pretty(&map)
                    && std::fs::write(&tmp, text).is_ok()
                {
                    let _ = std::fs::rename(&tmp, f);
                }
            }
            None => self.memory = map,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_on_disk_and_forgets_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state/sort.json");
        let mut m = SortMemory { file: Some(file.clone()), memory: BTreeMap::new() };
        let default = SortOrder::default();
        let downloads = Path::new("/Users/me/Downloads");
        assert_eq!(m.get(downloads, default), None);

        let by_added = SortOrder { key: SortKey::Added, reverse: true, ..default };
        m.set(downloads, by_added, default);
        assert!(std::fs::read_to_string(&file).unwrap().contains("\"-added\""));
        // Another window (a fresh reader) sees it.
        let other = SortMemory { file: Some(file.clone()), memory: BTreeMap::new() };
        assert_eq!(other.get(downloads, default), Some(by_added));

        m.set(downloads, default, default);
        assert_eq!(other.get(downloads, default), None);
    }
}
