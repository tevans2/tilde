//! Data to measure on: a synthetic home folder (deterministic, shaped like a real one),
//! written to disk when a suite needs real files, and the user's own index.
//!
//! On-disk trees are cached under `target/tilde-bench-data` and reused, so runs compare
//! like with like and don't spend minutes writing files. A tree is rebuilt only when its
//! version changes. Files are sparse: they have their logical size without using disk.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use tilde_index::{IndexConfig, ItemMeta, ScanConfig};
use tilde_types::EntryKind;

/// "Now" for synthetic data, so date filters find the same items every run.
pub const NOW: i64 = 1_790_000_000;
/// Bump to rebuild cached trees after changing the generator.
const VERSION: u32 = 2;

pub const WORDS: &[&str] = &[
    "report",
    "invoice",
    "notes",
    "draft",
    "final",
    "budget",
    "plan",
    "photo",
    "travel",
    "receipt",
    "project",
    "design",
    "meeting",
    "summary",
    "config",
    "server",
    "client",
    "render",
    "shader",
    "engine",
    "parser",
    "index",
    "search",
    "cache",
    "worker",
    "model",
    "router",
    "player",
    "camera",
    "physics",
    "audio",
    "network",
    "storage",
    "settings",
    "theme",
    "layout",
    "widget",
    "manager",
    "service",
    "handler",
    "loader",
    "export",
    "import",
    "starfield",
    "aurora",
    "nebula",
    "orbit",
    "galaxy",
    "sourdough",
    "garden",
    "synth",
    "pixel",
    "warp",
    "comet",
];
const UNICODE: &[&str] = &["Résumé", "café", "Übersicht", "日本旅行", "Ελλάδα", "naïve", "Málaga", "Zürich", "北京"];
const CODE_EXT: &[&str] = &["rs", "ts", "tsx", "js", "swift", "py", "go", "md", "json", "toml", "yaml", "css"];
const DOC_EXT: &[&str] = &["pdf", "docx", "md", "xlsx", "pages", "txt", "key", "csv"];
const DOWNLOAD_EXT: &[&str] = &["dmg", "zip", "pdf", "jpg", "png", "pkg", "mp4", "csv", "epub"];
const ARTISTS: &[&str] =
    &["Bonobo", "Tycho", "Khruangbin", "Nils Frahm", "Floating Points", "Caribou", "Four Tet", "Jon Hopkins"];

/// SplitMix64: deterministic, fast, no dependencies.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    pub fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }

    pub fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len() as u64) as usize]
    }

    /// Log-uniform between `lo` and `hi`: many small values, a few large ones.
    pub fn log_between(&mut self, lo: u64, hi: u64) -> u64 {
        let (a, b) = ((lo.max(1) as f64).ln(), (hi.max(2) as f64).ln());
        let u = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        (a + (b - a) * u).exp() as u64
    }
}

/// One item of a synthetic home, relative to it.
#[derive(Debug, Clone)]
pub struct Node {
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: i64,
}

/// A file size, drawn when the file is made.
enum Size {
    Range(u64, u64),
    /// Log-uniform: mostly small, a few large.
    Log(u64, u64),
}

struct Builder {
    rng: Rng,
    nodes: Vec<Node>,
    dirs: HashSet<String>,
    target: usize,
}

impl Builder {
    fn full(&self) -> bool {
        self.nodes.len() >= self.target
    }

    fn age(&mut self) -> i64 {
        // Most things are old, some are from this week.
        NOW - self.rng.log_between(60, 6 * 365 * 86_400) as i64
    }

    fn dir(&mut self, path: &str) {
        if path.is_empty() || self.dirs.contains(path) {
            return;
        }
        if let Some((parent, _)) = path.rsplit_once('/') {
            self.dir(parent);
        }
        self.dirs.insert(path.to_string());
        let modified = self.age();
        self.nodes.push(Node { path: path.to_string(), kind: EntryKind::Folder, size: 0, modified });
    }

    fn file(&mut self, dir: &str, name: &str, size: Size) {
        if self.full() {
            return;
        }
        let size = match size {
            Size::Range(lo, hi) => self.rng.range(lo, hi),
            Size::Log(lo, hi) => self.rng.log_between(lo, hi),
        };
        self.dir(dir);
        let path = format!("{dir}/{name}");
        if self.dirs.contains(&path) {
            return;
        }
        let modified = self.age();
        self.nodes.push(Node { path, kind: EntryKind::File, size, modified });
    }

