# Architecture

> Status: the crate structure exists; the pipeline is implemented starting in M1.

## Pipeline

```text
roots ─▶ Scan ─▶ Detect ─▶ Evaluate ─▶ Plan ─▶ [user approval] ─▶ Execute ─▶ Journal
         └────── cleanse-core: read-only ──────┘    data      CLI / GUI        cleanse-exec
```

1. **Scan** walks the chosen roots in parallel without following links or crossing
   filesystems, and measures sizes ([ADR 0007](adr/0007-directory-walker.md)).
2. **Detect** matches entries against [rules](rules.md). Each match records the rule, the risk
   tier the rule declares, and the evidence found, such as a `Cargo.toml` alongside.
3. **Evaluate** runs every match through the [safety gates](safety-model.md), which attach
   concerns, and turns it into a candidate. A candidate's tier can only rise from what its rule
   declared: any blocking concern makes it Protected, and it stays visible with the reason.
4. **Plan** is plain data: the items the user selected, each with a fingerprint of what was
   scanned. Plans can be saved to a file, reviewed, and applied later.
5. **Execute** re-checks every item against the live filesystem and the current rules
   immediately before acting, then deletes, quarantines, or trashes it and records the outcome.

`cleanse analyze` runs the same scan and also builds a tree of folder sizes. The walk adds up
the files directly in each folder it reads, without a lock or lookup per file. The items rules
match aren't walked, so their measured sizes become leaves. Totals are summed from the deepest
folders up. How much can be removed under a folder comes from the candidates, which are sorted by
path, so it isn't stored in the tree. A plain `scan` doesn't build the tree.

## Crates

| Crate | Responsibility | May change the filesystem? |
|---|---|---|
| `cleanse-model` | Shared types: candidates, risk tiers, plans, journal records | No |
| `cleanse-platform` | Per-OS facts: file identity, ownership, links and reparse points, cloud placeholders, allocated size, mounts, elevation, processes, system folders | Only through its `privileged` feature, which only `cleanse-exec` turns on, to create system cleanup's data folder ([ADR 0010](adr/0010-elevated-system-cleanup.md)) |
| `cleanse-core` | Scanning, rule matching, safety evaluation, planning, analysis | No |
| `cleanse-exec` | Executing approved plans, quarantine, journal, restore | **Yes, the only one** |
| `cleanse-engine` | Session API with progress and cancellation; the only API front-ends use | No (delegates to exec) |
| `apps/cli` | The `cleanse` command | No |
| `apps/desktop/views` | What the desktop app shows, built from a session, for its window and its helper | No |
| `apps/elevate` | The desktop app's elevated helper, which does system cleanup for its window | No (delegates to exec) |

Dependencies point one way: `model ← platform ← core ← exec ← engine ← apps`. Scanning, rules,
and safety are modules inside `cleanse-core` rather than separate crates. The boundary that
matters for safety, read-only code versus the single mutator, is a crate boundary; the others
can be split out later if they grow.

All `cfg(windows)` / `cfg(unix)` differences and any `unsafe` code live in `cleanse-platform`
behind a `Probe` trait. A fake implementation lets the rest of the code be tested for
Windows-specific situations (junctions, cloud placeholders, 8.3 names) on any OS.

## Front-ends

The CLI and the desktop app both call `cleanse-engine` in-process. The desktop app (Tauri v2
with React) exposes only narrow commands to its web view, and no command accepts a path: a scan
covers the folders picked in the system's folder picker, or the home folder. It goes through
the engine's `Session`, which holds the latest scan's results and runs one scan or change at a
time. Candidates, folders, and quarantine entries are named by IDs the session
gave out, and an ID from an earlier scan is refused, so a compromised web view can't direct
deletions or name something it wasn't shown. Results are served as summaries and pages, and the
size tree a folder at a time, rather than whole. [ADR 0009](adr/0009-desktop-web-view.md)
describes the rest of the web view's limits: only the app's own commands, a strict content
security policy, and no navigating elsewhere.

For system cleanup, the window never runs elevated. It starts a helper, `apps/elevate`, as an
administrator or root, and asks it for scans and changes over a protocol of one JSON message per
line ([ADR 0010](adr/0010-elevated-system-cleanup.md)). The helper holds a session of system
cleanup's own, and requests name things only by the IDs it gave out, as the window's commands do.
It treats the window as untrusted: it stops at the first message that isn't a request, or after
ten minutes with none. On Linux it talks over the standard input and output pkexec gives it; the
pipe it needs on Windows, and the app's System section, come next.

## Enforcement

- Clippy's `disallowed-methods` rejects deletion, moves, writes, permission and ownership
  changes, links, and process spawning outside `cleanse-exec`
  ([ADR 0002](adr/0002-single-mutator.md)), and calls into `cleanse-platform`'s `privileged`
  module too.
- cargo-deny rejects HTTP client crates ([ADR 0005](adr/0005-no-network.md)).
- `unsafe_code` is denied workspace-wide.
