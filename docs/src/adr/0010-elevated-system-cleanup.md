# 0010: System cleanup runs elevated, apart from everything else

**Status:** Accepted

## Context

Cleanse refuses to change anything while it runs as an administrator or root, and its safety
policy protects the operating system's folders whole. Some of the clutter people most want
gone sits there: the Windows temp folder, Windows Update's downloads, memory and crash dumps.
Only an administrator or root can remove those.

Running elevated is also where a mistake stops being a lost file and becomes a privilege
escalation. Any unelevated process can create files in `C:\Windows\Temp` and `/var/crash`. Any
user can create folders in `C:\ProgramData`. The current user can redirect `%SystemRoot%` for
their own elevated processes through `HKCU\Environment`. And a desktop app's web view, with its
npm dependencies, is the last thing that should hold administrator rights.

## Decision

System cleanup is a separate scope with its own rules, policy, data folder, and executor, which
runs only elevated. User cleanup doesn't change. It still refuses every change while elevated.

**The two scopes can't mix.** Three locks keep them apart, and any one of them would hold.
- **Rules:** system rules say `scope = "system"` and live in their own files. Loading user rules
  refuses a system rule, and loading system rules refuses a user rule.
- **Bases:** system rules name new bases: `system_root` and `program_data` on Windows, `var` on
  Linux. The user matcher resolves those to nothing, and the system matcher resolves the user
  bases to nothing.
- **Executor:** the system executor requires a system rule set and elevation. The user executor
  keeps its unconditional refusal when elevated.

**Every item removed meets all of these:**
1. **One level down.** It's directly inside a fixed system folder. The folders come from OS
   APIs, such as `GetSystemWindowsDirectoryW`, and never from environment variables. Scans list
   those folders without descending, and a system rule may use a wildcard only in its last path
   segment.
2. **A verified chain.** That folder, and each folder from its base down to it, is a real
   directory owned by the system. On Windows that's SYSTEM, a service account, or Administrators;
   on Linux, root. This is checked when scanning and again before each change.
3. **System-owned throughout.** The item, and everything inside it, is owned by the system. There
   are no links or special files inside, and no file with other hard links.
4. **The same item.** It still has the identity it had when it was scanned, checked before and
   after it's renamed to its tombstone.
5. **Trusted bookkeeping.** Its journal and quarantine records live in a folder only
   administrators or root can write. Restoring and finishing also check their targets against
   the system rules and the verified folders before acting.

**The policy's exceptions are narrow, and exist only for system cleanup.**
- **Where:** a `SYSTEM_DISPOSABLE` list in the policy names the items system cleanup may remove,
  such as `Temp/*`. It never names the folders themselves, so `C:\Windows\Temp` stays protected.
  Only the system-cleanup policy adds these exceptions.
- **The user policy:** it's unchanged, and still protects everything under the OS's folders.
- **A test:** it ties the exceptions to the removable system rules, so neither can grow without
  the other.

**Ownership is checked in both scopes.**
- **User cleanup:** an item must belong to the current user, or to Administrators on Windows,
  which covers the user's own elevated work.
- **System cleanup:** an item, and everything in it, must belong to the system.
- **Otherwise:** an item that belongs to another account, or whose owner can't be read, is kept.

**System cleanup reads nothing the unelevated user can write.**
- No settings file, no Git, and no plan files. There's no `cleanse system apply`.
- Its journal and quarantine live in a folder of its own: `ProgramData\Cleanse` on Windows,
  `/var/lib/cleanse` on Linux. That folder is always created and then verified: only
  administrators and SYSTEM, or root, may write it, and it isn't a link. One that someone else
  made first is refused, never repaired.
- System quarantine restores only into system locations, and is never mixed with a user's
  quarantine.

**Two front ends, one engine.**
- **The CLI:** `cleanse system …` runs in-process, from an administrator terminal or with
  `sudo`. Without elevation it says how to rerun.
- **The desktop:** a separate helper, `apps/elevate`, does system cleanup. The app's window
  stays unelevated and never runs a web view as administrator or root.
  - **Launch:** the window starts the helper through the UAC prompt on Windows, or through
    pkexec with a polkit policy on Linux.
  - **Requests:** the window sends only IDs from the helper's own session, as it does with the
    engine ([ADR 0009](0009-desktop-web-view.md)). The protocol has no request that carries a
    path, and the helper exits on any malformed message.
- **Who the helper trusts:** it treats the window as untrusted. On Windows it accepts only its
  launching process, checked by process ID, executable path, and session, over a local pipe
  only that process's logon session can open. On Linux the connection is pkexec's inherited
  stdin and stdout, and the polkit policy asks for an administrator's password every time
  rather than caching the authorization.
