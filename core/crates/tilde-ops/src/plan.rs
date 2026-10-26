//! Turns a request into concrete steps, catching problems before anything is touched.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::names::{keep_both, validate_name};
use crate::{BatchItem, Clash, Request, Step};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Items given a "keep both" name because the destination was taken.
    pub renamed: u32,
    /// Existing items moved to the Trash to make way (`Clash::Replace`).
    pub replaced: u32,
    /// Items left alone: nothing to do (moving into the folder they're in, tags they
    /// already have) or `Clash::Skip`.
    pub skipped: u32,
}

/// The sources of a move or copy whose name is already taken in the destination (the
/// clashes `Clash` decides about). Copies into their own folder are duplicates and
/// always keep both, so they aren't included.
pub fn clashes(request: &Request) -> Vec<PathBuf> {
    match request {
        Request::Move { sources, into, .. } | Request::Copy { sources, into, .. } => sources
            .iter()
            .filter(|s| s.parent() != Some(into.as_path()))
            .filter(|s| s.file_name().is_some_and(|n| into.join(n).symlink_metadata().is_ok()))
            .cloned()
            .collect(),
        _ => Vec::new(),
    }
}

/// Plans `request`. Errors are written for the user.
pub fn plan(request: &Request) -> Result<Plan, String> {
    let mut p = Plan { steps: Vec::new(), renamed: 0, replaced: 0, skipped: 0 };
    match request {
        Request::Move { sources, into, clash } | Request::Copy { sources, into, clash } => {
            let copying = matches!(request, Request::Copy { .. });
            require_dir(into)?;
            check_sources(sources)?;
            let mut reserved = HashSet::new();
            for src in sources {
                if src.is_dir() && (into == src || into.starts_with(src)) {
                    return Err(format!(
                        "Can't {} “{}” into itself.",
                        if copying { "copy" } else { "move" },
                        name(src)
                    ));
                }
                let in_place = src.parent() == Some(into.as_path());
                if !copying && in_place {
                    p.skipped += 1;
                    continue;
                }
                let target = into.join(name(src));
                // Duplicates in their own folder, and second items with the same name in
                // one request, always keep both.
                let decide = !in_place && !reserved.contains(&target) && target.symlink_metadata().is_ok();
                let to = match (decide, clash) {
                    (true, Clash::Skip) => {
                        p.skipped += 1;
                        continue;
                    }
                    (true, Clash::Replace) => {
                        if src.starts_with(&target) {
                            return Err(format!("Can't replace “{}” with an item inside it.", name(&target)));
                        }
                        p.steps.push(Step::Trash { path: target.clone() });
                        p.replaced += 1;
                        reserved.insert(target.clone());
                        target
                    }
                    _ => free_name(into, src, &mut reserved, &mut p.renamed),
                };
                p.steps.push(if copying {
                    Step::Copy { from: src.clone(), to }
                } else {
                    Step::Move { from: src.clone(), to }
                });
            }
        }
        Request::Trash { sources } => {
            check_sources(sources)?;
            p.steps.extend(sources.iter().map(|s| Step::Trash { path: s.clone() }));
        }
        Request::Rename { source, name: new_name } => {
            check_sources(std::slice::from_ref(source))?;
            validate_name(new_name)?;
            let to = source.with_file_name(new_name);
            if to == *source {
                p.skipped += 1;
            } else if to.symlink_metadata().is_ok() && !crate::fsops::same_file(source, &to) {
                return Err(format!("“{new_name}” already exists here."));
            } else {
                // Includes case-only renames, which rename(2) handles directly.
                p.steps.push(Step::Move { from: source.clone(), to });
            }
        }
        Request::NewFolder { parent, name: new_name } => {
            require_dir(parent)?;
            validate_name(new_name)?;
            let path = parent.join(new_name);
            if path.symlink_metadata().is_ok() {
                return Err(format!("“{new_name}” already exists here."));
            }
            p.steps.push(Step::MakeDir { path });
        }
        Request::Batch { items, .. } => plan_batch(items, &mut p)?,
        Request::Tag { sources, add, remove } => {
            check_sources(sources)?;
            for tag in add {
                validate_tag(tag)?;
            }
            let same = |a: &str, b: &str| a.trim().to_lowercase() == b.trim().to_lowercase();
            for src in sources {
                let current = crate::fsops::tags(src).map_err(|e| format!("“{}”: {e}", name(src)))?;
                let mut tags: Vec<String> =
                    current.iter().filter(|t| !remove.iter().any(|r| same(t, r))).cloned().collect();
                for tag in add {
                    if !tags.iter().any(|t| same(t, tag)) {
                        tags.push(tag.trim().to_string());
                    }
                }
                if tags == current {
                    p.skipped += 1;
                } else {
                    p.steps.push(Step::SetTags { path: src.clone(), tags });
                }
            }
        }
        Request::Compress { sources } => {
            check_sources(sources)?;
            let first = &sources[0];
            let dir = first.parent().ok_or("Can't compress the top of the disk.")?;
            let base = if sources.len() == 1 { format!("{}.zip", name(first)) } else { "Archive.zip".to_string() };
            let to = keep_both(dir, &base, false, |c| c.symlink_metadata().is_ok());
            if to.file_name().map(|n| n.to_string_lossy() != base).unwrap_or(false) {
                p.renamed += 1;
            }
            p.steps.push(Step::Compress { sources: sources.clone(), to });
        }
    }
    Ok(p)
}

