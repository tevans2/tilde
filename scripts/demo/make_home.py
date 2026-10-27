"""Builds a believable home folder for screenshots, belonging to someone who makes a space
game, watches the sky and bakes bread: projects, notes with maths, recipes, aurora and
telescope photos, downloads with their origins, game screenshots, an AirDrop, build output
in idle projects, app caches, duplicates, a big old video and a few things in the Trash.
Nothing in it is anyone's; every image is drawn here.

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


def _noise(img: Image.Image, amount: float = 0.05) -> Image.Image:
    """Grain, so JPEGs come out real-sized and nothing looks flat."""
    return Image.blend(img, Image.effect_noise(img.size, 18).convert("RGB"), amount)


def _gradient(d: ImageDraw.ImageDraw, w: int, h: int, top, bottom) -> None:
    for y in range(h):
        k = y / h
        d.line([(0, y), (w, y)], fill=tuple(int(top[i] + (bottom[i] - top[i]) * k) for i in range(3)))


def _stars(d: ImageDraw.ImageDraw, r: random.Random, w: int, h: int, n: int, max_y: float = 1.0) -> None:
    for _ in range(n):
        x, y = r.randrange(w), r.randrange(int(h * max_y))
        s = r.choice([1, 1, 1, 2, 2, 3])
        c = r.randint(170, 255)
        d.ellipse([x, y, x + s, y + s], fill=(c, c, min(255, c + 20)))


def _ridge(d: ImageDraw.ImageDraw, r: random.Random, w: int, h: int, base: float, amp: float, color) -> None:
    pts = [(0, h)]
    phase = r.random() * 6
    for x in range(0, w + 30, 30):
        pts.append((x, h * base + amp * math.sin(x / 210 + phase) + amp * 0.5 * math.sin(x / 67 + phase * 2) + r.randint(-6, 6)))
    pts.append((w, h))
    d.polygon(pts, fill=color)


def photo(path: Path, seed: int, size=(2400, 1600)) -> None:
    """A landscape at golden hour: sky gradient, a sun, layered hills."""
    r = random.Random(seed)
    w, h = size
    palettes = [
        ((255, 183, 128), (120, 86, 160), [(64, 52, 96), (40, 34, 70), (22, 20, 44)]),
        ((255, 220, 170), (255, 150, 120), [(120, 80, 90), (80, 50, 70), (40, 28, 46)]),
        ((120, 160, 210), (220, 230, 240), [(150, 160, 170), (90, 100, 115), (50, 58, 70)]),
    ]
    top, bottom, hills = palettes[seed % len(palettes)]
    img = Image.new("RGB", size)
    d = ImageDraw.Draw(img)
    _gradient(d, w, h, top, bottom)
    sx, sy = r.randint(w // 5, 4 * w // 5), r.randint(h // 6, h // 3)
    d.ellipse([sx - 110, sy - 110, sx + 110, sy + 110], fill=(255, 240, 210))
    for i, color in enumerate(hills):
        _ridge(d, r, w, h, 0.55 + 0.14 * i, 50 + 40 * i, color)
    img = _noise(img.filter(ImageFilter.GaussianBlur(1.2)), 0.06)
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path, quality=92)


def aurora(path: Path, seed: int, size=(2400, 1600)) -> None:
    """The northern lights over black mountains and a still lake."""
    from PIL import ImageChops
    r = random.Random(seed)
    w, h = size
    img = Image.new("RGB", size)
    d = ImageDraw.Draw(img)
    _gradient(d, w, h, (4, 8, 24), (14, 30, 52))
    _stars(d, r, w, h, 900, 0.7)
    glow = Image.new("RGB", size, (0, 0, 0))
    g = ImageDraw.Draw(glow)
    hue = [(40, 255, 140), (60, 230, 170), (150, 90, 255)][seed % 3]
    for band in range(3):
        phase, base = r.random() * 6, h * (0.42 + 0.07 * band)
        for x in range(0, w, 6):
            y = base + 120 * math.sin(x / 380 + phase) + 50 * math.sin(x / 120 + phase * 3)
            length = 260 + 140 * math.sin(x / 90 + band)
            for k in range(0, int(length), 8):
                fade = 1 - k / length
                c = tuple(int(v * fade * 0.55) for v in (hue if k < length * 0.7 else (200, 80, 220)))
                g.line([(x, y - k), (x + 5, y - k)], fill=c, width=3)
    # Soft curtains with a sharper core.
    img = ImageChops.add(img, glow.filter(ImageFilter.GaussianBlur(22)))
    img = ImageChops.add(img, glow.filter(ImageFilter.GaussianBlur(4)))
    d = ImageDraw.Draw(img)
    _ridge(d, r, w, h, 0.66, 70, (6, 8, 14))
    # The lake mirrors the sky, darker.
    lake = img.crop((0, int(h * 0.42), w, int(h * 0.62))).transpose(Image.Transpose.FLIP_TOP_BOTTOM)
    lake = Image.blend(lake, Image.new("RGB", lake.size, (2, 6, 14)), 0.55).filter(ImageFilter.GaussianBlur(3))
    img.paste(lake.resize((w, h - int(h * 0.8))), (0, int(h * 0.8)))
    img = _noise(img, 0.04)
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path, quality=92)


def nebula(path: Path, seed: int, size=(2880, 1800)) -> None:
    """Gas clouds and stars, as from a telescope with a colour camera."""
    from PIL import ImageChops
    r = random.Random(seed)
    w, h = size
    img = Image.new("RGB", size, (3, 3, 10))
    clouds = Image.new("RGB", size, (0, 0, 0))
    c = ImageDraw.Draw(clouds)
    # Many faint overlapping clouds along a diagonal, blurred into gas.
    for _ in range(700):
        t = r.random()
        x = w * (0.15 + 0.7 * t) + r.gauss(0, w * 0.08)
        y = h * (0.75 - 0.5 * t) + r.gauss(0, h * 0.1)
        rad = r.randint(20, 140)
        col = r.choice([(150, 40, 110), (210, 70, 90), (50, 90, 190), (240, 150, 90), (110, 50, 170)])
        c.ellipse([x - rad, y - rad, x + rad, y + rad], fill=tuple(v // 6 for v in col))
    wisps = clouds.filter(ImageFilter.GaussianBlur(40))
    clouds = clouds.filter(ImageFilter.GaussianBlur(110))
    img = ImageChops.add(img, ImageChops.add(clouds, clouds))
    img = ImageChops.add(img, wisps)
    d = ImageDraw.Draw(img)
    _stars(d, r, w, h, 2500)
    for _ in range(12):
        x, y = r.randrange(w), r.randrange(h)
        d.ellipse([x - 5, y - 5, x + 5, y + 5], fill=(255, 250, 235))
        d.line([(x - 26, y), (x + 26, y)], fill=(255, 240, 220), width=2)
        d.line([(x, y - 26), (x, y + 26)], fill=(255, 240, 220), width=2)
    img = _noise(img, 0.03)
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path, quality=92)


def moon(path: Path, size=(2000, 2000)) -> None:
    """A waxing gibbous Moon with maria and craters."""
    from PIL import ImageChops
    r = random.Random(3)
    w, h = size
    img = Image.new("RGB", size, (2, 2, 6))
    disc = Image.new("RGB", size, (0, 0, 0))
    d = ImageDraw.Draw(disc)
    cx, cy, rad = w // 2, h // 2, 820
    d.ellipse([cx - rad, cy - rad, cx + rad, cy + rad], fill=(196, 194, 186))
    maria = Image.new("RGB", size, (0, 0, 0))
    md = ImageDraw.Draw(maria)
    for _ in range(14):
        x, y, s = cx + r.randint(-450, 350), cy + r.randint(-450, 300), r.randint(90, 220)
        md.ellipse([x - s, y - s * 0.8, x + s, y + s * 0.8], fill=(46, 46, 44))
    disc = ImageChops.subtract(disc, maria.filter(ImageFilter.GaussianBlur(40)))
    d = ImageDraw.Draw(disc)
    for _ in range(110):
        x, y, s = cx + r.randint(-760, 760), cy + r.randint(-760, 760), r.choice([4, 6, 8, 10, 14, 22])
        if (x - cx) ** 2 + (y - cy) ** 2 < (rad - s) ** 2:
            d.ellipse([x - s, y - s, x + s, y + s], outline=(120, 118, 114), width=2)
    disc = _noise(disc.filter(ImageFilter.GaussianBlur(1.5)), 0.08)
    mask = Image.new("L", size, 0)
    m = ImageDraw.Draw(mask)
    m.ellipse([cx - rad, cy - rad, cx + rad, cy + rad], fill=255)
    m.ellipse([cx - rad - 1150, cy - rad, cx + rad - 1150, cy + rad], fill=0)
    m.ellipse([cx - rad, cy - rad, cx + rad, cy + rad], outline=0)
    mask = mask.filter(ImageFilter.GaussianBlur(26))
    img.paste(disc, (0, 0), mask)
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path, quality=92)


def star_chart(path: Path) -> None:
    """The Southern Cross and the Pointers, with names, as a PNG."""
    w, h = 1800, 1400
    img = Image.new("RGB", (w, h), (10, 16, 38))
    d = ImageDraw.Draw(img)
    r = random.Random(5)
    _stars(d, r, w, h, 700)
    for x in range(0, w, 150):
        d.line([(x, 0), (x, h)], fill=(22, 32, 64))
    for y in range(0, h, 150):
        d.line([(0, y), (w, y)], fill=(22, 32, 64))
    cross = {"Acrux": (900, 1050, 22), "Mimosa": (680, 720, 20), "Gacrux": (930, 330, 19),
             "Imai": (1120, 640, 13), "Ginan": (1010, 760, 9)}
    pointers = {"Hadar": (300, 980, 21), "Rigil Kentaurus": (110, 1180, 24)}
    gold = (255, 214, 120)
    d.line([cross["Acrux"][:2], cross["Gacrux"][:2]], fill=(120, 150, 230), width=3)
    d.line([cross["Mimosa"][:2], cross["Imai"][:2]], fill=(120, 150, 230), width=3)
    d.line([pointers["Rigil Kentaurus"][:2], pointers["Hadar"][:2]], fill=(120, 150, 230), width=2)
    for name, (x, y, s) in {**cross, **pointers}.items():
        d.ellipse([x - s, y - s, x + s, y + s], fill=(255, 250, 240))
        d.ellipse([x - s * 2, y - s * 2, x + s * 2, y + s * 2], outline=(255, 250, 240, 60), width=1)
        d.text((x + s + 16, y - 18), name, font=font(34), fill=gold)
    d.text((70, 60), "Crux, the Southern Cross", font=font(60, True), fill=(235, 240, 255))
    d.text((70, 140), "Follow Acrux to Gacrux 4.5 times for the south celestial pole", font=font(32), fill=(160, 175, 220))
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path)


def document(path: Path, brand: str, title: str, lines: list[str], total: str | None = None,
             band=(36, 41, 61), contact: str = "") -> None:
    """An A4 page as a PDF: a coloured header band, a title, lines, maybe a total."""
    w, h = 1240, 1754
    img = Image.new("RGB", (w, h), "white")
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, w, 150], fill=band)
    d.text((90, 52), brand, font=font(44, True), fill="white")
    if contact:
        d.text((w - 90, 62), contact, font=font(28), fill=(225, 230, 245), anchor="ra")
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


def game_screenshot(path: Path, seed: int) -> None:
    """A screenshot of the game: a pixel-art starship drifting past planets, with a HUD."""
    r = random.Random(seed)
    w, h = 1600, 1000
    px = 10
    img = Image.new("RGB", (w, h), (12, 10, 28))
    d = ImageDraw.Draw(img)
    for _ in range(320):
        x, y = r.randrange(0, w, px), r.randrange(0, h, px)
        d.rectangle([x, y, x + px - 1, y + px - 1], fill=r.choice([(90, 90, 140), (200, 200, 255), (255, 220, 160)]))
    for cx, cy, rad, col in [(1220, 260, 150, (226, 120, 90)), (330, 760, 90, (90, 170, 220))]:
        for y in range(cy - rad, cy + rad, px):
            for x in range(cx - rad, cx + rad, px):
                dist = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
                if dist < rad:
                    shade = 1 - max(0, (x - cx + y - cy) / (rad * 2.4))
                    stripe = 0.85 if (y // (px * 3)) % 2 else 1
                    d.rectangle([x, y, x + px - 1, y + px - 1], fill=tuple(int(v * shade * stripe) for v in col))
    ship = ["....##....", "...####...", "..##..##..", ".########.", "##.####.##", "#..####..#", "...#..#...", "..##..##.."]
    sx, sy = 700 + seed * 60, 470
    for j, row in enumerate(ship):
        for i, c in enumerate(row):
            if c == "#":
                d.rectangle([sx + i * px * 2, sy + j * px * 2, sx + i * px * 2 + px * 2 - 1, sy + j * px * 2 + px * 2 - 1],
                            fill=(220, 230, 255) if j < 4 else (140, 160, 220))
    for k in range(4):
        d.rectangle([sx + 6 * px + k * 8, sy + 16 * px, sx + 6 * px + k * 8 + 6, sy + 16 * px + 40 + r.randint(0, 40)],
                    fill=(255, 170, 60))
    d.rectangle([0, 0, w, 64], fill=(20, 18, 44))
    d.text((24, 16), f"SECTOR 7-{seed + 3}   HULL ███████░░   FUEL {62 - seed * 9}%   CREDITS 1 2{seed}0", font=font(28, True), fill=(140, 255, 180))
    path.parent.mkdir(parents=True, exist_ok=True)
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

    game = dev / "starfield-roguelike"
    text(game / "Cargo.toml", """
