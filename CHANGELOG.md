# Changelog

All notable changes to Cleanse are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/zhenga8533/cleanse/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/zhenga8533/cleanse/releases/tag/v0.1.0
