# Tilde: Specification and Build Plan

Status: M0 in progress (October 2026). Build progress is tracked in §13.
Stack: Rust core and Swift/AppKit UI

Tilde is a fast, keyboard-first file navigator for macOS. It is designed from first principles around what people actually do with files: get back to something known, find what just arrived, act on files, organize, and inspect. It does not copy Finder and runs alongside it.

Prototypes (open the HTML files in a browser):
- Combined prototype (the agreed design): [`docs/prototypes/tilde-prototype.html`](prototypes/tilde-prototype.html), also at https://claude.ai/artifact/Eicd7mjvxHRxZNsk3JXV95
- Earlier concept mockups, including the dropped summoned panel: [`docs/prototypes/tilde-mockups.html`](prototypes/tilde-mockups.html), also at https://claude.ai/artifact/PWCZWeeEoFbeXGDBLWczUB

The prototypes predate the decision to use real Neovim for EDIT, so their text mode is the built-in editor and matches lines by position instead of by concealed IDs.

---

## 1. Settled decisions

| Area | Decision |
|---|---|
| Product | Keyboard-first navigator. Not a Finder clone; coexists with Finder. |
| Navigation | One bar is both the path and the search. Results are split into **Here** (current folder) and **Everywhere** (global index). Space separates search terms. `tab` or `/` enters a folder (or reveals a file). `⌫` on an empty bar goes up. |
| Modes | **GO**: typing searches. `esc` → **SELECT**: single-key verbs and a shelf. `r` / `o` / `e` → **EDIT**: the folder as text. The search filter carries from GO into SELECT. |
| Activity | A place you visit (`@`), shown in the path bar. Not the home view. |
| EDIT | Built-in text editor by default. Neovim is optional, hosted in an embedded terminal, and used only for EDIT. |
| Line identity in EDIT | Oil-style concealed text prefix (`/0007 invoice.pdf`). Not extmarks, because extmarks do not survive yank and paste. |
| File operations | One engine, one batch model, one undo history for every source of change. |
| Stack | Rust core (engine) and Swift/AppKit UI, connected through UniFFI. |
| Distribution | Developer ID signed, notarized, not sandboxed, distributed directly with Sparkle. |
| Minimum OS | macOS 14 (Sonoma). Universal binary (arm64 and x86_64). |
| Deferred | Summoned panel, terminal pane, `!` shell commands on the selection, script actions, `tilde` CLI, remote/SSH browsing, privileged writes, Accessibility automation. |

---

## 2. Interaction model

### 2.1 Modes

| Mode | Enter | Leave | Keys mean |
|---|---|---|---|
| GO | App start, `/` from SELECT, clicking the bar | `esc` → SELECT (filter kept) | Text input. Arrows move the selection. |
| SELECT | `esc` from GO, after applying EDIT | `/` → GO, `r` `o` `e` → EDIT | Single-key verbs |
| EDIT | `r` (rename), `o` (new item), `e` (whole folder) | Apply → SELECT, discard → SELECT | Text editing (built-in or nvim) |

### 2.2 GO

| Key | Action |
|---|---|
| typing | Fuzzy match in Here, then Everywhere |
| space | Separate search terms |
| `tab`, `/` | Enter the highlighted folder, or reveal a file in its folder |
| `⏎` | Open the file or enter the folder |
| `⌫` (empty bar) | Up one level |
| `'` then a letter | Jump to a mark |
| `@` | Activity view (again to return) |
| `~` | Home |
| `↑` `↓`, `⌃N` `⌃P` | Move the selection |
| `esc` | Switch to SELECT, keeping the filter |

Query filters: `in:<fuzzy path>`, `in:.` (this folder and below), `ext:<ext>`, `kind:image|code|pdf|doc|archive|folder|text`, `mod:<N[m|h|d|w]`, `size:>N[K|M|G]`.

### 2.3 SELECT

| Key | Action |
|---|---|
| `j` `k`, `↑` `↓` | Move |
| `h` `l`, `←` `→` | Up a level, enter the folder or open the file |
| `x`, space | Toggle the highlighted item on the shelf |
| `a` | Toggle all shown items on the shelf |
| `m` | Move (destination picker) |
| `c` | Copy (destination picker) |
| `y` | Copy paths to the clipboard |
| `t` | Tag |
| `z` | Compress |
| `d`, `⌫` | Trash |
| `r` | Rename (EDIT, quick mode, cursor on the name) |
| `o` | New file or folder (EDIT, quick mode, new line) |
| `e` | Edit the whole folder as text |
| `u`, `⌃R` | Undo, redo |
| `/` | Back to GO |
| `@`, `'`, `~` | Activity, marks, home |
| `esc` | Clear the filter, then the shelf |
| `1`–`5`, `i` | In Activity: filter by origin, inbox toggle |

Verbs act on the shelf if it has items, otherwise on the highlighted row.

### 2.4 EDIT

| Input | Effect |
|---|---|
| Change a name | Rename |
| Change a name to a path (`~/Archive/x.pdf`, `sub/x.pdf`) | Move |
| Delete a line | Trash |
| Duplicate a line (`yyp`) | Copy |
| New line | Create a file; end it with `/` to create a folder |
| Built-in: `⌘S` / nvim: `:w` | Apply |
| Quick mode (`r`, `o`): `⏎` | Apply |
| Built-in: `esc` (twice if changed) / nvim: `:q`, `:q!` | Discard |
| nvim only: `⌘S`, `⌘.` | Apply, cancel (reserved by Tilde) |

The side pane shows a live list of pending changes for both editors.

---

## 3. Architecture

### 3.1 Process and layers

```
┌──────────────────────────── Tilde.app (Swift, AppKit) ───────────────────────────┐
│ ModeController · KeyDispatcher                                                   │
│ PathBar · ResultsView (NSTableView) · PreviewPane (Quick Look) · ShelfView       │
│ DestinationPicker · StatusBar · Settings/Onboarding (SwiftUI)                    │
│ EditHost ─┬─ BuiltInEditor (NSTextView, concealed IDs)                           │
│           └─ NvimEditor ── TerminalHost (GhosttyHost | SwiftTermHost)            │
│ EngineAdapter (Swift wrapper over generated bindings, hops to MainActor)         │
└───────────────────────────────────┬──────────────────────────────────────────────┘
                                    │ UniFFI (coarse commands ↓, events ↑)
┌───────────────────────────────────▼──────────────────────────────────────────────┐
│ tilde-ffi: Engine object, EngineListener callback interface                      │
├──────────────────────────────────────────────────────────────────────────────────┤
│ tilde-query   tilde-index   tilde-fs   tilde-ops   tilde-edit                    │
│ tilde-activity   tilde-nvim   tilde-config   tilde-types                         │
└──────────────────────────────────────────────────────────────────────────────────┘
                                    │ PTY owned by TerminalHost
                              nvim --listen <socket>  ◄── nvim-rs client (tilde-nvim)
```

### 3.2 Responsibility split

| Concern | Lives in | Notes |
|---|---|---|
| Folder listing, watching, metadata | Rust `tilde-fs` | `getattrlistbulk` via libc; FSEvents via `fsevent-sys` |
| Global index, frecency | Rust `tilde-index` | memmap2 binary index; JSON for frecency |
| Query parsing and ranking | Rust `tilde-query` | nucleo |
| File operations, undo history | Rust `tilde-ops` | `copyfile`, `rename`, trash via objc2 NSFileManager |
| Edit text format, diff to batch | Rust `tilde-edit` | Shared by both editors |
| Activity log, download origins | Rust `tilde-activity` | `kMDItemWhereFroms` xattr parsed with `plist` |
| nvim lifecycle and RPC | Rust `tilde-nvim` | nvim-rs on tokio |
| Config and keymaps (parsing) | Rust `tilde-config` | toml and serde |
| Drawing, keys, focus, modes | Swift | AppKit |
| Previews, thumbnails | Swift | QLPreviewView, QLThumbnailGenerator |
| Open, Open With | Swift | NSWorkspace, Launch Services |
| Terminal view and PTY | Swift `TerminalHost` | libghostty or SwiftTerm spawns nvim with argv from Rust |
| Clipboard | Swift | NSPasteboard |

### 3.3 Threading

- **Swift**: the main thread only renders and dispatches keys. `EngineAdapter` receives callbacks on Rust threads and hops to `@MainActor`.
- **Rust**:
  - One small tokio runtime owned by `Engine`, used for nvim RPC, timers and debouncing.
  - Blocking file I/O runs on dedicated threads (listing pool, ops executor thread, FSEvents thread).
  - nucleo manages its own matcher threads.
  - The index is behind an `RwLock`; queries read snapshots.
