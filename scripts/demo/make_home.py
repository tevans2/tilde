"""Builds a believable home folder for screenshots: projects, documents, photos, downloads
with their origins, screenshots, an AirDrop, build output in idle projects, app caches,
duplicates, a big old video and a few things in the Trash. Nothing in it is anyone's.

    python3 scripts/demo/make_home.py <dir> [--appearance light|dark]

Dates are relative to now, and set the way macOS records them (modified, created, date
added, last opened), so Activity and Clean Up sort them as they would real files. Needs
Pillow (pip3 install pillow).
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import json
import math
import os
import plistlib
import random
import shutil
import struct
import time
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

NOW = time.time()
DAY = 86_400
RNG = random.Random(7)

_libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)


class _AttrList(ctypes.Structure):
    _fields_ = [
        ("bitmapcount", ctypes.c_ushort),
        ("reserved", ctypes.c_ushort),
        ("commonattr", ctypes.c_uint32),
        ("volattr", ctypes.c_uint32),
        ("dirattr", ctypes.c_uint32),
        ("fileattr", ctypes.c_uint32),
        ("forkattr", ctypes.c_uint32),
    ]


ATTR_CMN_CRTIME = 0x200
ATTR_CMN_ADDEDTIME = 0x10000000


def _setattr_time(path: Path, attr: int, t: float) -> None:
    al = _AttrList(5, 0, attr, 0, 0, 0, 0)
    buf = struct.pack("qq", int(t), 0)
    if _libc.setattrlist(bytes(path), ctypes.byref(al), buf, len(buf), 0x1) != 0:  # FSOPT_NOFOLLOW
        raise OSError(ctypes.get_errno(), f"setattrlist {path}")


def _setxattr(path: Path, name: str, value: bytes) -> None:
    if _libc.setxattr(bytes(path), name.encode(), value, len(value), 0, 0x1) != 0:  # XATTR_NOFOLLOW
        raise OSError(ctypes.get_errno(), f"setxattr {name} {path}")


def date(path: Path, days_ago: float, added: float | None = None, opened: float | None = None) -> None:
    """Sets when `path` was made and changed (`days_ago`), arrived in its folder (`added`,
    default the same) and was last opened (`opened`)."""
    t = NOW - days_ago * DAY
    _setattr_time(path, ATTR_CMN_CRTIME, t)
    _setattr_time(path, ATTR_CMN_ADDEDTIME, NOW - (days_ago if added is None else added) * DAY)
    if opened is not None:
        _setxattr(path, "com.apple.lastuseddate#PS", struct.pack("qq", int(NOW - opened * DAY), 0))
    os.utime(path, (t, t), follow_symlinks=False)


def downloaded(path: Path, url: str, app: str = "Safari") -> None:
    _setxattr(path, "com.apple.quarantine", f"0081;{int(NOW):08x};{app};".encode())
    _setxattr(path, "com.apple.metadata:kMDItemWhereFroms", plistlib.dumps([url], fmt=plistlib.FMT_BINARY))


def airdropped(path: Path) -> None:
    _setxattr(path, "com.apple.quarantine", f"0083;{int(NOW):08x};sharingd;".encode())


def screenshot(path: Path) -> None:
    _setxattr(path, "com.apple.metadata:kMDItemIsScreenCapture", plistlib.dumps(True, fmt=plistlib.FMT_BINARY))


# Drawing ----------------------------------------------------------------------------------

def font(size: int, bold: bool = False):
    for name in ("/System/Library/Fonts/HelveticaNeue.ttc", "/System/Library/Fonts/Helvetica.ttc"):
        if Path(name).exists():
            return ImageFont.truetype(name, size, index=1 if bold else 0)
    return ImageFont.load_default()


def photo(path: Path, seed: int, size=(2400, 1600)) -> None:
    """A soft landscape: sky gradient, layered hills, a sun. Noisy enough to be a real-sized JPEG."""
    r = random.Random(seed)
    w, h = size
    palettes = [
        ((255, 183, 128), (120, 86, 160), [(64, 52, 96), (40, 34, 70), (22, 20, 44)]),
        ((170, 215, 240), (240, 245, 250), [(96, 140, 110), (60, 100, 80), (34, 64, 54)]),
        ((255, 220, 170), (255, 150, 120), [(120, 80, 90), (80, 50, 70), (40, 28, 46)]),
        ((120, 160, 210), (220, 230, 240), [(150, 160, 170), (90, 100, 115), (50, 58, 70)]),
    ]
    top, bottom, hills = palettes[seed % len(palettes)]
    img = Image.new("RGB", size)
    d = ImageDraw.Draw(img)
    for y in range(h):
        k = y / h
        d.line([(0, y), (w, y)], fill=tuple(int(top[i] + (bottom[i] - top[i]) * k) for i in range(3)))
    sx, sy = r.randint(w // 5, 4 * w // 5), r.randint(h // 6, h // 3)
    d.ellipse([sx - 110, sy - 110, sx + 110, sy + 110], fill=(255, 240, 210))
    for i, color in enumerate(hills):
        base = h * (0.55 + 0.14 * i)
        pts = [(0, h)]
        phase, amp = r.random() * 6, 60 + 50 * i
        for x in range(0, w + 40, 40):
            pts.append((x, base + amp * math.sin(x / (260 + 90 * i) + phase) + r.randint(-8, 8)))
        pts.append((w, h))
        d.polygon(pts, fill=color)
    img = img.filter(ImageFilter.GaussianBlur(1.2))
    noise = Image.effect_noise(size, 18).convert("RGB")
    img = Image.blend(img, noise, 0.06)
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path, quality=92)


def document(path: Path, title: str, lines: list[str], total: str | None = None) -> None:
    """An A4 page as a PDF: a header band, a title, lines, maybe a total."""
    w, h = 1240, 1754
    img = Image.new("RGB", (w, h), "white")
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, w, 150], fill=(36, 41, 61))
    d.text((90, 52), "Northwind Studio", font=font(44, True), fill="white")
    d.text((w - 420, 62), "hello@northwind.example", font=font(28), fill=(200, 205, 225))
    d.text((90, 240), title, font=font(64, True), fill=(30, 30, 40))
    y = 360
    for line in lines:
        if "....." in line:
            left, right = line.split(".....")
            d.text((90, y), left.strip(), font=font(32), fill=(60, 60, 70))
            d.text((w - 90, y), right.strip(), font=font(32), fill=(60, 60, 70), anchor="ra")
            d.line([(90, y + 52), (w - 90, y + 52)], fill=(225, 228, 235), width=2)
            y += 80
        else:
            d.text((90, y), line, font=font(32), fill=(60, 60, 70))
            y += 56
    if total:
        d.rectangle([w - 520, y + 40, w - 90, y + 140], fill=(240, 243, 250))
        d.text((w - 490, y + 70), "Total", font=font(36, True), fill=(30, 30, 40))
        d.text((w - 120, y + 70), total, font=font(36, True), fill=(30, 30, 40), anchor="ra")
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path, "PDF", resolution=150)


def fake_screenshot(path: Path, seed: int) -> None:
    """A screenshot of a made-up chart, as you'd take for a slide."""
    r = random.Random(seed)
    w, h = 1600, 1000
    img = Image.new("RGB", (w, h), (248, 249, 252))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, w, 70], fill=(232, 235, 242))
    for i, c in enumerate([(255, 95, 87), (254, 188, 46), (40, 200, 64)]):
        d.ellipse([24 + i * 34, 24, 46 + i * 34, 46], fill=c)
    d.text((80, 110), "Weekly active users", font=font(44, True), fill=(30, 30, 40))
    bars = [r.randint(200, 640) for _ in range(12)]
    for i, b in enumerate(bars):
        x = 100 + i * 118
        d.rounded_rectangle([x, 900 - b, x + 80, 900], 10, fill=(99, 102, 241) if i < 11 else (236, 72, 153))
    img.save(path)


