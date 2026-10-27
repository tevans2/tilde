# Contributing

Thanks for looking. Tilde is a Rust core (`core/`) with a Swift/AppKit app (`App/`), joined by
[UniFFI](https://github.com/mozilla/uniffi-rs). The design and build plan is
[docs/SPEC.md](docs/SPEC.md).

Bugs and ideas: [open an issue](https://github.com/tevans2/tilde/issues/new/choose). For anything
bigger than a fix, an issue first saves wasted work.

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

## Install your build

```sh
scripts/install.sh        # optimized build → ~/Applications/Tilde.app (or pass a folder)
scripts/update.sh         # rebuild the installed copy in place; reopens it if it was running
scripts/uninstall.sh      # remove it, with its permissions and preferences
```

Builds are signed with a local certificate made the first time (in its own keychain,
`~/Library/Keychains/tilde-signing.keychain-db`), so macOS sees every build as the same app
and permissions (Full Disk Access, Accessibility, folders) carry over between updates. A
copy installed before that was signed ad hoc and needs them granting once more after its
first update. `TILDE_SIGN_IDENTITY=…` signs with your own identity instead.

`uninstall.sh --purge` also deletes the index, undo history and sort orders (the dev build
shares them); your config is kept. The icon (`App/Icon/`, drawn by
`scripts/make-icon.swift`) has light and dark variants; the Dock icon follows the
appearance while Tilde runs.

## Test

```sh
cd core && cargo test --workspace
swift test --package-path App      # the app's own logic: keys, jump list, history graph
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release -p tilde-fs --example bench_list -- 10000
cargo run --release -p tilde-index --example bench_index        # scans your home folder
```

CI runs the same (format, clippy, tests, both builds) on every push and pull request.

### Snapshots

Debug builds can render their window to a PNG without screen-recording access, optionally
replaying keys first (`\t` tab, `\e` escape, `\r` return, `\b` delete, `^j` control,
`@s` command, `\w` a one-second pause). `TILDE_SNAPSHOT_WINDOW=settings|keys|welcome|history|cleanup|cleanup-settings`
snapshots that window instead:

```sh
TILDE_SNAPSHOT=/tmp/tilde.png TILDE_SNAPSHOT_KEYS='Dev\tprojects\t' App/.build/debug/Tilde
```

### Screenshots and GIFs

The images in the README and guide (`docs/images/`) are made by a script, so they can be
redone after a UI change:

```sh
pip3 install pillow                      # once
swift build --package-path App
python3 scripts/screenshots.py           # every scene; --list shows them
python3 scripts/screenshots.py hero      # or some
```

It builds a made-up home folder in `build/demo-home` (`scripts/demo/make_home.py`:
projects, invoices, photos, downloads with their sites, screenshots, an AirDrop, idle build
output, caches, a big old video) and runs the debug build in it with `HOME` pointed there,
so none of your files appear and your own Tilde's settings aren't touched. Each scene plays
keys and saves exactly Tilde's window at 1200×750 points: stills in light and dark, GIFs
with a caption of the keys pressed. AeroSpace, if running, is asked to float the window.
To add a scene, add a `Scene` to `SCENES` in the script.

## Pull requests

- Keep `cargo fmt`, `cargo clippy -D warnings` and the tests passing.
- Match the code around you: its naming, comment density and idiom.
- Update the README, [docs/GUIDE.md](docs/GUIDE.md) or [docs/CONFIG.md](docs/CONFIG.md) when behaviour changes,
  and add a line under **Unreleased** in [CHANGELOG.md](CHANGELOG.md).

## Releasing

1. Once: `scripts/make-release-cert.sh --upload` makes the signing certificate and stores it
   as the `TILDE_SIGNING_P12` / `TILDE_SIGNING_PASSWORD` secrets. Back up the folder it
   prints: releases signed with a different certificate lose users' permissions.
2. Set `version` in `core/Cargo.toml`, and rename **Unreleased** in `CHANGELOG.md` to
   `## [x.y.z] - date`.
3. Commit, then tag and push: `git tag vx.y.z && git push origin vx.y.z`.

The [Release workflow](.github/workflows/release.yml) checks the tag against the version,
builds a universal `Tilde.app` (`scripts/bundle.sh --universal`), packages it
(`scripts/package.sh`: `Tilde-macOS.zip`, `Tilde.dmg`, `SHA256SUMS.txt`) and publishes a
release with the changelog section as notes. A version with a `-` (`0.3.0-beta.1`) is
published as a prerelease. Running the workflow by hand from the Actions tab builds the
same files as an artifact without publishing.

To try the packaging locally: `scripts/bundle.sh --universal && scripts/package.sh` (needs
`rustup target add x86_64-apple-darwin`), then
`TILDE_RELEASE_URL=file://$PWD/dist bash scripts/get.sh /tmp/tilde-test`.