[package]
name = "starfield-roguelike"
version = "0.3.0"
edition = "2021"

[dependencies]
bevy = "0.14"
rand = "0.8"
serde = { version = "1", features = ["derive"] }
""")
    text(game / "src/main.rs", """
use bevy::prelude::*;

mod sectors;
mod ship;

/// A roguelike across procedurally generated star sectors.
/// Run out of fuel between stars and it's game over.
fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Starfield".into(),
                resolution: (1280.0, 800.0).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.05, 0.04, 0.11)))
        .add_systems(Startup, (sectors::generate, ship::spawn))
        .add_systems(Update, (ship::thrust, ship::warp, ship::burn_fuel))
        .run();
}
""")
    text(game / "src/ship.rs", """
use bevy::prelude::*;

#[derive(Component)]
pub struct Ship {
    pub fuel: f32,
    pub hull: u8,
    pub warp_charge: f32,
}

const WARP_COST: f32 = 18.0;

pub fn spawn(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn((
        Ship { fuel: 100.0, hull: 9, warp_charge: 0.0 },
        SpriteBundle { texture: assets.load("ship.png"), ..default() },
    ));
}

/// Hold space to charge the warp drive; let go to jump to the next sector.
pub fn warp(keys: Res<ButtonInput<KeyCode>>, time: Res<Time>, mut ships: Query<&mut Ship>) {
    for mut ship in &mut ships {
        if keys.pressed(KeyCode::Space) {
            ship.warp_charge = (ship.warp_charge + time.delta_seconds()).min(1.0);
        } else if ship.warp_charge >= 1.0 && ship.fuel >= WARP_COST {
            ship.fuel -= WARP_COST;
            ship.warp_charge = 0.0;
            info!("warp! {:.0}% fuel left", ship.fuel);
        }
    }
}
""")
    text(game / "src/sectors.rs", "use bevy::prelude::*;\n\npub fn generate(mut commands: Commands) {\n    // Stars, nebulae, a derelict or two.\n}\n")
    text(game / "README.md", "# Starfield\n\nA tiny space roguelike. `cargo run --release`, hold **space** to warp.\n")
    text(game / ".git/HEAD", "ref: refs/heads/main\n")
    game_screenshot(game / "assets/ship-preview.png", 0)
    blob(game / "target/debug/starfield-roguelike", 38)
    blob(game / "target/debug/deps/libbevy.rlib", 12)
    tree_dates(game, 0.1)
    date(game / "src/ship.rs", 0.02)

    synth = dev / "synth-garden"
    text(synth / "package.json", json.dumps({
        "name": "synth-garden", "private": True,
        "scripts": {"dev": "vite", "build": "vite build"},
        "dependencies": {"react": "18.3.1", "react-dom": "18.3.1", "tone": "15.0.4"},
    }, indent=2) + "\n")
    text(synth / "src/Garden.tsx", """
