# Tilde

A fast, keyboard-first file navigator for macOS. Rust core, Swift/AppKit UI.

- Design and build plan: [docs/SPEC.md](docs/SPEC.md)
- Interactive prototypes: [docs/prototypes/](docs/prototypes/)

## Requirements

- macOS 14 or later
- Rust (stable) via rustup
- Swift 6 toolchain (Xcode Command Line Tools are enough for development)

## Run

```sh
scripts/dev.sh            # build the core and the app, then launch Tilde
```

Or step by step:

```sh
scripts/build-core.sh     # Rust core → App/Vendor (static lib + generated Swift bindings)
swift build --package-path App
App/.build/debug/Tilde
```

Re-run `scripts/build-core.sh` after changing anything under `core/`.

## Test

```sh
cd core && cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release -p tilde-fs --example bench_list -- 10000
cargo run --release -p tilde-index --example bench_index        # scans your home folder
```

Debug builds can render their window to a PNG without screen-recording access, optionally
replaying keys first (`\t` tab, `\e` escape, `\r` return, `\b` delete):

```sh
TILDE_SNAPSHOT=/tmp/tilde.png TILDE_SNAPSHOT_KEYS='Dev\tprojects\t' App/.build/debug/Tilde
```

## Keys (so far)

| Mode | Keys |
|---|---|
| GO (typing searches) | type to search this folder and everywhere · `↑` `↓` move · `⏎` open · `tab` or `/` go into folder (or reveal an Everywhere file) · `⌫` on empty bar goes up · `~` home · `esc` → SELECT |
| SELECT | `j` `k` move · `h` `l` out / in · `⏎` open · `g` `G` top / bottom · `/` → GO · `esc` clear filter |
| Anywhere | `⌘⇧.` show hidden files · `⌘N` new window · `⌘,` open config file · `⌘R` reload config |

Filters: `ext:pdf`, `kind:image`, `mod:<7d`, `size:>100M`, `in:dev` (path contains), `in:.` (this folder and below).

## Search

Typing shows matches in the current folder first, then an **Everywhere** section from an
index of your home folder. The index is built in the background at launch (the status bar
shows progress); it skips `~/Library`, hidden files, `node_modules`, `.git` and similar
folders, and doesn't cross into other disks or mounts. Configure it under `[index]`.

If [zoxide](https://github.com/ajeetdsouza/zoxide) is installed, folders you visit often
rank higher, every folder zoxide knows is searchable, and folders you open in Tilde are
added to zoxide so `z` in your shell learns from Tilde. Configure it under `[zoxide]`.

## Config

`~/.config/tilde/config.toml` (or `$XDG_CONFIG_HOME/tilde/config.toml`). `⌘,` creates it from a
commented template and opens it; `⌘R` applies changes.

```toml
[font]
family = "JetBrains Mono"   # closest installed match is used, e.g. "JetBrainsMono Nerd Font"
size = 13                   # 8–32; other text scales from this

[index]                     # applies on next launch
roots = ["~"]
exclude = ["node_modules", ".git", "target", "build", "DerivedData", "Pods",
           "__pycache__", ".venv", "venv", "~/Library", "~/.Trash"]
include_hidden = false

[zoxide]
enabled = true              # rank by zoxide frecency, include its folders
add_on_visit = true         # tell zoxide about folders opened in Tilde
```