    fn word(&mut self) -> String {
        if self.rng.chance(0.02) { self.rng.pick(UNICODE).to_string() } else { self.rng.pick(WORDS).to_string() }
    }

    fn project(&mut self) {
        let name = format!("{}-{}", self.word(), self.word()).to_lowercase();
        let base = format!("Developer/{name}");
        self.file(&base, "README.md", Size::Range(500, 20_000));
        let manifest = self.rng.pick(&["Cargo.toml", "package.json", "Package.swift", "pyproject.toml", "go.mod"]);
        self.file(&base, manifest, Size::Range(200, 4_000));
        self.file(&format!("{base}/.git"), "HEAD", Size::Range(23, 23));
        for _ in 0..self.rng.range(5, 30) {
            let (dir, name) =
                (format!("{base}/.git/objects/{:02x}", self.rng.below(256)), format!("{:038x}", self.rng.next()));
            self.file(&dir, &name, Size::Range(100, 40_000));
        }
        for _ in 0..self.rng.range(2, 8) {
            let module = format!("{base}/src/{}", self.word().to_lowercase());
            let ext = self.rng.pick(CODE_EXT);
            for _ in 0..self.rng.range(3, 18) {
                let file = format!("{}_{}.{ext}", self.word().to_lowercase(), self.word().to_lowercase());
                self.file(&module, &file, Size::Log(300, 80_000));
            }
        }
        for _ in 0..self.rng.range(1, 10) {
            let file = format!("{}_test.{}", self.word().to_lowercase(), self.rng.pick(CODE_EXT));
            self.file(&format!("{base}/tests"), &file, Size::Range(500, 30_000));
        }
        if self.rng.chance(0.15) {
            // A dependency folder: many small files, deep.
            for _ in 0..self.rng.range(5, 25) {
                let pkg = format!("{base}/node_modules/{}-{}", self.word().to_lowercase(), self.rng.below(100));
                self.file(&pkg, "package.json", Size::Range(300, 3_000));
                self.file(&pkg, "README.md", Size::Range(500, 15_000));
                for _ in 0..self.rng.range(2, 12) {
                    let file = format!("{}.js", self.word().to_lowercase());
                    self.file(&format!("{pkg}/lib"), &file, Size::Log(200, 60_000));
                }
            }
        }
        if self.rng.chance(0.25) {
            for _ in 0..self.rng.range(10, 40) {
                let file = format!("{:016x}.o", self.rng.next());
                self.file(&format!("{base}/target/debug/deps"), &file, Size::Log(1_000, 4_000_000));
            }
        }
    }

    fn photos(&mut self) {
        let year = self.rng.range(2019, 2026);
        let dir = format!("Pictures/Photos {year}/{:02}", self.rng.range(1, 12));
        for _ in 0..self.rng.range(15, 120) {
            let ext = if self.rng.chance(0.7) { "HEIC" } else { "JPG" };
            let name = format!("IMG_{:04}.{ext}", self.rng.below(10_000));
            self.file(&dir, &name, Size::Range(1_500_000, 7_000_000));
        }
    }

    fn documents(&mut self) {
        let topic = self.word();
        let dir = if self.rng.chance(0.5) {
            format!("Documents/{topic}/{}", self.rng.range(2018, 2026))
        } else {
            format!("Documents/{topic}")
        };
        for _ in 0..self.rng.range(4, 30) {
            let ext = self.rng.pick(DOC_EXT);
            let name = format!("{} {} {}.{ext}", self.word(), self.word(), self.rng.range(2018, 2026));
            self.file(&dir, &name, Size::Log(5_000, 8_000_000));
        }
    }

    fn music(&mut self) {
        let artist = self.rng.pick(ARTISTS);
        let dir = format!("Music/{artist}/{} {}", self.word(), self.word());
        for track in 1..=self.rng.range(8, 14) {
            let name = format!("{track:02} {} {}.m4a", self.word(), self.word());
            self.file(&dir, &name, Size::Range(3_000_000, 12_000_000));
        }
    }

