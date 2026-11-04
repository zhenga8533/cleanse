import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import { App } from "./App";
import type { AppInfo } from "./bindings/AppInfo";
import type { Folders } from "./bindings/Folders";
import type { SystemStatus } from "./bindings/SystemStatus";

const SYSTEM: SystemStatus = { supported: true, unavailable: null, running: false };

function answer(info: AppInfo, system = SYSTEM): void {
  mockIPC((cmd) => {
    switch (cmd) {
      case "app_info":
        return info;
      case "system_status":
        return system;
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

it("has a System section where system cleanup is supported", async () => {
  answer({ version: "1.2.3", elevated: false });

  render(<App />);

  expect(await screen.findByRole("tab", { name: "System" })).toBeTruthy();
});

it("has no System section on macOS, which has no system cleanup", async () => {
  answer(
    { version: "1.2.3", elevated: false },
    {
      supported: false,
      unavailable: "system cleanup is only for Windows and Linux",
      running: false,
    },
  );

  render(<App />);

  await screen.findByText("1.2.3");
  expect(screen.queryByRole("tab", { name: "System" })).toBeNull();
});