def blob(path: Path, mb: float) -> None:
    """Incompressible filler (so sizes on disk are real)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as f:
        left = int(mb * 1024 * 1024)
        while left:
            n = min(left, 4 << 20)
            f.write(RNG.randbytes(n))
            left -= n


def text(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body.lstrip("\n"))


def tree_dates(root: Path, days_ago: float) -> None:
    for p in sorted(root.rglob("*"), key=lambda p: -len(p.parts)):
        date(p, days_ago)
    date(root, days_ago)


# The home folder --------------------------------------------------------------------------

def projects(home: Path) -> None:
    dev = home / "Developer"

    api = dev / "api-server"
    text(api / "Cargo.toml", """
[package]
name = "api-server"
version = "0.4.2"
edition = "2021"

[dependencies]
axum = "0.7"
serde = { version = "1", features = ["derive"] }
sqlx = { version = "0.8", features = ["postgres", "runtime-tokio"] }
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
""")
    text(api / "src/main.rs", """
use axum::{routing::get, Router};
use std::net::SocketAddr;

mod invoices;
mod routes;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let pool = db::connect(&std::env::var("DATABASE_URL")?).await?;

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest("/invoices", invoices::router())
        .with_state(pool);

    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
""")
    text(api / "src/invoices.rs", """
use axum::{extract::{Path, State}, routing::get, Json, Router};
use serde::Serialize;
use sqlx::PgPool;