- **Generations**: every query, listing and edit preview carries a `u64` generation. Rust drops work for stale generations and Swift ignores stale events.

---

## 4. FFI design (UniFFI)

### 4.1 Rules

1. Exchange coarse commands, not chatty calls. No per-file calls across the boundary except for fetching visible rows.
2. Data stays in Rust. Swift fetches row windows (for example rows 120–160) with highlight ranges, never full result sets.
3. Cancellation is by generation number.
4. Rust never blocks the calling Swift thread for more than about 1 ms. Long work returns immediately and reports through `EngineListener`.
5. Errors are typed (`EngineError` enum with user-presentable messages).
6. Start with UniFFI everywhere. Replace one hot path with a hand-written C interface only if profiling shows UniFFI overhead matters (target: under 0.5 ms per keystroke for all boundary crossings combined).

### 4.2 Surface (initial)

```rust
// tilde-ffi (UniFFI-exported)
object Engine {
  constructor(config: EngineConfig, listener: EngineListener)

  // navigation and query
  fn open_folder(path: String, gen: u64)
  fn open_activity(filter: ActivityFilter, gen: u64)
  fn set_query(text: String, gen: u64)            // scope = current location
  fn rows(gen: u64, start: u32, count: u32) -> Vec<RowView>
  fn preview_info(row_id: RowId) -> EntryInfo

  // shelf and marks
  fn shelf_toggle(ids: Vec<RowId>)
  fn shelf_clear()
  fn shelf() -> Vec<EntryInfo>
  fn marks() -> Vec<Mark>
  fn destinations(query: String, limit: u32) -> Vec<DestinationView>

  // operations
  fn submit_batch(batch: BatchRequest) -> OpId    // move, copy, trash, tag, compress
  fn undo() -> Option<OpId>
  fn redo() -> Option<OpId>
  fn cancel_op(op: OpId)

  // edit sessions (both editors)
  fn edit_begin(dir: String, mode: EditMode, focus: Option<RowId>) -> EditSessionView
  fn edit_preview(session: SessionId, text: String, gen: u64)  // emits pending ops
  fn edit_apply(session: SessionId, text: String) -> OpId
  fn edit_cancel(session: SessionId)

  // nvim
  fn nvim_status() -> NvimStatus                   // found, version, path
  fn nvim_launch_spec(config: NvimConfig) -> LaunchSpec  // argv, env, cwd, socket
  fn nvim_process_started(pid: i32)
  fn nvim_process_exited(code: i32)
  fn nvim_edit(session: SessionId)                 // loads buffer via RPC
  fn nvim_write()                                  // Tilde's ⌘S → :w
}

callback interface EngineListener {
  fn results_ready(gen: u64, here_count: u32, everywhere_count: u32)
  fn folder_changed(gen: u64)
  fn index_progress(scanned: u64, done: bool)
  fn op_progress(op: OpId, done_bytes: u64, total_bytes: u64, current: String)
  fn op_finished(op: OpId, result: OpResult)       // includes undo label, conflicts
  fn edit_pending(session: SessionId, gen: u64, ops: Vec<PendingOpView>, errors: Vec<LineError>)
  fn nvim_event(event: NvimEvent)                  // Applied, Cancelled, Error, Crashed
  fn external_change(paths: Vec<String>)           // not undoable
}
```

`RowView` carries the display name, path (for Everywhere rows), kind, size, modification time, origin, tags, shelf flag, section (Here or Everywhere) and highlight ranges as UTF-16 offsets, ready for `NSAttributedString`.

### 4.3 Build pipeline

The app is a Swift package (`App/Package.swift`), so development needs only the Command Line Tools, not Xcode. Xcode is needed later for signing and notarization (M7).

- `scripts/build-core.sh [debug|release]`:
  1. `cargo build -p tilde-ffi` for the host architecture, plus `x86_64-apple-darwin` on release builds when that target is installed.
  2. `lipo` into a universal static library when there are two architectures.
  3. `uniffi-bindgen generate --library` (a bin target in `tilde-ffi`, built in a separate target dir) for the Swift bindings, header and module map.
  4. Copies everything to `App/Vendor/` (gitignored).
- `App/Package.swift` has three targets:
  - `tilde_ffiFFI`: system library target over the generated header and module map.
  - `TildeFFI`: the generated Swift bindings (Swift 5 language mode), linking `libtilde_ffi.a`.
  - `Tilde`: the AppKit executable.
- `scripts/dev.sh` builds both and launches the app. A bundling script will assemble `Tilde.app` (Info.plist, signing) in M7.
- CI (GitHub Actions, macOS runners): `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, benchmark smoke run, `swift build`, perf regression check, notarized build on tags.

---

## 5. Repository layout

```
tilde/
  docs/
    SPEC.md
  core/                         # cargo workspace
    Cargo.toml
    crates/
      tilde-types/              # shared ids, entries, errors
      tilde-fs/                 # lister, watcher, metadata, cloud placeholder checks
      tilde-index/              # index storage, build, FSEvents catch-up, frecency
      tilde-query/              # parser, nucleo integration, ranking, row windows
      tilde-ops/                # batch, planner, executor, undo history
      tilde-edit/               # edit text format, parser, diff → batch
      tilde-activity/           # event log, origins, inbox
      tilde-nvim/               # discovery, launch spec, RPC client, plugin protocol
      tilde-config/             # TOML config, keymaps, marks
      tilde-ffi/                # UniFFI exports (the only crate Swift sees)
    benches/                    # criterion benchmarks
    tests/                      # integration tests on temp folders and disk images
  App/                          # Swift package
    Package.swift
    Sources/Tilde/              # AppKit app (modes, views, edit, terminal, settings, onboarding)
    Vendor/                     # generated by build-core.sh (gitignored)
      include/                  # UniFFI header + module map
      swift/                    # UniFFI Swift bindings
      lib/                      # libtilde_ffi.a
  Resources/
    tilde.nvim/                 # bundled Lua plugin
      lua/tilde/init.lua
      plugin/tilde.lua
  scripts/
    build-core.sh
    gen-tree.rs                 # synthetic trees for perf tests
