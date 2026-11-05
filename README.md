# Cleanse

Find and safely remove the files you don't need, with an explanation for every item.

Cleanse is a cross-platform (Windows, macOS, Linux) disk cleanup tool built with developers in
mind. It looks for regenerable build output, package-manager caches, AI coding agent leftovers,
and everyday junk; tells you why each item is safe to remove and how it comes back; and never
removes anything you haven't approved.

> **Status: v1.0.** The command line and the desktop app are complete: `cleanse scan` and
> `cleanse analyze` find what could be removed and where space goes, `cleanse explain` says why,
> and `cleanse clean` deletes what you approve, or quarantines it so `cleanse restore` can put it
> back. [`cleanse system`](#system-cleanup) does the same for the operating system's own folders
> on Windows and Linux, and the [desktop app](#desktop-app) does all of it in a window.
> [v1.0.0](https://github.com/zhenga8533/cleanse/releases/tag/v1.0.0) has prebuilt binaries and
> installers for both, not signed.

## Install

Each [release](https://github.com/zhenga8533/cleanse/releases/latest) has the command line and the
desktop app for Windows, macOS, and Linux. Every file has a `.sha256` checksum and build
provenance, and [Checking a release](docs/src/releasing.md#checking-a-release) shows how to check
them.

### Command line

On macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/zhenga8533/cleanse/releases/latest/download/cleanse-cli-installer.sh | sh
```

On Windows, in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/zhenga8533/cleanse/releases/latest/download/cleanse-cli-installer.ps1 | iex"
```

Both install the latest release's `cleanse` in Cargo's `bin` folder, `~/.cargo/bin` unless you've
set `CARGO_HOME`, and add that folder to your `PATH`. Each release also has the binary in an
archive for x64 Windows, Intel and Apple silicon Macs, and x64 and ARM64 Linux, or you can
[build it from source](#building-from-source).

On macOS and Linux, Homebrew can install it from Cleanse's own tap, and `brew upgrade` updates
it:

```sh
brew install zhenga8533/tap/cleanse-cli
```

Before using [`cleanse system`](#system-cleanup), install a copy that only administrators or root
can change, since the installers and Homebrew put `cleanse` in a folder your own account can
change. Download the archive for your system from the release and check it, then on Linux
(`aarch64` in place of `x86_64` on ARM64):

```sh
tar -xf cleanse-cli-x86_64-unknown-linux-gnu.tar.xz
sudo install -m 755 cleanse-cli-x86_64-unknown-linux-gnu/cleanse /usr/local/bin/
sudo /usr/local/bin/cleanse system scan
```

On Windows, in a terminal opened with Run as administrator, into a folder of its own, since
`Program Files\Cleanse` is the desktop app's:

```powershell
Expand-Archive -Force cleanse-cli-x86_64-pc-windows-msvc.zip "$env:ProgramFiles\Cleanse CLI"
& "$env:ProgramFiles\Cleanse CLI\cleanse.exe" system scan
```

Running that copy by its full path, as these do, means no other `cleanse` on your path runs in its
place.

### Desktop app

The desktop app does what the command line does, in a window: scan and clean, see where space
goes, and manage the quarantine, history, and settings. Releases from v0.5.0 on include its
installers:

| System | File | Installing it |
|---|---|---|
| Windows 10 and 11 | `Cleanse_<version>_x64_en-US.msi` | Run it. |
| macOS, Apple silicon and Intel | `Cleanse_<version>_universal.dmg` | Open it and drag Cleanse to Applications. |
| Linux | `Cleanse_<version>_amd64.AppImage` | `chmod +x` it, then run it. It needs FUSE 2 (`libfuse2`). |
| Debian and Ubuntu | `Cleanse_<version>_amd64.deb` | `sudo apt install ./Cleanse_<version>_amd64.deb` |

The Linux builds need glibc 2.35 or later (Ubuntu 22.04, Debian 12) and WebKitGTK 4.1.

The installers aren't signed with a publisher's certificate, so the first time you open Cleanse,
your system asks you to confirm it:

- **Windows:** SmartScreen says "Windows protected your PC". Choose **More info**, then **Run
  anyway**. If Smart App Control is on, it blocks unsigned programs outright, the command line
  included, and offers no way to allow them.
- **macOS:** the app is ad-hoc signed, so macOS opens it once you approve it. After the first
  attempt to open it, go to **System Settings → Privacy & Security** and choose **Open Anyway**.

## Try it

```sh
cleanse scan ~/code               # what could be removed, grouped by rule, largest first
cleanse scan --show-protected     # also what's kept and why (defaults to your home folder)
cleanse analyze ~/code            # where space goes, and how much under each folder can be removed
cleanse explain ~/code/app/node_modules
cleanse clean ~/code              # choose what to delete, then confirm; each item is re-checked first
cleanse clean --dry-run ~/code    # what `clean --yes --max-risk regenerable` would delete
cleanse scan ~/code --save plan.json  # save a plan to review and edit
cleanse apply plan.json           # remove its selected items, re-checking each first
cleanse quarantine                # what's in quarantine; `quarantine purge` deletes it for good
cleanse restore ID                # put a quarantined item back
cleanse history                   # what Cleanse has removed, restored, and purged
cleanse rules                     # every rule; `cleanse rules show <id>` for one
cleanse doctor                    # folders, rules, and capabilities on this system
cleanse scan --category temp,logs # only some categories; also for `clean` and `analyze`
```

Add `--json` for machine-readable output to `scan`, `analyze`, `explain`, `clean --dry-run`,
`quarantine`, `history`, `rules`, and `doctor`.

`cleanse completions <shell>` prints completions for Bash, Zsh, Fish, PowerShell, or Elvish. For
example, add this to your PowerShell profile:

```powershell
cleanse completions powershell | Out-String | Invoke-Expression
```

### Settings

To keep Cleanse away from some folders, or turn rules off, create `cleanse/config.toml` in your
settings folder (`%APPDATA%` on Windows, `~/Library/Application Support` on macOS, `~/.config`
on Linux). `cleanse doctor` shows where it is and what it says.

```toml
# Never touched: these folders, everything inside them, and anything containing them.
exclude = ["~/work/vendor-snapshot", "D:/archive"]
# Rule IDs from `cleanse rules`.
disabled_rules = ["unity.library"]
# Files this large or larger are offered for review: MiB, GiB, or TiB, at least 64 MiB.
large_file_size = "1 GiB"
```

## System cleanup

`cleanse system` cleans the operating system's own folders, which only an administrator or root
can: Windows' temp folder, Windows Update's downloads, sent error reports, and memory dumps, and
on Linux, core dumps and Ubuntu's crash reports. For package caches, Delivery Optimization's
cache, and the systemd journal, it shows the command that cleans them.

```sh
sudo cleanse system scan     # on Windows, without sudo, in a terminal opened with Run as administrator
sudo cleanse system clean    # choose what to remove, then confirm; each item is checked again first
cleanse system rules         # what it looks for, which needs no elevation
```

`explain`, `quarantine`, `restore`, and `history` work under `cleanse system` as they do for your
own files. System cleanup is kept apart from them
([ADR 0010](docs/src/adr/0010-elevated-system-cleanup.md)):

- It has rules of its own, and reads no settings or plan files.
- Its journal and quarantine are in a folder only administrators or root can open:
  `ProgramData\Cleanse` on Windows, `/var/lib/cleanse` on Linux.
- It looks only directly inside the folders its rules name.
- It removes an item only if the item, and everything in it, belongs to the system.

It isn't available on macOS.

The [desktop app](#desktop-app)'s System section does the same, through the same screens as for
your own files. The app itself never runs as an administrator. A separate part of it does system
cleanup, and asks for administrator rights when it starts, through the UAC prompt on Windows or
polkit's password prompt on Linux. Only the MSI and the deb install it, where only administrators
or root can change it, so the AppImage has no System section and points to `cleanse system`.

`sudo` and an administrator terminal run whichever `cleanse` comes first on your path, with full
control. If yours is in a folder your own account can change, as the shell installer's is,
anything you run could replace it. So for system cleanup, use a copy that only administrators or
root can change, such as one in `/usr/local/bin` or `C:\Program Files`, as
[Install](#command-line) shows.

## What makes it different

- **It explains itself.** Every item carries the rule that matched it, the evidence (for
  example "`Cargo.toml` alongside and a `CACHEDIR.TAG` inside"), and how to get it back.
  `cleanse explain <path>` answers why something is, or isn't, removable.
- **Git-aware.** Files tracked by git are never candidates, and projects with uncommitted work
  are flagged.
- **Honest numbers.** Guaranteed savings are reported separately from estimates (pnpm
  hardlinks, copy-on-write clones, cloud placeholders), and the measured free-space change is
  shown after every cleanup.
- **Knows AI coding agents.** Built around the documented data layouts of tools like Claude Code
  and Codex: verified disposable files can be cleaned, while conversations, memory, settings,
  and credentials are protected.
- **Rules are data.** Cleanup rules are declarative TOML with fixture tests. They can't run
  commands, and writing one doesn't require Rust.
- **Private.** No telemetry, and the engine has no network access.

## Safety promise

- Scanning is read-only.
- Nothing is removed without your explicit approval, and each approved item is re-checked
  immediately before it's touched.
- Age alone is never a reason to remove something.
- Protected locations (system folders, credentials, cloud-synced folders, agent history,
  git-tracked files) can't be overridden by any rule.
- Items that need judgment go to a quarantine you can restore from. Regenerable items such as
  `node_modules` are deleted, because rebuilding them is the recovery.
- Every cleanup supports `--dry-run`.

## What it will clean

| Area | Examples | Default tier |
|---|---|---|
| Project build output | `node_modules`, `target/`, `.venv`, `.next`, `bin/` + `obj/`, `.gradle` | Regenerable |
| Package-manager caches | npm, Yarn, pip, uv, Cargo, NuGet, Homebrew | Safe or Regenerable |
| AI coding agents | Claude Code debug logs and legacy caches, Codex logs | Safe; history and config are Protected |
| General | Temp files, app logs, crash dumps, browser caches (browser closed) | Safe |
| Needs your judgment | Large files, installers in Downloads, stale git worktrees | Review, never preselected |

Where a native tool is the right answer, Cleanse points you to it instead: `pnpm store prune`,
`go clean -modcache`, compacting WSL and Docker disk images, and so on.

## Roadmap

| Milestone | Scope |
|---|---|
| M0 | Repository foundation |
| M1 · v0.1 | Read-only engine and CLI: `scan`, `explain`, rules, safety checks |
| M2 · v0.2 | Cleanup with quarantine, restore, a journal, and saved plans; AI agent and browser cache rules |
| M3 · v0.3 | Removing files as well as folders, for temp files, logs, and crash dumps; disk analysis; large files and installers for review; advice for tools that clean themselves; CLI feature-complete |
| M4 · v0.5 | Desktop app (Tauri + React): scan and clean, disk usage with a treemap, quarantine, history, and settings; installers for Windows, macOS, and Linux |
| M5 · v1.0 | System cleanup on Windows and Linux, as an administrator or root, in the command line and the desktop app; keeping what belongs to other accounts |
| After 1.0 | crates.io, Homebrew, Scoop, and winget packages. Signed builds: until then, Windows builds are unsigned and macOS builds are ad-hoc signed. |

The design is described in the [architecture](docs/src/architecture.md) and
[safety model](docs/src/safety-model.md) docs.

## Building from source

You need Rust (via [rustup](https://rustup.rs)) and a C toolchain; on Windows that's Visual
Studio Build Tools with the "Desktop development with C++" workload. Then, in the repository:

```sh
rustup toolchain install          # installs the version pinned in rust-toolchain.toml
cargo build --release             # the binary is target/release/cleanse
cargo xtask ci                    # runs the same checks as CI
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Rule requests for tools Cleanse doesn't know yet are
especially welcome.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
