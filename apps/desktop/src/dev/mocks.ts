import { mockIPC } from "@tauri-apps/api/mocks";
import type { AppInfo } from "../bindings/AppInfo";

/** Answers the app's commands with sample data, for reviewing the UI in a browser. */
export function installMocks(): void {
  mockIPC((cmd) => {
    switch (cmd) {
      case "app_info":
        return { version: "0.0.0-preview", elevated: false } satisfies AppInfo;
      default:
        throw new Error(`The browser preview has no sample data for ${cmd}`);
    }
  });
}
