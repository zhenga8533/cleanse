# Changelog

All notable changes to Cleanse are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- A Scoop bucket for the command line on Windows, `zhenga8533/scoop-bucket`: after
  `scoop bucket add zhenga8533 https://github.com/zhenga8533/scoop-bucket`,
  `scoop install zhenga8533/cleanse` installs it.

## [1.1.0] - 2026-11-06

Cleaning with UAC off. On Windows machines where User Account Control is turned off, everything
runs as an administrator, so Cleanse refused to clean anything there, and told you to open it
again without elevation, which couldn't be done. It now cleans your own files on those machines,
with every other check, and keeps other accounts' home folders out of reach. With UAC on, an
elevated Cleanse still refuses, as before.

### Added

- `cleanse doctor --json` has `always_elevated`, true where everything the account runs is
  elevated, as on Windows with UAC off.

### Changed

- On Windows with UAC off, where everything you run runs as an administrator, Cleanse cleans
  your own files instead of refusing to change anything. That includes `clean`, `apply`,
  `restore`, `quarantine purge`, and `history --finish`, and the desktop app's buttons for them.
  Every other check still applies. With UAC on, Cleanse still refuses while it runs elevated.
  `cleanse doctor` and the app's settings say which applies
  ([ADR 0011](https://github.com/zhenga8533/cleanse/blob/main/docs/src/adr/0011-cleanup-with-uac-off.md)).
- On Windows, other accounts' home folders are protected, since an administrator could change
  them. The desktop app's System section explains that it isn't available when UAC is off,
  instead of telling you to open the app again without elevation.

## [1.0.0] - 2026-11-05

System cleanup. Cleanse now cleans the operating system's own folders on Windows and Linux as well
as yours: Windows' temp folder, update downloads, error reports, and memory dumps, and Linux's
core dumps and crash reports. That's the only thing it does as an administrator or root, with
rules of its own, and it removes an item only if the item, and everything in it, belongs to the
system. In the desktop app, a separate helper does that work, and the app's window still never
runs as an administrator. For your own files, Cleanse now also keeps what belongs to other
accounts.

### Added

- `cleanse system` cleans the operating system's own folders, as an administrator or root:
  Windows' temp folder, Windows Update's downloads, sent error reports, and memory dumps, and
  on Linux, core dumps and Ubuntu's crash reports. For apt's, dnf's, pacman's, and Delivery
  Optimization's caches, and the systemd journal, it shows the command that cleans them. `scan`,
  `clean`, `explain`, `quarantine`, `restore`, `history`, and `rules` work as they do for your
  own files.
  - It has rules of its own, and reads no settings or plan files.
  - Its journal and quarantine are in a folder only administrators or root can open:
    `ProgramData\Cleanse` or `/var/lib/cleanse`.
  - It looks only directly in the folders its rules name, and removes an item only if it, and
    everything in it, belongs to the system.
  - Without elevation it says how to run it, and on macOS that it's for Windows and Linux.
    `cleanse doctor` says how to run it, and where its folders are.
- The desktop app's System section does system cleanup too: scan, clean, quarantine, history,
  and rules, on the same screens as for your own files. The app's window still never runs as an
  administrator. A separate helper does the work, which the MSI and the deb install where only
  administrators or root can change it.
  - It asks for administrator rights when it starts, through the UAC prompt on Windows or
    polkit's password prompt on Linux, and again after ten minutes without use.
  - It accepts only the app that started it, and only the IDs of what its own scan found.
  - The AppImage doesn't have it, and points to `cleanse system` instead. There's no System
    section on macOS.

### Changed

- Items that belong to another account or to the operating system are kept, as are items whose
  owner can't be read. Before, they were offered like any other. On Windows, items the
  Administrators group owns still count as yours, since that's who owns what you make in an
  elevated program. Drives that don't record owners, such as FAT and exFAT, aren't affected.

## [0.5.0] - 2026-11-03

The desktop app. Cleanse now comes as an app for Windows, macOS, and Linux as well as a command
line, and it does everything the command line does. You can scan and clean with a confirmation,
see where space goes in a treemap, restore or purge what's in quarantine, and look through the
history, settings, and rules. As with the command line, nothing is removed without your approval,
and each item is checked again right before it's removed. The app's window can call only
Cleanse's own commands, and none of them takes a path to remove.

### Added

- The desktop app, with installers on each release:
  - an MSI for Windows;
  - a universal disk image for macOS, for Apple silicon and Intel;
  - an AppImage and a deb for Linux, which need glibc 2.35 or later and WebKitGTK 4.1.

  Each has a `.sha256` checksum and build provenance. They aren't signed with a publisher's
  certificate yet, so Windows and macOS ask you to confirm the first time you open the app, and
  the README says how. The macOS app is ad-hoc signed, so Apple silicon opens it once you approve
  it.
- **Scan:**
  - Scan your home folder, or folders you choose with the system's folder picker, with live
    progress and a way to cancel.
  - Results come in the command line's sections, grouped by rule, and each item opens to what
    `cleanse explain` says about it.
  - A cleanup starts from what `cleanse clean` would select, and says what will be deleted and
    what quarantined before anything happens. Afterwards it reports what was skipped and why.
  - Cleaning, restoring, purging, and finishing are off while the app runs as an administrator
    or root.
- **Disk usage:** a folder's largest entries in a sortable table, with how much under each can be
  removed, and a treemap of the same folder, opened folder by folder. A filter limits what counts
  as removable to the categories you choose, as `--category` does.
- **Quarantine:** what cleanups set aside, to restore, or to purge after a confirmation.
- **History:** past runs, described as `cleanse history` describes them. Removals that didn't
  finish can be completed with a button.
- **Settings:** what `cleanse doctor` reports, and every rule in detail, as `cleanse rules show`
  gives it.
- **Security:** the app's window can call only Cleanse's own commands, and nothing from Tauri or
  any plugin. It shows only the app's own content, and names everything by an ID the engine gave
  out, so it can never name a path to remove
  ([ADR 0009](https://github.com/zhenga8533/cleanse/blob/main/docs/src/adr/0009-desktop-web-view.md)).

## [0.3.0] - 2026-11-02

Files, disk analysis, and judgment calls. Cleanse now removes files as well as folders, which
brings rules for temp files, developer tools' logs, and crash dumps. `cleanse analyze` shows where
space goes and how much of it can be removed. Large files and installers in Downloads are offered
for your review, with how long ago each was modified, and tools with their own cleanup commands,
like pnpm and Go, get pointed to them instead. Every command that reports something can give it
as JSON, and shells can complete the command line.

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
- A rule for the temp folder on Windows. Each file and folder programs left in `%TEMP%` is Safe
  to remove once nothing in it has changed for 7 days, and nothing since the computer last
  started, when every program that could still need it was stopped. Rules can wait for a match
  to go idle like this with `idle_days`, name files and folders alike with `entry = "any"`, and
  match everything directly in the temp folder with `path = "*"`.
- `cleanse analyze [PATHS]` shows where space goes: each folder's size, largest first, down to
  `--depth` levels and `--top` entries per folder, with how much under each can be removed. It
  then breaks down what can be removed by tier and category. `--json` gives the same as data.
- Large files. Any file taking 1 GiB or more on disk is offered as a Review item, never selected
  by default and quarantined rather than deleted. `large_file_size` in the settings changes the
  size, down to 64 MiB. Folders apps keep their data in are skipped, as is anything with a name
  starting with `.` in its path, and large files where nothing may be removed, such as in a
  folder you excluded, aren't listed. One named like a database, a key, or Outlook's mail (`.pst`)
  is kept. Since quarantine needs the same drive, large files on other drives can't be removed
  yet.
- A rule for installers and disk images directly in Downloads: `.exe`, `.msi`, `.msix`, and
  `.iso` on Windows, `.dmg`, `.pkg`, and `.iso` on macOS, and `.deb`, `.rpm`, `.AppImage`, and
  `.iso` on Linux. They're Review items too. Rules can match by pattern directly in Downloads with
  `base = "downloads"`.
- `scan` and `clean` show how long ago each Review item was modified, to help decide, and
  `explain` shows it for everything. `scan` lists Review items under their rules, the largest 10
  of each unless `--all` is given. Items in `--json` have a `modified` time, in seconds since the
  Unix epoch.
- Windows' paging, hibernation, and swap files are protected.
- Advice for what a tool cleans better itself. pnpm's store and Go's module cache are kept, and
  `scan` and `analyze` list them with the command to run: `pnpm store prune`, which removes only
  the packages no project uses, and `go clean -modcache`, since Go makes its cache read-only.
  `--json` has them as `advice`. Rules name such a command with `advise`, which Cleanse only
  shows and never runs. A store or cache moved with `PNPM_HOME`, `GOPATH`, or `GOMODCACHE` isn't
  found yet.
- `--json` on `history`, `rules` (and `rules show`), `doctor`, and `clean --dry-run`, so every
  command that reports something can give it as data. Rules come out with the same fields their
  files have, without their tests.
- `--category` on `scan`, `clean`, and `analyze` keeps only items in the categories given, like
  `--category temp,logs`. In `analyze`, folder sizes still count everything, and only what can be
  removed is limited.
- `cleanse completions <shell>` prints completions for Bash, Zsh, Fish, PowerShell, or Elvish.

### Fixed

- On Windows, the temp folder is now found by its long names, even when `%TEMP%` uses 8.3 short
  names such as `C:\Users\RUNNER~1`, so locations under it match.
- Scanning a folder twice, such as `C:\Users\me` and `c:\users\me`, or a folder and one inside
  it, no longer lists items twice.

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

[Unreleased]: https://github.com/zhenga8533/cleanse/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/zhenga8533/cleanse/releases/tag/v1.1.0
[1.0.0]: https://github.com/zhenga8533/cleanse/releases/tag/v1.0.0
[0.5.0]: https://github.com/zhenga8533/cleanse/releases/tag/v0.5.0
[0.3.0]: https://github.com/zhenga8533/cleanse/releases/tag/v0.3.0
[0.2.0]: https://github.com/zhenga8533/cleanse/releases/tag/v0.2.0
[0.1.0]: https://github.com/zhenga8533/cleanse/releases/tag/v0.1.0
