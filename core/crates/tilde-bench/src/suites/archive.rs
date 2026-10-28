//! Archives as folders: listing a zip and a tar.gz (afresh and from the cache), and
//! unpacking one member or all of them.

use super::Ctx;
use crate::data;
use crate::runner::Runner;

const SUITE: &str = "archive";

pub fn run(r: &mut Runner, ctx: &Ctx) {
    let (zip, tgz) = data::archives(ctx.archive_members);
    let n = data::label(ctx.archive_members);
    for (kind, archive) in [("zip", &zip), ("tar.gz", &tgz)] {
        let ds = format!("{kind}-{n}");
        r.bench_setup(SUITE, &ds, "list top, afresh", tilde_archive::clear_cache, |_| {
            tilde_archive::list(archive, "", &|| false).map(|e| e.len()).unwrap_or(0)
        });
        let _ = tilde_archive::list(archive, "", &|| false);
        r.bench(SUITE, &ds, "list top, cached", || {
            tilde_archive::list(archive, "", &|| false).map(|e| e.len()).unwrap_or(0)
        });
        // A member to unpack: the first file in the first folder.
        let Some(member) = first_file(archive) else { continue };
        r.bench_setup(
            SUITE,
            &ds,
            "unpack one file",
            || tempfile::tempdir_in(&ctx.scratch).expect("scratch"),
            |dest| {
                tilde_archive::extract(archive, std::slice::from_ref(&member), dest.path())
                    .map(|p| p.len())
                    .unwrap_or(0)
            },
        );
        r.bench_slow(
            SUITE,
            &ds,
            "unpack everything",
            || tempfile::tempdir_in(&ctx.scratch).expect("scratch"),
            |dest| {
                tilde_archive::extract(archive, &[String::new()], dest.path()).map_err(|e| e.to_string())?;
                Ok(Default::default())
            },
        );
    }
}

fn first_file(archive: &std::path::Path) -> Option<String> {
    let mut inner = String::new();
    loop {
        let entries = tilde_archive::list(archive, &inner, &|| false).ok()?;
        if let Some(file) = entries.iter().find(|e| e.kind == tilde_types::EntryKind::File) {
            return Some(if inner.is_empty() { file.name.clone() } else { format!("{inner}/{}", file.name) });
        }
        let folder = entries.iter().find(|e| e.kind == tilde_types::EntryKind::Folder)?;
        inner = if inner.is_empty() { folder.name.clone() } else { format!("{inner}/{}", folder.name) };
    }
}