import * as Tone from "tone";

// Every flower is an oscillator: tap one and it blooms into a chord.
const scale = ["C4", "D4", "E4", "G4", "A4", "C5"];

export function Flower({ index }: { index: number }) {
  const bloom = async () => {
    await Tone.start();
    const synth = new Tone.PolySynth().toDestination();
    synth.triggerAttackRelease([scale[index], scale[(index + 2) % 6]], "8n");
  };
  return <button className="flower" onClick={bloom}>🌸</button>;
}
""")
    text(synth / "README.md", "# synth-garden\n\nA garden that plays music. `npm run dev`\n")
    text(synth / ".git/HEAD", "ref: refs/heads/main\n")
    for i, pkg in enumerate(["tone", "react", "react-dom", "typescript", "vite", "esbuild", "rollup"]):
        blob(synth / f"node_modules/{pkg}/dist/index.js", [24, 3, 6, 22, 31, 14, 8][i])
    tree_dates(synth, 140)

    bread = dev / "sourdough-monitor"
    text(bread / "monitor.py", """
\"\"\"Watches the sourdough starter rise with a Raspberry Pi and a distance sensor.\"\"\"
import time
from gpiozero import DistanceSensor

sensor = DistanceSensor(echo=24, trigger=23)
EMPTY_JAR_CM = 14.0

