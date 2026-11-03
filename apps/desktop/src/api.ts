// The app's commands, one typed function each. The types come from the Rust side's
// `src-tauri/src/views.rs`, through `src/bindings/`.

import { invoke } from "@tauri-apps/api/core";
import type { AppInfo } from "./bindings/AppInfo";

export function appInfo(): Promise<AppInfo> {
  return invoke("app_info");
}
