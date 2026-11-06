# 0011: The user's cleanup runs elevated only when there's no other way

**Status:** Accepted. It narrows [ADR 0010](0010-elevated-system-cleanup.md)'s "User cleanup
doesn't change. It still refuses every change while elevated."

## Context

Cleanse's cleanup of the user's own files refused every change while it ran as an
administrator or root. Elevated, it could remove what the account itself couldn't. And an
elevated program deleting in folders other programs can write to is how a cleanup tool becomes
a privilege escalation: a less privileged program plants a link, and the elevated one follows
it. Refusing costs nothing where the user can run Cleanse unelevated instead, by opening it
normally or leaving out `sudo`.

On Windows with User Account Control turned off, they can't. Every program the account starts,
including the desktop app from the Start menu, gets the account's full administrator token.
That's how GitHub's Windows runners and some managed machines are set up, and the built-in
Administrator account works the same way by default. There, the refusal blocked every cleanup,
and its advice to open Cleanse again without elevation couldn't be followed.

The refusal doesn't protect anything there either. With no unelevated session for the account,
every program it runs is already an administrator, so there's no less privileged program of
the account's to plant a link for Cleanse to follow.

## Decision

**The user's cleanup refuses only while the account has an unelevated way to run.** The
platform reports one of three elevations:

- **`None`:** neither an administrator nor root. Cleanup runs.
- **`Elevated`:** an administrator or root while the account also runs things without those
  rights. On Windows, that's a program started with Run as administrator while UAC is on, whose
  token is `TokenElevationTypeFull`. On Linux and macOS, it's root, `sudo` included. Cleanup
  refuses, as before.
- **`AlwaysElevated`:** on Windows, an elevated token of type `TokenElevationTypeDefault`, for a
  person's account: a local or domain account (S-1-5-21-…), or a Microsoft Entra ID one
  (S-1-12-1-…). Windows gives that type to an account with no filtered token, as with UAC off.
  Cleanup runs.

Service accounts get `TokenElevationTypeDefault` too, such as SYSTEM, `LOCAL SERVICE`, and the
`NT SERVICE` accounts. But they're never a person cleaning their own files, so they count as
`Elevated`.

**Every other check applies as before:**

- **Ownership:** items owned by SYSTEM, a service, or another account stay blocked. Items the
  Administrators group owns still count as the user's, since that's who owns what the account's
  programs make with UAC off.
- **Everything else:** the protected locations, the re-check before each removal, the identity
  checks around the tombstone, quarantine, and the journal are all unchanged.

**Other accounts' home folders are protected.**
- **Why:** their permissions keep an unelevated Cleanse out, but not an administrator. With UAC
  off, most of another administrator's files are owned by the Administrators group, which the
  ownership check accepts.
- **What:** on Windows, the folder every account's home is made in, `FOLDERID_UserProfiles`, is a
  protected subtree, apart from the user's own home folder.
- **How the exception matches:** by identity as well as by spelling, so the home folder reached
  through an 8.3 short name, as `%TEMP%` often is, still counts as the user's. A path through `.`
  or `..` never counts as inside it, since the folders above a path are read from how it's
  spelled.
- **When:** however Cleanse runs. Unelevated, it changes nothing there anyway.

**System cleanup doesn't change.** It runs with either kind of elevation. The desktop app's
System section still doesn't start its helper while the app itself is elevated, with UAC off
too, and says so. Whether it should is a separate decision. `cleanse system` works from any
terminal there.

## Consequences

- **With UAC off:**
  - `cleanse clean`, `apply`, `restore`, `quarantine purge`, and `history --finish` work.
  - The desktop app's Clean, Restore, Purge, and Finish are on.
  - `cleanse doctor` and the app's settings say everything the account runs is elevated.
- **With UAC on, nothing changes:** an elevated Cleanse still refuses, and still says to open it
  again without elevation.
- **What an administrator can reach:** with UAC off, Cleanse can remove what an administrator
  can, within its rules, the protected locations, and the ownership check. That includes rule
  matches the account couldn't change unelevated, if the Administrators group owns them, such as
  `__pycache__` in a Python installed for all users.
- **Testing:** CI's Windows runners have UAC off, so the CLI's refusal tests can't check the
  refusal there. They're named `when_elevated`, so CI's Linux job runs them as root instead. The
  executor's tests check each elevation with a fake one.

**Residual risks:**

- **Low-integrity programs:** a browser's sandboxed process, say, could try to plant a link in
  the few folders it can write to, such as `AppData\LocalLow`. As always, Cleanse doesn't follow
  links. It also refuses items with links inside, and re-checks each item's identity before and
  after renaming it.
- **Administrative shares:** protection goes by path and by file identity. A path that hides
  both, such as `\\localhost\C$\Users\…`, misses the protected locations. But file identities
  can't be read there either, so everything reached that way is kept as unverifiable.
- **A junction in your own home:** scans don't follow links, so only a plan file naming a path
  through one could reach the other account's folder it points into. Only the user or an
  administrator can make such a junction, and with UAC off they're the same.
