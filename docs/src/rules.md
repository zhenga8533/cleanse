# Rules

> Status: design. The rule format and loader are implemented in M1.

Rules describe what Cleanse can recognize. They are declarative TOML files in `rules/`,
embedded into the binary at build time, and they only match paths and describe what they found:
they can't run commands ([ADR 0003](adr/0003-declarative-rules.md)). Writing one doesn't require
Rust.

## Example

```toml
[[rule]]
id = "node.node_modules"
name = "Node.js dependencies"
category = "dev.dependencies"
kind = "project_artifact"
dir_name = "node_modules"
markers = ["package.json"]
risk = "regenerable"
explain = "Installed packages. Your package manager recreates them from package.json and the lockfile."
regenerate = "npm install / pnpm install / yarn"
```

## Kinds

| Kind | Matches | Example |
|---|---|---|
| `project_artifact` | A directory name next to a project marker, and not tracked by git | `target/` next to `Cargo.toml` |
| `known_location` | A path under a known base directory | npm's cache in the user cache folder |
| `agent_data` | Entries inside an AI agent's data directory; anything unmatched there is Protected | Claude Code's `debug/` |
| `insight` | Something worth knowing about, with advice and an optional command to copy. Never executed | A large WSL disk image |

## Constraints

- Path templates resolve only under their declared base (`{home}`, `{cache}`, `{temp}`, an
  agent root, and so on). `..` and absolute paths are rejected when the rule loads.
- Regular expressions use a linear-time engine, so a rule can't stall a scan.
- Rules for package-manager caches state whether direct deletion is verified safe or whether
  Cleanse should only suggest the tool's own command (for example `pnpm store prune`).
- Rules for AI agents record the agent versions and documentation they were verified against.
- Custom rules in the user's config folder are labeled as custom and still pass every safety
  gate.

## Tests

Every rule needs at least one positive and one negative fixture: a small directory tree and the
matches expected in it. CI fails if a rule has none, and `cleanse rules test` runs them locally.