```

---

## 6. Module specifications

### 6.1 tilde-fs

- **Lister**: `getattrlistbulk` returning name, object type, file ID, size, allocated size, modification and creation time, flags (hidden, package), and finder info.
  - Streams batches of about 512 entries.
  - Cancellable by generation.
  - Sorting uses Finder-style natural ordering (`file2` before `file10`) with case-insensitive, normalization-insensitive comparison.
- **Watcher**: FSEvents with `kFSEventStreamCreateFlagFileEvents`, about 50 ms latency, coalesced into folder-changed events. The open folder is re-listed incrementally.
- **Metadata**: tags (`com.apple.metadata:_kMDItemUserTags`), download origin (`kMDItemWhereFroms`), quarantine flag, iCloud and File Provider status.
- **Never read contents of dataless (cloud placeholder) files.** Check `SF_DATALESS` / ubiquitous status before any content access.
- Names are kept as raw bytes plus a normalized (NFC, case-folded) form for matching. Display uses the original bytes.

### 6.2 tilde-index

- **Storage** (memory-mapped, column-oriented):
  - `parent_id: u32`, `name_offset: u32`, `name_len: u16`, `flags: u16`, `mtime: u32` (minutes since epoch), `size: u64`.
  - A name byte arena, plus a normalized-name arena for matching.
  - Target: 40–60 bytes per entry, about 100 MB for 2 million entries.
- **Build**: parallel walk with the `ignore` crate's walker (exclusions as gitignore-style rules) feeding `getattrlistbulk` per directory. Runs in the background with progress events.
- **Persistence**: index file and a header containing the last processed FSEvents event ID and volume UUID.
- **Catch-up**: on launch, start an FSEvents stream with `sinceWhen = last_event_id`. On `kFSEventStreamEventFlagMustScanSubDirs` or history-dropped flags, rescan the affected subtree. On volume UUID change, rebuild.
- **Default scope**: the home folder. Contents of `~/Library`, `.git`, `node_modules`, `.cache`, `DerivedData`, `target`, `build` and `.Trash` are excluded, but the folders themselves are indexed. Configurable roots and exclusions.
- **Frecency**: a score per path for opens and visits, with exponential decay (JSON file). Feeds a ranking boost.

### 6.3 tilde-query

- **Parser**: terms and filters (section 2.2). Unknown filter keys are treated as plain text.
- **Matching**: nucleo over the normalized name, with a path-aware second pass for terms that match across segments (`tilde swift`).
- **Ranking** = nucleo score
  + filename-match bonus
  + frecency boost
  + recency boost
  − path depth penalty
  + Here-section priority.
- **Incremental**: when the new query extends the previous one, restrict matching to the previous candidates.
- **Scopes**: Here (children of the current folder, or Activity items) and Everywhere (index minus Here). Everywhere is limited to the top 200 results.
- **Row windows**: results are held in Rust; `rows(gen, start, count)` returns views with UTF-16 highlight ranges.
- **Debug**: optional score breakdown per row for tuning.

### 6.4 tilde-ops

- **Batch model**: an ordered list of `Rename`, `Move`, `Copy`, `Trash`, `CreateFile`, `CreateDir`, `SetTags` and `Compress`, each with source identity (path, file ID, mtime).
- **Planner** (pure, no side effects):
  - Each source must exist and match its recorded file ID and mtime; otherwise report a stale conflict.
  - Destinations must be free, or follow the conflict policy (ask, skip, replace, keep both).
  - Cycles and swaps (a→b, b→a) are resolved with temporary names.
  - Case-only renames on case-insensitive volumes go through a temporary name.
  - Invalid names are rejected: empty, contains `:` or NUL, longer than 255 bytes, or `.` / `..`.
  - Folder-into-itself and overlapping operations are rejected.
  - Output: an executable plan or a list of conflicts for the UI.
- **Executor** (dedicated thread):
  - Runs operations in order, emits progress and is cancellable between and within copies.
  - `copyfile(3)` with `COPYFILE_CLONE | COPYFILE_ALL | COPYFILE_RECURSIVE` and a progress callback.
  - Cross-volume moves are copy, verify, then delete.
  - Trash uses `NSFileManager trashItemAtURL:resultingItemURL:` through objc2, recording the resulting Trash location.
  - Compress uses `ditto -c -k --sequesterRsrc --keepParent` (or a zip crate) into the target folder.
- **Undo history**: each applied batch stores its inverse.
  - Rename and move invert to the reverse move.
  - Trash inverts to a move back from the recorded Trash location.
  - Copy and create invert to a trash of the result.
  - Tags restore the previous tags.
  - Redo is supported. The history is kept in memory and persisted for the session.
  - Changes detected through FSEvents that Tilde did not make are reported as `external_change` and are not undoable.

### 6.5 tilde-edit

- **Text format**: one entry per line, `/<id> <name>`, with folders ending in `/`. IDs are session-local integers formatted to a fixed width (`/0007`), mapped to an entry's file ID and path.
- **Diff rules**:

| Line state | Result |
|---|---|
| Known ID, same name | No-op |
| Known ID, new plain name | Rename |
| Known ID, name contains `/` (after stripping a trailing `/`) or starts with `~/` | Move (with rename) |
| Known ID appears more than once | First occurrence is the original; the others are copies |
| Known ID missing | Trash |
| No ID, ends with `/` | Create folder |
| No ID | Create empty file |
| Unknown ID (not in this session) | Error on that line |
| Duplicate target names, invalid names | Error on that line; Apply is blocked |

- **Paths**: relative paths resolve against the session folder; `~/` resolves against home. Missing parent folders are created as part of the batch.
- **Output**: `PendingOpView` list (for the live preview) or a `BatchRequest` for the planner.
- **Folder changes during a session**: emit a banner event. On apply, the planner's stale checks produce conflicts instead of guessing.

### 6.6 tilde-activity

- **Event log** (SQLite): creates, modifications, moves and Tilde-initiated filing, sourced from FSEvents within indexed roots. Retained for 90 days (configurable).
- **Origin detection**:
  - **Download**: has `kMDItemWhereFroms` or a quarantine flag. The source domain comes from the first URL.
  - **Screenshot**: in the screenshot location (`com.apple.screencapture location`) and the `kMDItemIsScreenCapture` attribute.
  - **AirDrop**: arrived in Downloads with an AirDrop quarantine agent.
  - **Edited / created**: the remaining file-change events.
- **Inbox**: files in Downloads or on the Desktop that are not yet filed. Moving a file out (by Tilde or externally) marks it filed.

### 6.7 tilde-config

- `~/.config/tilde/config.toml` holds:
  - index roots and exclusions
  - marks
  - keymap overrides per mode
  - editor (`builtin` or `nvim`), nvim path, nvim config mode (`user` or `clean`)
  - preview visibility
  - hjkl on/off
- Parsed and validated in Rust and exposed to Swift as structs. Reloaded on change.

### 6.8 Swift UI

- **ResultsView**: view-based NSTableView with reused, lightweight row views drawn with Core Text.
  - Data source pulls row windows from the Engine.
  - Section headers for Here and Everywhere.
  - Group headers in Activity.
- **PathBar**: crumbs plus an NSTextField. Owns GO-mode key handling and forwards to `ModeController`.
- **ModeController**: state machine for GO, SELECT and EDIT. Maps keys through the keymap from config.
- **PreviewPane**: QLPreviewView with metadata; thumbnails via QLThumbnailGenerator, async and cached.
- **ShelfView** and **DestinationPicker**: overlay with its own query against `destinations()`.
- **BuiltInEditor**: NSTextView.
  - The layout manager hides `/id ` prefixes with zero-width glyphs.
  - Cursor and selection are clamped so they never land inside a prefix.
  - Copy and paste keep prefixes because they are real text.
  - Quick mode selects the name stem (before the extension).
- **Settings and onboarding**: SwiftUI.

### 6.9 Neovim EDIT (optional)

- **Discovery** (Rust): configured path, then the login shell `PATH` (`$SHELL -l -c 'command -v nvim'`), then `/opt/homebrew/bin` and `/usr/local/bin`. Requires 0.10 or later. If not found, the setting is disabled with an explanation.
- **Launch spec** (Rust) → spawn (Swift):
  ```
  nvim --listen $TMPDIR/tilde-<pid>-<window>.sock \
       --cmd "set rtp^=<bundle>/Resources/tilde.nvim" \
       [--clean -u ~/.config/tilde/init.lua]
  ```
  `TerminalHost` spawns the process in its PTY. Rust connects to the socket with nvim-rs, retrying until it is ready (2 s timeout).
- **Lifecycle**:
  - Lazily started on the first EDIT, one process per window, then kept running in the background.
  - Between sessions nvim sits on an empty scratch buffer with the terminal view detached.
  - On a crash, Tilde restarts nvim on the next EDIT. If the crash happened mid-session, the last pending ops preview is kept and the user can reopen the session.
- **TerminalHost protocol** (Swift): `start(spec)`, `resize`, `focus`, `send(keys)`, `onExit`, `view`. Implemented as `GhosttyHost` (libghostty) and `SwiftTermHost`. M0 picks the default.
  - While editing, the terminal replaces the results list in the main pane; the side pane shows pending ops.
  - All keys go to the terminal except `⌘S` (Tilde asks Rust to send `:w`) and `⌘.` (cancel).
- **tilde.nvim plugin** (Lua, bundled):
  - `require('tilde').open({ dir, lines, focus_line, quick })` is called over RPC.
    - Creates the buffer `tilde://<dir>` with `buftype=acwrite`, `filetype=tilde`, `bufhidden=wipe`.
    - Hides the `/id ` prefix with `conceallevel=3` and a syntax/extmark conceal rule.
    - Keeps the cursor out of the ID column (as oil does).
    - Quick mode starts in insert mode with the name stem selected.
  - `BufWriteCmd` → `rpcrequest(chan, 'tilde_apply', lines)`. On success, wipe the buffer; on failure, show the error with `vim.notify` and keep the buffer.
  - `TextChanged` / `TextChangedI`, debounced 100 ms → `rpcnotify(chan, 'tilde_preview', lines)`.
  - `BufWipeout` without a successful apply → `rpcnotify(chan, 'tilde_cancel')`.
  - Quick mode maps `<CR>` in insert mode to write.
  - Users can disable format-on-save and similar autocommands through the `tilde` filetype. Document known conflicts (for example oil taking over directory buffers).

### 6.10 Previews and opening

- `QLPreviewView` in the side pane, and a full Quick Look panel on a key.
- Open via NSWorkspace. Open With lists apps from Launch Services.
- Thumbnails only for visible rows, never for dataless files.

