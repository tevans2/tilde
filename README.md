# Tilde

A fast, keyboard-first file navigator for macOS. Rust core, Swift/AppKit UI.

- Design and build plan: [docs/SPEC.md](docs/SPEC.md)
- Interactive prototypes: [docs/prototypes/](docs/prototypes/)

## Requirements

- macOS 14 or later
- Rust (stable) via rustup
- Swift 6 toolchain (Xcode Command Line Tools are enough for development)

## Install

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
swift test --package-path App      # the app's own logic: keys, jump list, history graph
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release -p tilde-fs --example bench_list -- 10000
cargo run --release -p tilde-index --example bench_index        # scans your home folder
```

Debug builds can render their window to a PNG without screen-recording access, optionally
replaying keys first (`\t` tab, `\e` escape, `\r` return, `\b` delete, `^j` control,
`@s` command, `\w` a one-second pause). `TILDE_SNAPSHOT_WINDOW=settings|keys|welcome`
snapshots that window instead:

```sh
TILDE_SNAPSHOT=/tmp/tilde.png TILDE_SNAPSHOT_KEYS='Dev\tprojects\t' App/.build/debug/Tilde
```

## First launch

A welcome window covers Full Disk Access (with its status, updated as you grant it), asks
for Desktop, Documents and Downloads up front if you'd rather not have macOS ask mid-search,
picks the editor for EDIT, and shows the modes. Help › Welcome to Tilde opens it again.
Run from a terminal instead of as an app, macOS asks about the terminal, and the window
says which one.

## Keys

These are the defaults. Every GO and SELECT key can be changed in Settings › Keys (press
the new key) or in the config file (see below).

| Mode | Keys |
|---|---|
| GO (typing searches) | type to search this folder and everywhere · `↑` `↓` or `⌃j` `⌃k` move · `⏎` open · `tab`, `/` or `⌃l` go into folder (or reveal an Everywhere file) · `⌫` on empty bar or `⌃h` goes up · `~` home · `@` Activity · `⌃o` / `⌃i` back / forward · `esc` → SELECT |
| SELECT | `j` `k` (or `⌃j` `⌃k`) move · `h` `l` (or `⌃h` `⌃l`) out / in · `⏎` open · `g` `G` top / bottom · `⌃o` / `⌃i` back / forward · `s` sort · `/` → GO · `esc` clear filter, then shelf |
| SELECT: files | `x` or `space` add to / remove from shelf · `a` all shown · `m` move · `c` copy · `d` trash · `t` tags · `p` Quick Look · `w` open with · `r` rename · `n` new folder · `z` compress · `y` copy paths · `u` undo · `U` or `⌃R` redo · `H` history · `X` hide from search |
| Activity (`@`, again or `⌫` to go back) | `1` all · `2` downloads · `3` screenshots · `4` edited · `5` AirDrop · `i` inbox only · `tab` reveal in its folder · file actions as in SELECT |
| EDIT (the folder as text) | `e` edit the folder · `r` rename (cursor on the name, `⏎` applies) · `o` new line below (`⏎` applies) · `⌘S` apply · `esc` discard (twice if changed) · in Neovim: `:w` apply, `:q` discard, `⌘.` cancel |
| Anywhere | `⌥⌘Z` History · `⌘Y` Quick Look · `⌘[` / `⌘]` back / forward · `⌘Z` / `⌘⇧Z` undo / redo · `⌘⌫` trash · `⌘.` stop a running operation · `⌘⇧N` new folder · `⌘⇧.` show hidden files · `⌘⇧I` search index · `⌘N` new window · `⌘,` settings · `⌥⌘,` open config file · `⌘R` reload config |

File actions apply to the shelf when it has anything, otherwise to the highlighted row.
Move and copy open a folder picker: type to fuzzy-search folders (your most-visited zoxide
folders show first), or type a path like `~/Documents/`. If something with the same name is
already there, Tilde asks: `k` keep both (`report 2.pdf`, like the Finder), `r` replace (the
old one goes to the Trash, and undo brings it back) or `s` skip. Copies on the same disk
are instant APFS clones; copies to another disk show progress in bytes and can be stopped
with `⌘.` (the partial copy is removed). Trashed items can be put back with undo or from
the Finder.

**Drag and drop.** Drag a row to another app (Finder, Mail, a browser's upload area): it
always arrives as a copy, so nothing moves without Tilde knowing. Drag a shelved row and
the whole shelf goes. Drop files onto the list to bring them into the open folder, or
onto a folder row to put them in that folder: a move on the same disk, a copy to another
(hold ⌥ to copy). Drops ask about name clashes and undo like any move.

**The preview pane** is instant: a thumbnail rendered for the pane (images, the first
page of PDFs and documents, a frame of video), the icon and details for files without
one, bat for code and text, Quick Look's view for folders and apps. The full, interactive
preview (every page of a PDF, video playback, zoom) comes on request with `p`. To make that
quick, it's loaded invisibly once you rest on a file for a moment, and dropped when you
move on; so `p` usually shows it at once. That background load is skipped where it would
cost something: files only in iCloud (they'd be downloaded), on network drives, video and
audio, and anything over 50 MB. Those load when you press `p`, with a spinner.

**File dialogs.** Turn on Settings › General › File dialogs (or `[dialog] enabled = true`)
and give Tilde the Accessibility permission. Then, in any app's Open or Save dialog,
press `⌃⌥Space` (change it in Settings or `[dialog] hotkey`): Tilde's search opens over
the dialog, with the folder open in Tilde first. Pick a folder and the dialog goes
there; pick a file and an Open dialog selects it (`confirm = true` also presses Open),
while a Save dialog goes to its folder and keeps the name you typed. It works with
dialogs that use the standard macOS panel (most apps and browsers); apps with their own
pickers aren't supported. You can also drag from Tilde onto any dialog.

**Quick Look and Open With.** `p` toggles **PREVIEW**: the full preview fills 75% of the
window (`[preview] mode_width`, or Settings › Preview) and has the keyboard, so `j` / `k`
scroll, `space` / `⇧space` page and `g` / `G` jump to the top or bottom; `f` gives it the
whole window, hiding the list, and `f` again brings the list back (`f` from the list goes
straight there). `p` again, `esc`, `h` or a click on the list goes back. PDFs,
code and text scroll in small steps; documents Quick Look draws (Office, Pages and so on)
move a page at a time, since Quick Look only takes page keys. `P` (or `⌘Y`) opens the
Quick Look window; with items on the shelf, or the preview pane turned off, `p` opens it
too. `w` lists the apps that can open the item, your default first; type to narrow it down.
Right-click a row for Open, Open With, Quick Look, Show in Finder, Copy Path, Tags, Rename
and Move to Trash. The shelf is kept when you quit, and comes back in the first window.

`t` edits Finder tags as a comma-separated list (`Work, Red`; colour names show in colour
in the Finder). With several items it shows the tags they share: removing one removes it
from all of them, adding one adds it to all, and their other tags stay. Tags show under the
preview.

**History** (`H`, or Edit › History… `⌥⌘Z`) shows the undo tree in place of the list:
every operation is a node, newest at the top, on the branch it was done on, down to a
**Start** node. Undoing something and then doing something new starts a new branch; the
old one stays (struck through) and can be returned to. The current state is ringed and
marked **now**. The side shows what the selected operation did and what each key would do:

- `⏎` puts files back the way they were right after that operation, across branches:
  everything not on its line is undone (newest first), and what's missing on its line is
  redone (oldest first). Use **Start** to go back before everything.
- `u` undoes just that operation. Later operations that touched the same files (or
  things inside a folder it touched) are undone first, newest first, so nothing is left
  half-reverted.
- `r` redoes it, after redoing the undone operations it builds on.

Anything involving more than one operation lists them and asks first. If something
outside Tilde got in the way (a file moved again, a name now taken), the run stops there
and says why; what was done so far stays recorded. `u` / `⌘Z` and `U` / `⌘⇧Z` move back
and forward along the current branch. `j` / `k` move, `esc` goes back. The history is
shared by all windows and kept across restarts (the last 200 operations from the past 30
days, in `~/Library/Application Support/Tilde/undo.json`).

While typing in the bar, `⌘⌫` and `⌃⌫` clear what's typed; Move to Trash (`⌘⌫`) only
works from the list in SELECT.

`⌃o` goes back to where you were before, like Vim's jump list or a browser's Back:
the folder you came from (not necessarily the parent), with what you'd typed in the bar
and the item you had highlighted, so the same view comes back; or Activity. `⌃i` goes forward again. Every move counts (into a folder, up, `~`, a reveal
from Everywhere, `@`), and a new move after going back drops the forward history. Each
window keeps its own list.

`s` sorts the folder: then `n` name, `m` date modified, `a` date added, `s` size or `k`
kind (by extension); picking the current order again reverses it. Also View › Sort By
(`⌃⌘1`–`⌃⌘5`). Each folder remembers its order (Downloads by date added, a project by
name), and the status bar shows it when it isn't the default. Folders stay on top; a size
sort orders folders by how many items they hold. Typing a search ranks by match, while
filters like `ext:pdf` keep the folder's order. Sorting by date added shows when items
arrived instead of when they changed.

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

## Editing a folder as text

`e` turns the folder into text, one line per item (folders end with `/`):

- edit a name to **rename**
- delete a line to **trash** it
- change a line to a path (`archive/x.pdf`, `~/Documents/x.pdf`) to **move** it
- duplicate a line to **copy** it
- add a line to **create** a file, or a folder if it ends with `/`

Each line carries a hidden ID, so reordering, cut and paste keep track of which item is
which. The side pane lists what will happen as you type, and lines with problems (a name
that's taken, a bad character) are tinted. `⌘S` applies everything as one operation, and
`u` undoes all of it.

### In Neovim

Set `editor = "nvim"` under `[edit]` and EDIT runs your Neovim (0.10 or later) inside
Tilde, with your own config. It starts on the first edit and stays running for the next
ones.

- `:w` (or `⌘S`) applies, `:q` / `:q!` / `ZQ` discards, `:wq` and `ZZ` apply. `⌘.` cancels
  from any mode.
- `r` and `o` start in insert mode (the name's stem selected for `r`); `⏎` applies.
- The IDs are concealed and the cursor is kept out of them. `yyp`, `dd`, `p` and macros all
  work, since lines carry their IDs.
- Lines with problems show as diagnostics; a write with problems is refused with a message
  and the buffer stays open.
- The buffer has `filetype=tilde`, so a `FileType tilde` autocommand can turn off
  format-on-save or anything else that gets in the way. Changing a line with `cc` or `S`
  drops its ID (it becomes a new file, and the old one is trashed).
- `nvim_path` picks a specific nvim; otherwise Tilde looks on your login shell's `PATH`, then
  in Homebrew's folders. `nvim_config = "clean"` skips your config and loads
  `~/.config/tilde/init.lua` instead, if it exists.
- If nvim can't be found, EDIT says why and uses the built-in editor.

## Activity

`@` shows what arrived or changed recently: downloads (with the site they came from),
screenshots and screen recordings, AirDrops, and files you created or edited, newest first
under Today / Yesterday / This week / This month / Earlier. It covers everything in your
**inbox folders** (Downloads, the Desktop and wherever screenshots are saved) plus files
anywhere in the index changed in the last 30 days. `i` narrows it to what's still in an
inbox folder; move things out with `m` (or trash them with `d`) and they leave the inbox.
Downloads that live somewhere else now are marked **filed**.

Nothing is logged: origins come from the attributes macOS attaches to files, so the feed
has history from the first launch and counts moves made in the Finder too. Configure the
inbox folders and how far back it goes under `[activity]`.

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

`~/.config/tilde/config.toml` (or `$XDG_CONFIG_HOME/tilde/config.toml`). Settings (`⌘,`)
edits it for you: each change is written to the file straight away, keeping your comments,
and applied to open windows. `⌥⌘,` creates it from a commented template and opens it; `⌘R`
applies changes made by hand.

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
mode_width = 0.75           # in preview mode (p), 0.3–0.95; f there takes the whole window
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

[sort]                      # unless a folder has its own order (s)
by = "name"                 # name, modified, added, size, kind
reverse = false             # Z→A, oldest first, smallest first
folders_first = true
remember = true             # remember the order chosen for each folder

[keys.go]                   # while typing in the bar: needs ctrl+/alt+ unless a named key
down = ["down", "ctrl+j"]
select_mode = ["esc", "ctrl+g"]

[keys.select]               # in the list
move = "M"                  # replaces m
tag = ["t", "#"]
trash = []                  # unbound
```

A key is the character it types (`j`, `G`, `@`) or a name (`down` `up` `left` `right`
`return` `tab` `esc` `space` `backspace` `delete` `home` `end` `pageup` `pagedown`), with
`ctrl+`, `alt+` or (for names) `shift+` in front. Setting an action replaces its default
keys; a key you give one action is taken from any action that only had it by default.
Settings › Keys lists every action with its keys. Mistakes (unknown actions, a key bound
twice, `cmd+`) are reported in the status bar and skipped.
