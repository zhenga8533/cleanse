# Safety model

> Status: the gates are implemented (see [what's not covered yet](#not-covered-yet)).
> `cleanse clean` and `cleanse apply` delete or quarantine approved items. The trash isn't built
> yet.

Cleanse's job is to remove files, so its first job is to not remove the wrong ones. The
principles:

- Scanning is read-only.
- Nothing is removed without explicit approval.
- Every item says why it's removable and how it comes back.
- Age alone is never a reason to remove anything.
- Unknown means keep. Inside sensitive locations, such as AI agent directories, only entries a
  rule explicitly recognizes are candidates.

## Risk tiers

| Tier | Meaning | Examples | Default action |
|---|---|---|---|
| **Safe** | The owning app recreates it silently | Temp files, logs, HTTP caches | Delete |
| **Regenerable** | Recreatable at a cost: time, bandwidth, a command | `node_modules`, `target/`, virtualenvs, model caches | Delete |
| **Review** | Might matter; needs a person to decide | Large files, old downloads, stale worktrees, agent checkpoints | Quarantine; never preselected |
| **Protected** | Shown for transparency, never actionable | Agent transcripts, credentials, git-tracked files, caches a tool cleans itself | None, or the tool's own command to run |

## Safety gates

Every candidate passes these gates when the plan is built and again immediately before it is
executed. A gate can allow an item, add a warning (the item keeps its tier but isn't selected
by default), or block it (the item becomes Protected). Only uncommitted changes, recent project
activity, and data shared through hard links or clones are warnings; every other concern
blocks, including any new kind of concern until it's deliberately classified.

1. **Protected policy.** System directories, volume roots, the home folder and its top-level
   folders, SSH and GPG keys, credential stores, `.git` internals, browser profiles (only their
   caches are eligible), AI agent history, settings and credentials, VM and WSL disk images,
   Windows' paging, hibernation, and swap files, and the inside of macOS bundles. No candidate
   may be, or contain, a protected path. Paths are
   compared by file identity as well as case-folded text, which defeats 8.3 short names, case
   and Unicode differences, and firmlinks. Rules can't override this policy. Folders excluded in
   the settings file are protected the same way, with everything inside them. Inside an AI
   agent's folder, the only exceptions are the folders the agent's own documentation says can be
   deleted. Inside a browser's, they're each profile's HTTP cache and compiled-script cache in
   Chrome and Edge, and `cache2` in Firefox, as Chromium's and Firefox's source code name them;
   sites' stored data, such as `Service Worker/CacheStorage`, stays protected. Exceptions are
   listed with the safety checks rather than in rules. An exception lifts only that one
   protection, so a folder the user excluded stays excluded.
2. **Containment.** A candidate must be inside a root the user chose and inside the base its
   rule declares.
3. **Links and mounts.** Links, junctions, and other reparse points are never followed;
   removing a link removes only the link. Scans don't cross into other filesystems.
4. **Content.** Project artifacts must contain no git-tracked files. Folders with ambiguous names
   (`build/`, `dist/`, `out/`, `bin/`) also need ecosystem evidence and are blocked if they
   contain files that look like secrets. A large file could be anything, so one named like a
   secret, a database, or Outlook's mail (`.pst`) is blocked too. Anything Cleanse couldn't
   fully read is blocked rather than assumed empty.
5. **Activity.** Recently modified items, and items whose owning app, agent, or browser is
   running, are blocked as in use. A rule names its owner's programs; if the running programs
   can't be listed, its matches are blocked too. Where no app owns what a rule matches, as in
   the temp folder, the rule can instead keep each match until nothing in it has changed for a
   number of days and nothing since the computer last started, when every program that could
   be relying on it was stopped. If the start time can't be read, those matches are blocked.
6. **Cloud sync.** OneDrive, iCloud Drive, Dropbox, and Google Drive folders are protected by
   default because deletions sync to the cloud and other devices. Placeholder file contents are
   never read, since that would download them.
7. **Ownership and elevation.** An item must belong to the account Cleanse runs as, or on
   Windows to the Administrators group, which owns what that account's elevated programs make.
   Items another account or the operating system owns are blocked, and so are items whose owner
   can't be read. Drives that don't record owners, such as FAT and exFAT, pass this gate. On
   Windows, other accounts' home folders are protected locations too. Cleanse refuses to change
   anything while running as an administrator or root if the account could run it without.
   With UAC off, everything the account runs is elevated, so it doesn't refuse
   ([ADR 0011](adr/0011-cleanup-with-uac-off.md)).

Every match is measured in one walk that doesn't follow links: its size, file count, newest
modification time, and anything inside that couldn't be read, is another filesystem, looks like
a secret, or is a virtual machine disk. A file modified in the last ten minutes counts as in use.

### Review items

Large files and installers in Downloads are Review items: nothing says what they are or whether
anyone still needs them. `scan` and `clean` show how long ago each was modified, to help decide,
but age is never a reason on its own, and they're never selected by default.

A large file is only a match because of its size, so the rule is narrow about where it looks.
Size means space on disk, so cloud files that aren't downloaded never count. Folders apps keep
their data in are skipped, since rules for those folders say what's safe there: the app data,
cache, settings, and temp folders (all of `AppData` on Windows, `~/Library` on macOS, `~/snap` on
Linux), and anything with a name starting with `.` in its path, apart from the folders the home
folder is in. Large files
where nothing may be removed, such as in Windows' folders or a folder excluded in the settings,
aren't listed at all,
and a file another rule recognizes, such as a crash dump, stays under that rule. Before removal,
a large file is measured again the same way, against the size in the settings then.

### Not covered yet

- **Owners on network shares and in WSL** are whatever the server reports, which may not be the
  account Cleanse runs as, so items there may be kept as another account's.
- **Owners started another way**: programs are recognized by name, so Claude Code installed
  with npm, which runs as `node`, isn't recognized as running. Recent modification still
  blocks folders it's writing to.
- **Relocated agent folders**: if `CLAUDE_CONFIG_DIR` moves Claude Code's data out of
  `~/.claude`, that folder isn't protected or cleaned.
- **Other browsers**: Chrome's, Chromium's, and Firefox's profiles are protected, and Edge's on
  Windows. Other browsers' profiles, such as Brave's and Safari's, and Edge's on macOS and Linux,
  aren't recognized yet, and no browser rule matches inside them. Neither are profiles kept
  somewhere else, such as Chrome started with `--user-data-dir`.
- **Uncommitted changes** and **recent project activity** warnings: need a working-tree scan.
- **Sizes on Windows** come from the directory listing, which has no allocation data. They count
  each file's length, or nothing for cloud files that aren't downloaded. Compression isn't
  reflected, and hard links are only noticed in pnpm's store and for items that are single files.

## System cleanup

> Status: implemented in the command line, as `cleanse system`. The desktop app's comes next.

System cleanup removes clutter from the operating system's own folders, such as the Windows temp
folder, and runs only as an administrator or root
([ADR 0010](adr/0010-elevated-system-cleanup.md)). Anyone can create files in some of those
folders, so it never mixes with the user's cleanup, and adds gates of its own:

- **Its own rules.** It uses only [system rules](rules.md#system-rules), which name items directly
  inside fixed system folders.
- **Its own protected policy.** The Windows folder, `ProgramData`, and `/var` are protected
  whole. The only exceptions are the items system rules name, never the folders holding them:
  what's in `C:\Windows\Temp` can be removed, but not `C:\Windows\Temp` itself. The exceptions are
  listed with the safety checks, and a test keeps them the same as the rules' locations. A path
  through `..` never reaches an exception.
- **Checked folders.** Each folder items are removed from, and every folder from its base down to
  it, must be a real folder, not a link, that the system owns: SYSTEM, a service's account, or
  Administrators on Windows, and root on Linux. An item is removed only from directly inside one,
  and only while that folder is still the one that was checked. The folders come from the
  operating system, never from environment variables.
- **The system's throughout.** The item must belong to the system, and so must everything inside
  it. A link or special file inside, or a file with other hard links, keeps it too. A scan
  checks the item and what folder listings show: links inside, and hard links on Linux. Who owns
  each entry inside, and hard links on Windows, take opening every entry, which can take minutes
  for an update with hundreds of thousands of files, so they're checked right before removal.
- **No Git.** It doesn't read Git, since anyone can create a repository above a system folder.
- **Advice.** A rule that only advises a command, such as `apt-get clean`, isn't checked this
  way, since nothing it matches is removed.
- **Its own data folder.** Its journal and quarantine are in `ProgramData\Cleanse` on Windows and
  `/var/lib/cleanse` on Linux, apart from the user's. Anyone can create folders in `ProgramData`,
  so the folder is created with permissions that let only Administrators and SYSTEM, or root, in,
  and checked before every run: a real folder, owned by one of them, that no one else can open,
  inside folders no one else can change. One someone else made first is refused, never repaired.
- **What it reads.** No settings file, no Git, and no plan files. A plan's roots are ignored, so
  only its items count, each checked again before it's touched.
- **Putting back and finishing.** An item is restored, or a removal that stopped is finished,
  only where a system rule names one, directly inside a system folder that checks out, whatever
  its record says.

The other gates apply as they do to the user's cleanup: activity, owner programs, idle days,
contents that couldn't be read, and other filesystems mounted inside.

## Execution and recovery

- Nothing is removed while Cleanse runs as an administrator or root and the account could run
  it without, or if it can't start a journal. On Windows with UAC off, there's no way to run it
  without, so cleanup goes ahead with every other check
  ([ADR 0011](adr/0011-cleanup-with-uac-off.md)).
- Before acting, each item is re-checked: it still exists, it's still the same file or folder
  (by identity, not just path), it's still not a link, its rule still recognizes it, and it still
  passes every gate. It's also skipped if its tier has risen, or a new kind of warning has
  appeared, since it was approved. Anything modified after the run starts counts as modified
  moments ago.
- **Saved plans.** `cleanse scan --save` writes what can be removed to a plan file, selecting
  only what `clean` would preselect, so Review items and items with warnings start unselected.
  `cleanse apply` removes the selected items with the same re-checks. The file can be edited,
  so nothing in it is trusted: an item is removed only if a rule still recognizes it at that
  path, with that identity, inside the plan's roots, and every gate passes. Lowering an item's
  tier in the file makes it skipped as risen, and a Review item the plan says to delete is
  skipped, since Review items are only quarantined. Paths must be full paths. Saving replaces an
  existing file only if it's a plan Cleanse saved.
- **Delete** first renames the item, a file or a folder, to a sibling tombstone, so a project
  never sees a half-deleted `node_modules`. If what landed there isn't the approved item,
  because something replaced it after the re-check, it's put back untouched. Otherwise the
  tombstone is removed, along with any links inside, never what they point to. On Windows the
  rename fails while a program has the file, or a file inside the folder, open, so nothing is
  removed. A program can open a file in a way that allows deleting it, and then, as on macOS and
  Linux, the file disappears but its space isn't freed until the program closes it.
- **Quarantine** moves the item into the `quarantine` folder in Cleanse's data folder, with a
  record of where it came from written first. It's a rename, so it only works on the same drive;
  items elsewhere are skipped rather than copied. If what moved isn't the approved item, it's
  put back. `cleanse restore` moves an item back only if nothing has taken its place, and
  `cleanse quarantine purge` deletes it for good. Quarantined space isn't freed until the purge,
  and the commands say so. Restoring and purging act only on items named by records Cleanse
  wrote, inside the quarantine, and purging only if the item is the kind its record says.
- **Trash** uses the operating system's trash, after checking that the item will really land
  there.
- Every run writes an append-only journal in Cleanse's data folder, which is itself protected.
  Each removal is flushed to disk before it starts and after it ends. `cleanse history` reads
  it back.
- If a removal stops partway, such as when the computer shuts down, what's left stays in its
  tombstone. `cleanse history` lists it, and `cleanse history --finish` removes it, but only if
  the tombstone still holds the file or folder that was approved, by identity, under the name
  Cleanse gave it, beside where it was.

### Not built yet

- **Trash**: items approved for it are skipped.
- **Quarantine on other drives**: items on a different drive from Cleanse's data folder are
  skipped rather than quarantined. Since every Review item is quarantined, large files on other
  drives can be found but not removed until the trash is built.
- **Free space**: the size reported for a deleted item is its measured size, not the measured
  change in free space.