/// Plans a mixed batch so every step can succeed in order: trash first (freeing names),
/// then new folders (shallowest first, so items can move into them), then moves (through
/// temporary names when one item takes another's name), then copies (from wherever their
/// source ended up), then new files.
fn plan_batch(items: &[BatchItem], p: &mut Plan) -> Result<(), String> {
    // Case-insensitive, like the default macOS volume, to catch clashes conservatively.
    let key = |path: &Path| path.to_string_lossy().to_lowercase();

    // Sources exist, and nothing leaves twice.
    let mut leaving = HashSet::new();
    for item in items {
        match item {
            BatchItem::Move { from, to } if from != to => {
                check_sources(std::slice::from_ref(from))?;
                if !leaving.insert(key(from)) {
                    return Err(format!("“{}” is changed twice.", name(from)));
                }
                let _ = to;
            }
            BatchItem::Trash { path } => {
                check_sources(std::slice::from_ref(path))?;
                if !leaving.insert(key(path)) {
                    return Err(format!("“{}” is changed twice.", name(path)));
                }
            }
            BatchItem::Copy { from, .. } => check_sources(std::slice::from_ref(from))?,
            _ => {}
        }
    }

    // Destinations: valid, unique, free (or freed by this batch), with a parent folder that
    // exists or is created here and isn't itself moving.
    let new_dirs: HashSet<String> = items
        .iter()
        .filter_map(|i| if let BatchItem::CreateDir { path } = i { Some(key(path)) } else { None })
        .collect();
    let mut arriving = HashSet::new();
    for item in items {
        let to = match item {
            BatchItem::Move { from, to } if from != to => to,
            BatchItem::Copy { to, .. } | BatchItem::CreateFile { path: to } | BatchItem::CreateDir { path: to } => to,
            _ => continue,
        };
        validate_name(&name(to))?;
        if !arriving.insert(key(to)) {
            return Err(format!("Two items would both be called “{}”.", name(to)));
        }
        if to.symlink_metadata().is_ok() && !leaving.contains(&key(to)) {
            return Err(format!("“{}” already exists.", name(to)));
        }
        let parent = to.parent().ok_or("Can't create items at the top of the disk.")?;
        if leaving.contains(&key(parent)) {
            return Err(format!(
                "“{}” is renamed or removed in the same edit; use its new name for items inside it.",
                name(parent)
            ));
        }
        if !parent.is_dir() && !new_dirs.contains(&key(parent)) {
            return Err(format!("The folder “{}” doesn't exist.", name(parent)));
        }
    }

    for item in items {
        if let BatchItem::Trash { path } = item {
            p.steps.push(Step::Trash { path: path.clone() });
        }
    }
    let mut dirs: Vec<&PathBuf> =
        items.iter().filter_map(|i| if let BatchItem::CreateDir { path } = i { Some(path) } else { None }).collect();
    dirs.sort_by_key(|d| d.components().count());
    p.steps.extend(dirs.into_iter().map(|d| Step::MakeDir { path: d.clone() }));

    let moves: Vec<(&PathBuf, &PathBuf)> = items
        .iter()
        .filter_map(|i| match i {
            BatchItem::Move { from, to } if from != to => Some((from, to)),
            _ => None,
        })
        .collect();
    let sources: HashSet<String> = moves.iter().map(|(f, _)| key(f)).collect();
    // A swap (a→b, b→a) or chain needs every move to park under a temporary name first.
    let needs_temp = moves.iter().any(|(f, t)| key(f) != key(t) && sources.contains(&key(t)));
    if needs_temp {
        let parked: Vec<(PathBuf, &PathBuf)> = moves
            .iter()
            .enumerate()
            .map(|(i, (from, to))| {
                let tmp = from.with_file_name(format!(".tilde-moving-{}-{i}", std::process::id()));
                p.steps.push(Step::Move { from: (*from).clone(), to: tmp.clone() });
                (tmp, *to)
            })
            .collect();
        for (tmp, to) in parked {
            p.steps.push(Step::Move { from: tmp, to: to.clone() });
        }
    } else {
        p.steps.extend(moves.iter().map(|(f, t)| Step::Move { from: (*f).clone(), to: (*t).clone() }));
    }

    for item in items {
        if let BatchItem::Copy { from, to } = item {
            let source = moves.iter().find(|(f, _)| *f == from).map_or(from, |(_, t)| *t);
            p.steps.push(Step::Copy { from: source.clone(), to: to.clone() });
        }
    }
    for item in items {
        if let BatchItem::CreateFile { path } = item {
            p.steps.push(Step::CreateFile { path: path.clone() });
        }
    }
    Ok(())
}

