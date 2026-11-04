import type { Channel } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import type { Item } from "../bindings/Item";
import type { Section } from "../bindings/Section";
import type { SystemStatus } from "../bindings/SystemStatus";
import { details, HISTORY, ITEMS, QUARANTINE, RULE_VIEWS, results } from "../dev/sample";
import { findGrid } from "../test/grids";
import { SystemScreen } from "./SystemScreen";

const AVAILABLE: SystemStatus = { supported: true, unavailable: null, running: false };

/** Answers system cleanup's commands with the sample data, and records each one asked. */
function helper(): string[] {
  const calls: string[] = [];
  mockIPC((cmd, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    calls.push(cmd);
    switch (cmd) {
      case "system_scan":
        (args.progress as Channel<unknown>).onmessage({ kind: "walking", entries: 1024 });
        return results(1);
      case "system_candidates": {
        const listed = ITEMS[args.section as Section].filter((item) => item.rule === args.rule);
        return { items: listed, offset: 0, total: listed.length };
      }
      case "system_explain": {
        const [item, section] = find(args.item as number);
        return details(item, section);
      }
      case "system_quarantined":
        return QUARANTINE;
      case "system_history":
        return HISTORY;
      case "system_rules":
        return RULE_VIEWS;
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
  return calls;
}

function find(id: number): [Item, Section] {
  for (const section of Object.keys(ITEMS) as Section[]) {
    const item = ITEMS[section].find((item) => item.id === id);
    if (item) {
      return [item, section];
    }
  }
  throw new Error(`no item ${id}`);
}

it("says why system cleanup isn't available, and how the command line does it", () => {
  const calls = helper();

  render(
    <SystemScreen
      status={{
        supported: true,
        unavailable: "this copy of Cleanse wasn't installed with its system cleanup helper",
        running: false,
      }}
    />,
  );

  expect(
    screen.getByText(/isn't available here: this copy of Cleanse wasn't installed/),
  ).toBeTruthy();
  expect(screen.getByText("cleanse system scan")).toBeTruthy();
  expect(screen.queryByRole("tab")).toBeNull();
  expect(calls).toEqual([]);
});

it("asks the helper for nothing until a scan, which covers the system's own folders", async () => {
  const calls = helper();
  const user = userEvent.setup();
  render(<SystemScreen status={AVAILABLE} />);

  expect(screen.getByRole("heading", { name: "System scan" })).toBeTruthy();
  expect(screen.queryByRole("heading", { name: "Folders to scan" })).toBeNull();
  expect(calls).toEqual([]);

  await user.click(screen.getByRole("button", { name: "Scan" }));

  expect(await screen.findByRole("heading", { name: /^Can be removed/ })).toBeTruthy();
  expect(calls).toEqual(["system_scan"]);
});

it("lists and explains a system scan's items through the helper", async () => {
  const calls = helper();
  const user = userEvent.setup();
  render(<SystemScreen status={AVAILABLE} />);
  await user.click(screen.getByRole("button", { name: "Scan" }));
  await screen.findByRole("heading", { name: /^Can be removed/ });

  await user.click(screen.getByRole("button", { name: /^Node\.js dependencies/ }));
  const list = await findGrid("Node.js dependencies, Can be removed");
  const first = within(list).getAllByRole("row")[0] as HTMLElement;
  await user.click(within(first).getByRole("button", { name: "Details" }));

  expect(await screen.findByRole("complementary", { name: "node_modules" })).toBeTruthy();
  expect(calls).toEqual(["system_scan", "system_candidates", "system_explain"]);
});

it("opens system cleanup's quarantine, history, and rules when they're chosen", async () => {
  const calls = helper();
  const user = userEvent.setup();
  render(<SystemScreen status={AVAILABLE} />);

  await user.click(screen.getByRole("tab", { name: "Quarantine" }));
  expect(await screen.findByRole("heading", { name: "System quarantine" })).toBeTruthy();
  await user.click(screen.getByRole("tab", { name: "History" }));
  expect(await screen.findByRole("heading", { name: "System history" })).toBeTruthy();
  await user.click(screen.getByRole("tab", { name: "Rules" }));
  expect(await screen.findByText(RULE_VIEWS[0]?.name ?? "")).toBeTruthy();

  await waitFor(() =>
    expect(calls).toEqual(["system_quarantined", "system_history", "system_rules"]),
  );
});
