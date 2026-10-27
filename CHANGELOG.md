# Changelog

What changed in each release. The format follows [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

- **Markdown is rendered by Tilde.** The pane shows it as formatted text in 10–30 ms
  (bat took up to 330 ms on long files, and Markdown no longer needs bat). `p` shows it
  like GitHub, with tables, images, HTML, highlighted code and LaTeX maths, prepared in
  the background so it appears at once, and scrolls with `j` / `k`. `[preview] markdown`
  picks `web` (default), `native` or `quicklook`. Remote images aren't fetched.
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

[Unreleased]: https://github.com/tevans2/tilde/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/tevans2/tilde/releases/tag/v0.1.0
