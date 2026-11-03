import type { FolderEntry } from "../bindings/FolderEntry";
import type { FolderView } from "../bindings/FolderView";
import { count } from "../format";
import { RISKS } from "../scan/sections";

/** A line of a folder's breakdown: one of its entries, or the files directly in it. */
export interface Row {
  key: string;
  name: string;
  onDisk: number;
  removable: number;
  what: string;
  /** Part of it couldn't be read, so it takes more than shown. */
  incomplete: boolean;
  /** The folder it opens, if it has entries to show. */
  opens: number | null;
}

export function rows(view: FolderView): Row[] {
  const rows: Row[] = view.entries.map((entry) => ({
    key: String(entry.node),
    name: entry.name,
    onDisk: entry.usage.onDisk,
    removable: entry.removable,
    what: describe(entry),
    incomplete: entry.incomplete,
    opens: entry.kind === "folder" && entry.hasChildren ? entry.node : null,
  }));
  if (view.ownFiles > 0) {
    rows.push({
      key: "files",
      name: view.ownFiles === 1 ? "1 file" : `${count(view.ownFiles)} files`,
      onDisk: view.ownUsage.onDisk,
      removable: view.ownRemovable,
      what: "Files directly in this folder",
      incomplete: false,
      opens: null,
    });
  }
  return rows;
}

/** What an entry is, as `cleanse analyze` labels it. */
export function describe(entry: FolderEntry): string {
  switch (entry.kind) {
    case "folder":
      return "Folder";
    case "otherFilesystem":
      return "Another filesystem, not measured";
    case "item":
      if (!entry.item) {
        return "Removed since the scan";
      }
      return entry.item.command
        ? `${entry.item.ruleName}, clean with ${entry.item.command}`
        : `${entry.item.ruleName}, ${RISKS[entry.item.risk]}`;
  }
}
