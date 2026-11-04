# Rules

> Status: project artifacts, known locations, large files, advice, and Git checks are
> implemented, including rules for AI agents' and browsers' folders, and system rules for
> `cleanse system`.

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
| `downloads` | Downloads, usually `%USERPROFILE%\Downloads` | `~/Downloads` | `XDG_DOWNLOAD_DIR`, unless it's the home folder |

In names after the first, `*` matches any run of characters, for folders whose names vary, like
a browser's profiles: `Google/Chrome/User Data/Profile */Cache` matches `Profile 1/Cache` but not
`Profile 1/Extensions/Cache`. The first name can't contain `*`, so a location stays inside one
app's folder. The exceptions are `temp` and `downloads`, which hold whatever anyone put there, so
`path = "*"` under `temp` is each item directly in the temp folder, and `path = "*.dmg"` under
`downloads` is each disk image directly in Downloads.

Locations name folders unless the rule says `entry = "file"`, which makes them name files
instead, or `entry = "any"`, which makes them name both. Each match is a separate item:

```toml
[rule.match]
kind = "known_location"
entry = "file"
locations = [
    { os = "windows", base = "data_local", path = "CrashDumps/*.dmp" },
]
```

A location only matches the kind of item it names, so a folder named `crash.dmp` isn't matched
here, and a file named like a folder location isn't matched there. Links are never matched, and
neither is what's left of a removal Cleanse didn't finish. Project artifacts are always folders.

Locations match case-insensitively on Windows and macOS. Two rules can't claim the same
location.

A location inside a protected folder, such as an AI agent's or a browser's, matches nothing
removable unless the protected policy in `cleanse-core/src/safety/policy.rs` lists it as an
exception. Exceptions are added only for folders the app's own documentation or source code
shows are disposable, and the rule file or the policy says where that is.

## Large files

```toml
[rule.match]
kind = "large_file"
```

A `large_file` rule matches any file that takes at least `large_file_size` from the settings on
disk (1 GiB unless changed), outside the folders apps keep their data in. It's checked after
every other rule, so a file another rule recognizes stays under that rule. There's one, built
in, and it must be `review`, since nothing about a large file says whether it's still needed. See
the [safety model](safety-model.md#review-items) for where it doesn't look.

## Advice

Some tools have their own command for cleaning what they keep, and it does better than deleting
would. pnpm's store is hard-linked into projects, so deleting it frees little, but
`pnpm store prune` removes just the packages no project uses. Go makes its module cache
read-only, so `go clean -modcache` is the way to remove it. A rule for one of these is
`protected`, so Cleanse never removes its matches, and names the command:

```toml
risk = "protected"
advise = "pnpm store prune"
```

`scan` and `analyze` list these items with the command, and `--json` has them as `advice`.
Cleanse only shows the command and never runs it ([ADR 0003](adr/0003-declarative-rules.md)).
Only `protected` rules can have `advise`, and it must be one line. An item a safety check keeps
for another reason, such as a folder excluded in the settings, is listed as kept with that
reason instead.

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

## Waiting until a match is idle

Some places hold files no one app owns, like the temp folder, so there's no program to check.
For those, `idle_days` keeps each match until nothing in it has changed for that many days, and
nothing since the computer last started:

```toml
idle_days = 7
```

A change is a file's contents being modified, or the entries of a folder inside changing, which
also shows when unpacked files arrived even though they keep their original dates. When files
were last opened doesn't count, since scanners open them without anything needing them. If when
the computer started can't be read, the matches are kept.

## System rules

System cleanup removes clutter from the operating system's own folders, such as the Windows
temp folder, and runs only as an administrator or root
([ADR 0010](adr/0010-elevated-system-cleanup.md)). Its rules are in
[`rules/system/`](https://github.com/zhenga8533/cleanse/tree/main/crates/cleanse-core/rules/system),
and each says so:

```toml
[[rule]]
id = "windows.system_temp"
scope = "system"

[rule.match]
kind = "known_location"
entry = "any"
locations = [{ os = "windows", base = "system_root", path = "Temp/*" }]
```

System rules and the user's rules never mix. The user's rules can't say `scope = "system"`, and
system cleanup loads only rules that do. A system rule is always a known location, under one of
the system's folders, which come from the operating system rather than environment variables:

| `base` | System | Usually |
|---|---|---|
| `system_root` | Windows | `C:\Windows` |
| `program_data` | Windows | `C:\ProgramData` |
| `var` | Linux | `/var` |

The user's rules can't use these bases, and system rules can't use the user's. A system rule's
location names one item directly inside a fixed folder, so `*` can appear only in its last name,
and not directly in the base folder: `Temp/*` and `Minidump/*.dmp` are allowed, but `*` and
`Temp/*/x` aren't. System cleanup doesn't read Git, so `git` in a system rule's test changes
nothing.

## Fields

| Field | |
|---|---|
| `id` | Lowercase, two or more `.`-separated parts: `ecosystem.thing` |
| `scope` | Optional: `system` for a [system rule](#system-rules) |
| `name` | A short heading, like "Node.js dependencies" |
| `category` | `dependencies`, `build_output`, `package_caches`, `tool_caches`, `ai_agents`, `ai_models`, `git`, `temp`, `logs`, `browser_caches`, `downloads`, or `large_files` |
| `risk` | `safe` (the app recreates it on its own), `regenerable` (it takes a command, a download, or time), `review`, or `protected`. See the [safety model](safety-model.md) |
| `explain` | Why it's safe to remove, in a sentence or two |
| `regenerate` | Optional: how to get it back, when that takes a deliberate step |
| `owner` | Optional: the app that writes what the rule matches; see [an owner](#an-owner) |
| `idle_days` | Optional: how many days a match must go unchanged; see [waiting until a match is idle](#waiting-until-a-match-is-idle) |
| `advise` | Optional, `protected` rules only: the command that cleans the match instead; see [advice](#advice) |

Paths in rules are `/`-separated and relative. Cleanse rejects a rule file with `.` or `..`
segments, absolute paths, backslashes, colons (drive letters and NTFS alternate data streams),
or unknown fields.

## Tests

Every rule needs at least one test that matches something and one that matches nothing, and CI
fails otherwise. A test builds `tree` in an empty directory (a trailing `/` makes a directory;
anything else is an empty file), runs only that rule over it, and expects exactly `matches`.
Known locations resolve under folders named after their base (`home/`, `cache/`, `config/`,
`data_local/`, `temp/`, `downloads/`, and for system rules `system_root/`, `program_data/`, and
`var/`), and a test can be limited to one OS with `os = "windows"`.

For rules that look at size, `sizes` creates files with that many bytes in them. Tests count
files taking 64 KiB or more on disk as large:

```toml
[[rule.test]]
tree = []
sizes = { "home/Videos/raw.mov" = 100_000, "home/small.mp4" = 10_000 }
matches = ["home/Videos/raw.mov"]
```

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
