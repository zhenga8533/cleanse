import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import { App } from "./App";
import type { AppInfo } from "./bindings/AppInfo";
import type { Folders } from "./bindings/Folders";

function answer(info: AppInfo): void {
  mockIPC((cmd) => {
    switch (cmd) {
      case "app_info":
        return info;
      case "folders":
        return { folders: ["C:\\Users\\sample"], home: true } satisfies Folders;
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
}

it("shows the version once the engine answers", async () => {
  answer({ version: "1.2.3", elevated: false });

  render(<App />);

  expect(await screen.findByText("1.2.3")).toBeTruthy();
  expect(screen.queryByRole("alert")).toBeNull();
});

it("warns that nothing changes while elevated", async () => {
  answer({ version: "1.2.3", elevated: true });

  render(<App />);

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("won't change anything");
});

it("says why when the engine couldn't start", async () => {
  mockIPC(() => {
    throw "your settings file has a problem";
  });

  render(<App />);

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("your settings file has a problem");
});

it("moves between sections with the keyboard", async () => {
  answer({ version: "1.2.3", elevated: false });
  const user = userEvent.setup();
  render(<App />);

  await user.click(screen.getByRole("tab", { name: "Scan" }));
  await user.keyboard("{ArrowDown}");

  expect(screen.getByRole("tab", { name: "Disk usage" }).getAttribute("aria-selected")).toBe(
    "true",
  );
  expect(screen.getByRole("heading", { name: "Disk usage" })).toBeTruthy();
});
