import { expect, it } from "vitest";
import { ago, bytes, count, items, size } from "./format";

it("writes sizes in binary units, as the CLI does", () => {
  expect(bytes(0)).toBe("0 B");
  expect(bytes(1023)).toBe("1023 B");
  expect(bytes(1024)).toBe("1.0 KiB");
  expect(bytes(3.4 * 1024 ** 3)).toBe("3.4 GiB");
  expect(size({ logical: 0, onDisk: 2048, guaranteed: 1024 })).toBe(
    "2.0 KiB (at least 1.0 KiB freed)",
  );
  expect(size({ logical: 0, onDisk: 2048, guaranteed: 2048 })).toBe("2.0 KiB");
});

it("writes counts with separators", () => {
  expect(count(374735)).toBe("374,735");
  expect(items(1)).toBe("1 item");
  expect(items(1200)).toBe("1,200 items");
});

it("says how long ago in whole days", () => {
  const day = 24 * 60 * 60;
  expect(ago(1000, 1000 + day - 1)).toBe("today");
  expect(ago(1000, 1000 + day)).toBe("yesterday");
  expect(ago(1000, 1000 + 12 * day)).toBe("12 days ago");
  expect(ago(2000, 1000)).toBe("today");
});
