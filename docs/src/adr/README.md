# Architecture decision records

Each record captures one significant decision: the context, what was decided, and the
consequences, including the downsides. Records aren't edited after they're accepted; a later
record supersedes an earlier one instead.

| # | Decision | Status |
|---|---|---|
| [0001](0001-rust-core-tauri-gui.md) | Rust core with a Tauri desktop app | Accepted |
| [0002](0002-single-mutator.md) | One crate may change the filesystem | Accepted |
| [0003](0003-declarative-rules.md) | Rules are data, never commands | Accepted |
| [0004](0004-risk-tiers-and-disposal.md) | Risk tiers and tiered disposal | Accepted |
| [0005](0005-no-network.md) | No telemetry and no network in the engine | Accepted |
| [0006](0006-license.md) | Dual MIT and Apache-2.0 license | Accepted |
| [0007](0007-directory-walker.md) | A std-based parallel directory walker | Accepted |
