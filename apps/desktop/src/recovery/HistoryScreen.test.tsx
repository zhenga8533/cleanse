import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import type { Handled } from "../bindings/Handled";
import { HISTORY } from "../dev/sample";
import { HistoryScreen } from "./HistoryScreen";

function engine(): string[] {
  const calls: string[] = [];
  let history = HISTORY;
  mockIPC((cmd) => {
    calls.push(cmd);
    switch (cmd) {
      case "history":
        return history;
      case "finish":
        history = { ...history, leftovers: [] };
        return {
          done: { items: 1, size: { logical: 0, onDisk: 300 * 1024 * 1024, guaranteed: 0 } },
          failed: [],
          journal: "C:\\journal.jsonl",
        } satisfies Handled;
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
  return calls;
}

it("lists runs most recent first, saying what each did as the CLI does", async () => {
  engine();
  render(<HistoryScreen elevated={false} />);

  const table = await screen.findByRole("grid", { name: "Runs, most recent first" });
  const rows = within(table)
    .getAllByRole("row")
    .slice(1)
    .map((row) => row.textContent);
  expect(rows).toEqual([
    "2 days agocleanDeleted 2 (3.1 GiB), quarantined 1 (220.0 MiB), skipped 1",
    "30 days agopurgePurged 1 (90.0 MiB)",
    "41 days agocleanDeleted 1 (400.0 MiB), quarantined 1 (640.0 MiB) (cut short)",
  ]);
});

it("removes what's left of removals that didn't finish", async () => {
  const calls = engine();
  const user = userEvent.setup();
  render(<HistoryScreen elevated={false} />);

  const leftovers = await screen.findByRole("region", { name: /1 removal didn't finish/ });
  expect(leftovers.textContent).toContain(".cleanse-removing-node_modules");
  await user.click(within(leftovers).getByRole("button", { name: "Remove what's left" }));

  expect(
    await screen.findByRole("region", {
      name: "Removed what was left of 1 item, freeing 300.0 MiB.",
    }),
  ).toBeTruthy();
  expect(calls).toContain("finish");
  expect(screen.queryByRole("region", { name: /didn't finish/ })).toBeNull();
});

it("turns finishing off while elevated", async () => {
  engine();
  render(<HistoryScreen elevated={true} />);

  const button = await screen.findByRole("button", { name: "Remove what's left" });
  expect(button.hasAttribute("disabled")).toBe(true);
});
