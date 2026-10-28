# Configuring Tilde

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
bat = true                  # syntax-highlight code and text with bat
bat_theme_light = ""        # empty: matches the preset, else "GitHub"
bat_theme_dark = ""         # empty: matches the preset, else your bat config
max_lines = 1000
markdown = "web"            # Markdown in preview mode (p): web, native or quicklook

[index]                     # apply with Rebuild (⌘⇧I)
enabled = true
roots = ["~"]
exclude = ["node_modules", ".git", "target", "build", "DerivedData", "Pods",
           "__pycache__", ".venv", "venv", "~/Library", "~/.Trash"]   # skip contents
hide = []                   # leave out entirely: names, *.log-style patterns, or paths
include_hidden = false
max_items = 1000000         # indexing stops here; ⌘⇧I shows what to exclude

[frecency]
enabled = true              # rank by the folders and files you use
zoxide = true               # also count zoxide's scores (terminal use), if installed
tell_zoxide = false         # also tell zoxide about folders opened in Tilde

[search]                    # text:launch searches inside files
contents = true             # the open folder and below, then everywhere else
spotlight = true            # beyond the folder, Spotlight (when it's indexing)

[sort]                      # unless a folder has its own order (s)
by = "name"                 # name, modified, added, size, kind
reverse = false             # Z→A, oldest first, smallest first
folders_first = true
remember = true             # remember the order chosen for each folder

[cleanup]                   # sizes: 500M, 2G; ages: 90d, 6mo, 1y
big_file = "500M"           # big files at least this big...
big_file_age = "6mo"        # ...not opened or changed for this long
download_age = "60d"        # old downloads: in Downloads or on the Desktop this long
project_idle = "90d"        # build output: projects untouched this long
duplicates_min = "1M"       # smaller duplicates are ignored
build_folders = ["node_modules", "target", "DerivedData", ".venv", "venv", "build",
                 ".gradle", "Pods", ".next", "dist"]
ignore = []                 # never suggested: ~/paths, names or *.patterns (X adds)

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

## Themes

Colour keys: `background bar border text text_dim selection selection_text match
match_selected go select shelf badge_label badge_folder badge_app badge_image badge_video
badge_audio badge_pdf badge_document badge_code badge_text badge_archive badge_other`.
Values are `#RRGGBB` or `#RRGGBBAA`. Anything unset uses the preset, or the system colour
for `default`. Mistakes are reported in the status bar and skipped.

The preview pane shows code and text through [bat](https://github.com/sharkdp/bat) when
it's installed (with line numbers, in a bat theme that matches light or dark), Markdown
rendered by Tilde, and everything else through Quick Look. In PREVIEW (`p`), code and text
stay in bat, showing the whole file in a larger font. HTML files are rendered (scripts off,
nothing fetched from the network). `v` in preview mode shows any rendered file's source.

Markdown is drawn by Tilde, so it looks the same on every Mac and in your theme. The pane
shows it as native text, which takes 10–30 ms even for long files. In PREVIEW,
`markdown` picks how:

- `"web"` (the default): rendered like GitHub, with tables, images next to the file,
  HTML, code highlighting, Obsidian callouts and maths typeset with KaTeX (see
  [Maths](GUIDE.md#maths) in the guide). It's prepared in the background while you rest
  on a Markdown file, so `p` shows it at once. Images on the web aren't fetched (their
  alt text shows), and nothing in the file runs.
- `"native"`: the pane's text, larger. Lighter; tables show as aligned text, HTML is
  left out and maths shows as LaTeX.
- `"quicklook"`: macOS's own preview. It doesn't scroll with Tilde's keys.