while True:
    height = EMPTY_JAR_CM - sensor.distance * 100
    if height > 9:
        print("🍞 Peak rise: feed me or bake!")
    time.sleep(600)
""")
    text(bread / "requirements.txt", "gpiozero==2.0.1\nmatplotlib==3.9.2\n")
    text(bread / ".git/HEAD", "ref: refs/heads/main\n")
    blob(bread / ".venv/lib/python3.12/site-packages/matplotlib/_path.so", 24)
    tree_dates(bread, 210)

    text(dev / "ideas.md", "# Ideas\n\n- asteroid mining minigame\n- synth-garden: seasons change the scale\n- starter monitor: text me at peak rise\n")
    date(dev / "ideas.md", 3)


def documents(home: Path) -> None:
    docs = home / "Documents"
    text(docs / "Space/orbital-mechanics.md", r"""
# Getting to Mars, the cheap way

> [!tip] Hohmann transfer
> Two engine burns and one half-ellipse: the least fuel between two circular orbits.

The speed anywhere on an orbit comes from the **vis-viva equation**:

$$
v^2 = \mu \left( \frac{2}{r} - \frac{1}{a} \right)
$$

where $\mu = GM_\odot$ and $a$ is the semi-major axis. Leaving Earth's orbit ($r_1$)
for Mars ($r_2$), the first burn is