#[derive(Serialize, sqlx::FromRow)]
pub struct Invoice {
    pub id: i64,
    pub client: String,
    pub total_cents: i64,
    pub paid: bool,
}

pub fn router() -> Router<PgPool> {
    Router::new()
        .route("/", get(list))
        .route("/:id", get(show))
}

async fn list(State(db): State<PgPool>) -> Json<Vec<Invoice>> {
    let rows = sqlx::query_as("select * from invoices order by id desc limit 50")
        .fetch_all(&db)
        .await
        .unwrap_or_default();
    Json(rows)
}

async fn show(State(db): State<PgPool>, Path(id): Path<i64>) -> Option<Json<Invoice>> {
    sqlx::query_as("select * from invoices where id = $1")
        .bind(id)
        .fetch_optional(&db)
        .await
        .ok()
        .flatten()
        .map(Json)
}
""")
    text(api / "src/routes.rs", "pub mod health;\npub mod invoices;\n")
    text(api / "README.md", "# api-server\n\nThe invoices API. `cargo run`, then `curl localhost:8080/health`.\n")
    text(api / ".git/HEAD", "ref: refs/heads/main\n")
    blob(api / "target/debug/api-server", 38)
    blob(api / "target/debug/deps/libaxum.rlib", 12)
    tree_dates(api, 0.1)
    date(api / "src/invoices.rs", 0.02)

    site = dev / "portfolio-site"
    text(site / "package.json", json.dumps({
        "name": "portfolio-site", "private": True,
        "scripts": {"dev": "next dev", "build": "next build"},
        "dependencies": {"next": "14.2.5", "react": "18.3.1", "react-dom": "18.3.1"},
    }, indent=2) + "\n")
    text(site / "src/app/page.tsx", """
export default function Home() {
  return (
    <main className="mx-auto max-w-2xl p-8">
      <h1 className="text-4xl font-bold">Hi, I make things.</h1>
      <p className="mt-4 text-lg text-zinc-600">Design and code, mostly for small teams.</p>
    </main>
  );
}
""")
    text(site / "README.md", "# portfolio-site\n\n`npm run dev`\n")
    text(site / ".git/HEAD", "ref: refs/heads/main\n")
    for i, pkg in enumerate(["next", "react", "react-dom", "typescript", "@swc/core", "caniuse-lite", "postcss"]):
        blob(site / f"node_modules/{pkg}/dist/index.js", [42, 3, 6, 22, 31, 4, 2][i])
    tree_dates(site, 140)

    inv = dev / "invoice-generator"
    text(inv / "main.py", """
from pathlib import Path
from jinja2 import Template

def render(client: str, items: list[tuple[str, float]]) -> str:
    template = Template(Path("templates/invoice.html").read_text())
    return template.render(client=client, items=items, total=sum(p for _, p in items))
