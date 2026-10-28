# Safety model

> Status: design. The gates are implemented in M1 and execution in M2.

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
executed. A gate can allow an item, downgrade it to a riskier tier with a warning, or block it.

1. **Protected policy.** System directories, volume roots, the home folder and its top-level
   folders, SSH and GPG keys, credential stores, `.git` internals, browser profiles (only their
   cache subfolders are eligible), AI agent history, settings and credentials, VM and WSL disk
   images, and the inside of macOS bundles. No candidate may be, or contain, a protected path.
   Paths are compared by file identity as well as case-folded text, which defeats 8.3 short
   names, case and Unicode differences, and firmlinks. Rules can't override this policy.
2. **Containment.** A candidate must be inside a root the user chose and inside the base its
   rule declares.
3. **Links and mounts.** Links, junctions, and other reparse points are never followed;
   removing a link removes only the link. Scans don't cross into other filesystems.
4. **Content.** Project artifacts must contain no git-tracked files. Folders with ambiguous names
   (`build/`, `dist/`, `out/`, `bin/`) also need ecosystem evidence and are blocked if they
   contain files that look like secrets. Anything Cleanse couldn't fully read is blocked rather
   than assumed empty.
5. **Activity.** Recently modified items, and items whose owning app, agent, or browser is
   running, are blocked as in use.
6. **Cloud sync.** OneDrive, iCloud Drive, Dropbox, and Google Drive folders are protected by
   default because deletions sync to the cloud and other devices. Placeholder file contents are
   never read, since that would download them.
7. **Ownership and elevation.** Other users' files are blocked, and the main app refuses to
   apply a plan while running as administrator or root.

## Execution and recovery

- Before acting, each item is re-checked: it still exists, it's still the same file or folder
  (by identity, not just path), it's still not a link, and it still passes every gate.
- **Delete** first renames the item to a sibling tombstone, so a project never sees a
  half-deleted `node_modules`, then removes it.
- **Quarantine** moves the item into a Cleanse-managed area on the same drive with a manifest,
  so it can be restored until it's purged. Quarantined space isn't freed until the purge, and
  the UI says so.
- **Trash** uses the operating system's trash, after checking that the item will really land
  there.
- Every action is written to an append-only journal, and the measured change in free space is
  reported afterwards.
