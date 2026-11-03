import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { folder, HOME_FOLDER, results } from "../dev/sample";
import { findGrid } from "../test/grids";
import { DiskScreen } from "./DiskScreen";

/** Answers with the sample tree, and records what each `folder` call asked for. */
function engine(): { node: unknown; categories: unknown }[] {
  const asked: { node: unknown; categories: unknown }[] = [];
  mockIPC((cmd, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    switch (cmd) {
      case "summary":
        return results(1);
      case "folder":
        asked.push({ node: args.node, categories: args.categories });
        return folder(
          args.node as number | null,
          args.limit as number,
          args.categories as string[],
        );
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
  return asked;
}

function names(table: HTMLElement): string[] {
  return within(table)
    .getAllByRole("rowheader")
    .map((cell) => cell.textContent ?? "");
}

it("asks for a scan first", async () => {
  const onScan = vi.fn();
  const user = userEvent.setup();
  render(<DiskScreen latest={null} onScan={onScan} />);

  await user.click(screen.getByRole("button", { name: "Go to Scan" }));

  expect(onScan).toHaveBeenCalledOnce();
});

it("starts inside the one folder scanned, largest first, with a picture of the same", async () => {
  engine();
  render(<DiskScreen latest={{ scan: 1, version: 1 }} onScan={() => {}} />);

  const table = await findGrid(`What's in ${HOME_FOLDER}`);
  expect(names(table)).toEqual(["Videos", "code", "AppData", "Downloads", "8 files"]);
  expect(
    screen.getByRole("img", { name: /What takes the space in C:\\Users\\sample/ }),
  ).toBeTruthy();
  const code = within(table).getByRole("row", { name: /^code/ });
  expect(code.textContent).toContain("3.6 GiB");
  expect(code.textContent).toContain("Folder");
});

it("opens a folder from its row, and goes back up from the trail", async () => {
  engine();
  const user = userEvent.setup();
  render(<DiskScreen latest={{ scan: 1, version: 1 }} onScan={() => {}} />);
  const table = await findGrid(`What's in ${HOME_FOLDER}`);

  await user.click(within(table).getByRole("rowheader", { name: "code" }));

  const inside = await findGrid("What's in code");
  expect(names(inside)).toEqual(["engine", "web", "site", "notes"]);
  await user.click(within(inside).getByRole("rowheader", { name: "engine" }));
  const engineTable = await findGrid("What's in engine");
  const target = within(engineTable).getByRole("row", { name: /^target/ });
  expect(target.textContent).toContain("Rust build output, Regenerable");

  const trail = screen.getByRole("navigation", { name: "Folders" });
  await user.click(within(trail).getByText(HOME_FOLDER));
  expect(await findGrid(`What's in ${HOME_FOLDER}`)).toBeTruthy();
});

it("sorts by name", async () => {
  engine();
  const user = userEvent.setup();
  render(<DiskScreen latest={{ scan: 1, version: 1 }} onScan={() => {}} />);
  const table = await findGrid(`What's in ${HOME_FOLDER}`);

  await user.click(within(table).getByRole("columnheader", { name: "Name" }));

  await waitFor(() =>
    expect(names(table)).toEqual(["8 files", "AppData", "code", "Downloads", "Videos"]),
  );
});

it("counts only the categories chosen as removable", async () => {
  const asked = engine();
  const user = userEvent.setup();
  render(<DiskScreen latest={{ scan: 1, version: 1 }} onScan={() => {}} />);
  const table = await findGrid(`What's in ${HOME_FOLDER}`);
  const code = () => within(table).getByRole("row", { name: /^code/ }).textContent;
  expect(code()).toContain("3.6 GiB3.5 GiB");

  await user.click(screen.getByRole("button", { name: "Removable counts all categories" }));
  await user.click(await screen.findByRole("checkbox", { name: /Build output/ }));
  await user.keyboard("{Escape}");

  await waitFor(() => expect(code()).toContain("3.6 GiB2.3 GiB"));
  expect(asked.at(-1)?.categories).toEqual(["build_output"]);
  expect(screen.getByRole("button", { name: "Removable counts Build output" })).toBeTruthy();
});

it("opens a folder with the keyboard", async () => {
  engine();
  const user = userEvent.setup();
  render(<DiskScreen latest={{ scan: 1, version: 1 }} onScan={() => {}} />);
  const table = await findGrid(`What's in ${HOME_FOLDER}`);

  within(table).getByRole("columnheader", { name: "Name" }).focus();
  await user.keyboard("{ArrowDown}{ArrowDown}{Enter}");

  expect(await findGrid("What's in code")).toBeTruthy();
});