    fn downloads(&mut self) {
        for _ in 0..self.rng.range(3, 15) {
            let ext = self.rng.pick(DOWNLOAD_EXT);
            let name = format!("{}-{}-{}.{ext}", self.word(), self.word(), self.rng.below(1000));
            let size = if ext == "dmg" || ext == "mp4" {
                Size::Log(20_000_000, 900_000_000)
            } else {
                Size::Log(10_000, 50_000_000)
            };
            self.file("Downloads", &name, size);
        }
    }

    fn desktop(&mut self) {
        for _ in 0..self.rng.range(1, 6) {
            let name = format!(
                "Screenshot 2026-{:02}-{:02} at {:02}.{:02}.{:02}.png",
                self.rng.range(1, 12),
                self.rng.range(1, 28),
                self.rng.below(24),
                self.rng.below(60),
                self.rng.below(60)
            );
            self.file("Desktop", &name, Size::Range(200_000, 3_000_000));
        }
    }

    fn movies(&mut self) {
        let name = format!("{} {}.mov", self.word(), self.word());
        self.file("Movies", &name, Size::Log(50_000_000, 1 << 30));
    }
}

/// A home folder of about `n` items (folders and files), the same for the same `n`.
pub fn home(n: usize) -> Vec<Node> {
    let mut b = Builder {
        rng: Rng::new(0x7110_DE00 + n as u64),
        nodes: Vec::with_capacity(n),
        dirs: HashSet::new(),
        target: n,
    };
    while !b.full() {
        match b.rng.below(100) {
            0..=54 => b.project(),
            55..=69 => b.photos(),
            70..=81 => b.documents(),
            82..=89 => b.music(),
            90..=94 => b.downloads(),
            95..=97 => b.desktop(),
            _ => b.movies(),
        }
    }
    b.nodes.truncate(n);
    b.nodes
}

/// `home(n)` as saved-index items.
pub fn items(nodes: &[Node]) -> Vec<(String, ItemMeta)> {
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let meta = ItemMeta {
                id: i as u32,
                kind: n.kind,
                size: n.size,
                modified: n.modified,
                child_count: None,
                extra: false,
            };
            (n.path.clone(), meta)
        })
        .collect()
}

/// Index settings for a synthetic home at `dir` (like Tilde's defaults: hidden files and
/// dependency folders' contents left out).
pub fn index_config(dir: &Path, cache: Option<PathBuf>) -> IndexConfig {
    IndexConfig {
        home: dir.to_path_buf(),
        scan: ScanConfig {
            roots: vec![dir.to_path_buf()],
            exclude_names: ["node_modules", ".git", "target", "build", "DerivedData"].map(String::from).into(),
            ..Default::default()
        },
        frecency: None,
        zoxide: false,
        max_items: 50_000_000,
        cache_path: cache,
    }
}

/// Where cached trees live.
pub fn cache_root() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    target.join("tilde-bench-data")
}

/// A cached tree: built by `make` into a fresh folder once, reused afterwards.
fn cached(name: &str, make: impl FnOnce(&Path)) -> PathBuf {
    let dir = cache_root().join(format!("{name}-v{VERSION}"));
    let done = dir.join(".tilde-bench-complete");
    if !done.exists() {
        eprintln!("  building {name} in {} (once)…", dir.display());
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create bench data folder");
        make(&dir);
        fs::write(&done, b"").expect("mark bench data complete");
    }
    dir
}

/// Writes `nodes` under `root`: folders, and sparse files of their sizes and dates.
pub fn write_nodes(root: &Path, nodes: &[Node]) {
    for n in nodes {
        let path = root.join(&n.path);
        match n.kind {
            EntryKind::Folder => fs::create_dir_all(&path).expect("create folder"),
            _ => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).expect("create folder");
                }
                let f = fs::File::create(&path).expect("create file");
                f.set_len(n.size).expect("size file");
                let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(n.modified as u64);
                let _ = f.set_modified(t);
            }
        }
    }
}

/// A synthetic home of about `n` items on disk.
pub fn home_tree(n: usize) -> PathBuf {
    cached(&format!("home-{n}"), |dir| write_nodes(dir, &home(n)))
}

