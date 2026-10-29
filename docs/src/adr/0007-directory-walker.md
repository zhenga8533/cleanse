# 0007: A std-based parallel directory walker

**Status:** Accepted

## Context

Every scan starts with a directory walk, so the walker sets both the speed of a scan and some
of its safety guarantees. Cleanse needs a walker that:

- reports links and junctions without following them, and doesn't enter other filesystems;
- reports the path of every directory it couldn't read, because the safety model blocks any
  candidate whose subtree is incomplete ([safety model](../safety-model.md), gate 4);
- keeps file names that aren't valid Unicode, and handles Windows paths longer than 260
  characters;
- is fast on Windows, macOS, and Linux.

Candidates:

- **jwalk**, the walker behind earlier versions of dua-cli. Dropped before benchmarking: its
  0.9.0 release describes itself as "Use `dua-core` instead".
- **dua-core** 5.0.1, its successor. A work-stealing walker that reads metadata from the native
  directory listing on Windows (including allocated size and file IDs) and on macOS
  (`getattrlistbulk`, optionally with APFS clone IDs). Directory read errors come back as a
  bare `io::Error` without the directory's path.
- **A custom walker**: `std::fs::read_dir` on a rayon scope, about 100 lines with no `unsafe`.

## Measurements

Both walkers walked a 200,000-entry synthetic tree from `cargo xtask gen-tree` (164,937 files,
35,063 directories, 149 MiB): one warm-up run each, then 7 timed runs alternating between them.
Medians:

| Machine | Threads | std + rayon | dua-core |
|---|---|---|---|
| GitHub `windows-latest` | 4 | **0.352 s** | 0.566 s |
| GitHub `macos-latest` | 5 | **0.456 s** | 0.500 s |
| GitHub `ubuntu-latest` | 4 | **0.135 s** | 0.269 s |
| Windows 11 workstation with Datto AV | 12 | 16.0 s | 15.4 s |
| Same workstation, a real 374,735-entry folder (3 runs) | 12 | 19.0 s | 19.1 s |

Both walkers found exactly the same entries in every run. The runner numbers come from the
`Walker benchmark` workflow in the pull request that added this record.

On the workstation, real-time antivirus inspects every directory open. Both walkers ended up
bound by it at roughly 5 ms of serialized time per directory, so those rows say nothing about
the walkers themselves.

## Decision

Use the custom walker, `cleanse_core::scan::walk`, and don't depend on a walker crate.

- It's faster on all three runners, by 1.1 to 2.0 times, and walks 200,000 entries in about a
  third of a second on Windows.
- It reports the path of every unreadable directory, which the safety model needs and
  dua-core 5.0.1 doesn't provide.
- It's small enough to audit in full and adds only rayon, which the engine needs anyway.

## Consequences

- Windows listings through std carry attributes but not allocated sizes or file IDs. Size
  accounting will fetch those only for candidates, and only where the attributes call for it
  (compressed, sparse, or cloud-placeholder files, and known hardlink patterns), never for every
  entry.
- macOS gets no APFS clone IDs from the listing, so clone-aware estimates will also need
  per-candidate queries.
- With antivirus active, the same tree took about 45 times longer to walk on the workstation than
  on a runner. Scans must show progress and support cancellation from the start, and performance
  targets are stated for machines without real-time scanning.
- `cargo bench -p cleanse-core --bench walk` and the `Walker benchmark` workflow stay, to catch
  regressions. If dua-core starts reporting error paths, the comparison can be re-run from this
  record's pull request.
