# Contributing to Cleanse

Thanks for helping. Cleanse removes files on other people's machines, so the bar for
correctness is high. This guide covers setup, conventions, and how changes are reviewed.

## Ways to help

- **Report false positives**: anything Cleanse flags that should have been kept. These are
  handled before anything else.
- **Request or write rules** for tools and apps Cleanse doesn't know about yet.
- Fix bugs, improve the docs, or test on a platform you use.

## Development setup

1. Install Rust with [rustup](https://rustup.rs).
2. Install a C toolchain for your platform:
   - **Windows**: Visual Studio 2022 Build Tools with the "Desktop development with C++"
     workload, for example
     `winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"`.
   - **macOS**: Xcode Command Line Tools (`xcode-select --install`).
   - **Linux**: your distribution's compiler package, such as `build-essential` on Debian and
     Ubuntu.
3. In the repository, run `rustup toolchain install` to get the toolchain version pinned in
   `rust-toolchain.toml`.
4. Install the tools that `cargo xtask ci` uses:

   ```sh
   cargo install --locked cargo-nextest cargo-deny typos-cli taplo-cli
   ```

## Everyday commands

| Task | Command |
|---|---|
| Build | `cargo build` |
| Run the CLI | `cargo run --bin cleanse -- <args>` |
| Run the tests | `cargo nextest run --workspace` |
| Run every CI check | `cargo xtask ci` |
| Preview the docs | `mdbook serve docs` |
| Create a synthetic tree for benchmarks | `cargo xtask gen-tree <dir> [--entries <n>]` |
| Benchmark the directory walker (read-only) | `cargo bench -p cleanse-core --bench walk -- <dir>` |

Antivirus software inspects every file and directory open, so walker benchmarks on a machine
with real-time scanning measure the scanner as much as the walker. The `Walker benchmark`
workflow runs the same comparison on GitHub's runners.

## Project layout

```
crates/
  cleanse-model/     shared data types
  cleanse-platform/  read-only, per-OS filesystem facts
  cleanse-core/      scanning, rules, safety checks, planning (read-only)
  cleanse-exec/      executes approved plans: the only code that changes the filesystem
  cleanse-engine/    the API the CLI and desktop app call
apps/cli/            the `cleanse` binary
docs/                design docs and architecture decision records (mdBook)
xtask/               repository automation
```

## Architecture rules

These are enforced by the crate graph, clippy, and cargo-deny, and reviewers check them too.
The reasoning is in the [decision records](docs/src/adr/README.md).

- **Only `cleanse-exec` changes the filesystem or spawns processes.** Everywhere else, clippy
  rejects `std::fs::remove_*`, `rename`, `copy`, `write`, `File::create`, `set_permissions`, and
  `Command::new`. Tests that need to build fixtures opt in with a narrowly scoped
  `#[allow(clippy::disallowed_methods, reason = "...")]`.
- **Rules are data.** They match paths and describe what they found. They never run commands.
- **No network access** in the engine or CLI.
- **`unsafe` is denied** across the workspace. Per-OS code that needs it will be confined to
  `cleanse-platform`.

## Commits and pull requests

- Branch from `main` and open a pull request; nothing is pushed to `main` directly.
- PR titles follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`,
  `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `ci:`). A check enforces this, and since PRs
  are squash-merged, the title becomes the commit that drives the changelog.
- Keep each PR focused, add tests for behavior changes, and say how you tested it, including on
  which operating systems.

## Changes that need extra care

Safety checks (`crates/cleanse-core/src/safety/`), plan execution (`crates/cleanse-exec/`), AI
agent rules (`rules/agents/`), and the elevated helper (`apps/elevate/`) always need a
maintainer's review. In those PRs, describe what could go wrong and how the change prevents it,
and add a test that would have caught the failure.

## Adding a rule

The rule format lands with the first release (M1). Each rule will be a TOML file under
`rules/<category>/` that explains what it matches and how the files come back, with at least
one positive and one negative fixture. No Rust required. The design is in
[docs/src/rules.md](docs/src/rules.md).

## License

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in the work by you, as defined in the Apache-2.0 license, shall be dual licensed under MIT OR
Apache-2.0, without any additional terms or conditions.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).
