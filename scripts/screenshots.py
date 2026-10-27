"""Renders the README and guide images from a demo home folder (scripts/demo/make_home.py).

    python3 scripts/screenshots.py              every scene
    python3 scripts/screenshots.py search hero  just these
    python3 scripts/screenshots.py --list

Each scene launches the debug build (`swift build --package-path App` first) with HOME
pointing at the demo folder, plays keys, and saves exactly Tilde's window (see
DebugSnapshot.swift). Stills are made in light and dark; GIFs get a caption of the keys
being pressed. Nothing outside the demo folder is read or changed: the window size and
shelf of your own Tilde are left alone. Needs Pillow (pip3 install pillow).
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts/demo"))
import make_home  # noqa: E402

APP = ROOT / "App/.build/debug/Tilde"
DEMO = ROOT / "build/demo-home"
OUT = ROOT / "docs/images"
STILL_WIDTH = 1600  # px; shown at 800 on GitHub, so still sharp on Retina
GIF_WIDTH = 1000
FPS = 10


@dataclass
class Scene:
    name: str
    keys: str
    start: str = ""  # relative to the demo home
    gif: bool = False
    window: str = ""  # TILDE_SNAPSHOT_WINDOW
    changes_files: bool = False  # the demo home is rebuilt afterwards
    key_delay: float = 0.15
    hold: float = 1.5  # seconds the GIF stays on its last frame
    env: dict = field(default_factory=dict)


SCENES = [
    Scene("search", r"\w\winv\w"),
    Scene("browse", r"\w\e\w", start="Developer/api-server/src"),
    Scene("activity", r"\w@\w\w"),
    # r: duplicates are skipped in Low Power Mode until asked for.
    Scene("cleanup", r"\w\w\eC\w\wr\w\w\w\wjl\w\w"),
    Scene("preview", r"\w\ejjjp\w\w", start="Documents/Invoices/2026"),
    Scene("hero", r"\winvo\w^j^j^j\t\w\el\wjj\wp\wjjj\wp\w", gif=True, key_delay=0.3),
    # Collect two PDFs in Downloads, file them, then undo.
    Scene("shelf", r"\w\ejjjjxjx\wm\w2026\w\r\w\wu\w\w", start="Downloads", gif=True,
          key_delay=0.3, changes_files=True),
]


def run_tilde(scene: Scene, out: Path, appearance: str, frames: Path | None) -> None:
    make_home.config(DEMO, appearance)
    env = {
        **os.environ,
        "HOME": str(DEMO),
        "XDG_CONFIG_HOME": str(DEMO / ".config"),
        "TILDE_SNAPSHOT": str(out),
        "TILDE_SNAPSHOT_KEYS": scene.keys,
        "TILDE_SNAPSHOT_KEY_DELAY": str(scene.key_delay),
        "TILDE_SNAPSHOT_DELAY": "1.5",
        **scene.env,
    }
    env.pop("TILDE_SNAPSHOT_FRAMES", None)
    if scene.start:
        env["TILDE_START"] = str(DEMO / scene.start)
    if scene.window:
        env["TILDE_SNAPSHOT_WINDOW"] = scene.window
    if frames:
        env["TILDE_SNAPSHOT_FRAMES"] = str(frames)
        env["TILDE_SNAPSHOT_FPS"] = str(FPS)
    proc = subprocess.Popen([str(APP)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    float_window(proc.pid)
    try:
        proc.wait(timeout=180)
    except subprocess.TimeoutExpired:
        proc.kill()
        raise SystemExit(f"{scene.name}: Tilde didn't finish")
    if not out.exists():
        raise SystemExit(f"{scene.name}: no image was written")


# AeroSpace workspace the windows go to, out of the way of whatever you're working on.
WORKSPACE = os.environ.get("TILDE_SNAPSHOT_WORKSPACE", "9")


def float_window(pid: int) -> None:
    """With AeroSpace, the window is floated (tiling would resize it) and sent to
    WORKSPACE, so it doesn't land on the workspace you're using. Tilde captures its own
    window, which works on a workspace that isn't showing."""
    if not shutil.which("aerospace"):
        return
    for _ in range(100):
        r = subprocess.run(
            ["aerospace", "list-windows", "--monitor", "all", "--pid", str(pid), "--format", "%{window-id}"],
            capture_output=True, text=True,
        )
        if "disabled" in r.stderr:
            return
        ids = r.stdout.split()
        if ids:
            for wid in ids:
                subprocess.run(["aerospace", "move-node-to-workspace", "--window-id", wid, WORKSPACE], capture_output=True)
                subprocess.run(["aerospace", "layout", "--window-id", wid, "floating"], capture_output=True)
            return
        time.sleep(0.02)


def save_still(src: Path, dest: Path) -> None:
    img = Image.open(src)
    if img.width > STILL_WIDTH:
        img = img.resize((STILL_WIDTH, round(img.height * STILL_WIDTH / img.width)), Image.Resampling.LANCZOS)
    img.save(dest, optimize=True)
    print(f"  {dest.relative_to(ROOT)}  {img.width}×{img.height}  {dest.stat().st_size // 1024} KB")


# Key captions -----------------------------------------------------------------------------

def caption_font(size: int):
    for name in ("/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/Helvetica.ttc"):
        if Path(name).exists():
            return ImageFont.truetype(name, size)
    return ImageFont.load_default()


