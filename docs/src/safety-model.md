# Safety model

> Status: the gates are implemented (see [what's not covered yet](#not-covered-yet)).
> `cleanse clean` deletes or quarantines approved items. The trash isn't built yet.

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
| **Protected** | Shown for transparency, never actionable | Agent transcripts, credentials, git-tracked files | None |

## Safety gates

Every candidate passes these gates when the plan is built and again immediately before it is
executed. A gate can allow an item, add a warning (the item keeps its tier but isn't selected
by default), or block it (the item becomes Protected). Only uncommitted changes, recent project
activity, and data shared through hard links or clones are warnings; every other concern
blocks, including any new kind of concern until it's deliberately classified.

1. **Protected policy.** System directories, volume roots, the home folder and its top-level
   folders, SSH and GPG keys, credential stores, `.git` internals, browser profiles (only their
   cache subfolders are eligible), AI agent history, settings and credentials, VM and WSL disk
   images, and the inside of macOS bundles. No candidate may be, or contain, a protected path.
   Paths are compared by file identity as well as case-folded text, which defeats 8.3 short
   names, case and Unicode differences, and firmlinks. Rules can't override this policy.
   Folders excluded in the settings file are protected the same way, with everything inside
   them. Inside an AI agent's folder, the only exceptions are the folders the agent's own
   documentation says can be deleted, listed with the safety checks rather than in rules. An
   exception lifts only that one protection, so a folder the user excluded stays excluded.
2. **Containment.** A candidate must be inside a root the user chose and inside the base its
   rule declares.
3. **Links and mounts.** Links, junctions, and other reparse points are never followed;
   removing a link removes only the link. Scans don't cross into other filesystems.
4. **Content.** Project artifacts must contain no git-tracked files. Folders with ambiguous names
   (`build/`, `dist/`, `out/`, `bin/`) also need ecosystem evidence and are blocked if they
   contain files that look like secrets. Anything Cleanse couldn't fully read is blocked rather
   than assumed empty.
5. **Activity.** Recently modified items, and items whose owning app, agent, or browser is
   running, are blocked as in use. A rule names its owner's programs; if the running programs
   can't be listed, its matches are blocked too.
6. **Cloud sync.** OneDrive, iCloud Drive, Dropbox, and Google Drive folders are protected by
   default because deletions sync to the cloud and other devices. Placeholder file contents are
   never read, since that would download them.
7. **Ownership and elevation.** Other users' files are blocked, and the main app refuses to
   apply a plan while running as administrator or root.

Every match is measured in one walk that doesn't follow links: its size, file count, newest
modification time, and anything inside that couldn't be read, is another filesystem, looks like
a secret, or is a virtual machine disk. A file modified in the last ten minutes counts as in use.

### Not covered yet

- **Other users' files**: need an ownership check.
- **Owners started another way**: programs are recognized by name, so Claude Code installed
  with npm, which runs as `node`, isn't recognized as running. Recent modification still
  blocks folders it's writing to.
- **Relocated agent folders**: if `CLAUDE_CONFIG_DIR` moves Claude Code's data out of
  `~/.claude`, that folder isn't protected or cleaned.
- **Browser profiles**: no rule matches inside them yet; they'll be protected before browser
  cache rules arrive.
- **Uncommitted changes** and **recent project activity** warnings: need a working-tree scan.
- **Sizes on Windows** come from the directory listing, which has no allocation data. They count
  each file's length, or nothing for cloud files that aren't downloaded. Compression and hard
  links other than pnpm's store aren't reflected.

## Execution and recovery

- Nothing is removed while Cleanse runs as an administrator or root, or if it can't start a
  journal.
- Before acting, each item is re-checked: it still exists, it's still the same folder (by
  identity, not just path), it's still not a link, its rule still recognizes it, and it still
  passes every gate. It's also skipped if its tier has risen, or a new kind of warning has
  appeared, since it was approved. Anything modified after the run starts counts as modified
  moments ago.
- **Delete** first renames the item to a sibling tombstone, so a project never sees a
  half-deleted `node_modules`. If what landed there isn't the approved folder, because
  something replaced it after the re-check, it's put back untouched. Otherwise the tombstone is
  removed, along with any links inside, never what they point to. On Windows the rename fails
  while a program has a file inside open, so the folder stays whole.
- **Quarantine** moves the item into the `quarantine` folder in Cleanse's data folder, with a
  record of where it came from written first. It's a rename, so it only works on the same drive;
  items elsewhere are skipped rather than copied. If what moved isn't the approved folder, it's
  put back. `cleanse restore` moves an item back only if nothing has taken its place, and
  `cleanse quarantine purge` deletes it for good. Quarantined space isn't freed until the purge,
  and the commands say so. Restoring and purging act only on folders named by records Cleanse
  wrote, inside the quarantine.
- **Trash** uses the operating system's trash, after checking that the item will really land
  there.
- Every run writes an append-only journal in Cleanse's data folder, which is itself protected.
  Each removal is flushed to disk before it starts and after it ends. `cleanse history` reads
  it back.
- If a removal stops partway, such as when the computer shuts down, what's left stays in its
  tombstone. `cleanse history` lists it, and `cleanse history --finish` removes it, but only if
  the tombstone still holds the folder that was approved, by identity, under the name Cleanse
  gave it, beside where it was.

### Not built yet

- **Trash**: items approved for it are skipped.
- **Quarantine on other drives**: items on a different drive from Cleanse's data folder are
  skipped rather than quarantined.
- **Free space**: the size reported for a deleted item is its measured size, not the measured
  change in free space.