- **Installed copies only:** the app offers system cleanup only when the helper is installed
  where only administrators or root can change it, such as under Program Files or `/usr/lib`. So
  the Linux AppImage doesn't, and points to `sudo cleanse system` instead.

**Where the code goes** (extending [ADR 0002](0002-single-mutator.md)):
- **Unsafe code:** `cleanse-platform` gains a `privileged` module behind a feature. It holds the
  Windows calls system cleanup needs, such as creating the protected folder, the UAC launch,
  and the pipe. It stays the only crate with `unsafe`.
- **The lint:** `clippy.toml` lists those functions, and the standard-library functions it
  missed (`DirBuilder::create`, the `symlink` and `chown` families), under
  `disallowed-methods`. Only `cleanse-exec` and `apps/elevate` opt in.
- **Starting processes:** `cleanse-exec` starts the helper, since it's the only crate that
  starts processes.
- **Review:** CODEOWNERS covers the system rules, `cleanse-platform`, and the app's capabilities
  as well as the helper.

**Deleting under system folders relies on what's already there.**
- **The tombstone:** today's rename to a tombstone with an identity check after it is kept.
- **Folders:** the standard library's `remove_dir_all` removes them, and it doesn't follow links
  even when one appears mid-removal (CVE-2022-21658, fixed in Rust 1.58.1). Windows lets
  ordinary users create entries inside some SYSTEM-owned folders after they're checked, so that
  matters here.

## Consequences

**What system cleanup covers at first:**
- **Windows:** the temp folder (when idle and nothing has changed since startup), Windows
  Update's downloads, archived error reports, and memory dumps.
- **Linux:** systemd core dumps and root-owned crash reports.

**What's only advised:** package caches, the journal, and Delivery Optimization's cache, through
their own commands.

**What's left out:**
- `/tmp` and `/var/tmp`, which every user shares.
- Error reports not yet sent.
- Windows' component store, whose size Cleanse can't measure honestly.

**Costs:**
- System cleanup reads the owner of every entry inside an item, which costs a file handle per
  entry on Windows.
- Tests of the elevated paths run where CI is elevated: Windows' runners, and a Linux job that
  runs only those tests as root.

**How it fails safe:** a folder or pipe name someone else created first stops system cleanup
with an error. It never runs anyway.

**Residual risks, accepted and documented:**
- **Drive letters:** Windows lets each logon session map drive letters of its own. System
  cleanup's folders come from OS APIs and are verified by owner, but it doesn't look for such a
  mapping separately.
- **A writable `cleanse`:** running `cleanse` with `sudo` or from an administrator terminal
  runs whatever binary is on the path. The README says to install it where only
  administrators or root can write.
- **A compromised window:** the helper's protections bound it, but don't eliminate it. A
  compromised window, after the user approved the prompt, can make the helper remove what system
  cleanup would have offered to remove.

**What changes for users:**
- A user cleanup now keeps items another account owns, which it previously tried and failed to
  remove.
- In the desktop app, system cleanup asks for administrator rights each time its helper starts.

### Threats considered

| An attacker who can… | …tries to | Stopped by |
|---|---|---|
| Write in `C:\Windows\Temp`, WER's folders, or `/var/crash` | Plant files, junctions, or symlinks for Cleanse to follow or remove | Links are never followed or matched; only system-owned items qualify; one level down; identity checks |
| Create entries inside a SYSTEM-owned temp folder | Get their entries deleted with it, or swap a link in mid-removal | Ownership of everything inside; links inside block; `remove_dir_all` doesn't follow links |
| Hard-link a protected file into a scanned folder | Have the original removed | Hard-linked files block; removing one name frees nothing anyway |
| Create `C:\ProgramData\Cleanse` first | Plant journal or quarantine records that restore or delete elsewhere | The folder is verified after creation and refused if wrong; restore and finish re-check their targets |
| Set `windir` or `SystemRoot` in `HKCU\Environment` | Point system cleanup at their own folders | Folders come from OS APIs and are verified, never from the environment |
| Edit the user's settings, a plan file, or plant a `.git` above a system folder | Steer what's removed | System cleanup reads no settings, plans, or Git |
| Drive the app's window after the user approved | Remove more than system cleanup offers | The helper accepts only IDs from its own session, re-checks every item, and exits after 10 minutes idle |
| Start the helper themselves, or squat its pipe | Talk to an elevated helper, or impersonate it | Parent, path, and session checks; the pipe opens for the parent's logon session only; both ends check each other's process ID |
| Replace the helper or plant DLLs beside it | Run their code elevated | It's offered only from folders only administrators or root can write; DLLs load from System32 only |
| Reuse a cached polkit authorization | Start the helper without a prompt | The policy requires a password every time (`auth_admin`) |
