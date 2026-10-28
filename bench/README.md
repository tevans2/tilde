# tilde-bench

Measures Tilde's core so changes can be judged by numbers: the filename index, folder
listing, folder sizes, searching inside files, archives, and the engine the app talks to.
Source: `core/crates/tilde-bench`.

```sh
cd core
cargo run --release -p tilde-bench                       # every suite, standard profile
cargo run --release -p tilde-bench -- --profile quick    # a minute, for a first look
cargo run --release -p tilde-bench -- --real             # also your own index (a copy)
cargo run --release -p tilde-bench -- --suite index,engine --filter "typing"
cargo run --release -p tilde-bench -- compare            # baseline vs the newest run
cargo run --release -p tilde-bench -- list               # the suites
```

Each run prints a line per case as it goes and saves everything, every sample included,
to `bench/results/<time>-<commit>[-label].json` (not committed). `--baseline` also saves
it as `bench/baseline.json`, the run others are compared against; commit that one when
it should change.

## Getting numbers you can trust

- Release builds only (the tool warns otherwise).
- Plugged in, Low Power Mode off, a cool machine, other apps closed. The tool records
  power, Low Power Mode, thermal throttling and the load average before and after, and
  warns about each; `compare` points out when two runs differ in any of them or in
  machine, macOS or compiler.
- The same profile for runs you compare. `thorough` for decisions, `quick` for a look.
- Compare, don't eyeball: `compare` resamples both runs (bootstrap, 2,000 rounds) and
  calls a change real only when its 95% interval excludes "no change" and it's at least
  3% (`--threshold`). Instructions and allocations are steadier than time; when time
  moves and they don't, suspect the machine.
- Run once more after test data is first built. Folders just written list noticeably
  cheaper (about a third of the instructions) until macOS has processed the new files;
  after that the numbers settle and repeat to within a fraction of a percent.
- `compare --fail-above 10` exits with an error when anything got significantly slower
  by more than 10%, for scripts.

## What it measures

Every sample records, for the whole process (all threads, so the index's matcher and
FSEvents threads count):

| Metric | What it tells you |
|---|---|
| wall time | what the user waits for |
| CPU time | work across all threads (more than wall time = parallel work) |
| instructions, cycles | work done, steadier than time |
| energy | nanojoules the kernel billed: battery cost |
| allocations, bytes allocated | allocator pressure, a common hidden cost |
| heap change, footprint change | memory kept by the operation |
| page faults, page-ins, disk read/written | memory and I/O behaviour |
| idle wakeups, runnable time, context switches | scheduling cost, contention |

Slow one-off operations (loading, scanning, unpacking) also record the peak extra heap
held while they ran. Cases add their own numbers: items, hits, bytes per item, rates,
the index's internal counts.

Fast operations run repeatedly per sample (about 2 ms a sample) and are reported per
operation. Each case warms up, then samples until it has enough samples and time, or
hits a cap (see the profiles in `runner.rs`).

## Suites

| Suite | Data | Cases |
|---|---|---|
| `index` | synthetic indexes (100k, 500k by default; `--sizes`), your own with `--real` | save and load the index file; memory held per item; searches: 1–2 characters, words common and rare, several words, paths, Unicode, no match, each filter, filters with words, `in:`; the same text again; typing words a key at a time; Activity's recent files; Clean Up's selection; the Index window's numbers |
| `live` | a synthetic home on disk (`--live`), real FSEvents | time until a new, deleted or renamed item shows in search; editing 10%, 50%, 100% of files: time to apply, dead entries, memory, and searches before and after |
| `scan` | synthetic homes on disk (`--trees`), your home with `--real-heavy` | building the index from disk; listing every folder without indexing |
| `folder` | folders of 100 to 50,000 items | listing, sorting by each key, matching typed text |
| `sizes` | the synthetic homes; `~/Developer` etc. with `--real-heavy` | totalling folder sizes |
| `contents` | text files in folders, one 2 MB file | `text:` scans for rare, common, absent and capitalised words |
| `archive` | a zip and a tar.gz | listing afresh and cached, unpacking one file and everything |
| `engine` | a synthetic home on disk with its index | opening folders, typing and whole queries (until results arrive), fetching rows, sorting |

Synthetic data is deterministic: a home shaped like a real one (code projects with
`.git`, dependency and build folders; photos; documents; music; downloads; some
non-English names), the same for the same size. Trees written to disk are cached in
`core/target/tilde-bench-data` and reused; files are sparse, so they have their sizes
without using the disk. Disk caches are warm after the first sample, so scans measure
warm listing; a first scan after a restart is slower.

Your own data is only read: the index is a copy, home scans and folder sizes only list.
Reports name your queries by rank ("name word #10"), not by the words, so results don't
carry your file names.
