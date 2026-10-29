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

## Crates

| Crate | Responsibility | May change the filesystem? |
|---|---|---|
| `cleanse-model` | Shared types: candidates, risk tiers, plans, journal records | No |
| `cleanse-platform` | Per-OS facts: file identity, links and reparse points, cloud placeholders, allocated size, mounts, elevation, processes | No |
| `cleanse-core` | Scanning, rule matching, safety evaluation, planning, analysis | No |
| `cleanse-exec` | Executing approved plans, quarantine, journal, restore | **Yes, the only one** |
| `cleanse-engine` | Session API with progress and cancellation; the only API front-ends use | No (delegates to exec) |
| `apps/cli` | The `cleanse` command | No |

Dependencies point one way: `model ← platform ← core ← exec ← engine ← apps`. Scanning, rules,
and safety are modules inside `cleanse-core` rather than separate crates. The boundary that
matters for safety, read-only code versus the single mutator, is a crate boundary; the others
can be split out later if they grow.

All `cfg(windows)` / `cfg(unix)` differences and any `unsafe` code live in `cleanse-platform`
behind a `Probe` trait. A fake implementation lets the rest of the code be tested for
Windows-specific situations (junctions, cloud placeholders, 8.3 names) on any OS.

## Front-ends

The CLI and the desktop app both call `cleanse-engine` in-process. The desktop app (Tauri v2
with React) exposes only narrow commands to its web view; no command accepts a path to delete.
Applying a plan takes plan item IDs held by the engine, so a compromised web view can't direct
deletions. Large results are served as aggregated, paged views rather than whole trees.

## Enforcement

- Clippy's `disallowed-methods` rejects deletion, moves, writes, permission changes, and process
  spawning outside `cleanse-exec` ([ADR 0002](adr/0002-single-mutator.md)).
- cargo-deny rejects HTTP client crates ([ADR 0005](adr/0005-no-network.md)).
- `unsafe_code` is denied workspace-wide.
