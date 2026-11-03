// The app's commands, one typed function each. The types come from the Rust side's
// `src-tauri/src/views.rs`, through `src/bindings/`.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { AppInfo } from "./bindings/AppInfo";
import type { CleanReport } from "./bindings/CleanReport";
import type { Details } from "./bindings/Details";
import type { Folders } from "./bindings/Folders";
import type { FolderView } from "./bindings/FolderView";
import type { Handled } from "./bindings/Handled";
import type { HistoryView } from "./bindings/HistoryView";
import type { ItemPage } from "./bindings/ItemPage";
import type { QuarantineView } from "./bindings/QuarantineView";
import type { Results } from "./bindings/Results";
import type { RuleView } from "./bindings/RuleView";
import type { ScanProgress } from "./bindings/ScanProgress";
import type { Section } from "./bindings/Section";
import type { SettingsView } from "./bindings/SettingsView";

export function appInfo(): Promise<AppInfo> {
  return invoke("app_info");
}

/** The folders a scan covers: those chosen, or the home folder. */
export function folders(): Promise<Folders> {
  return invoke("folders");
}

/** Opens the system's folder picker, and adds what's chosen to the folders a scan covers. */
export function pickFolders(): Promise<Folders> {
  return invoke("pick_folders");
}

/** Stops covering the chosen folder at `index`. */
export function removeFolder(index: number): Promise<Folders> {
  return invoke("remove_folder", { index });
}

/** Scans the folders. The results replace the last scan's. */
export function scan(onProgress: (progress: ScanProgress) => void): Promise<Results> {
  return invoke("scan", { progress: new Channel(onProgress) });
}

/** Asks the scan or cleanup running now to stop. */
export function cancel(): Promise<void> {
  return invoke("cancel");
}

/** A scan's results as they are now, after any cleanups. */
export function summary(scan: number): Promise<Results> {
  return invoke("summary", { scan });
}

/** The items in `section`, and of `rule` if given, largest first. */
export function candidates(
  scan: number,
  section: Section,
  rule: string | null,
  offset: number,
  limit: number,
): Promise<ItemPage> {
  return invoke("candidates", { scan, section, rule, offset, limit });
}

export function explain(scan: number, item: number): Promise<Details> {
  return invoke("explain", { scan, item });
}

/**
 * Folder `node` of scan `scan`, or its roots, with its largest entries up to `limit`. Only items
 * in `categories` count as removable, or every item if there are none, as `--category` does.
 */
export function folder(
  scan: number,
  node: number | null,
  limit: number,
  categories: string[],
): Promise<FolderView> {
  return invoke("folder", { scan, node, limit, categories });
}

/** Removes `items` from scan `scan`, each the way its tier says, after checking each again. */
export function clean(
  scan: number,
  items: number[],
  onProgress: (done: number) => void,
): Promise<CleanReport> {
  return invoke("clean", { scan, items, progress: new Channel(onProgress) });
}

/** What's in quarantine, oldest first. */
export function quarantined(): Promise<QuarantineView> {
  return invoke("quarantined");
}

/** Puts back the quarantined items `ids` names, unless something has taken their place. */
export function restore(ids: string[]): Promise<Handled> {
  return invoke("restore", { ids });
}

/** Deletes the quarantined items `ids` names for good. */
export function purge(ids: string[]): Promise<Handled> {
  return invoke("purge", { ids });
}

/** What Cleanse has done before, most recent first, and what's left of removals that didn't finish. */
export function history(): Promise<HistoryView> {
  return invoke("history");
}

/** Removes what's left of every removal that didn't finish, if it's still what was approved. */
export function finish(): Promise<Handled> {
  return invoke("finish");
}

/** How Cleanse is set up on this system, as `cleanse doctor` reports it. */
export function settings(): Promise<SettingsView> {
  return invoke("settings");
}

/** Every rule the settings don't turn off. */
export function rules(): Promise<RuleView[]> {
  return invoke("rules");
}
