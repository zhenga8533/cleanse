# 0008: Read Git repositories with gix, never by running git

**Status:** Accepted

## Context

Cleanse needs two facts from Git about directories it might remove: whether the repository
ignores them, and whether it tracks any files inside them. Scans cross many repositories the
user didn't write: cloned dependencies, downloaded examples, archives.

Running the `git` command in such a repository isn't safe. Its config can run arbitrary
commands: `core.fsmonitor` runs a program on `git status`, and other settings name filters and
helpers to execute. The user's global and system config also vary from machine to machine.

## Decision

Read repositories with [gix](https://github.com/GitoxideLabs/gitoxide), a pure-Rust Git
implementation, using only its index and ignore-file support. Open every repository with
`gix::open::Options::isolated()`, so only the repository's own config is read: not the user's,
not the system's, and not the environment.

Each repository's index is read once per scan and cached, along with the set of directories
holding tracked files. Each lookup opens its own lightweight repository handle, because gix's
handles aren't thread-safe without its `parallel` feature, so lookups are safe from any thread.

## Consequences

- No scanned repository can make Cleanse run a program.
- Results don't depend on the user's global Git config. A global excludes file
  (`core.excludesFile`) isn't read, so a directory ignored only there doesn't count as ignored.
  For rules that require an ignored directory, that only means fewer matches.
- A repository Cleanse can't read is reported as unreadable rather than guessed about, and the
  safety checks can treat it conservatively.
- gix adds a sizable set of crates to the build. Only the `index`, `excludes`, and `sha1`
  features are enabled.
