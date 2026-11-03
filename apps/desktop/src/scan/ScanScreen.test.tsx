import type { Channel } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import type { CleanReport } from "../bindings/CleanReport";
import type { Item } from "../bindings/Item";
import type { Section } from "../bindings/Section";
import { details, HOME_FOLDER, ITEMS, results } from "../dev/sample";
import { findGrid } from "../test/grids";
import { ScanScreen } from "./ScanScreen";

const REPORT: CleanReport = {
  deleted: { items: 2, size: { logical: 3212, onDisk: 3212 * 1024 * 1024, guaranteed: 0 } },
  quarantined: { items: 0, size: { logical: 0, onDisk: 0, guaranteed: 0 } },
  skipped: [
    { path: "C:\\Users\\sample\\code\\engine\\target", reason: "Something else is there now" },
  ],
  failed: [],
  journal: "C:\\journal.jsonl",
};

/** Answers the commands with the sample results, and records each call. */
function engine(): { cmd: string; args: Record<string, unknown> }[] {
  const calls: { cmd: string; args: Record<string, unknown> }[] = [];
  mockIPC((cmd, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    calls.push({ cmd, args });
    switch (cmd) {
      case "folders":
        return { folders: [HOME_FOLDER], home: true };
      case "scan":
        (args.progress as Channel<unknown>).onmessage({ kind: "walking", entries: 1024 });
        return results(1);
      case "summary":
        return results(1, { ...ITEMS, removable: ITEMS.removable.slice(2) });
      case "candidates": {
        const listed = ITEMS[args.section as Section].filter((item) => item.rule === args.rule);
        return { items: listed, offset: 0, total: listed.length };
      }
      case "explain": {
        const [item, section] = find(args.item as number);
        return details(item, section);
      }
      case "clean":
        return REPORT;
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

async function scanned(elevated = false) {
  const calls = engine();
  const user = userEvent.setup();
  render(<ScanScreen elevated={elevated} onResults={() => {}} />);
  await user.click(await screen.findByRole("button", { name: "Scan" }));
  await screen.findByRole("heading", { name: /^Can be removed/ });
  return { calls, user };
}

it("scans the home folder and lists what it found, section by section", async () => {
  const { calls } = await scanned();

  expect(screen.getByText(HOME_FOLDER, { selector: ".folders .path" })).toBeTruthy();
  const headings = screen.getAllByRole("heading", { level: 2 }).map((h) => h.textContent);
  expect(headings).toEqual([
    "Folders to scan",
    "Can be removed: 3 items, 3.5 GiB",
    "Needs your review: 1 item, 96.0 MiB",
    "Best cleaned by their own tools: 1 item, 1.8 GiB",
    "Kept: 1 item, 12.0 MiB",
  ]);
  expect(calls.some((call) => call.cmd === "scan")).toBe(true);
  // What `cleanse clean` would select: the two items without warnings.
  expect(screen.getByText("Selected: 2 items, 3.1 GiB")).toBeTruthy();
});

it("lists a rule's items once it's opened, and chooses with their checkboxes", async () => {
  const { user } = await scanned();

  await user.click(screen.getByRole("button", { name: /^Node\.js dependencies/ }));
  const list = await findGrid("Node.js dependencies, Can be removed");
  const rows = within(list).getAllByRole("row");
  expect(rows.map((row) => row.textContent)).toEqual([
    expect.stringContaining("web\\node_modules"),
    expect.stringContaining("The project has uncommitted changes"),
  ]);
  const boxes = within(list).getAllByRole("checkbox") as HTMLInputElement[];
  expect(boxes.map((box) => box.checked)).toEqual([true, false]);

  await user.click(boxes[1] as HTMLInputElement);

  expect(screen.getByText("Selected: 3 items, 3.5 GiB")).toBeTruthy();
});

it("asks before cleaning, starting at no", async () => {
  const { calls, user } = await scanned();

  await user.click(screen.getByRole("button", { name: "Clean…" }));
  const dialog = await screen.findByRole("alertdialog");

  expect(within(dialog).getByRole("heading").textContent).toBe("Delete 2 items (3.1 GiB)?");
  expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: "Cancel" }));
  await user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(calls.some((call) => call.cmd === "clean")).toBe(false);
});

it("says what goes to quarantine when Review items are chosen", async () => {
  const { user } = await scanned();

  await user.click(screen.getByRole("button", { name: /^Installers in Downloads/ }));
  const list = await findGrid("Installers in Downloads, Needs your review");
  expect(within(list).getByRole("row").textContent).toContain("modified 210 days ago");
  await user.click(within(list).getByRole("checkbox"));
  await user.click(screen.getByRole("button", { name: "Clean…" }));

  const dialog = await screen.findByRole("alertdialog");
  expect(within(dialog).getByRole("heading").textContent).toBe(
    "Delete 2 items (3.1 GiB) and quarantine 1 item (96.0 MiB)?",
  );
});

it("cleans what was chosen once confirmed, and reports what happened", async () => {
  const { calls, user } = await scanned();

  await user.click(screen.getByRole("button", { name: "Clean…" }));
  await user.click(await screen.findByRole("button", { name: "Delete" }));

  const report = await screen.findByRole("region", { name: "Cleanup finished" });
  const clean = calls.find((call) => call.cmd === "clean");
  expect(clean?.args.scan).toBe(1);
  expect(clean?.args.items).toEqual([1, 2]);
  expect(report.textContent).toContain("Deleted 2 items, 3.1 GiB.");
  expect(report.textContent).toContain("Skipped: 1 item");
  expect(report.textContent).toContain("Something else is there now");
  expect(document.activeElement).toBe(within(report).getByRole("heading", { level: 2 }));
  // The results are read again, without what was removed, and nothing stays chosen.
  expect(await screen.findByRole("heading", { name: /^Can be removed: 1 item/ })).toBeTruthy();
  expect(screen.getByText("Nothing selected")).toBeTruthy();
});

it("turns cleaning off while elevated", async () => {
  await scanned(true);

  const clean = screen.getByRole("button", { name: "Clean…" });
  expect(clean.hasAttribute("disabled") || clean.getAttribute("aria-disabled") === "true").toBe(
    true,
  );
  expect(screen.getByText("Cleaning is off while Cleanse runs elevated.")).toBeTruthy();
});

it("explains an item when it's opened", async () => {
  const { user } = await scanned();

  await user.click(screen.getByRole("button", { name: /^Git repository data/ }));
  const list = await findGrid("Git repository data, Kept");
  await user.click(within(list).getByText(/notes\\\.git/));

  const panel = await screen.findByRole("complementary", { name: ".git" });
  expect(panel.textContent).toContain("Git repository data");
  expect(panel.textContent).toContain("Protected, kept");
  expect(panel.textContent).toContain("Git tracks it, or files inside it keeps it");
});

it("opens a chosen item's details from its button, and goes back there on closing", async () => {
  const { user } = await scanned();
  await user.click(screen.getByRole("button", { name: /^Node\.js dependencies/ }));
  const list = await findGrid("Node.js dependencies, Can be removed");
  const first = within(list).getAllByRole("row")[0] as HTMLElement;
  const button = within(first).getByRole("button", { name: "Details" });

  await user.click(button);

  const panel = await screen.findByRole("complementary", { name: "node_modules" });
  expect(panel.textContent).toContain("Next to package.json");
  expect(screen.getByText("Selected: 2 items, 3.1 GiB")).toBeTruthy();
  await user.click(within(panel).getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("complementary")).toBeNull();
  expect(document.activeElement).toBe(button);
});
