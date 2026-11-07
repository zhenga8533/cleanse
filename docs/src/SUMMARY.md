# Summary

[Introduction](introduction.md)

# Design

- [Architecture](architecture.md)
- [Safety model](safety-model.md)
- [Rules](rules.md)

# Decisions

- [Architecture decision records](adr/README.md)
  - [0001: Rust core with a Tauri desktop app](adr/0001-rust-core-tauri-gui.md)
  - [0002: One crate may change the filesystem](adr/0002-single-mutator.md)
  - [0003: Rules are data, never commands](adr/0003-declarative-rules.md)
  - [0004: Risk tiers and tiered disposal](adr/0004-risk-tiers-and-disposal.md)
  - [0005: No telemetry and no network in the engine](adr/0005-no-network.md)
  - [0006: Dual MIT and Apache-2.0 license](adr/0006-license.md)
  - [0007: A std-based parallel directory walker](adr/0007-directory-walker.md)
  - [0008: Read Git repositories with gix, never by running git](adr/0008-read-git-with-gix.md)
  - [0009: The desktop app's web view can only name what the engine showed it](adr/0009-desktop-web-view.md)
  - [0010: System cleanup runs elevated, apart from everything else](adr/0010-elevated-system-cleanup.md)
  - [0011: The user's cleanup runs elevated only when there's no other way](adr/0011-cleanup-with-uac-off.md)
  - [0012: Cleanse removes folders through their handles, so it can stop partway](adr/0012-remove-folders-through-handles.md)

# Maintaining

- [Releasing](releasing.md)