---

## 7. Permissions and onboarding

- **Full Disk Access**: requested during onboarding with a guided screen. Without it Tilde still works and shows which folders are unreadable.
- **Per-folder prompts** (Desktop, Documents, Downloads, removable and network volumes): explained before macOS shows them.
- **No Accessibility or Automation permissions in v1.**
- **Optional setting** to make other apps' "Reveal in Finder" open Tilde (`NSFileViewer`), off by default.

---

## 8. Performance targets

| Metric | Target | Measured by |
|---|---|---|
| Cold launch to first rows | < 200 ms | signposts, launch test |
| 10k-file folder: first rows / full list | < 16 ms / < 100 ms | criterion + signposts |
| Keystroke to updated results (2M-entry index) | < 16 ms at p95 | criterion, UI signposts |
| All FFI crossings per keystroke | < 0.5 ms | signposts around adapter calls |
| Scrolling | 120 fps, no dropped frames | Instruments |
| Enter EDIT: built-in / warm nvim | < 30 ms / < 80 ms | signposts |
| Idle memory with 2M-entry index | < 200 MB | Instruments, CI memory check |
| Index first build (500k files, SSD) | < 60 s in background | benchmark |

Synthetic trees of 10k, 100k and 1M files are generated by `scripts/gen-tree.rs` for repeatable benchmarks.

---

## 9. Milestones

Durations assume one experienced developer.

| # | Milestone | Contents | Exit criteria | Size |
|---|---|---|---|---|
| M0 | Spikes | ① Build pipeline: cargo → universal static lib → xcframework → Xcode, plus CI. ② UniFFI round trip with streaming callbacks and generation cancellation. ③ nucleo over 2M synthetic paths (full and incremental). ④ Rust `getattrlistbulk` lister benchmark. ⑤ NSTableView with 100k rows pulled from Rust in windows. ⑥ nvim in libghostty vs SwiftTerm: embedding, keys, resize, focus, socket RPC. ⑦ FSEvents `sinceWhen` replay. | Written decisions on terminal library and boundary design. Measured numbers against section 8. | 2 wk |
| M1 | Navigate | Engine skeleton, `tilde-fs`, window, path bar, streaming listing, GO matching in Here, typed path (`tab`, `/`, `⌫`, `~`), marks, preview, open, live updates | Usable daily for browsing. Folder targets met. | 3 wk |
| M2 | Search everywhere | `tilde-index` build, persistence, catch-up; `tilde-query` filters and ranking; Everywhere section; frecency | Instant global search; index survives restarts and catches up correctly. | 3 wk |
| M3 | SELECT and operations | Mode controller, shelf, verbs, destination picker, `tilde-ops` planner, executor, progress, conflicts, trash, undo and redo | Every action undoable. Integration suite green on APFS, case-sensitive APFS and odd filenames. | 3–4 wk |
| M4 | Built-in EDIT | `tilde-edit` format and diff, NSTextView with concealed IDs, quick rename and new item, live pending ops, apply through the engine | Bulk rename, move, trash, create, copy via duplicate lines. Cut and paste keeps identity. | 2 wk |
| M5 | Activity | `tilde-activity` log, origins, Activity view, filters, inbox | `@` → inbox → filing flow works. | 1–2 wk |
| M6 | Neovim EDIT | `tilde-nvim` discovery, launch spec, RPC; TerminalHost; tilde.nvim; settings; live preview; crash recovery | Everything M4 does also works in nvim with a user config. Headless nvim tests green. | 3 wk |
| M7 | Beta | Settings UI, TOML keymaps, onboarding, Sparkle, notarization pipeline, performance pass, docs | Signed, notarized beta. | 2–3 wk |

Total: about 5 months to beta. M4 precedes M6 so the shared edit logic is proven with the simpler editor first.

---

## 10. Testing

- **Rust unit tests** (`cargo test`):
  - query parser and ranking
  - edit diff: one table-driven case per row of section 6.5, plus `dd`/`p`, `yyp`, reorder, and mixed batches
  - planner: cycles, case-only renames, conflicts, invalid names
- **Property tests** (proptest):
  - apply then undo restores the original tree
  - planner output never overwrites without a policy
  - edit text round-trips (render → parse → no-op)
- **Integration tests**: temporary folders and disk images created with `hdiutil`, including case-sensitive APFS. Cover:
  - Unicode normalization (NFC vs NFD names)
  - names with newlines, leading dashes, and 255-byte names
  - packages, symlinks, aliases, read-only files
  - files changed mid-batch
- **nvim tests**: headless `nvim --embed` driven by nvim-rs in Rust tests. Open a session, simulate edits (including yank and paste), write, and assert the resulting batch.
- **Swift tests** (XCTest): `EngineAdapter` behavior, mode transitions, the main prototype flows as UI tests.
- **Benchmarks**: criterion in `core/benches`; CI fails on regressions above 10% on tracked metrics.

---

## 11. Risks and mitigations

| Risk | Mitigation |
|---|---|
| libghostty embedding API changes or does not fit | `TerminalHost` protocol with SwiftTerm fallback; decided in M0 |
| FFI boundary friction (async, Sendable, build complexity) | Coarse commands, row windows, generation cancellation; pipeline built first in M0; hand-written C path reserved for hot spots |
| Edit applies wrongly and loses data | Concealed IDs, planner stale checks, live preview of every change, full undo, table-driven and property tests |
| User nvim config breaks the buffer | `tilde` filetype hook, clean mode, errors surfaced in nvim, pending ops kept on crash |
| Index memory or build time on huge disks | Exclusions, configurable roots, compact storage, background build with progress |
| Ranking feels wrong | Frecency and filename boosts, score debug overlay, tuning with real usage |
| iCloud and File Provider downloads triggered | Dataless checks before any read; tests with cloud folders |
| Two-language debugging cost | `tracing` → os_log with shared subsystem; signposts on both sides; most logic testable in Rust alone |

---

## 12. Open questions

These do not block M0–M2.

1. After entering a folder, stay in GO (as in the prototype) or switch to SELECT?
2. Show Everywhere results always, or only when Here has no matches?
3. hjkl on by default, or arrows only with vim keys as an option?
4. Should the shelf survive a restart?
5. Confirm trash and move in EDIT, or rely on undo?
6. Preview pane always visible, or toggled?
7. Business model and license (open source, paid, freemium).

---

## 13. Build progress

### M0 (done)

Done:
- Cargo workspace with `tilde-types`, `tilde-fs`, `tilde-query`, `tilde-ffi`.
- `getattrlistbulk` lister: files, folders, packages, symlinks, hidden flag, child counts; cancellable.
- Natural sort with precomputed keys (`file2` before `file10`; folders first).
- Query parser (`ext:`, `kind:`, `mod:`, `size:`, `in:`) and nucleo ranking for the Here scope, with UTF-16 highlight ranges.
- UniFFI `Engine` with a coalescing background worker, generation numbers, row windows and `EngineListener` callbacks.
- Build pipeline: `build-core.sh` → `App/Vendor` → SwiftPM app.
- AppKit window: path bar with breadcrumbs, GO mode (fuzzy filter, `tab` / `/` to enter, `⌫` up, `~` home), SELECT mode (`j k h l g G`, `/`, `esc`), Quick Look preview pane, status bar with mode pill, hidden-files toggle, debug snapshot hook.

Measured (M-series, APFS, release build, best of 20):

| Folder | getattrlistbulk | natural sort | read_dir + metadata |
|---|---|---|---|
| 10k files | 6.7 ms | 3.5 ms | 22.7 ms (unsorted) |
| 100k files | 128 ms | 46 ms | 304 ms (unsorted) |

### M2, first cut (done)

- `tilde-index`: parallel scan of the configured roots on launch (rayon + `getattrlistbulk`), skipping excluded folders' contents, hidden files and mount points; items held in nucleo with keys relative to home.
- Everywhere search: nucleo match, then the top 3,000 re-ranked with a filename score, depth and path-length penalties, a recency bonus and frecency boosts (zoxide's at first; Tilde's own since the frecency store below); filters (`ext:`, `kind:`, `mod:`, `size:`, `in:`, `in:.`) applied per item; top 200 returned. Items directly in the current folder are left to Here.
- zoxide: scores read at start and every 60 s, folders it knows that the scan misses are added to the index, folders opened in Tilde are recorded with `zoxide add` (`[zoxide]` config).
- Engine publishes Here + Everywhere rows, re-publishes while the scan runs, and exposes `index_status`. The app shows an Everywhere header, dimmed locations with highlights, reveal on `tab`, scan progress in the status bar, and defers `tab`/`⏎`/`/` until results match the typed text.