$$
\Delta v_1 = \sqrt{\frac{\mu}{r_1}} \left( \sqrt{\frac{2 r_2}{r_1 + r_2}} - 1 \right) \approx 2.94\ \text{km/s}
$$

and the trip takes half an orbit of the transfer ellipse:

$$
t_H = \pi \sqrt{\frac{(r_1 + r_2)^3}{8 \mu}} \approx 259\ \text{days}
$$

| Planet | $a$ (AU) | Period | Moons |
|---|---|---|---|
| Earth | 1.00 | 365 d | 1 |
| Mars | 1.52 | 687 d | 2 |
| Jupiter | 5.20 | 11.9 y | 95 |

> [!note] Launch windows
> Earth and Mars line up every $\approx 26$ months: next window is late 2026.

```python
from math import pi, sqrt
mu, r1, r2 = 1.327e20, 1.496e11, 2.279e11
print(pi * sqrt((r1 + r2) ** 3 / (8 * mu)) / 86400)  # 258.9 days
```
""")
    date(docs / "Space/orbital-mechanics.md", 0.3)
    text(docs / "Space/observing-log.md", """
# Observing log

| Date | Target | Seeing | Notes |
|---|---|---|---|
| 2026-10-17 | Moon | 4/5 | Terminator through Copernicus, gorgeous |
| 2026-10-21 | Orion Nebula | 3/5 | The Trapezium split at 120× |
| 2026-10-24 | Saturn | 4/5 | Cassini division visible! |
""")
    date(docs / "Space/observing-log.md", 3)
    poster = docs / "Space/Mars launch windows.pdf"
    document(poster, "Mission Planning Club", "Mars launch windows", [
        "Window 1 ..... Nov 2026", "Window 2 ..... Dec 2028", "Window 3 ..... Feb 2031",
        "", "Hohmann transfer: about 259 days, 2.94 km/s from LEO.",
    ], band=(150, 52, 40), contact="ad astra")
    date(poster, 12)

    text(docs / "Recipes/sourdough.md", """
