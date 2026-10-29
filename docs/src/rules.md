# Rules

> Status: project artifacts and known locations are implemented. Git checks, AI agent rules,
> and insights come later in M1 and M2.

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

## Fields

| Field | |
|---|---|
| `id` | Lowercase, two or more `.`-separated parts: `ecosystem.thing` |
| `name` | A short heading, like "Node.js dependencies" |
| `category` | `dependencies`, `build_output`, `package_caches`, `tool_caches`, `ai_agents`, `ai_models`, `git`, `temp`, `logs`, `browser_caches`, `downloads`, or `large_files` |
| `risk` | `safe` (the app recreates it on its own), `regenerable` (it takes a command, a download, or time), `review`, or `protected`. See the [safety model](safety-model.md) |
| `explain` | Why it's safe to remove, in a sentence or two |
| `regenerate` | Optional: how to get it back, when that takes a deliberate step |

Paths in rules are `/`-separated and relative. Cleanse rejects a rule file with `.` or `..`
segments, absolute paths, backslashes, colons (drive letters and NTFS alternate data streams),
or unknown fields.

## Tests

Every rule needs at least one test that matches something and one that matches nothing, and CI
fails otherwise. A test builds `tree` in an empty directory (a trailing `/` makes a directory;
anything else is an empty file), runs only that rule over it, and expects exactly `matches`.
Known locations resolve under folders named after their base (`home/`, `cache/`, `config/`,
`data_local/`, `temp/`), and a test can be limited to one OS with `os = "windows"`.

```sh
cargo nextest run -p cleanse-core rules
```
