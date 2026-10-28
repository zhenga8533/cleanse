# 0004: Risk tiers and tiered disposal

**Status:** Accepted

## Context

Not everything a cleaner finds is equally safe to remove, and a single "junk" label hides the
difference between a log file and a folder of downloads. Recovery options have their own
trade-offs:

- The OS trash doesn't free space until it's emptied, is slow for folders with hundreds of
  thousands of files (a large `node_modules` on Windows), doesn't exist on network or removable
  drives, and the Windows Recycle Bin silently deletes items that exceed its size limit.
- A Cleanse-managed quarantine can always be restored from but also frees nothing until it's
  purged.
- For regenerable items, rebuilding them is already the recovery.

## Decision

Every candidate gets one of four tiers: **Safe**, **Regenerable**, **Review**, or **Protected**
(defined in the [safety model](../safety-model.md)). The default action depends on the tier:

- Safe and Regenerable: delete, via a tombstone rename so nothing is left half-deleted.
- Review: move to a same-volume Cleanse quarantine with a manifest, restorable until purged.
  Never preselected.
- Protected: never actionable.

Users can change the action per tier. Non-interactive runs (`--yes`) never include Review items.

## Consequences

- Space from regenerable items is freed immediately.
- Items that need judgment are recoverable even if the user changes their mind later.
- The UI must state clearly that quarantined space isn't freed until it's purged.
- Quarantine needs a writable location on each volume; where there isn't one, Cleanse falls
  back to the trash and says so.
