import type { Channel } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { AppInfo } from "../bindings/AppInfo";
import type { CleanReport } from "../bindings/CleanReport";
import type { Folders } from "../bindings/Folders";
import type { Handled } from "../bindings/Handled";
import type { Item } from "../bindings/Item";
import type { ItemPage } from "../bindings/ItemPage";
import type { ScanProgress } from "../bindings/ScanProgress";
import type { Section } from "../bindings/Section";
import type { SystemStatus } from "../bindings/SystemStatus";
import {
  details,
  folder,
  HISTORY,
  HOME_FOLDER,
  ITEMS,
  QUARANTINE,
  RULE_VIEWS,
  results,
  SETTINGS,
} from "./sample";

/** Answers the app's commands with sample data, for reviewing the UI in a browser. */
export function installMocks(): void {
  let chosen: string[] = [];
  let scan = 0;
  let items = structuredClone(ITEMS);
  let removed = 0;
  let cancelled = false;
  let quarantine = structuredClone(QUARANTINE);
  let history = structuredClone(HISTORY);

  const folders = (): Folders =>
    chosen.length > 0 ? { folders: chosen, home: false } : { folders: [HOME_FOLDER], home: true };
  const find = (id: number): [Item, Section] | undefined => {
    for (const section of Object.keys(items) as Section[]) {
      const found = items[section].find((item) => item.id === id);
      if (found) {
        return [found, section];
      }
    }
    return undefined;
  };

  mockIPC(async (asked, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    // System cleanup's commands answer with the same sample data as the user's own, which is
    // enough to review how its screens look. `?system=unavailable` shows the section without it.
    const cmd = asked.replace(/^system_/, "");
    switch (cmd) {
      case "status":
        return {
          supported: true,
          unavailable: new URLSearchParams(window.location.search).has("system")
            ? "this copy of Cleanse wasn't installed with its system cleanup helper"
            : null,
          running: false,
        } satisfies SystemStatus;
      case "app_info":
        return { version: "0.0.0-preview", elevated: false } satisfies AppInfo;
      case "folders":
        return folders();
      case "pick_folders":
        chosen = [...new Set([...chosen, `${HOME_FOLDER}\\code`])];
        return folders();
      case "remove_folder":
        chosen.splice(args.index as number, 1);
        return folders();
      case "scan": {
        const progress = args.progress as Channel<ScanProgress>;
        cancelled = false;
        for (let entries = 0; entries <= 480_000 && !cancelled; entries += 40_000) {
          progress.onmessage({ kind: "walking", entries });
          await pause(120);
        }
        for (let done = 1; done <= 6 && !cancelled; done += 1) {
          progress.onmessage({ kind: "assessing", done, total: 6 });
          await pause(120);
        }
        scan += 1;
        items = structuredClone(ITEMS);
        removed = 0;
        return { ...results(scan, items), cancelled };
      }
      case "cancel":
        cancelled = true;
        return null;
      case "summary":
        return {
          ...results(scan, items),
          removed: { logical: removed, onDisk: removed, guaranteed: removed },
        };
      case "candidates": {
        const { section, rule, offset, limit } = args as {
          section: Section;
          rule: string | null;
          offset: number;
          limit: number;
        };
        const listed = items[section].filter((item) => rule === null || item.rule === rule);
        return {
          items: listed.slice(offset, offset + limit),
          offset,
          total: listed.length,
        } satisfies ItemPage;
      }
      case "explain": {
        const found = find(args.item as number);
        if (!found) {
          throw `The scan has no item ${String(args.item)}`;
        }
        return details(...found);
      }
      case "folder":
        return folder(
          args.node as number | null,
          args.limit as number,
          args.categories as string[],
          items,
        );
      case "clean":
        return clean(args.items as number[], args.progress as Channel<number>);
      case "quarantined":
        return quarantine;
      case "restore":
      case "purge": {
        const ids = args.ids as string[];
        const handled = quarantine.entries.filter((entry) => ids.includes(entry.id));
        await pause(400);
        quarantine = {
          ...quarantine,
          entries: quarantine.entries.filter((entry) => !ids.includes(entry.id)),
        };
        const size = handled.reduce((sum, entry) => sum + entry.size.onDisk, 0);
        return {
          done: { items: handled.length, size: { logical: size, onDisk: size, guaranteed: size } },
          failed: [],
          journal: `${HOME_FOLDER}\\AppData\\Local\\cleanse\\journal\\run-4.jsonl`,
        } satisfies Handled;
      }
      case "history":
        return history;
      case "finish": {
        const left = history.leftovers;
        await pause(400);
        history = { ...history, leftovers: [] };
        return {
          done: { items: 0, size: { logical: 0, onDisk: 0, guaranteed: 0 } },
          failed: left.map((leftover) => ({
            path: leftover.tombstone,
            reason: "Something else is there now",
          })),
          journal: `${HOME_FOLDER}\\AppData\\Local\\cleanse\\journal\\run-5.jsonl`,
        } satisfies Handled;
      }
      case "settings":
        return SETTINGS;
      case "rules":
        return RULE_VIEWS;
      default:
        throw new Error(`The browser preview has no sample data for ${asked}`);
    }
  });

  /** Removes everything but the Review items, which are skipped as if they'd changed. */
  async function clean(ids: number[], progress: Channel<number>): Promise<CleanReport> {
    const report: CleanReport = {
      deleted: { items: 0, size: { logical: 0, onDisk: 0, guaranteed: 0 } },
      quarantined: { items: 0, size: { logical: 0, onDisk: 0, guaranteed: 0 } },
      skipped: [],
      failed: [],
      journal: `${HOME_FOLDER}\\AppData\\Local\\cleanse\\journal\\run-1.jsonl`,
    };
    for (const [done, id] of ids.entries()) {
      progress.onmessage(done);
      await pause(200);
      const found = find(id);
      if (!found) {
        continue;
      }
      const [item, section] = found;
      if (section === "review") {
        report.skipped.push({ path: item.path, reason: "It changed after you approved it" });
        continue;
      }
      report.deleted.items += 1;
      report.deleted.size.onDisk += item.size.onDisk;
      report.deleted.size.logical += item.size.logical;
      report.deleted.size.guaranteed += item.size.guaranteed;
      removed += item.size.onDisk;
      items[section] = items[section].filter((other) => other.id !== id);
    }
    return report;
  }
}

function pause(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
