import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import type { Handled } from "../bindings/Handled";
import { QUARANTINE } from "../dev/sample";
import { findGrid } from "../test/grids";
import { QuarantineScreen } from "./QuarantineScreen";

const PURGED: Handled = {
  done: { items: 1, size: { logical: 0, onDisk: 640 * 1024 * 1024, guaranteed: 0 } },
  failed: [],
  journal: "C:\\journal.jsonl",
};

/** Answers with the sample quarantine, and records each call. */
function engine(): { cmd: string; args: Record<string, unknown> }[] {
  const calls: { cmd: string; args: Record<string, unknown> }[] = [];
  let entries = QUARANTINE.entries;
  mockIPC((cmd, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    calls.push({ cmd, args });
    switch (cmd) {
      case "quarantined":
        return { entries, unreadable: [] };
      case "restore":
        entries = entries.filter((entry) => !(args.ids as string[]).includes(entry.id));
        return {
          done: { items: 1, size: { logical: 0, onDisk: 1, guaranteed: 0 } },
          failed: [
            {
              path: "C:\\Users\\sample\\Downloads\\old-setup.msi",
              reason: "Something is there now",
            },
          ],
          journal: "C:\\journal.jsonl",
        } satisfies Handled;
      case "purge":
        return PURGED;
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
  return calls;
}

it("lists what's in quarantine, oldest first", async () => {
  engine();
  render(<QuarantineScreen elevated={false} />);

  const table = await findGrid("What's in quarantine");
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows.map((row) => row.textContent)).toEqual([
    expect.stringContaining("driver-pack.zip"),
    expect.stringContaining("old-setup.msi"),
  ]);
  expect(rows[0]?.textContent).toContain("40 days ago");
  expect(screen.getByText(/2 items in quarantine, 860\.0 MiB/)).toBeTruthy();
});

it("restores what's chosen, and says what couldn't be", async () => {
  const calls = engine();
  const user = userEvent.setup();
  render(<QuarantineScreen elevated={false} />);
  const table = await findGrid("What's in quarantine");

  await user.click(within(table).getAllByRole("checkbox")[2] as HTMLElement);
  await user.click(screen.getByRole("button", { name: "Restore" }));

  const report = await screen.findByRole("region", { name: "Restored 1 item." });
  expect(calls.find((call) => call.cmd === "restore")?.args.ids).toEqual(["q-2026-7"]);
  expect(report.textContent).toContain("Couldn't restore: 1 item");
  expect(report.textContent).toContain("Something is there now");
  await waitFor(() => expect(within(table).getAllByRole("row").slice(1)).toHaveLength(1));
});

it("asks before purging, starting at no", async () => {
  const calls = engine();
  const user = userEvent.setup();
  render(<QuarantineScreen elevated={false} />);
  const table = await findGrid("What's in quarantine");

  await user.click(within(table).getAllByRole("checkbox")[1] as HTMLElement);
  await user.click(screen.getByRole("button", { name: "Purge…" }));
  const dialog = await screen.findByRole("alertdialog");

  expect(within(dialog).getByRole("heading").textContent).toBe("Purge 1 item (640.0 MiB)?");
  expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: "Cancel" }));
  await user.keyboard("{Escape}");
  expect(calls.some((call) => call.cmd === "purge")).toBe(false);

  await user.click(screen.getByRole("button", { name: "Purge…" }));
  await user.click(await screen.findByRole("button", { name: "Purge" }));

  expect(
    await screen.findByRole("region", { name: "Purged 1 item, freeing 640.0 MiB." }),
  ).toBeTruthy();
  expect(calls.find((call) => call.cmd === "purge")?.args.ids).toEqual(["q-2025-1"]);
});

it("turns restoring and purging off while elevated", async () => {
  engine();
  const user = userEvent.setup();
  render(<QuarantineScreen elevated={true} />);
  const table = await findGrid("What's in quarantine");

  await user.click(within(table).getAllByRole("checkbox")[1] as HTMLElement);

  for (const name of ["Restore", "Purge…"]) {
    expect(screen.getByRole("button", { name }).hasAttribute("disabled")).toBe(true);
  }
  expect(
    screen.getByText("Restoring and purging are off while Cleanse runs elevated."),
  ).toBeTruthy();
});