/// One folder holding `n` files directly.
pub fn flat_folder(n: usize) -> PathBuf {
    cached(&format!("flat-{n}"), |dir| {
        let mut rng = Rng::new(n as u64);
        for i in 0..n {
            let ext = rng.pick(&["pdf", "png", "rs", "md", "jpg", "zip", "txt", "mov"]);
            let name = format!("{} {} {i}.{ext}", rng.pick(WORDS), rng.pick(WORDS));
            let f = fs::File::create(dir.join(name)).expect("create file");
            f.set_len(rng.log_between(100, 50_000_000)).expect("size file");
        }
        for i in 0..n / 20 {
            fs::create_dir(dir.join(format!("{} folder {i}", rng.pick(WORDS)))).expect("create folder");
        }
    })
}

/// `n` text files of 2–16 KB across folders; about 1 in 100 says "warpdrive", 1 in 20 is
/// binary.
pub fn text_tree(n: usize) -> PathBuf {
    cached(&format!("text-{n}"), |dir| {
        let mut rng = Rng::new(0x7E47 + n as u64);
        for i in 0..n {
            let folder = dir.join(format!("{}/{}", rng.pick(WORDS), rng.pick(WORDS)));
            fs::create_dir_all(&folder).expect("create folder");
            let binary = rng.chance(0.05);
            let mut body = String::new();
            let target = rng.range(2_000, 16_000) as usize;
            while body.len() < target {
                body.push_str(rng.pick(WORDS));
                body.push(if rng.chance(0.08) { '\n' } else { ' ' });
            }
            if rng.chance(0.01) {
                body.insert_str(body.len() / 2, " the warpdrive spins up ");
            }
            let mut bytes = body.into_bytes();
            if binary {
                bytes[10] = 0;
            }
            let ext = if binary { "bin" } else { rng.pick(&["md", "txt", "rs", "ts", "json"]) };
            fs::write(folder.join(format!("{}-{i}.{ext}", rng.pick(WORDS))), bytes).expect("write file");
        }
    })
}

/// A zip and a tar.gz of `n` small text files in folders (made with the system's tools).
pub fn archives(n: usize) -> (PathBuf, PathBuf) {
    let dir = cached(&format!("archives-{n}"), |dir| {
        let src = dir.join("src");
        let mut rng = Rng::new(0xA4C + n as u64);
        for i in 0..n {
            let folder = src.join(format!("{}/{}", rng.pick(WORDS), rng.pick(WORDS)));
            fs::create_dir_all(&folder).expect("create folder");
            let body: String = (0..rng.range(20, 400)).map(|_| format!("{} ", rng.pick(WORDS))).collect();
            fs::write(folder.join(format!("{}-{i}.txt", rng.pick(WORDS))), body).expect("write file");
        }
        let zip = std::process::Command::new("/usr/bin/zip")
            .args(["-qr", "../sample.zip", "."])
            .current_dir(&src)
            .status()
            .expect("run zip");
        assert!(zip.success(), "zip failed");
        let tar = std::process::Command::new("/usr/bin/tar")
            .args(["-czf", "sample.tar.gz", "-C", "src", "."])
            .current_dir(dir)
            .status()
            .expect("run tar");
        assert!(tar.success(), "tar failed");
    });
    (dir.join("sample.zip"), dir.join("sample.tar.gz"))
}

/// The user's saved index, copied so nothing touches the original, with the settings it
/// was built with. `None` without one.
pub fn real_index(scratch: &Path) -> Option<IndexConfig> {
    let mut config = tilde_ffi::user_index_config()?;
    let saved = config.cache_path.clone()?;
    if !saved.exists() {
        return None;
    }
    let copy = scratch.join("real-index.bin");
    fs::copy(&saved, &copy).ok()?;
    config.cache_path = Some(copy);
    Some(config)
}

/// Item counts like `100k` or `1m`.
pub fn parse_count(s: &str) -> Option<usize> {
    let s = s.trim().to_lowercase();
    let (num, mult) = match s.chars().last()? {
        'k' => (&s[..s.len() - 1], 1_000.0),
        'm' => (&s[..s.len() - 1], 1_000_000.0),
        _ => (s.as_str(), 1.0),
    };
    num.parse::<f64>().ok().map(|n| (n * mult) as usize)
}

pub fn label(n: usize) -> String {
    match n {
        n if n >= 1_000_000 && n % 100_000 == 0 => format!("{}m", n as f64 / 1e6),
        n if n >= 1_000 && n % 1_000 == 0 => format!("{}k", n / 1_000),
        n => n.to_string(),
    }
}