fn validate_tag(tag: &str) -> Result<(), String> {
    if tag.trim().is_empty() {
        return Err("A tag can't be empty.".into());
    }
    if tag.contains('\n') || tag.contains(',') {
        return Err("Tags can't contain commas or line breaks.".into());
    }
    Ok(())
}

fn require_dir(dir: &Path) -> Result<(), String> {
    if dir.is_dir() { Ok(()) } else { Err(format!("“{}” isn't a folder that exists.", name(dir))) }
}

fn check_sources(sources: &[PathBuf]) -> Result<(), String> {
    if sources.is_empty() {
        return Err("Nothing selected.".into());
    }
    let mut seen = HashSet::new();
    for s in sources {
        if s.symlink_metadata().is_err() {
            return Err(format!("“{}” no longer exists.", name(s)));
        }
        if !seen.insert(s) {
            return Err(format!("“{}” is selected twice.", name(s)));
        }
    }
    Ok(())
}

/// The destination for `src` inside `dir`, avoiding existing items and earlier items in
/// the same batch.
fn free_name(dir: &Path, src: &Path, reserved: &mut HashSet<PathBuf>, renamed: &mut u32) -> PathBuf {
    let n = name(src);
    let to = keep_both(dir, &n, src.is_dir(), |c| reserved.contains(c) || c.symlink_metadata().is_ok());
    if to != dir.join(&n) {
        *renamed += 1;
    }
    reserved.insert(to.clone());
    to
}

pub(crate) fn name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned())
}
