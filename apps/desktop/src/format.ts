// How the app writes numbers, sizes, and times, the same way the CLI does (`apps/cli/src/format.rs`).

import type { Size } from "./bindings/Size";

const UNITS = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
const SECONDS_PER_DAY = 24 * 60 * 60;

/** Bytes in binary units, like `3.4 GiB`. */
export function bytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${UNITS[unit]}`;
}

/** A count with thousands separators, like `374,735`. */
export function count(count: number): string {
  return count.toLocaleString("en-US");
}

/** How much space something takes and frees, like `1.2 GiB (at least 800.0 MiB freed)`. */
export function size(size: Size): string {
  if (size.guaranteed < size.onDisk) {
    return `${bytes(size.onDisk)} (at least ${bytes(size.guaranteed)} freed)`;
  }
  return bytes(size.onDisk);
}

/** `1 item`, or `3 items`. */
export function items(n: number): string {
  return n === 1 ? "1 item" : `${count(n)} items`;
}

export function seconds(seconds: number): string {
  return `${seconds.toFixed(1)} s`;
}

/** How long before `now` something happened, in whole days, like `12 days ago`. Both are seconds since the Unix epoch. */
export function ago(at: number, now: number): string {
  const days = Math.floor(Math.max(0, now - at) / SECONDS_PER_DAY);
  switch (days) {
    case 0:
      return "today";
    case 1:
      return "yesterday";
    default:
      return `${days} days ago`;
  }
}

/** When something was last modified, like `modified 12 days ago`. */
export function modified(at: number, now: number): string {
  return `modified ${ago(at, now)}`;
}

/** Now, in seconds since the Unix epoch. */
export function now(): number {
  return Math.floor(Date.now() / 1000);
}
