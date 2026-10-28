# Making the index faster: ideas

Ideas for speeding up and hardening Tilde's filename index (`core/crates/tilde-index`),
collected on 2026-10-28. Nothing here is built yet. How the index works today is in
[SPEC.md](SPEC.md) §6.2.

## Where it stands

- One filename index of the home folder, shared by every window. On the author's Mac:
  108,070 items, an 11 MB save file (`~/Library/Caches/Tilde/index.bin`).
- Items live in nucleo's append-only list (path relative to home + `ItemMeta`: id, kind,
  size, modified, item count). Deleted or changed items are marked in a dead bitset
  and skipped; a changed item is added again.
- A per-folder table (name hash, size/date hash, id) lets each FSEvents batch re-list only
  the changed folders and diff them.
- A search waits up to 250 ms for nucleo, takes matches in nucleo's order until 3,000
  pass the filters, re-scores those (file name, recency, frecency, depth), and keeps the
  best 200. Filter-only queries scan every item in parallel.

## What's actually wrong today

Checked against the code, not assumed:

| Issue | Real? | Detail |
|---|---|---|
| Filters cut good results | Partly | Filters run *before* the 3,000 cap, so non-matching items don't use slots. But candidates come in nucleo's path-score order, so a file Tilde would rank first (strong name match, heavy use) can be past the first 3,000 and never re-scored. Only bites with very broad queries. |
| The 250 ms wait blocks the UI | No | It's on the engine's worker thread. It does delay results, though, and a newer keystroke can't interrupt it. |
| Size or date changes re-add items | Yes | The per-folder diff hashes size and date, so editing a file kills its entry and adds a new one, though its path (all nucleo matches on) didn't change. |
| Dead entries pile up | Yes | They're only dropped on the next launch. Long sessions with lots of file churn grow the matcher. |
| Activity scans everything | Yes | `recent()` walks every item on each refresh. Milliseconds at 100k, linear in the index. |
| Folder sizes walk the disk | Yes | The index has file sizes but never totals them by folder. |
| An unreadable folder looks empty | Check | A re-list that fails with a permission error must not delete what's indexed under it. `refresh_dir` ignores errors other than not-found, which looks right, but needs a test. |

## Ideas, by stage

### 1. Correctness and responsiveness

- **Rank everything that matches, not the first 3,000.** Score every nucleo match with
  Tilde's full score (it's cheap per item), or re-rank in score bands until a band can't
  beat the current 200th result. Add a test where the right answer sits past position
  3,000.
- **Don't block on nucleo.** Publish what's matched after ~10 ms, then publish again
  when nucleo finishes (its notify callback already wakes the index). Newer queries drop
  older work (the engine already has generations).
- **Benchmarks and relevance tests first** (see below), so every change after this is
  measured.

### 2. Cheaper live updates

- **Metadata separate from search strings.** Keep size, date and kind in a side table by
  item id; nucleo holds only the path. A size or date change updates the table and
  touches nothing in nucleo. Only renames, moves, adds and deletes change nucleo.
- **Compact in the background.** When dead entries pass ~20% of the list, build a fresh
  nucleo list from the live items on a background thread, replay changes that arrived
  meanwhile, then swap. Item ids used elsewhere (frecency, folder tables) must not change.
- **A safe save point.** Save the FSEvents id of the last batch fully applied, not the
  last one received, so a crash mid-batch replays it.
- **Test the hard FSEvents cases:** renaming a folder of 10,000 files, moving a folder
  into or out of an excluded place, dropped events (`MustScanSubDirs`), a disk unplugged,
  a folder losing permission, a crash while saving.

### 3. Smarter queries

- **Folder totals from the index** (ties into folder sizes). Keep per folder: file count,
  folder count, total bytes, newest date. A change adds its difference to each folder
  above it, one step per level. Folder sizes would show at once for everything indexed
  and update live; only excluded places (`~/Library`, `node_modules`, hidden folders)
  still need a disk walk. Also speeds up Activity, Clean Up and the Index window's
  largest folders.
- **Indexes for filters:** items by extension and kind (bitsets), by date and size
  (sorted). `ext:pdf mod:<7d` intersects two small sets instead of scanning 108k items;
  `report ext:pdf` fuzzy-matches only the PDFs (with `nucleo-matcher` directly). Activity's
  "changed since" becomes a lookup in the date index. Only worth it where measurement
  shows the plain scan is slow: at 100k items it may not be.
- **File name first.** Rank exact name, then name prefix, then fuzzy name, then fuzzy path;
  a query with `/` in it favours the path. An exact name (`Cargo.toml`) could be a hash
  lookup before any fuzzy matching.
- **Context in ranking.** Favour the project you're in (the nearest folder with `.git`,
  `Cargo.toml`, `package.json`…), then folders you use, then the rest, without letting a
  weak match beat a strong one. Searching `cargo` inside a Rust project should find its
  `Cargo.toml` first.

### 4. Scale (only if benchmarks call for it)

- **Store folders once.** A folder table (id, parent, name) and files that point at their
  folder, instead of every path spelled out. Saves memory and makes folder renames one
  update in the table (nucleo's search strings still need rebuilding for what's inside).
  Don't rely on inode numbers staying the same across launches or disks.
- **Leaner memory.** Precomputed name offsets in each path, metadata in flat arrays,
  shared strings for extensions. Measure real memory against the 11 MB file first.
- **Memory-mapped save file** for faster launches at millions of items.

## Benchmarks and tests to add first

- A `tilde-index-bench` tool over the real home folder and synthetic trees of 100k, 500k,
  1M and 5M items, measuring: first and final results per query (p50, p95), memory,
  full-scan time, load time, time from a file change to updated results, and disk writes.
- A relevance suite: queries with the expected top results, so ranking changes can't
  silently get worse.
- Targets to try for: under 10 ms p95 at 100k items, under 50 ms at 1M, no nucleo changes
  for a size or date change, no waiting on the UI thread.

## Parked

- **Replacing nucleo**: unlikely to be the bottleneck at this size.
- **Trigram (n-gram) indexes**: they help literal substrings, not fuzzy matches like
  `rprt` → `report`, and a trigram prefilter would drop real matches.
- **`fst` sets**: compact and fast for exact and prefix lookups, but awkward to update
  live; needs a mutable overlay. Only at millions of items.
- **A database (SQLite and the like)**: no gain over the current structures at this scale.
- **Switching folder listing libraries**: Tilde already uses `getattrlistbulk`.

## Suggested order

1. Benchmarks and the relevance suite.
2. Rank all matches; publish early, then finish.
3. Metadata side table (no re-adds for size or date changes), background compaction, the
   safe save point and the FSEvents tests.
4. Folder totals from the index, then replace the disk walk for folder sizes in indexed
   places.
5. Filter indexes, name-first ranking and project context, each kept only if the
   benchmarks or the relevance suite show a gain.
6. Folder table and memory layout, if 1M+ items becomes a goal.