def key_tokens(keys_tsv: Path) -> list[tuple[float, str]]:
    """Keys with when they were pressed; typed letters in a row become one word."""
    tokens: list[tuple[float, str]] = []
    for line in keys_tsv.read_text().splitlines():
        t, _, label = line.partition("\t")
        t = float(t)
        word = len(label) == 1 and label.isalnum()
        if tokens and word and tokens[-1][1][-1:].isalnum() and len(tokens[-1][1]) < 12 \
                and t - tokens[-1][0] < 0.6 and tokens[-1][1] not in ("esc", "tab", "space"):
            tokens[-1] = (t, tokens[-1][1] + label)
        else:
            tokens.append((t, label))
    return tokens


def draw_caption(img: Image.Image, text: str, alpha: float) -> Image.Image:
    """A dark rounded label centred near the bottom, above the status bar."""
    scale = img.width / 1200
    f = caption_font(int(30 * scale))
    d = ImageDraw.Draw(img)
    l, t, r, b = d.textbbox((0, 0), text, font=f)
    pad_x, pad_y = int(22 * scale), int(12 * scale)
    w, h = r - l + 2 * pad_x, b - t + 2 * pad_y
    x, y = (img.width - w) // 2, img.height - int(90 * scale) - h
    layer = Image.new("RGBA", img.size, (0, 0, 0, 0))
    ld = ImageDraw.Draw(layer)
    ld.rounded_rectangle([x, y, x + w, y + h], radius=int(14 * scale), fill=(20, 20, 26, int(215 * alpha)))
    ld.text((x + pad_x - l, y + pad_y - t), text, font=f, fill=(255, 255, 255, int(255 * alpha)))
    return Image.alpha_composite(img, layer)


def make_gif(frames: Path, dest: Path, hold: float) -> None:
    """Captions the frames and writes a GIF with Pillow: one palette for every frame, no
    dithering (flat UI colours don't need it), and each frame stored as just the part that
    changed, with repeated frames merged. (ffmpeg stores every frame whole: 50 MB vs 1 MB.)"""
    tokens = key_tokens(frames / "keys.tsv") if (frames / "keys.tsv").exists() else []
    pngs = sorted(frames.glob("[0-9]*.png"))
    final_size = Image.open(pngs[-1]).size
    shown = []
    for i, png in enumerate(pngs):
        img = Image.open(png).convert("RGBA")
        # Frames from before the window took its final size.
        if img.size != final_size:
            continue
        t = i / FPS
        recent = [(kt, k) for kt, k in tokens if kt <= t]
        if recent and t - recent[-1][0] < 1.2:
            last = recent[-1][0]
            text = "  ".join(k for kt, k in recent if last - kt < 1.5)[-40:].strip()
            img = draw_caption(img, text, min(1.0, (1.2 - (t - last)) / 0.3))
        w = min(GIF_WIDTH, img.width)
        shown.append(img.resize((w, round(img.height * w / img.width)), Image.Resampling.LANCZOS))

    sample = shown[:: max(1, len(shown) // 20)]
    mosaic = Image.new("RGB", (shown[0].width, shown[0].height * len(sample)))
    for i, f in enumerate(sample):
        mosaic.paste(f.convert("RGB"), (0, i * f.height))
    palette = mosaic.quantize(255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
    out = []
    for f in shown:
        q = f.convert("RGB").quantize(palette=palette, dither=Image.Dither.NONE)
        # Index 255 is transparent: the window's rounded corners.
        q.paste(255, mask=f.getchannel("A").point(lambda a: 255 if a < 128 else 0))
        out.append(q)
    durations = [1000 // FPS] * len(out)
    durations[-1] += int(hold * 1000)
    out[0].save(dest, save_all=True, append_images=out[1:], duration=durations, loop=0, transparency=255, disposal=1)
    print(f"  {dest.relative_to(ROOT)}  {len(out)} frames  {dest.stat().st_size // 1024} KB")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("scenes", nargs="*")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--keep-home", action="store_true", help="reuse build/demo-home as it is")
    args = ap.parse_args()
    if args.list:
        for s in SCENES:
            print(f"{s.name:10} {'gif' if s.gif else 'png'}  {s.start or '~'}  {s.keys}")
        return
    scenes = [s for s in SCENES if not args.scenes or s.name in args.scenes]
    if not scenes:
        raise SystemExit(f"no such scene; --list shows them")
    if not APP.exists():
        raise SystemExit("build the app first: swift build --package-path App")
    OUT.mkdir(parents=True, exist_ok=True)
    if not (args.keep_home and DEMO.exists()):
        subprocess.run([sys.executable, str(ROOT / "scripts/demo/make_home.py"), str(DEMO)], check=True)

    # Scenes that change files go last, so the others see the demo as made.
    for scene in sorted(scenes, key=lambda s: s.changes_files):
        print(scene.name)
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            if scene.gif:
                # Kept in build/frames to look at or re-encode.
                frames = ROOT / "build/frames" / scene.name
                shutil.rmtree(frames, ignore_errors=True)
                run_tilde(scene, tmp / "last.png", "dark", frames)
                make_gif(frames, OUT / f"{scene.name}.gif", scene.hold)
            else:
                for appearance in ("dark", "light"):
                    run_tilde(scene, tmp / f"{appearance}.png", appearance, None)
                    save_still(tmp / f"{appearance}.png", OUT / f"{scene.name}-{appearance}.png")
        if scene.changes_files:
            subprocess.run([sys.executable, str(ROOT / "scripts/demo/make_home.py"), str(DEMO)], check=True,
                           stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
