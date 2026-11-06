import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import type { SettingsView } from "../bindings/SettingsView";
import { RULE_VIEWS, SETTINGS } from "../dev/sample";
import { findGrid } from "../test/grids";
import { matches, SettingsScreen } from "./SettingsScreen";

function engine(settings: SettingsView = SETTINGS): void {
  mockIPC((cmd) => {
    switch (cmd) {
      case "settings":
        return settings;
      case "rules":
        return RULE_VIEWS;
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
}

it("reports how Cleanse is set up, as `cleanse doctor` does", async () => {
  engine();
  render(<SettingsScreen />);

  const doctor = await screen.findByRole("region", {
    name: "Cleanse 0.0.0-preview on windows (x86_64)",
  });
  expect(doctor.textContent).toContain("5 built in, 1 turned off in your settings");
  expect(doctor.textContent).toContain("C:\\Users\\sample\\AppData\\Roaming\\cleanse\\config.toml");
  expect(doctor.textContent).toContain("C:\\Users\\sample\\code\\keep");
  expect(doctor.textContent).toContain("1.0 GiB or more on disk");
  expect(doctor.textContent).toContain("Temp folderC:\\Users\\sample\\AppData\\Local\\Temp");
  expect(doctor.textContent).toContain("Running as administrator or rootNo");
});

it("says whether cleaning works when Cleanse runs elevated", async () => {
  for (const [settings, says] of [
    [{ ...SETTINGS, elevated: true }, "Yes, so cleaning your own files is off"],
    [
      { ...SETTINGS, elevated: true, alwaysElevated: true },
      "Yes, as everything this account runs is, as with UAC off, so cleaning your own files still works",
    ],
  ] as const) {
    engine(settings);
    const { unmount } = render(<SettingsScreen />);
    const doctor = await screen.findByRole("region", {
      name: "Cleanse 0.0.0-preview on windows (x86_64)",
    });
    expect(doctor.textContent).toContain(`Running as administrator or root${says}`);
    unmount();
  }
});

it("shows a rule in detail when it's opened, and goes back on closing", async () => {
  engine();
  const user = userEvent.setup();
  render(<SettingsScreen />);
  const table = await findGrid("Rules");
  const row = within(table).getByRole("row", { name: /pnpm store/ });

  await user.click(within(row).getByText("pnpm store"));

  const panel = await screen.findByRole("complementary", { name: "pnpm store" });
  expect(panel.textContent).toContain("pnpm.store");
  expect(panel.textContent).toContain("Clean withpnpm store prune");
  expect(panel.textContent).toContain("MatchesDataLocal/pnpm/store");
  await user.click(within(panel).getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("complementary")).toBeNull();
});

it("says what each kind of rule matches, as `cleanse rules show` does", () => {
  expect(RULE_VIEWS.map((rule) => matches(rule.matches))).toEqual([
    ["a folder named node_modules", "next to any of package.json"],
    ["DataLocal/pnpm/store"],
    ["files and folders at Temp/*"],
    [
      "files taking 1.0 GiB or more on disk (large_file_size in your settings),",
      "outside the folders apps keep their data in",
    ],
  ]);
  expect(matches({ kind: "knownLocation", locations: [], entry: "folder" })).toEqual([
    "nothing on this operating system",
  ]);
});