""")
    text(inv / "templates/invoice.html", "<h1>Invoice for {{ client }}</h1>\n")
    text(inv / "requirements.txt", "jinja2==3.1.4\nweasyprint==62.3\n")
    text(inv / ".git/HEAD", "ref: refs/heads/main\n")
    blob(inv / ".venv/lib/python3.12/site-packages/weasyprint/core.so", 24)
    tree_dates(inv, 210)

    text(dev / "notes.md", "# Ideas\n\n- invoice reminders by email\n- dark mode for the site\n")
    date(dev / "notes.md", 3)


def documents(home: Path) -> None:
    docs = home / "Documents"
    clients = ["Acme Studio", "Blue Fern Café", "Kestrel Labs", "Orbit Fitness", "Juniper & Co"]
    for month in range(1, 11):
        client = clients[month % len(clients)]
        hours = 8 + (month * 7) % 30
        p = docs / f"Invoices/2026/invoice-2026-{month:02d}.pdf"
        document(p, f"Invoice 2026-{month:02d}", [
            f"Billed to: {client}", f"Date: 2026-{month:02d}-28", "",
            f"Design and development, {hours} h ..... R {hours * 950:,}".replace(",", " "),
            "Hosting, monthly ..... R 450",
        ], total=f"R {hours * 950 + 450:,}".replace(",", " "))
        date(p, (10 - month) * 30 + 3)
    contract = docs / "Contracts/Kestrel Labs - contract.pdf"
    document(contract, "Services agreement", [
        "Between Northwind Studio and Kestrel Labs (Pty) Ltd.", "",
        "1. Scope: design and build of the customer portal.",
        "2. Term: six months from signature.",
        "3. Fees: as per the schedule, invoiced monthly.",
    ])
    date(contract, 75)
    taxes = docs / "Taxes/Tax return 2025.pdf"
    document(taxes, "Tax return 2025", ["Income ..... R 412 300", "Deductions ..... R 38 950"], total="R 373 350")
    date(taxes, 120)
    text(docs / "Notes/meeting-notes.md", """
# Kestrel Labs, kickoff

