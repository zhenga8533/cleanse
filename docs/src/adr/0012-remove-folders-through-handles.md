# 0012: Cleanse removes folders through their handles, so it can stop partway

**Status:** Accepted. It replaces [ADR 0010](0010-elevated-system-cleanup.md)'s reliance on the
standard library's `remove_dir_all` for removing folders.

## Context

Cleanse removed each approved folder with std's `remove_dir_all`, after renaming it to its
tombstone.

- **It can't be stopped.** Stopping a cleanup took effect only between items. One Rust `target`
  folder can hold over 170,000 files, and npm's cache more, so Stop could wait minutes. The
  re-check before each removal, which reads every file inside the item, couldn't stop either.
- **It removes one entry at a time.** Removing a file mostly waits on the filesystem, and on
  Windows on antivirus. On a laptop with 12 logical processors and endpoint protection, it
  removed about 400 small files a second. Threads of Cleanse's own removed about 2,100 to 2,200
  a second with 16 to 24 of them, and no more with 32.
- **What it guarantees matters.** It works through each folder's handle, by name, and never
  follows a link, even one swapped in while it runs (CVE-2022-21658). ADR 0010 relies on that for
  system cleanup, since ordinary users can create entries inside some SYSTEM-owned folders.

## Decision

**Folders are removed through each folder's handle, as std's `remove_dir_all` does.**
- **Where:** `cleanse-platform`'s `privileged::Folder`, behind the `privileged` feature.
  `clippy.toml` lets only `cleanse-exec` call its removals (ADR 0002).
- **Windows:** entries are listed with `GetFileInformationByHandleEx`, and opened with
  `NtOpenFile` relative to the folder's handle, with `FILE_OPEN_REPARSE_POINT`. They're removed
  with POSIX semantics, read-only ones included, falling back where a filesystem lacks those.
- **Unix:** `openat` with `O_NOFOLLOW`, and `unlinkat`.
- **Links:** symbolic links, junctions, and mount points are only ever removed as themselves. An
  entry listed as a folder is checked again through its handle once it's open, and if it's a
  link by then, it's removed as one.

**Many at once.** `cleanse-exec` removes what's inside a folder on a pool of twice as many
threads as processors, from 4 to 32, using rayon, which the scan already uses.

**Stop works partway.**
- **During the re-check:** measuring goes no deeper once Stop is pressed. An item whose check
  was cut short is skipped as cancelled and left untouched, since what the check found can't be
  relied on.
- **During the removal:** nothing more is removed once Stop is pressed. What's left stays in the
  tombstone, and the item is reported as stopped. History lists it with what crashes left, for
  the app's History or `cleanse history --finish` to remove.

## Consequences

- **Faster:** folders of many small files, such as `node_modules`, `target`, and package caches,
  go about five times as fast on that laptop. A single file, or a folder of a few large ones,
  goes no faster.
- **Stop:** takes effect within the item being checked or removed, after the entry in hand.
- **Code to maintain:** Cleanse's own `unsafe` code on Windows replaces a call to std, and has to
  keep std's guarantees. Tests cover links inside a folder, the primitives on links, read-only
  files, a folder of hundreds of files and folders, and stopping.
- **A partly removed item is ordinary now:** Stop leaves one, not only a crash. It's out of
  sight in its tombstone, and its space is only partly freed until History removes the rest.
- **ADR 0010's threat table:** where it says `remove_dir_all` doesn't follow links, Cleanse's own
  removal doesn't, for the same reasons.
