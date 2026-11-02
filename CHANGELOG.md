# Changelog

All notable changes to Cleanse are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cleanse can remove files as well as folders. A known location in a rule can name files with
  `entry = "file"`, and each file it matches is checked, deleted, quarantined, restored, and
  purged the way a folder is. Plans, the journal, and quarantine records now say whether each
  item is a file or a folder; ones saved by 0.2 are read as folders, which is all they held.
- Rules for crash dumps: the memory dumps Windows Error Reporting saves in
  `%LOCALAPPDATA%\CrashDumps` once local dumps are turned on, and macOS crash reports in
  `~/Library/Logs/DiagnosticReports`. They're Review items, never selected by default and
  quarantined rather than deleted, since only you know whether you still need one to debug or
  report a crash.
- Rules for developer tools' logs: npm's debug logs, VS Code's session logs, JetBrains IDEs'
  logs, and Gradle daemon logs. They're Safe, since each tool writes new ones as it runs. VS
  Code's are kept while it's running, and any log modified in the last few minutes is kept as in
  use.

## [0.2.0] - 2026-11-01

Cleanup and recovery. Cleanse now removes what you approve: it deletes what comes back on its
own or with a rebuild, quarantines what needs your judgment so you can restore it, and records
every run in a journal. It still asks before removing anything, checks each item again right
before touching it, and removes nothing while running as an administrator or root.

### Added

- `cleanse clean [PATHS]` deletes what you choose from a scan. It lists what can be deleted,
  with items that have warnings unselected, and asks again before deleting anything. Each item
  is checked again right before it's deleted and skipped if anything about it changed.
  `--yes --max-risk TIER` deletes the preselected items without asking, and `--dry-run` lists
  what that would delete. Every run is recorded in a journal in Cleanse's data folder, and
  nothing is deleted while running as an administrator or root.
- Quarantine. Review items that `clean` removes are moved into Cleanse's data folder instead of
  deleted, if they're on the same drive. `cleanse quarantine` lists what's there, `cleanse restore
  ID` puts an item back if nothing has taken its place, and `cleanse quarantine purge` deletes
  items for good, by ID, `--all`, or `--older-than DAYS`. Review items are never preselected.
- Rules for Claude Code's data in `~/.claude`, limited to the folders its documentation says
  can be deleted. Logs, caches, legacy folders, and leftovers from crashed sessions are Safe.
  Checkpoints, pastes, plans, `/insights` reports, settings backups, and removed skills and
  plugins are Review, so they go to quarantine. Transcripts, memory, prompt history, tasks,
  settings, plugins, and credentials stay protected. Nothing is removed while Claude Code or
  Claude Desktop is running.
- Rules for browser caches: Chrome's and Firefox's on every OS, and Edge's on Windows. They match
  only the caches in each profile, which the browser fills again as you browse, and nothing is
  removed while the browser is running, even in the background. The rest of Chrome's,
  Chromium's, Edge's, and Firefox's profiles is protected: history, cookies, saved passwords,
  extensions, and sites' stored data. Known locations in rules can now use `*` in folder names
  after the first.
- Saved plans. `cleanse scan --save plan.json` writes what can be removed to a JSON file, with
  only what `clean` would preselect selected. Review it, change `selected` on any item, then
  `cleanse apply plan.json` removes the selected items, re-checking each one as `clean` does.
  `--yes` skips the question. Editing a plan can't remove anything the rules and safety checks
  wouldn't, and a Review item the plan says to delete is skipped, since Review items are only
  quarantined. Saving replaces an existing file only if it's a plan Cleanse saved.
- `cleanse history` lists what each run removed, quarantined, restored, or purged. If a
  removal stopped partway, it shows what's left, and `cleanse history --finish` removes it once
  it's confirmed to be the folder that was approved.
- A settings file, `cleanse/config.toml` in your settings folder. `exclude` lists folders Cleanse
  never touches, and `disabled_rules` turns rules off. A mistake in it, such as a relative path or
  an unknown rule, stops Cleanse with a message naming the file. `cleanse doctor` shows where it
  is and what it says.

## [0.1.0] - 2026-10-30

The read-only preview. Cleanse finds what could be removed and explains why, on Windows, macOS,
and Linux. It never removes anything; cleanup arrives in 0.2.

### Added

- `cleanse scan [PATHS]` lists what could be removed, grouped by rule and largest first, and
  what's kept and why (`--show-protected`). It scans your home folder when no folders are
  given. `--all` lists every item and `--json` gives machine-readable output.
- `cleanse explain PATH` shows whether a file or folder could be removed: the rule that
  recognizes it or the folder containing it, the evidence, how it comes back, its size, and
  every safety concern.
- `cleanse rules` lists the 29 built-in rules, and `cleanse rules show ID` describes one.
- `cleanse doctor` shows the folders, rules, and capabilities Cleanse sees on this system.
- Rules for dependencies and build output in JavaScript, Python, Rust, Java (Maven and Gradle),
  .NET, CMake, Zig, Swift, Dart, Elixir, Haskell, Terraform, Godot, and Unity, and for the npm,
  pip, Yarn, Cargo, NuGet, Homebrew, and Go caches. Ambiguous folders such as `dist/`,
  `build/`, and `bin/` match only when Git ignores them.
- Safety checks that keep anything protected (system and user folders, credentials, editor
  extensions, installed apps, AI agent data), anything Git tracks, anything recently modified,
  cloud-synced folders, folders containing possible secrets or virtual machine disks, and
  anything that couldn't be fully read.

[Unreleased]: https://github.com/zhenga8533/cleanse/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/zhenga8533/cleanse/releases/tag/v0.2.0
[0.1.0]: https://github.com/zhenga8533/cleanse/releases/tag/v0.1.0
