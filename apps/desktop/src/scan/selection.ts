import type { Choice } from "../bindings/Choice";
import type { Item } from "../bindings/Item";
import type { Size } from "../bindings/Size";

/** The items chosen for a cleanup, by ID. */
export type Chosen = ReadonlyMap<number, Choice>;

export interface Amount {
  items: number;
  size: Size;
}

export function total(sizes: Iterable<Size>): Size {
  const sum: Size = { logical: 0, onDisk: 0, guaranteed: 0 };
  for (const size of sizes) {
    sum.logical += size.logical;
    sum.onDisk += size.onDisk;
    sum.guaranteed += size.guaranteed;
  }
  return sum;
}

/** What a cleanup of `chosen` quarantines, and what it deletes, as `cleanse clean` asks. */
export function tally(chosen: Chosen): { deleted: Amount; quarantined: Amount } {
  const choices = [...chosen.values()];
  const amount = (group: Choice[]): Amount => ({
    items: group.length,
    size: total(group.map((choice) => choice.size)),
  });
  return {
    deleted: amount(choices.filter((choice) => choice.disposal !== "quarantine")),
    quarantined: amount(choices.filter((choice) => choice.disposal === "quarantine")),
  };
}

/**
 * `chosen`, with the items of `shown` chosen exactly when `keys` has them. Items elsewhere keep
 * their choice, and items that can't be removed are never chosen.
 */
export function choose(chosen: Chosen, shown: Item[], keys: "all" | Set<unknown>): Chosen {
  const next = new Map(chosen);
  for (const item of shown) {
    if (item.disposal !== null && (keys === "all" || keys.has(item.id))) {
      next.set(item.id, { id: item.id, size: item.size, disposal: item.disposal });
    } else {
      next.delete(item.id);
    }
  }
  return next;
}