# Weekend sourdough

| Ingredient | Grams | Baker's % |
|---|---|---|
| Bread flour | 450 | 90% |
| Wholewheat | 50 | 10% |
| Water | 375 | 75% |
| Starter | 100 | 20% |
| Salt | 10 | 2% |

- [x] Feed the starter (the monitor says when it peaks)
- [x] Autolyse 1 h, then add starter and salt
- [ ] Four sets of stretch and folds, 30 min apart
- [ ] Cold proof overnight, bake at 250 °C in a Dutch oven
""")
    date(docs / "Recipes/sourdough.md", 2)
    text(docs / "Recipes/tonkotsu-ramen.md", "# Tonkotsu ramen\n\n12 hours of broth. Worth it.\n\n1. Blanch the bones\n2. Boil hard, keep topping up\n3. Tare, noodles, chashu, ajitama\n")
    date(docs / "Recipes/tonkotsu-ramen.md", 30)
    text(docs / "Recipes/lemon-tart.md", "# Lemon tart\n\nSweet pastry, lemon curd, a blowtorch for the top.\n")
    date(docs / "Recipes/lemon-tart.md", 60)

    text(docs / "Game design/boss-fights.md", "# Boss fights\n\n## The Leviathan\nA space whale that eats your fuel. Lure it into the nebula.\n\n## The Archivist\nSteals your map. Every jump is blind until you beat it.\n")
    date(docs / "Game design/boss-fights.md", 5)
    text(docs / "Game design/level-ideas.md", "# Sectors\n\n- Ice field: drift slowly or crack the hull\n- Pulsar: the screen pulses with the beat\n- Derelict station: loot, and something still alive\n")
    date(docs / "Game design/level-ideas.md", 9)

    trip = docs / "Travel/Iceland aurora trip.pdf"
    document(trip, "Aurora Tours", "Iceland, 3–10 Nov", [
        "Fly Cape Town → Keflavík ..... 3 Nov", "Golden Circle and geysers ..... 4 Nov",
        "Jökulsárlón ice lagoon ..... 6 Nov", "Aurora hunt, Kirkjufell ..... 8 Nov",
        "Blue Lagoon, fly home ..... 10 Nov",
    ], band=(24, 92, 84), contact="aurora forecast: Kp 5")
    date(trip, 20)


def pictures(home: Path) -> list[Path]:
    shots = []
    trip = home / "Pictures/Iceland 2025"
    for i in range(6):
        p = trip / f"IMG_{2041 + i}.jpg"
        (aurora if i % 2 == 0 else photo)(p, i)
        date(p, 360 - i)
        shots.append(p)
    best = trip / "aurora over Kirkjufell.jpg"
    aurora(best, 0)
    date(best, 1.5)
    astro = home / "Pictures/Astrophotography"
    nebula(astro / "orion-nebula.jpg", 1)
    date(astro / "orion-nebula.jpg", 6)
    moon(astro / "moon-2026-10-17.jpg")
    date(astro / "moon-2026-10-17.jpg", 10)
    star_chart(astro / "southern-cross.png")
    date(astro / "southern-cross.png", 25)
    nebula(home / "Pictures/Wallpapers/carina.jpg", 4)
    date(home / "Pictures/Wallpapers/carina.jpg", 90)
    return shots


def downloads(home: Path, photos: list[Path]) -> None:
    dl = home / "Downloads"
    dl.mkdir(parents=True, exist_ok=True)

    bp = dl / "boarding-pass-CPT-KEF.pdf"
    document(bp, "Icelandair", "Boarding pass", [
        "Passenger ..... SAM RIVERA", "Flight ..... FI 0451", "From ..... Cape Town (CPT)",
        "To ..... Keflavík (KEF)", "Seat ..... 23A, window", "Boarding ..... 05:40",
    ], band=(16, 52, 120), contact="Have a great trip!")
    date(bp, 0.15)
    downloaded(bp, "https://mail.google.com/mail/u/0/?attid=0.1", "Google Chrome")

    fc = dl / "aurora-forecast-week44.pdf"
    document(fc, "Icelandic Met Office", "Aurora forecast, week 44", [
        "Mon ..... Kp 3, cloudy", "Tue ..... Kp 4, clearing", "Wed ..... Kp 5, clear skies",
        "Thu ..... Kp 6, clear: go!", "Fri ..... Kp 2",
    ], band=(30, 70, 60))
    date(fc, 1.1)
    downloaded(fc, "https://en.vedur.is/weather/forecasts/aurora/", "Safari")

    planetarium = dl / "Stellarium-24.3.dmg"
    blob(planetarium, 6)
    date(planetarium, 4)
    downloaded(planetarium, "https://github.com/Stellarium/stellarium/releases/download/v24.3/Stellarium-24.3.dmg")

    # An AirDrop from a phone: last night's sky.
    air = dl / "IMG_4021.jpg"
    aurora(air, 2)
    date(air, 0.4)
    airdropped(air)

    # A copy of a photo that's already in Pictures (a duplicate).
    dup = dl / photos[2].name
    shutil.copyfile(photos[2], dup)
    date(dup, 95)
    downloaded(dup, "https://photos.google.com/share/AF1Qip")

    poster = dl / "Mars launch windows (1).pdf"
    shutil.copyfile(home / "Documents/Space/Mars launch windows.pdf", poster)
    date(poster, 13)
    downloaded(poster, "https://mail.google.com/mail/u/0/?attid=0.2", "Google Chrome")

    sprites = dl / "pixel-ship-sprites.png"
    game_screenshot(sprites, 2)
    date(sprites, 2.2)
    downloaded(sprites, "https://itch.io/game-assets/free/tag-space")

    # Old downloads.
    for name, size, days, url in [
        ("Godot_v4.3-stable_macos.universal.zip", 120, 160, "https://github.com/godotengine/godot/releases/download/4.3-stable/Godot_v4.3-stable_macos.universal.zip"),
        ("Steam.dmg", 9, 130, "https://cdn.cloudflare.steamstatic.com/client/installer/steam.dmg"),
        ("lofi-synth-samples.zip", 14, 88, "https://freesound.org/people/synthgarden/packs/"),
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
        game_screenshot(p, i + 1)
        date(p, when)
        screenshot(p)
    text(desk / "todo.txt", "- pack thermals for Iceland\n- feed the starter\n- fix the warp drive bug\n- clean the telescope mirror\n")
    date(desk / "todo.txt", 0.8)


def media(home: Path) -> None:
    raw = home / "Movies/aurora-timelapse-raw.mov"
    blob(raw, 160)
    date(raw, 340, opened=330)


def library(home: Path) -> None:
    caches = home / "Library/Caches"
    for bundle, mb in [("com.valvesoftware.steam", 46), ("com.spotify.client", 28), ("Homebrew", 34)]:
        blob(caches / bundle / "data.bin", mb)
    # Tilde's own frecency, so search ranks like a home folder that's been in use.
    t = int(NOW)
    entries = {
        str(home / p): {"score": s, "at": t - 3600, "dir": d}
        for p, s, d in [
            ("Developer/starfield-roguelike", 14.0, True),
            ("Documents/Space", 9.0, True),
            ("Downloads", 8.0, True),
            ("Developer", 6.0, True),
            ("Documents/Recipes", 3.0, True),
            ("Documents/Space/orbital-mechanics.md", 4.0, False),
        ]
    }
    support = home / "Library/Application Support/Tilde"
    support.mkdir(parents=True, exist_ok=True)
    (support / "frecency.json").write_text(json.dumps({"version": 1, "entries": entries}))


def trash(home: Path) -> None:
    t = home / ".Trash"
    text(t / "ship-sprite-v1.svg", "<svg xmlns='http://www.w3.org/2000/svg'/>\n")
    blob(t / "trailer-draft.mov", 22)
    text(t / "burnt-loaf-notes.txt", "too hot, too long\n")


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
