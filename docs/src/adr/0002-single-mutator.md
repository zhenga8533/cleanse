# 0002: One crate may change the filesystem

**Status:** Accepted

## Context

Most of Cleanse only reads: scanning, rule matching, safety evaluation, planning, and analysis.
The code that deletes or moves files is small but carries nearly all of the risk. If deletion
calls could appear anywhere, every change would need to be reviewed as if it could destroy
data.

## Decision

`cleanse-exec` is the only crate that may delete, move, write, or change permissions on files,
or spawn processes. It acts only on items from an approved plan, after re-running the safety
gates on each one.

This is enforced mechanically:

- The crate graph: `cleanse-core`, which holds rules and safety logic, doesn't depend on
  `cleanse-exec`.
- `clippy.toml` lists the mutating standard-library functions under `disallowed-methods`.
  `cleanse-exec`, the future elevated helper, `xtask`, and test fixtures opt in with a scoped
  `#[allow]` that states a reason.
- CODEOWNERS requires a maintainer's review for `cleanse-exec`.

## Consequences

- Review effort concentrates where the risk is.
- Writing Cleanse's own files (saved plans, the journal, quarantine manifests) also goes
  through `cleanse-exec`, which is slightly less direct than writing them where they're
  produced.
- Tests that create fixture trees need an explicit `#[allow]`. That friction is intended.
- The lint covers the standard library. Third-party functions with the same effect, such as
  `trash::delete`, are added to the list when the dependency is introduced.
