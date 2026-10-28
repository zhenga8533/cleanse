# Cleanse

Find and safely remove the files you don't need, with an explanation for every item.

Cleanse is a cross-platform (Windows, macOS, Linux) disk cleanup tool built with developers in
mind. It looks for regenerable build output, package-manager caches, AI coding agent leftovers,
and everyday junk; tells you why each item is safe to remove and how it comes back; and never
removes anything you haven't approved.

> **Status: pre-alpha.** This repository holds the project foundation. Nothing has been
> released yet, and the first release (v0.1) will be read-only.

## What makes it different

- **It explains itself.** Every item carries the rule that matched it, the evidence (for
  example "`Cargo.toml` alongside and a `CACHEDIR.TAG` inside"), and how to get it back.
  `cleanse explain <path>` answers why something is, or isn't, removable.
- **Git-aware.** Files tracked by git are never candidates, and projects with uncommitted work
  are flagged.
- **Honest numbers.** Guaranteed savings are reported separately from estimates (pnpm
  hardlinks, copy-on-write clones, cloud placeholders), and the measured free-space change is
  shown after every cleanup.
- **Knows AI coding agents.** Built around the documented data layouts of tools like Claude Code
  and Codex: verified disposable files can be cleaned, while conversations, memory, settings,
  and credentials are protected.
- **Rules are data.** Cleanup rules are declarative TOML with fixture tests. They can't run
  commands, and writing one doesn't require Rust.
- **Private.** No telemetry, and the engine has no network access.

## Safety promise

- Scanning is read-only.
- Nothing is removed without your explicit approval, and each approved item is re-checked
  immediately before it's touched.
- Age alone is never a reason to remove something.
- Protected locations (system folders, credentials, cloud-synced folders, agent history,
  git-tracked files) can't be overridden by any rule.
- Items that need judgment go to a quarantine you can restore from. Regenerable items such as
  `node_modules` are deleted, because rebuilding them is the recovery.
- Every cleanup supports `--dry-run`.

## What it will clean

| Area | Examples | Default tier |
|---|---|---|
| Project build output | `node_modules`, `target/`, `.venv`, `.next`, `bin/` + `obj/`, `.gradle` | Regenerable |
| Package-manager caches | npm, Yarn, pip, uv, Cargo, NuGet, Homebrew | Safe or Regenerable |
| AI coding agents | Claude Code debug logs and legacy caches, Codex logs | Safe; history and config are Protected |
| General | Temp files, app logs, crash dumps, browser caches (browser closed) | Safe |
| Needs your judgment | Large files, old installers in Downloads, stale git worktrees | Review, never preselected |

Where a native tool is the right answer, Cleanse points you to it instead: `pnpm store prune`,
`go clean -modcache`, compacting WSL and Docker disk images, and so on.

## Roadmap

| Milestone | Scope |
|---|---|
| M0 | Repository foundation (current) |
| M1 · v0.1 | Read-only engine and CLI: `scan`, `explain`, rules, safety checks |
| M2 · v0.2 | Cleanup with quarantine, restore, and a journal; AI agent and general rules |
| M3 · v0.3 | Disk analysis and insights; CLI feature-complete |
| M4 · v0.5 | Desktop app (Tauri + React) |
| M5 · v1.0 | Elevated system cleanup on Windows and Linux, signed builds, package managers |

The design is described in the [architecture](docs/src/architecture.md) and
[safety model](docs/src/safety-model.md) docs.

## Building from source

You need Rust (via [rustup](https://rustup.rs)) and a C toolchain; on Windows that's Visual
Studio Build Tools with the "Desktop development with C++" workload. Then, in the repository:

```sh
rustup toolchain install          # installs the version pinned in rust-toolchain.toml
cargo run --bin cleanse -- --help
cargo xtask ci                    # runs the same checks as CI
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Rule requests for tools Cleanse doesn't know yet are
especially welcome.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
