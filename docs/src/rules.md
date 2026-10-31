# Rules

> Status: project artifacts, known locations, and Git checks are implemented. AI agent rules
> and insights come in M2.

Rules describe what Cleanse can recognize. They're TOML files in
[`crates/cleanse-core/rules/`](https://github.com/zhenga8533/cleanse/tree/main/crates/cleanse-core/rules),
embedded into the binary at build time. A rule only matches paths and describes what it found:
it can't run commands ([ADR 0003](adr/0003-declarative-rules.md)). Writing one doesn't require
Rust.

## A project artifact

```toml
[[rule]]
id = "python.venv"
name = "Python virtual environments"
category = "dependencies"
risk = "regenerable"
explain = "A virtual environment holding the project's installed packages. It can be recreated from the project's dependency list."
regenerate = "Recreate it with your tool, such as uv sync."

[rule.match]
kind = "project_artifact"
names = [".venv", "venv"]
markers = ["pyproject.toml", "requirements.txt", "uv.lock"]
contains = ["pyvenv.cfg"]

[[rule.test]]
tree = ["proj/pyproject.toml", "proj/.venv/pyvenv.cfg"]
matches = ["proj/.venv"]

[[rule.test]]
tree = ["scratch/.venv/pyvenv.cfg"]
matches = []
```

A directory matches when its name is in `names` and its evidence holds:

- **`markers`**: at least one of these files exists next to the directory. A pattern with `*`
  matches one file name (`*.csproj`); anything else is a path relative to the parent
  (`ProjectSettings/ProjectVersion.txt`).
- **`contains`**: every one of these files exists inside the directory.

A rule needs at least one of the two, because a directory name alone isn't evidence. Markers
and contained files must be regular files, not links. A `CACHEDIR.TAG` inside a match is always
recorded as evidence, since it's the standard way tools mark regenerable caches.

Some names are too common to trust even with a marker: plenty of projects keep hand-written
files in `build/`, `dist/`, or `bin/`. For those, set `require_git_ignored = true`, and the rule
matches only where the project's Git repository ignores the directory. Outside a repository it
never matches. Whenever a match is inside a repository, Cleanse also records whether Git ignores
it (as evidence) and whether Git tracks any files inside it (which the safety checks treat as a
reason to keep it). Repositories are read without running `git`
([ADR 0008](adr/0008-read-git-with-gix.md)).

Once a directory matches, Cleanse doesn't look inside it for more matches: everything in it is
part of that match.

## A known location

```toml
[rule.match]
kind = "known_location"
locations = [
    { os = "windows", base = "data_local", path = "pip/Cache" },
    { os = "macos", base = "cache", path = "pip" },
    { os = "linux", base = "cache", path = "pip" },
]
```

The location must be exactly this path under one of the user's standard folders:

| `base` | Windows | macOS | Linux |
|---|---|---|---|
| `home` | `%USERPROFILE%` | `~` | `~` |
| `cache` | `%LOCALAPPDATA%` | `~/Library/Caches` | `~/.cache` |
| `config` | `%APPDATA%` | `~/Library/Application Support` | `~/.config` |
| `data_local` | `%LOCALAPPDATA%` | `~/Library/Application Support` | `~/.local/share` |
| `temp` | `%TEMP%` | `$TMPDIR` | `/tmp` |

Locations match case-insensitively on Windows and macOS. Two rules can't claim the same
location.

A location inside a protected folder, such as an AI agent's, matches nothing removable unless
the protected policy in `cleanse-core/src/safety/policy.rs` lists it as an exception. Exceptions
are added only for folders the app's own documentation says can be deleted, and the rule file
says which page that is.

## An owner

```toml
[rule.owner]
app = "Claude Code"
processes = ["claude"]
```

While any of these programs is running, the rule's matches are kept. Program names are
lowercase and without `.exe`, and they're compared ignoring case, so `claude` matches
`claude.exe` on Windows and `Claude` on macOS. If the running programs can't be listed, the
matches are kept too.

## Fields

| Field | |
|---|---|
| `id` | Lowercase, two or more `.`-separated parts: `ecosystem.thing` |
| `name` | A short heading, like "Node.js dependencies" |
| `category` | `dependencies`, `build_output`, `package_caches`, `tool_caches`, `ai_agents`, `ai_models`, `git`, `temp`, `logs`, `browser_caches`, `downloads`, or `large_files` |
| `risk` | `safe` (the app recreates it on its own), `regenerable` (it takes a command, a download, or time), `review`, or `protected`. See the [safety model](safety-model.md) |
| `explain` | Why it's safe to remove, in a sentence or two |
| `regenerate` | Optional: how to get it back, when that takes a deliberate step |
| `owner` | Optional: the app that writes what the rule matches; see [an owner](#an-owner) |

Paths in rules are `/`-separated and relative. Cleanse rejects a rule file with `.` or `..`
segments, absolute paths, backslashes, colons (drive letters and NTFS alternate data streams),
or unknown fields.

## Tests

Every rule needs at least one test that matches something and one that matches nothing, and CI
fails otherwise. A test builds `tree` in an empty directory (a trailing `/` makes a directory;
anything else is an empty file), runs only that rule over it, and expects exactly `matches`.
Known locations resolve under folders named after their base (`home/`, `cache/`, `config/`,
`data_local/`, `temp/`), and a test can be limited to one OS with `os = "windows"`.

For rules that depend on Git, `git = true` makes the test's root a repository, `files` creates
files with contents (such as a `.gitignore`), and `tracked` lists files to add to the index:

```toml
[[rule.test]]
git = true
tree = ["web/package.json", "web/dist/app.js"]
files = { "web/.gitignore" = "dist/\n" }
tracked = ["web/package.json"]
matches = ["web/dist"]
```

```sh
cargo nextest run -p cleanse-core rules
```