- Portal launches before the end of November
- Invoices go out on the 28th
- Next call: Thursday 10:00
""")
    date(docs / "Notes/meeting-notes.md", 1.2)
    text(docs / "Notes/reading-list.md", "# Reading\n\n- The Design of Everyday Things\n- Shape Up\n")
    date(docs / "Notes/reading-list.md", 40)


def pictures(home: Path) -> list[Path]:
    trip = home / "Pictures/Iceland 2024"
    shots = []
    for i in range(8):
        p = trip / f"IMG_{2041 + i}.jpg"
        photo(p, i)
        date(p, 360 - i)
        shots.append(p)
    for i, name in enumerate(["dunes.jpg", "harbour.jpg"]):
        p = home / "Pictures/Wallpapers" / name
        photo(p, 10 + i, (2880, 1800))
        date(p, 90)
    return shots


def downloads(home: Path, photos: list[Path]) -> None:
    dl = home / "Downloads"
    dl.mkdir(parents=True, exist_ok=True)

    q3 = dl / "Q3 report.pdf"
    document(q3, "Q3 report", ["Revenue ..... R 1 240 000", "Costs ..... R 860 000", "New clients ..... 4"])
    date(q3, 0.15)
    downloaded(q3, "https://docs.google.com/document/d/1x/export?format=pdf", "Google Chrome")

    inv = dl / "invoice-kestrel-oct.pdf"
    document(inv, "Invoice KL-2210", ["Billed to: Northwind Studio", "Hosting, October ..... R 1 200"], total="R 1 200")
    date(inv, 1.1)
    downloaded(inv, "https://mail.google.com/mail/u/0/?attid=0.1", "Google Chrome")

    figma = dl / "Figma-124.3.dmg"
    blob(figma, 6)
    date(figma, 4)
    downloaded(figma, "https://desktop.figma.com/mac-arm/Figma-124.3.dmg")

    # An AirDrop from a phone.
    air = dl / "IMG_3310.jpg"
    photo(air, 21)
    date(air, 0.4)
    airdropped(air)

    # A copy of a photo that's already in Pictures (a duplicate).
    dup = dl / photos[2].name
    shutil.copyfile(photos[2], dup)
    date(dup, 95)
    downloaded(dup, "https://photos.google.com/share/AF1Qip")

    contract = dl / "Kestrel Labs - contract (1).pdf"
    shutil.copyfile(home / "Documents/Contracts/Kestrel Labs - contract.pdf", contract)
    date(contract, 76)
    downloaded(contract, "https://mail.google.com/mail/u/0/?attid=0.2", "Google Chrome")

    # Old downloads.
    for name, size, days, url in [
        ("Xcode_15.4.xip", 120, 160, "https://developer.apple.com/services-account/download?path=/Developer_Tools/Xcode_15.4/Xcode_15.4.xip"),
        ("Zoom.pkg", 9, 130, "https://zoom.us/client/latest/Zoom.pkg"),
        ("brand-assets.zip", 14, 88, "https://drive.google.com/uc?id=1b"),
    ]:
        p = dl / name
        blob(p, size)
        date(p, days)
        downloaded(p, url)


def desktop(home: Path) -> None:
    desk = home / "Desktop"
    desk.mkdir(parents=True, exist_ok=True)
    for i, (when, label) in enumerate([(0.05, "09.41.12"), (2.3, "16.02.55")]):
        p = desk / f"Screenshot 2026-10-{27 - int(when)} at {label}.png"
        fake_screenshot(p, i)
        date(p, when)
        screenshot(p)
    text(desk / "todo.txt", "- send Kestrel invoice\n- renew domain\n- back up photos\n")
    date(desk / "todo.txt", 0.8)


def media(home: Path) -> None:
    raw = home / "Movies/Iceland 2024 - raw footage.mov"
    blob(raw, 160)
    date(raw, 340, opened=330)


def library(home: Path) -> None:
    caches = home / "Library/Caches"
    for bundle, mb in [("com.spotify.client", 46), ("com.tinyspeck.slackmacgap", 28), ("Homebrew", 34)]:
        blob(caches / bundle / "data.bin", mb)
    # Tilde's own frecency, so search ranks like a home folder that's been in use.
    t = int(NOW)
    entries = {
        str(home / p): {"score": s, "at": t - 3600, "dir": d}
        for p, s, d in [
            ("Developer/api-server", 14.0, True),
            ("Documents/Invoices/2026", 9.0, True),
            ("Downloads", 8.0, True),
            ("Developer", 6.0, True),
            ("Developer/portfolio-site", 3.0, True),
            ("Documents/Invoices/2026/invoice-2026-10.pdf", 4.0, False),
        ]
    }
    support = home / "Library/Application Support/Tilde"
    support.mkdir(parents=True, exist_ok=True)
    (support / "frecency.json").write_text(json.dumps({"version": 1, "entries": entries}))


def trash(home: Path) -> None:
    t = home / ".Trash"
    text(t / "old-logo-v1.svg", "<svg xmlns='http://www.w3.org/2000/svg'/>\n")
    blob(t / "screen-recording-draft.mov", 22)
    text(t / "notes copy.txt", "old\n")


def config(home: Path, appearance: str) -> None:
    text(home / ".config/tilde/config.toml", f"""
[font]
family = "JetBrains Mono"
size = 14

[theme]
appearance = "{appearance}"
preset = "catppuccin"

[index]
include_hidden = false

[frecency]
zoxide = false

[edit]
editor = "builtin"

[cleanup]
big_file = "100M"
duplicates_min = "100K"
ignore = ["~/Library/Caches/Tilde"]
""")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--appearance", default="dark", choices=["light", "dark"])
    args = ap.parse_args()
    home = Path(args.dir).resolve()
    if home.exists():
        shutil.rmtree(home)
    home.mkdir(parents=True)
    projects(home)
    documents(home)
    photos = pictures(home)
    downloads(home, photos)
    desktop(home)
    media(home)
    library(home)
    trash(home)
    config(home, args.appearance)
    for top in ["Developer", "Documents", "Pictures", "Downloads", "Desktop", "Movies"]:
        date(home / top, 0.02)
    print(f"demo home: {home}")


if __name__ == "__main__":
    main()
