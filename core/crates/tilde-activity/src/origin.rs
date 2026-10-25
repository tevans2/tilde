//! Where a file came from, read from the extended attributes macOS attaches: browsers and
//! AirDrop set `com.apple.quarantine` (whose agent field names the app), browsers also
//! record the URLs in `kMDItemWhereFroms`, and screenshots and screen recordings get
//! `kMDItemIsScreenCapture` / `kMDItemScreenCaptureType` / `kMDItemIsScreenRecording`.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::Origin;

const WHERE_FROMS: &str = "com.apple.metadata:kMDItemWhereFroms";
const QUARANTINE: &str = "com.apple.quarantine";
const SCREEN_CAPTURE: &[&str] = &[
    "com.apple.metadata:kMDItemIsScreenCapture",
    "com.apple.metadata:kMDItemScreenCaptureType",
    "com.apple.metadata:kMDItemIsScreenRecording",
];
/// Quarantine agents that mean AirDrop.
const AIRDROP_AGENTS: &[&str] = &["sharingd", "AirDrop"];
/// Created this close to its last modification counts as new rather than edited.
const CREATED_SLACK: i64 = 120;

/// The origin of `path` and, for downloads, the site it came from. `created` and
/// `modified` (seconds since the epoch) tell new files from edited ones.
pub fn detect(path: &Path, created: i64, modified: i64) -> (Origin, Option<String>) {
    let names = xattr_names(path);
    let has = |n: &str| names.iter().any(|x| x == n);
    if SCREEN_CAPTURE.iter().any(|n| has(n)) {
        return (Origin::Screenshot, None);
    }
    let agent = has(QUARANTINE).then(|| get_xattr(path, QUARANTINE)).flatten().and_then(|v| quarantine_agent(&v));
    if agent.as_deref().is_some_and(|a| AIRDROP_AGENTS.contains(&a)) {
        return (Origin::AirDrop, None);
    }
    if has(WHERE_FROMS) {
        let source = get_xattr(path, WHERE_FROMS).and_then(|v| source_site(&v));
        return (Origin::Download, source);
    }
    if agent.is_some() {
        return (Origin::Download, None);
    }
    if (modified - created).abs() <= CREATED_SLACK { (Origin::Created, None) } else { (Origin::Edited, None) }
}

/// The app named in a quarantine value (`0281;6addd698;LibreWolf;<uuid>`), if any.
pub(crate) fn quarantine_agent(value: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(value);
    let agent = text.split(';').nth(2)?.trim();
    (!agent.is_empty()).then(|| agent.to_string())
}

/// The site a download came from: the page it was linked from when the browser recorded
/// one (the file URL itself is often a CDN), else the file's own host. `www.` is dropped.
pub(crate) fn source_site(where_froms: &[u8]) -> Option<String> {
    let urls: Vec<String> = plist::from_bytes(where_froms).ok()?;
    urls.get(1).and_then(|u| host(u)).or_else(|| urls.first().and_then(|u| host(u)))
}

fn host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then(|| host.to_lowercase())
}

fn c_path(path: &Path) -> Option<CString> {
    CString::new(path.as_os_str().as_bytes()).ok()
}

fn xattr_names(path: &Path) -> Vec<String> {
    let Some(p) = c_path(path) else { return Vec::new() };
    let mut buf = vec![0u8; 1024];
    loop {
        let n = unsafe { libc::listxattr(p.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), libc::XATTR_NOFOLLOW) };
        if n >= 0 {
            buf.truncate(n as usize);
            break;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::ERANGE) || buf.len() > 1 << 20 {
            return Vec::new();
        }
        buf.resize(buf.len() * 4, 0);
    }
    buf.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

fn get_xattr(path: &Path, name: &str) -> Option<Vec<u8>> {
    let p = c_path(path)?;
    let n = CString::new(name).ok()?;
    let size = unsafe { libc::getxattr(p.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0, 0, libc::XATTR_NOFOLLOW) };
    if size <= 0 || size > 1 << 20 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let read =
        unsafe { libc::getxattr(p.as_ptr(), n.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), 0, libc::XATTR_NOFOLLOW) };
    if read < 0 {
        return None;
    }
    buf.truncate(read as usize);
    Some(buf)
}

#[cfg(test)]
pub(crate) fn set_xattr(path: &Path, name: &str, value: &[u8]) {
    let p = c_path(path).unwrap();
    let n = CString::new(name).unwrap();
    let r = unsafe { libc::setxattr(p.as_ptr(), n.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) };
    assert_eq!(r, 0, "setxattr {name}: {}", std::io::Error::last_os_error());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn where_froms(urls: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &urls.to_vec()).unwrap();
        out
    }

    #[test]
    fn parses_quarantine_agents() {
        assert_eq!(quarantine_agent(b"0281;6addd698;LibreWolf;2077BF43").as_deref(), Some("LibreWolf"));
        assert_eq!(quarantine_agent(b"0081;00000000;;"), None);
        assert_eq!(quarantine_agent(b"garbage"), None);
    }

    #[test]
    fn source_prefers_the_referring_page() {
        let v = where_froms(&["https://objects.githubusercontent.com/x.dmg?sig=1", "https://www.LearnHarness.ai/"]);
        assert_eq!(source_site(&v).as_deref(), Some("learnharness.ai"));
        let v = where_froms(&["http://user@example.com:8080/a"]);
        assert_eq!(source_site(&v).as_deref(), Some("example.com"));
        // Mail attachments record addresses, not URLs.
        let v = where_froms(&["someone@example.com", "message:%3C123%3E"]);
        assert_eq!(source_site(&v), None);
    }

    #[test]
    fn detects_origins_from_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let f = |n: &str| {
            let p = dir.path().join(n);
            std::fs::write(&p, b"x").unwrap();
            p
        };
        let shot = f("shot.png");
        set_xattr(&shot, "com.apple.metadata:kMDItemIsScreenCapture", b"1");
        assert_eq!(detect(&shot, 0, 0).0, Origin::Screenshot);

        let dl = f("file.pdf");
        set_xattr(&dl, QUARANTINE, b"0281;6addd698;Safari;ABC");
        set_xattr(&dl, WHERE_FROMS, &where_froms(&["https://example.org/file.pdf"]));
        assert_eq!(detect(&dl, 0, 0), (Origin::Download, Some("example.org".into())));

        let air = f("photo.heic");
        set_xattr(&air, QUARANTINE, b"0083;6addd698;sharingd;ABC");
        assert_eq!(detect(&air, 0, 0).0, Origin::AirDrop);

        let plain = f("notes.txt");
        // Quarantine without an agent (set by some tools) isn't a download.
        set_xattr(&plain, QUARANTINE, b"0081;00000000;;");
        assert_eq!(detect(&plain, 1000, 1030).0, Origin::Created);
        assert_eq!(detect(&plain, 1000, 9000).0, Origin::Edited);
    }
}
