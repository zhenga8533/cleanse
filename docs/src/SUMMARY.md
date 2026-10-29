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
