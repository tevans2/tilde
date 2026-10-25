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
| GO (typing searches) | type to search this folder and everywhere · `↑` `↓` or `⌃j` `⌃k` move · `⏎` open · `tab`, `/` or `⌃l` go into folder (or reveal an Everywhere file) · `⌫` on empty bar or `⌃h` goes up · `~` home · `esc` → SELECT |
| SELECT | `j` `k` (or `⌃j` `⌃k`) move · `h` `l` (or `⌃h` `⌃l`) out / in · `⏎` open · `g` `G` top / bottom · `/` → GO · `esc` clear filter, then shelf |
| SELECT: files | `x` or `space` add to / remove from shelf · `a` all shown · `m` move · `c` copy · `d` trash · `r` rename · `n` new folder · `z` compress · `y` copy paths · `u` undo · `U` or `⌃R` redo · `X` hide from search |
| Anywhere | `⌘Z` / `⌘⇧Z` undo / redo · `⌘⌫` trash · `⌘⇧N` new folder · `⌘⇧.` show hidden files · `⌘⇧I` search index · `⌘N` new window · `⌘,` open config file · `⌘R` reload config |

File actions apply to the shelf when it has anything, otherwise to the highlighted row.
Move and copy open a folder picker: type to fuzzy-search folders (your most-visited zoxide
folders show first), or type a path like `~/Documents/`. Name clashes are resolved like the
Finder (`report 2.pdf`); copies on the same disk are instant APFS clones; trashed items can
be put back with undo or from the Finder. Undo history lasts for the session and is shared
by all windows.

Filters: `ext:pdf`, `kind:image`, `mod:<7d`, `size:>100M`, `in:dev` (path contains), `in:.` (this folder and below).

## Search

Typing shows matches in the current folder first, then an **Everywhere** section from an
index of your home folder. The first launch builds it in the background (the status bar
shows progress); after that it's saved to `~/Library/Caches/Tilde/index.bin` and loaded at
launch, with FSEvents replaying whatever changed while Tilde was closed. While Tilde runs,
changes are picked up as they happen. It skips `~/Library`, hidden files, `node_modules`,
`.git` and similar folders, and doesn't cross into other disks or mounts. The open folder
also refreshes by itself when its contents change.

**Search Index window (`⌘⇧I`):** item count against the limit, memory, size on disk, and the
folders holding the most items, each with **Exclude contents** (keep the folder, skip what's
inside) and **Hide** (leave it out). **Rebuild** re-reads the config and indexes again;
**Clear** deletes the saved index and turns Everywhere off until you rebuild. In SELECT,
`X` hides the highlighted item. Exclusions are written to the config, so you can undo them
there.

If [zoxide](https://github.com/ajeetdsouza/zoxide) is installed, folders you visit often
rank higher, every folder zoxide knows is searchable, and folders you open in Tilde are
added to zoxide so `z` in your shell learns from Tilde. Configure it under `[zoxide]`.

## Themes and preview

Colour keys: `background bar border text text_dim selection selection_text match
match_selected go select shelf badge_label badge_folder badge_app badge_image badge_video
badge_audio badge_pdf badge_document badge_code badge_text badge_archive badge_other`.
Values are `#RRGGBB` or `#RRGGBBAA`. Anything unset uses the preset, or the system colour
for `default`. Mistakes are reported in the status bar and skipped.

The preview pane shows code and other text files through [bat](https://github.com/sharkdp/bat)
when it's installed (with line numbers, in a bat theme that matches light or dark), and
everything else, including Markdown, through Quick Look.

## Config

`~/.config/tilde/config.toml` (or `$XDG_CONFIG_HOME/tilde/config.toml`). `⌘,` creates it from a
commented template and opens it; `⌘R` applies changes.

```toml
[font]
family = "JetBrains Mono"   # closest installed match is used, e.g. "JetBrainsMono Nerd Font"
size = 13                   # 8–32; other text scales from this

[theme]
appearance = "system"       # system, light or dark
preset = "default"          # default (macOS colours), catppuccin, gruvbox, nord, solarized
[theme.colors]              # override any colour for both appearances
# match = "#ff9e64"
[theme.dark]                # ...or for one appearance ([theme.light] too)
# background = "#101014"

[preview]
visible = true
width = 0.5                 # share of the window (0.15–0.85)
bat = true                  # syntax-highlight code and text with bat; Markdown uses Quick Look
bat_theme_light = ""        # empty: matches the preset, else "GitHub"
bat_theme_dark = ""         # empty: matches the preset, else your bat config
max_lines = 1000

[index]                     # apply with Rebuild (⌘⇧I)
enabled = true
roots = ["~"]
exclude = ["node_modules", ".git", "target", "build", "DerivedData", "Pods",
           "__pycache__", ".venv", "venv", "~/Library", "~/.Trash"]   # skip contents
hide = []                   # leave out entirely: names, *.log-style patterns, or paths
include_hidden = false
max_items = 1000000         # indexing stops here; ⌘⇧I shows what to exclude

[zoxide]
enabled = true              # rank by zoxide frecency, include its folders
add_on_visit = true         # tell zoxide about folders opened in Tilde
```