Measured on the developer's home folder (release, M-series): 55,560 items scanned in 0.2 s, queries 3–8 ms, 38 MB peak.

### M2, live updates (done)

- `tilde-fs::watch`: direct FSEvents bindings (CoreServices + a dispatch queue), folder-level events with IDs and rescan flags, exclusion paths.
- Index updates in place: every item has an ID; a per-folder table of name and metadata hashes lets a fresh listing of a changed folder be diffed. New items are pushed, changed items re-pushed, removed items (and whole removed subtrees) marked dead and skipped by search. Rescan flags re-walk the subtree. Events that arrive during the initial scan are applied after it.
- The open folder has its own watcher: it re-lists on change without a new generation (the UI keeps the selection) and moves to the nearest existing parent if the folder is deleted. Index changes refresh Everywhere results while a search is active.

### M2, saved index and index controls (done)

- Saved to `~/Library/Caches/Tilde/index.bin` (header with format version, settings fingerprint and the FSEvents ID it's current to; one record per item; written atomically). Saved after the first scan, every 5 minutes when changed, and on quit. On launch a matching saved index is loaded and FSEvents replays changes since its ID; a mismatched or damaged file means a fresh scan.
- One `IndexService` shared by every window (previously each window built its own).
- `[index]` gains `enabled`, `hide` (names, `*`/`?` patterns, paths; left out entirely) and `max_items` (indexing stops there and reports it). Config edits from the app go through `toml_edit`, keeping comments.
- Search Index window (⌘⇧I): status, items against the limit, approximate memory, size on disk, saved path, roots, largest folders (skipping folders mostly made of one subfolder) with Exclude contents / Hide, Rebuild, Clear, Edit Config. `X` in SELECT hides the highlighted item.

Measured on the developer's home folder: 55,581 items; saved file 5.3 MB written in 73 ms; loading it takes 37 ms instead of a 0.2–0.45 s scan.

Still to do for M2: compaction of dead items, folder counts and dates in Everywhere rows kept current.

### M3: acting on files (done)

- `tilde-ops`: requests (move, copy, trash, rename, new folder, compress) are planned first: sources must exist and not repeat, destinations must be folders, folders can't go inside themselves, names are validated, clashes get Finder-style "keep both" names, and moving into the current folder is a no-op. Steps run on one background thread with progress and cancellation; every step checks its destination is free (rename(2) would silently replace). Copies use `copyfile` with `COPYFILE_CLONE` (instant on APFS); cross-volume moves copy then remove; compress uses `ditto` (several items via a cloned staging folder); trash uses `NSFileManager` and records where items landed.
- Undo history: each completed step records how to reverse it (moves back, created items to the Trash, trashed items back from the Trash). Undo runs the reversals newest first; a partial undo keeps the rest undoable. Redo re-plans the original request. History is per session, shared by all windows.
- Tests run against temporary folders with a stand-in Trash, including randomized sequences of operations that must undo back to the exact starting tree; one opt-in test uses the real Trash.
- App: shelf (`x`, `space`, `a`, shown as dots and a shelf bar), actions (`m` `c` `d` `r` `n` `z` `y`), a folder picker (fuzzy over indexed folders, zoxide favourites when empty, typed-path completion), rename and new-folder prompts, progress and results in the status bar, `u` / `U` / `⌃R`, and Edit › Undo/Redo naming the operation. Folder views refresh through FSEvents, so results appear without a reload.

Finished later (see "M3, finished" below): tags, conflict prompts, persistent undo history, progress for single large copies.

### Navigation, preview and themes (done)

- `⌃j` `⌃k` move and `⌃h` `⌃l` go out / in, in GO, SELECT and the folder picker (a local key monitor, ahead of the search field's text bindings).
- Preview pane: `tilde-preview` runs bat (true colour) and parses its SGR output into styled spans (UTF-16 offsets); used for code, text and unknown types that look like text, up to 5 MB and `max_lines`. Markdown and everything else use Quick Look. `[preview]` sets visibility, width (default half the window), bat on/off, light and dark bat themes (defaulting to the preset's match) and line count.
- `[theme]`: appearance (system / light / dark), presets (default, catppuccin, gruvbox, nord, solarized; each with light and dark palettes), 24 colour keys overridable for both appearances or either. Resolved in Rust with validation; Swift builds appearance-aware colours with system colours as fallbacks.
- Fixed along the way: Everywhere search returned nothing when a matching pass outlasted the 250 ms wait (very large indexes, debug builds); nucleo's completion callback now refreshes results, and dependencies are optimized in debug builds.

### M4: built-in text mode (done)

- `tilde-edit`: sessions render a folder as `/<id> name` lines and diff edited text into renames, moves, copies (repeated IDs), trash (missing IDs), new files and folders, with per-line errors (unknown IDs, clashing names, invalid names).
- `tilde-ops` `Request::Batch`: planned so every step can succeed in order (trash, new folders shallowest first, moves via temporary names for swaps, copies from where their source ended up, new files), with clashes, missing parents and moves into a renamed folder caught first. One undo reverses the whole edit.
- App: `FolderEditor` (TextKit 1) hides the ID prefixes by giving them no glyphs, keeps the cursor out of them and refuses edits that cut into them; a pending-changes pane with errors replaces the preview; `e` / `r` / `o`, ⌘S (File › Apply Edits), double-esc discard; a failed apply keeps the editor open.


### M5: Activity (done)

- Built differently from §6.6: there's no event log. `tilde-activity` works the feed out when it's opened, from the index (visible files modified within `[activity] days`, skipping package internals and library packages, at most 500) and listings of the inbox folders (everything in them, timed by the later of modification and date added, `ATTR_CMN_ADDEDTIME`). History is there from the first launch, nothing can drift out of sync with the disk, and moves made outside Tilde count as filing.
- Origins from extended attributes: screen capture keys → screenshot; quarantine agent `sharingd` → AirDrop; `kMDItemWhereFroms` (or a quarantine agent) → download, with the site taken from the referring page when recorded; otherwise created (birth time near modification) or edited. The screenshot folder comes from `com.apple.screencapture` preferences.
- Inbox: `[activity] inbox` (default Downloads and Desktop) plus the screenshot folder. Downloads, screenshots and AirDrops found elsewhere are marked filed.
- Engine: `open_activity(filter)` makes Activity the location (the previous folder is kept for going back); Here rows are Activity items, searchable, with date headings (Today, Yesterday, This week, This month, Earlier) when not searching; Everywhere searches the whole index. The inbox folders are watched directly; other index changes refresh the feed at most every 2 s.
- App: `@` in GO (empty bar) and SELECT, back with `@`, `⌫`, `h`; `1`–`5` origin filters, `i` inbox; origin column and source / filed notes on rows; `tab` reveals in the folder; file actions work as anywhere else.

### M6: Neovim EDIT (done)

- Terminal: SwiftTerm (`LocalProcessTerminalView`) behind the `TerminalHost` protocol. libghostty has no packaged embedding API yet (building it means compiling Ghostty with Zig), so it waits; the protocol keeps the swap small. This settles spike ⑥ for now.
- `tilde-nvim`: discovery (configured path, login shell `PATH`, Homebrew; 0.10+), launch spec (`--listen $TMPDIR/tilde-<pid>-<n>.sock`, `--clean -u ~/.config/tilde/init.lua` in clean mode, login `PATH`, `TERM`), and a small blocking msgpack-rpc client (`rmpv`, a reader thread) instead of nvim-rs on tokio.
- The plugin (`lua/tilde.lua`) is embedded and loaded over RPC as `package.loaded.tilde`, not via `--cmd "set rtp^=…"`: plugin managers such as lazy.nvim reset `runtimepath`.
- Buffer: `tilde://<dir>`, `acwrite`, concealed `/<id> ` prefixes via extmarks (redone on each change), cursor clamped, folders highlighted, diagnostics for bad lines. `:w` → `tilde_apply` (refused with a message if the text has errors); `:q`, `:q!`, `ZQ` → `tilde_cancel` through buffer-local abbreviations, so nvim keeps running; `:wq`, `ZZ` write. `r` selects the stem in select mode and `o` opens a line, both with `⏎` to apply.
- App: `[edit] editor = "nvim"`; one nvim per window, started on the first edit, hidden between edits; the pending-changes pane follows `tilde_preview`; ⌘S writes, ⌘. (File › Cancel Edit) cancels; a failed apply is reported in nvim and keeps the buffer; nvim exiting mid-edit discards it and the next edit starts a new one; if nvim can't be found, EDIT says why and uses the built-in editor.
- Tests: headless nvim driven over RPC (normal-mode edits, `yyp`, typed `:q!`, rename and new-line modes, cursor clamping, errors, nvim exiting).

### M0, remaining spikes (done)

- **CI**: `.github/workflows/ci.yml` on macOS 15: `cargo fmt --check`, `clippy -D warnings`, `cargo test` (Neovim installed, so the headless tests run), `build-core.sh`, `swift build`.
- **FSEvents `sinceWhen` replay**: built with the saved index (M2); covered by `saves_and_resumes_with_changes_since`.
- **Index at 2M paths**: `cargo run --release -p tilde-index --example bench_synthetic -- [items]` writes a synthetic saved index (2–7-level folders of 10–60 files) and loads it in a fresh process. M4, 10 cores:

| Items | Load | Memory after load | Text query (fresh) | Typing, per key | Filter only (`ext:pdf`) |
|---|---|---|---|---|---|
| 100k | 35 ms | 31 MB | 6–10 ms | 5–11 ms | ~1 ms |
| 500k | 180 ms | 133 MB | 27–40 ms | 16–43 ms | ~6 ms |
| 2M | 0.98 s | 496 MB | 110–140 ms | 85–240 ms | 23 ms |

  Search time and memory grow linearly (~60 ns and ~250 bytes per path). The cost is nucleo's matching itself (250–500 ns of CPU per path for real queries, measured single-threaded with `nucleo-matcher`), spread across cores; our re-ranking is capped at 3,000 candidates and takes 2–6 ms. One-letter queries at 2M hit the 250 ms wait and return partial results. The developer's real index is 55k paths, well inside the targets.

  Fixed along the way: filter-only queries walked every item on one thread and allocated a lowercase extension per item (200 ms at 2M); they now walk in parallel without allocating (23 ms).

  **Decision needed** before the §8 targets for 2M (16 ms per key, 200 MB) can be met: replace nucleo's full pass with our own index — paths in one arena (~60 bytes each instead of ~250), a per-path character bitmask that rejects most paths in a few nanoseconds before any fuzzy scoring, parallel scoring with a top-k heap, and narrowing from the previous result set as the query grows. Until then the practical ceiling for "instant" is roughly 300–500k paths.

- **Scrolling at 100k rows**: `TILDE_SCROLL_BENCH=<steps>` (optionally `TILDE_SCROLL_STEP=<pt>`) scrolls the list in steps, forcing layout and drawing each time. Row cells built from seven text fields and ~20 constraints cost ~0.5 ms per newly visible row (Auto Layout measuring text, CoreText shaping JetBrains Mono's ligatures), so a fast flick (600 pt, ~23 new rows) took 12 ms a frame. Rows are now drawn directly in one view with no subviews or constraints:

| Step | Before p50 / p95 | After p50 / p95 |
|---|---|---|
| 26 pt (one row) | 0.82 / 1.11 ms | 0.33 / 0.67 ms |
| 100 pt | 2.28 / 2.75 ms | 0.92 / 1.42 ms |
| 600 pt | 11.8 / 13.2 ms | 4.85 / 5.75 ms |

  Release build with the debug hooks (`swift build -c release -Xswiftc -DDEBUG`), 100,200-row folder; the 8.3 ms budget for 120 Hz now holds at p95 even for large steps. Fetching rows from Rust was ~2% of the time. Not measured: compositor frame pacing, which needs Instruments.

### M3, finished

- **Clashes**: Move and Copy carry a `Clash` policy (keep both, replace, skip). The app asks first (`FileOps.clashes` lists names already taken in the destination): `k` keep both, `r` replace, `s` skip. Replace trashes the existing item as its own step before the move or copy, so undo moves the new item back and restores the old one from the Trash. A folder can't be replaced by something inside it. Duplicates in their own folder and repeats within one request always keep both. Plan outcomes count renamed, replaced and skipped items.
- **Tags** (`t`): `Request::Tag { add, remove }` plans one `SetTags` step per item from its current tags (case-insensitive; unchanged items are skipped) through `NSURLTagNamesKey`, recording the previous tags for undo. Several items show the tags they share; edits apply as adds and removes, so their other tags stay. Tags show under the preview.
- **Persistent undo**: the journal (requests, completed steps and timestamps, plus redo) is JSON in `~/Library/Application Support/Tilde/undo.json`, rewritten atomically after every operation; the last 100 entries from the past 30 days are kept, and a damaged file is an empty history. Undoing old entries goes through the same checked steps, so moved-away items are reported, not guessed at. Tested across restarts.
- **Large copies**: copies that move data (to another volume) use a `copyfile` status callback: bytes copied of the total (summed up front), reported at most every 100 ms and shown in the status bar ("1.2 GB of 4 GB"), and `⌘.` (File › Stop Operation) stops mid-file and removes the partial copy. Same-volume copies clone and skip the size walk. The randomized undo test now mixes in every clash policy and tags.

### M7, first part: keys, settings, onboarding (done)

- **Keymap**: `[keys.go]` and `[keys.select]` in config.toml bind actions (`move`, `down`, `select_mode`, …; the list is in `tilde-config/src/keys.rs` with descriptions and defaults) to one key or a list; `[]` unbinds. Keys are characters or names with `ctrl+`/`alt+`/`shift+` (names only); `cmd+` is refused (menus own it); plain characters in GO are refused (they're typed into the bar). A user key takes the key away from actions that only had it by default; collisions between user keys and bad specs are warnings, not errors. The app matches key presses by key code for named keys and by character otherwise, so letters follow the keyboard layout. GO keys are caught before the search field; SELECT keys go through the list. Status bar hints, the path bar placeholder and the welcome text read from the keymap.
- **Settings** (`⌘,`, SwiftUI): General (appearance, theme preset, monospaced font family and size, Activity days, Full Disk Access status), Preview (visibility, width, bat), Editing (built-in or Neovim, path with live discovery status, clean config), Search (index on/off, hidden files, item limit, zoxide), Keys (every action with its keys; Set records the next key press, + adds one, Reset removes the override). Each change is written to config.toml with `toml_edit`, keeping comments and the comment beside the old value, then applied to open windows. Config problems show at the top. `⌥⌘,` opens the file.
- **Welcome** (first launch, Help › Welcome to Tilde): Full Disk Access status polled every second with a button to the right Privacy pane; "Ask now" for Desktop, Documents and Downloads (read off the main thread so macOS asks while the reason is on screen); editor choice with Neovim discovery; the three modes with the current keys. Run unbundled, it names the terminal app (via `__CFBundleIdentifier`, so tmux reports the terminal hosting it) since macOS attributes permissions to it.
- Remaining for M7: a performance pass on launch and EDIT entry. Notarization and Sparkle wait for an Apple Developer ID; until then releases are signed with a stable self-made certificate (see Releases below).

### Releases (done, without a Developer ID)

- `scripts/bundle.sh [--universal]` builds and signs `Tilde.app` (install.sh uses it); `scripts/package.sh` makes `Tilde-macOS.zip`, `Tilde.dmg` and `SHA256SUMS.txt` under version-free names, so `releases/latest/download/…` always works.
- `.github/workflows/release.yml`: a `v*` tag checks it against `core/Cargo.toml`, builds a universal app signed with the release certificate (`TILDE_SIGNING_P12` secret, made by `scripts/make-release-cert.sh`; ad hoc with a warning without it) and publishes a release with that version's CHANGELOG section. `-` versions are prereleases.
- `scripts/get.sh` (the README's `curl … | bash`) installs or updates from the latest release after checking its checksum; curl downloads aren't quarantined, so Gatekeeper doesn't block the unnotarized app. The same certificate on every release keeps TCC permissions across updates.

### Sorting (done)

- `SortKey` (name, modified, added, size, kind) with a natural direction each (A→Z, newest first, largest first) and `reverse`; ties go by name; folders stay first (`folders_first`), and a size sort keeps folders apart, ordered by item count. Kind sorts by extension.
- The engine sorts each listing once; text queries rank by match, filter-only queries keep the order (the matcher preserves input order when there's no text). Sorting by added makes rows show the date added.
- `s` in SELECT (a choice prompt; the current key reverses) and View › Sort By (`⌃⌘1`–`⌃⌘5`, Reverse Order). `[sort]` sets the default; with `remember`, orders chosen per folder live in `~/Library/Application Support/Tilde/sort.json` (re-read on each open so windows share it; choosing the default removes the entry). The status bar names the order when it isn't name A–Z. Activity keeps time order.


### Window chrome (done)

- Browser windows have no system title bar: full-size content, a transparent hidden title bar and an empty compact toolbar (which sizes the bar and keeps the window buttons centred). The path bar is the title bar: it takes the title bar's height, starts after the window buttons (from the edge in full screen) and drags the window from its empty parts. The window title is still set (Window menu, Mission Control) but not shown. Settings, Welcome and the Index window keep standard title bars.

### Jump list (done)

- Per window, browser-style: every navigation (open, up, home, reveal, Activity) pushes the place left and the item highlighted there; `⌃o` / `⌃i` (actions `back` / `forward` in GO and SELECT, Go › Back `⌘[` / Forward `⌘]`) move through it without recording, restoring the highlight. A new move clears forward history; folders that no longer exist are skipped; a failed open drops its entry. The place is tracked when navigation starts, so repeated `⌃o` before results arrive still steps correctly. Capped at 100.

### Drag and drop, Quick Look, Open With, shelf (done)

- Drag out: rows write file URLs; dragging a shelved row drags the whole shelf as a pile (`ResultsTableView.beginDraggingSession`). Outside the app the only operation offered is copy, so other apps never move files behind Tilde's undo history; inside, move and copy.
- Drop in: file URLs onto a folder row (that folder) or anywhere else (the open folder; not Activity). Move on the same volume when the source allows it, else copy (⌥ narrows the source to copy). Goes through the clash prompt and the operations queue, so it's undoable.
- Quick Look: `quick_look` (`p`) and File › Quick Look (`⌘Y`) toggle `QLPreviewPanel` for the shelf or highlighted row; it follows selection changes, and `j`/`k`/`p` work while the panel is key (`previewPanel(_:handle:)`).
- Open With: `open_with` (`w`) shows the apps from `NSWorkspace.urlsForApplications(toOpen:)`, default first, fuzzy by name. A right-click menu on rows offers Open, Open With, Quick Look, Show in Finder, Copy Path, Tags, Rename, Move to Trash.
- The shelf is saved in user defaults on every change and restored into the first window, dropping paths that no longer exist (open question 4: yes).

### History, safer typing, jump list views, staged previews, file-dialog helper (done)

- **What went wrong**: `⌘⌫` while typing in the bar reached File › Move to Trash and trashed the highlighted search result (the saved history showed `~/.local/share/rustup`, `~/Applications`, `~/Developer/uni` and others trashed that way). Now `⌘⌫` / `⌃⌫` in the bar clear the typed text, and Move to Trash is enabled only while the list has focus in SELECT with no prompt open (so prompts and the editor get normal text editing too).
- **History**: the journal is a list of records (request, steps in place, steps last applied, times, state applied / undone / partly undone). Records are never dropped by undo or new operations, only by count (200) and age (30 days), so undone branches stay redoable. Any applied record can be undone on its own (`undo_record`) and any undone one redone (`redo_record`, re-planned). Every step checks its source still exists ("no longer exists where Tilde left it"), so undoing something later operations depend on is refused step by step. ⌘Z / ⌘⇧Z keep stack semantics (redo is cleared by a new operation). The first on-disk format is upgraded on load. History window: `H` / `⌥⌘Z`, `j`/`k`, `u` undo this, `r` redo this, details pane listing each change.
- **Jump list** entries carry the typed query and the highlighted path; going back restores the query (cursor at the end) and the highlight, by name in the folder or by path for Everywhere results.
- **Preview pane**: QLThumbnailGenerator (icon → low-quality → full-size thumbnail) shows at once with a spinner; the interactive QLPreviewView starts after the selection rests 250 ms and the full-size thumbnail is in (or 1.5 s), and fades in over it. A new selection cancels the thumbnail request and pending work and clears the previous preview.
- **File-dialog helper** (`[dialog]`): a Carbon global hotkey (needs ctrl, alt or cmd; key code from the current layout) opens a non-activating panel with the folder picker's UI searching files and folders (`IndexService.find`), the front window's folder first. The dialog is found through Accessibility (focused window or sheet with identifier `open-panel` / `save-panel`) and driven like a person would: bring its app to the front (and stop if that fails), ⌘⇧G, wait for `GoToWindow` / `PathTextField`, set the path through AX (typing races the field's completion), Return, wait for the sheet to close, and optionally press `OKButton`. Save dialogs go to a picked file's folder and keep the typed name. Verified against real NSOpenPanel / NSSavePanel instances; apps with custom pickers aren't covered. Settings › General: on/off (asks for Accessibility), hotkey recorder, confirm, permission status.

### Undo tree (done)

- Records have a `parent` (the head when they ran) and the journal a `head`, so history is a tree: undo then a new operation branches instead of discarding. Older files are linear and get parents on load (format version 2). ⌘Z undoes the head (or the nearest applied record above it) and moves the head up; ⌘⇧Z redoes the most recently undone child of the head.
- `tree.rs` plans without touching the disk: `goto_plan(target)` undoes every applied record off the target's line (newest first) and redoes every undone one on it (oldest first), so the files match the state right after the target; `undo_plan(r)` adds the applied records after `r` that touch overlapping paths (same path, or one inside the other; transitively), newest first; `redo_plan(r)` adds the undone ancestors it overlaps, oldest first. A partly undone record on the target's line blocks a go-to with a message.
- Runs are one queued job of undo/redo moves that stops at the first failure; each move updates its record and the head as it goes. Previews (`preview_undo/redo/goto`) feed the UI. Tests: branching and restoring, dependency cascades, prerequisites, and a randomized walk (operations, undos, redos, go-tos across branches) that must return to the exact original tree.
- History is now a page in the browser window (replacing the separate window): a lane graph (nodes by state, the head ringed, branches joining where they started, a Start node), the details and the effect of `u` / `r` / `⏎` beside it, confirmation for anything involving more than one operation, theme fonts and colours throughout.

### Preview on request (done)

- The pane shows an instant preview only: a `QLThumbnailGenerator` thumbnail sized to the pane (re-rendered when the pane resizes), the icon at icon size with name, kind, size and date when there's no thumbnail, bat for code and text, and Quick Look's own view for folders and packages. A dim "p full preview" hint sits in the corner.
- The full `QLPreviewView` preview is shown only on `p`. After 300 ms of rest it's loaded invisibly underneath (with `autostarts` off) for local files up to 50 MB that aren't video or audio, cloud-only (`ubiquitousItemDownloadingStatus`), dataless (`SF_DATALESS`) or on non-local volumes; moving on drops it. `p` reveals it at once if ready, else shows a spinner and reveals it once Quick Look's content view has attached. A second `p` opens the Quick Look window.
- PREVIEW (first `p`): the preview takes the keyboard and 75% of the window (or the configured width if larger). SELECT's move / top / bottom keys scroll it, space and ⇧space page; for Quick Look's out-of-process view these become arrow, page, home and end key events sent to the window (Quick Look scrolls PDFs with them), for bat's text the clip view scrolls directly. `esc`, `h`, back or a click on the list ends it; so do a selection change, navigation and History. The focus guard lets the preview keep the keyboard only in PREVIEW or after a click.
- `p` toggles PREVIEW; `P` (`quick_look_window`) and ⌘Y open the Quick Look window. PDFs' full preview is a `PDFView` (in-process) rather than `QLPreviewView`: Quick Look's out-of-process view ignores synthetic scroll-wheel events and maps arrow keys to page jumps, so only an in-process view can scroll in small steps. PDF and text scrolling glide 56 pt per step; a new PDF is pinned to the top for 1.5 s (or until the first scroll) because PDFKit rescales after the pane widens.
- PREVIEW's width is `[preview] mode_width` (0.3–0.95, Settings › Preview); `preview_full` (`f`) toggles the whole window, hiding the list, and is remembered for the window's next PREVIEW (from SELECT it enters PREVIEW that way). Width changes keep a PDF's position as the fraction of the document at the top of the view, restored after PDFKit rescales (it fits the width, so the fraction maps to the same spot).

### Clean Up (done)

- `tilde-cleanup` (new crate) works out suggestions; `CleanupService` (FFI) runs them on a background thread at utility QoS and reports each category as it changes (`CleanupListener`), with states waiting / working / ready / skipped. Order: one pass over the index for big files, old downloads and the Trash (sizes from the index, then `lstat`); a second for each project's newest change, then measuring each build folder in idle projects; app caches; duplicates last.
- Sizes are what trashing would free: `ATTR_CMNEXT_PRIVATESIZE` (getattrlist with `FSOPT_ATTR_CMN_EXTENDED`) for files, so APFS clones count as freeing nothing; folders are measured by allocated size, hard links once, without crossing volumes, cloud placeholders (`SF_DATALESS`) as nothing. "Not opened" means neither `com.apple.lastuseddate#PS` (set when apps open a file through Launch Services) nor the modification date is within the age.
- Duplicates in stages: group the index's files ≥ `duplicates_min` by size; hash 3 × 16 KB samples (start, middle, end) in parallel (6 threads, `F_NOCACHE`), and publish those groups as "likely"; then hash in full (xxh3-128), biggest groups first, in batches of 8 groups, publishing as they're confirmed. Hashes are cached in `~/Library/Caches/Tilde/hashes.json` by device, inode, size and modification time. On the author's home folder (8,339 files ≥ 1 MB): likely after 1.3 s, all confirmed after 4.7 s cold (6.2 GB read); 0.36 s with the cache. The copy kept is outside Downloads, the Desktop and caches if possible, then the oldest, then the shortest path.
- The page (`C`, `cleanup` in `[keys.select]`; View › Clean Up…) replaces the list, the preview following the highlighted item (`row_for_path`). Acting uses the window's shelf and operations, so everything is undoable, except emptying the Trash (`d` in the Trash category, with a confirmation). Before trashing, items are re-checked off the main thread: unchanged size and modification time (`unchanged`), and for a duplicate a byte-for-byte comparison with the kept copy (`same_contents`); the kept copy, and caches of running apps (folder name = bundle ID), can't be collected or trashed. After each operation, `forget_missing` drops suggestions that are gone. `X` adds to `[cleanup] ignore` (`add_to_list`). Low Power Mode skips duplicates until `r`.
- `[cleanup]` thresholds (`big_file`, `big_file_age`, `download_age`, `project_idle`, `duplicates_min`, `build_folders`, `ignore`) parse sizes as decimal K/M/G/T and ages as h/d/w/mo/y; Settings › Clean Up edits them.

### Own frecency (done)

- Tilde's own scores, combined with zoxide's when it's installed. `tilde-index::Frecency` keeps one score per absolute path (folder or file) with the time it was last brought up to date; reading decays it by a 10-day half-life (`score · 2^(−Δt / 10 d)`), and a use adds to the decayed value: 1 for a file opened (`⏎`, Open With, a file-dialog pick: `record_open`), 0.5 for a folder picked as a move/copy destination (`record_destination`), and for a folder opened in a window 1, or 0.15 if the next folder came within 2 s (pass-through; the visit is held until the next one, or a save, settles it). No aging passes are needed; entries under 0.02 are dropped on save, at most 5,000 kept.
- Stored in `~/Library/Application Support/Tilde/frecency.json` (written atomically at most every 30 s while in use, and on quit), owned by `IndexService` so rebuilds keep it. zoxide's scores (`zoxide query --list --score`, re-read every 60 s) are added as √score, which keeps their order but puts them on the scale of a few weeks' use in Tilde; so terminal use and Tilde use both count, without importing (and double-counting) zoxide's history.
- Search derives scores by key once a minute or after a new use; the boost is unchanged (15·ln(1+s) for the item, 5·ln(1+s) for its folder), now applying to files too. The folder picker's empty list is the top folders (combined) that still exist. Folders either knows that the scan misses are added to the index as before.
- `[frecency] enabled`, `zoxide` (count its scores), `tell_zoxide` (also `zoxide add` on visits). The old `[zoxide] enabled` / `add_on_visit` are still read and win if set. Settings › Search › Ranking.

### Previewing text and Markdown (done)

- Code and text: the pane shows bat's first `max_lines`; `p` keeps the same view (no switch to Quick Look) and re-renders the whole file (up to 200,000 lines) in the list font, keeping the top line; `j`/`k`/`space`/`g`/`G` scroll it.
- Markdown (`tilde-preview::markdown`): one pulldown-cmark parse (GFM: tables, task lists, strikethrough, footnotes; maths) feeds two renderers. `to_blocks` gives styled runs that the app turns into attributed text (headings, lists, quotes with a bar, code in boxes, tables as aligned text, local images, links). `to_html` gives a body for a `WKWebView` page styled from the theme. Code blocks are highlighted with syntect. Maths (`tilde-preview::math`) is taken out before the Markdown parser sees it and swapped for private-use tokens, so a `$$` block with blank lines or `-`/`+`/`>` lines inside stays whole; inline `$` follows Obsidian's rules (no space after the opening `$`, no digit after the closing one, never across a code span); `\( \)`, `\[ \]` on their own lines, `` $`…`$ `` and table cells are handled too. The HTML has the TeX in `math-inline`/`math-display` spans, which KaTeX 0.19.0 (bundled in Resources/katex, served to the page by a `tilde-asset:` scheme handler, run as a user script in its own content world) renders; `tilde-math.js` holds the options (macros across the note, no `\href`/`\includegraphics`, unnumbered `align`/`equation`, `\DeclareMathOperator` as `\def`, `\require` dropped). The page is shown once KaTeX has run and its fonts have loaded. Obsidian callouts become a title span styled by kind with `blockquote:has(…)`. Images next to the file are embedded as data URIs; remote ones become their alt text. The page has a CSP of `default-src 'none'; img-src data:; style-src 'unsafe-inline'` and content JavaScript off; links open in the browser.
- The pane always shows the native rendering (10–30 ms; bat took 30–330 ms on the same files). `p` shows `[preview] markdown`: `web` (default), `native` (the same text at the list size) or `quicklook`. One web view is made on first use and reused; the page is loaded into it once the selection rests 0.3 s, keyed by path, size, date and appearance, so `p` usually shows it at once.
- HTML (`html`, `htm`, `xhtml`): the pane shows bat's source; `p` loads the file into the same web view (`loadFileURL` with its folder readable, so local styles and images work), with content JavaScript off and a content rule list blocking `http(s)`, `ws(s)` and `ftp` loads (compiled once; if it fails the page isn't loaded). Preloaded like Markdown.
- Source view (`v`, `preview_source`): in PREVIEW, bat renders the whole file over the rendered view (web page, Quick Look, PDF, or native Markdown), which is hidden and put back on the second `v` (native Markdown is rendered again, since the source replaced it in the text view). Beeps when bat finds no text, or the file is already shown as source. From SELECT, `v` enters PREVIEW and shows the source.
- `TILDE_PREVIEW_BENCH=<files>` (debug builds) times each way of previewing from selection to drawn: bat, native, web (new and reused view), Quick Look thumbnail and full.
- Maths check: `examples/math_survey.rs` lists every formula the preview would find in a folder; `scripts/math-check.js` renders them with the bundled KaTeX and `tilde-math.js`. On a 665-note Obsidian vault: 23,312 formulas, 23,311 render (the failure is a `%` in `\text{}`, a TeX error). `tests/maths/should-render/` must render fully (CI); `tests/maths/limits.md` lists what KaTeX can't do. Heaviest note (1,459 formulas, 124 KB): 163 ms warm in a reused view (debug build), hidden by the preload.

### Keys panel and the mouse (done)

- Status bar hints are the essentials per mode, ending in `? keys`. `help` (`?` in SELECT, `ctrl+/` in GO; also a click on the mode pill or hints) opens a `PromptOverlay` of kind `.commands`: the keys for where the window is (list, GO, PREVIEW, History, Clean Up, Activity), grouped, from the keymap's descriptions and keys, then the menu bar's enabled items that have shortcuts. Typing filters by entry and key; `⏎` or a click runs an entry by sending its key press (`Keymap.event`) once the panel has closed, so it does exactly what the key does there.
- Mouse-only, no key changes: path bar crumbs are buttons (`Crumb`: click to go, drop to move/copy via `dropFiles`); a `⋯` button on the hovered row (tracking area; hidden on any key, scroll or exit) pops the row menu; the row menu gained shelf, move to, copy to, compress, preview, source; right-click on empty space gives a folder menu; ⌘-click toggles a row on the shelf, ⇧-click shelves a range; a click on the status bar's summary shows sort, hidden files and a jump to Everywhere; a drag held 0.9 s over a folder row opens it.

