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
```

Debug builds can render their window to a PNG without screen-recording access, optionally
replaying keys first (`\t` tab, `\e` escape, `\r` return, `\b` delete):

```sh
TILDE_SNAPSHOT=/tmp/tilde.png TILDE_SNAPSHOT_KEYS='Dev\tprojects\t' App/.build/debug/Tilde
```

## Keys (so far)

| Mode | Keys |
|---|---|
| GO (typing searches) | type to filter · `↑` `↓` move · `⏎` open · `tab` or `/` go into folder · `⌫` on empty bar goes up · `~` home · `esc` → SELECT |
| SELECT | `j` `k` move · `h` `l` out / in · `⏎` open · `g` `G` top / bottom · `/` → GO · `esc` clear filter |
| Anywhere | `⌘⇧.` show hidden files · `⌘N` new window |

Filters: `ext:pdf`, `kind:image`, `mod:<7d`, `size:>100M`.
