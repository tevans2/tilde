# Changelog

What changed in each release. The format follows [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

- **Search inside files**: `text:launch` or `text:"launch window"` lists files whose text
  matches, with the matching line, and previews them at that line. The open folder is read
  on request (skipping ignored files, build folders and binaries, cancelled by the next
  key); beyond it, Spotlight, or recently changed text files from Tilde's index when
  Spotlight is off. No content index, so nothing changes for ordinary searches. Settings ›
  Search can turn it off, or Spotlight's part (`[search]`). `text!:launch` searches
  everything in the folder, `.gitignore`d files, build folders and hidden files included.

Some default keys changed, to be more consistent and more like the Finder and Vim. Keys
you set yourself are kept.

- **`space` previews**, as in the Finder (it was `p`); `⇧space` opens the Quick Look window
  (was `P`). In preview mode, `⌃d` / `⌃u` page.
- **`y` / `p` copy and paste files**, Vim style, and `P` pastes by moving them. `⌘C`,
  `⌘V` and `⌥⌘V` do the same, as in the Finder, and work with the Finder and other apps.
  Copying paths moved to `Y`.
- **`g` goes places**: a list of places (`g h` Home, `g d` Downloads, `g D` Desktop…), views
  (`g a` Activity, `g c` Clean Up, `g u` History) and your bookmarks, one key away. `g g` is
  the top (it was `g`).
- **Bookmarks**: `b` and a key bookmarks the folder, for `g` and that key.
- **The Finder's Go menu**: `⌘↑` enclosing folder, `⌘↓` / `⌘O` open, `⇧⌘H` Home, `⌥⌘L`
  Downloads, `⇧⌘D` Desktop, `⇧⌘O` Documents, `⇧⌘A` Applications, `⇧⌘C` Computer, `⇧⌘F`
  Activity, `⇧⌘G` go to folder.
- **`.` shows hidden files** (or hides them again).
- **Leaving is the same everywhere**: `esc` and `h` back out one layer, the key that opened
  a view closes it, and `q` closes preview, History, Clean Up or Activity.
- **One meaning per key**: `R` reads the folder again (in Clean Up: looks again, was `r`);
  redo in History is `U` / `⌃R` as everywhere (was `r`); History follows your own keys.
- `space` no longer adds to the shelf (`x` does).
- **Counts**: a number before a move repeats it, as in Vim: `5j`, `3k`, `2h` (two folders
  up), `4x` (shelve four), `12G` (row 12). Not in Activity, where `1`–`5` filter.
- **`S` reverses the sort** in one key.
- **Clean Up numbers**: `1`–`6` open a category straight away; each shows its number.

## [0.2.0] - 2026-10-27

- **Markdown is rendered by Tilde.** The pane shows it as formatted text in 10–30 ms
  (bat took up to 330 ms on long files, and Markdown no longer needs bat). `p` shows it
  like GitHub, with tables, images, HTML, highlighted code and LaTeX maths, prepared in
  the background so it appears at once, and scrolls with `j` / `k`. `[preview] markdown`
  picks `web` (default), `native` or `quicklook`. Remote images aren't fetched.
- **Maths typeset with KaTeX** in Markdown previews: `$…$`, `$$…$$` (blank lines and
  `aligned` steps inside, in callouts, lists and tables), `\( \)`, `\[ \]`, ```` ```math ````,
  chemistry, `\newcommand` across a note. Written for Obsidian: of 23,312 formulas in a
  real vault, all but one render (that one is a TeX error). A formula that can't render
  shows its TeX in red with the error on hover.
- **Obsidian callouts** (`> [!note] Title`) show as titled, coloured boxes.
- **Fewer hints, every key a `?` away**: the status bar keeps the essentials; `?` (`⌃/` in
  the bar, or a click on the status bar) lists every key for where you are, with the menu's
  shortcuts. Type to find one; `⏎` or a click runs it.
- **More for the mouse**, with no change to any key: click a folder in the path bar to go
  there (or drop files on it), a `⋯` button on the row under the pointer, a fuller row
  menu (move to, copy to, compress, shelf, source), a menu for empty space (new folder,
  sort, hidden files, open in Terminal), ⌘/⇧-click to shelve rows, a click on the item
  count for sort and hidden files, and folders that open when a drag is held over them.
- **Source view**: `v` in preview mode swaps a rendered Markdown, HTML or SVG file for its
  whole source through bat, and back (`v` from the list goes straight there).
- **HTML pages render** in preview mode, with their own styles, and scroll with `j` / `k`.
  Scripts don't run and nothing is fetched from the network.
- **Code and text in preview mode stay highlighted.** `p` shows the whole file through
  bat in a larger font, instead of Quick Look's plain text, and scrolls with `j` / `k`.

## [0.1.0] - 2026-10-27

The first release.

- **Two modes**: GO, where typing searches this folder and everywhere at once, and SELECT,
  with Vim-style keys (`j` `k` `h` `l`) for moving and acting on files.
- **Search everywhere** from an index of your home folder that's kept up to date live,
  with filters (`ext:pdf`, `kind:image`, `mod:<7d`, `size:>100M`, `in:dev`).
- **Frecency**: the folders and files you use rank first, and zoxide's terminal history
  counts too when it's installed.
- **File operations** on a shelf (`x` to collect): move and copy with a fuzzy folder
  picker, trash, rename, tags, compress, Open With. Every operation can be undone, and
  **History** (`H`) shows them as an undo tree you can go back to any point of.
- **EDIT**: a folder as text. Rename, move, copy, create and trash by editing lines, in
  the built-in editor or your own Neovim.
- **Activity** (`@`): what arrived or changed recently: downloads, screenshots, AirDrops,
  edits. Plus an inbox of what's still to file.
- **Preview**: instant thumbnails and syntax-highlighted code (with bat), and the full,
  scrollable preview with `p`.
- **Clean Up** (`C`): big files you haven't opened, old downloads, duplicates, build
  output in idle projects, app caches and the Trash, with checks before anything goes.
- **File dialogs**: `⌃⌥Space` in any app's Open or Save dialog opens Tilde's search over it.
- Jump list (`⌃o` / `⌃i`), sorting per folder, drag and drop, Quick Look, themes,
  configurable keys, a Settings window and a welcome window.

[Unreleased]: https://github.com/tevans2/tilde/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/tevans2/tilde/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/tevans2/tilde/releases/tag/v0.1.0
