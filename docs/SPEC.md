# Tilde: Specification and Build Plan

Status: design complete, pre-build (October 2026)
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
| Global index, frecency | Rust `tilde-index` | memmap2 binary index; SQLite for frecency |
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

- `scripts/build-core.sh`:
  1. `cargo build --release` for `aarch64-apple-darwin` and `x86_64-apple-darwin`.
  2. `lipo` into a universal static library.
  3. `uniffi-bindgen generate` for the Swift bindings and module map.
  4. Package as `TildeCore.xcframework`.
- Xcode consumes the xcframework through a local Swift package (`Packages/TildeCoreFFI`) that also contains `EngineAdapter`.
- A pre-build Xcode phase runs the script when Rust sources changed. Debug builds use the host architecture only for speed.
- CI (GitHub Actions, macOS runners): `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `cargo bench` smoke run, xcodebuild, XCTest, perf regression check, notarized build on tags.

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
  App/                          # Xcode app target (AppKit)
    Modes/  Views/  Edit/  Terminal/  Settings/  Onboarding/
  Packages/
    TildeCoreFFI/               # xcframework + generated bindings + EngineAdapter
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
- **Frecency**: SQLite table of opens and visits per path, with exponential decay. Feeds a ranking boost.

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
