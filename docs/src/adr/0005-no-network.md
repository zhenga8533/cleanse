# 0005: No telemetry and no network in the engine

**Status:** Accepted

## Context

Cleanse sees a complete map of a user's files, including project names, paths that reveal
personal details, and the locations of credentials. Users need to trust that this information
never leaves their machine.

## Decision

The engine and CLI make no network connections and collect no telemetry. cargo-deny bans HTTP
client crates from the dependency graph. The desktop app may offer an opt-in update check
through Tauri's updater, which is the only network access in the project, and it sends no data
about the user's files.

## Consequences

- Users and reviewers can verify the promise from the dependency graph.
- There's no usage data to guide priorities, so we rely on issues and false-positive reports.
- Rule updates ship with releases rather than being downloaded at runtime.
