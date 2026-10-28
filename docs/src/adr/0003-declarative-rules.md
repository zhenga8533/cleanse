# 0003: Rules are data, never commands

**Status:** Accepted

## Context

Cleanup knowledge (where each tool keeps its caches, what counts as a build artifact) changes
constantly and is best contributed by people who use those tools. Some existing cleaners let
rule files run programs; BleachBit's CleanerML has a `process` action, for example. A rule
format that can run commands turns every rule contribution, and every rule file a user
downloads, into a potential remote-code-execution vector.

## Decision

Rules are declarative TOML. A rule can match paths (names, markers, globs, linear-time regular
expressions, path templates under declared bases) and attach descriptive text: why it's
removable, how it's regenerated, and an optional command to *display* to the user. Rules can't
cause a command to run.

Where running a tool is the right cleanup (for example `pnpm store prune`), Cleanse shows the
command for the user to run. Any future built-in actions will be fixed-argument Rust code,
reviewed like the rest of `cleanse-exec`, and referenced by identifier from built-in rules only.

## Consequences

- Rule contributions can be reviewed as data, tested with fixtures, and need no Rust.
- Some cleanups can't be expressed and remain advice until a built-in action exists.
- The rule schema is a public interface and needs versioning once custom rules are supported.
