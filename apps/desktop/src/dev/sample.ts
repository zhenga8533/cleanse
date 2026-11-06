// Made-up results, for reviewing the UI in a browser and for tests. Nothing here is real.

import type { CategoryTotal } from "../bindings/CategoryTotal";
import type { Details } from "../bindings/Details";
import type { FolderEntry } from "../bindings/FolderEntry";
import type { FolderView } from "../bindings/FolderView";
import type { HistoryView } from "../bindings/HistoryView";
import type { Item } from "../bindings/Item";
import type { QuarantineView } from "../bindings/QuarantineView";
import type { Results } from "../bindings/Results";
import type { RuleTotal } from "../bindings/RuleTotal";
import type { RuleView } from "../bindings/RuleView";
import type { Section } from "../bindings/Section";
import type { SettingsView } from "../bindings/SettingsView";
import type { Size } from "../bindings/Size";

const HOME = "C:\\Users\\sample";
const DAY = 24 * 60 * 60;
const MIB = 1024 * 1024;

function bytes(onDisk: number, guaranteed = onDisk): Size {
  return { logical: onDisk, onDisk, guaranteed };
}

interface Rule {
  rule: string;
  name: string;
  category: [id: string, name: string];
  explain: string;
  regenerate: string | null;
}

const RULES: Record<string, Rule> = {
  "javascript.node_modules": {
    rule: "javascript.node_modules",
    name: "Node.js dependencies",
    category: ["dependencies", "Dependencies"],
    explain: "Installed packages, which the project's package manager installs again.",
    regenerate: "Run npm install, or your package manager's install, in the project.",
  },
  "rust.target": {
    rule: "rust.target",
    name: "Rust build output",
    category: ["build_output", "Build output"],
    explain: "Compiled output, which Cargo builds again.",
    regenerate: "Run cargo build in the project.",
  },
  "downloads.installers": {
    rule: "downloads.installers",
    name: "Installers in Downloads",
    category: ["downloads", "Downloads"],
    explain:
      "Installers are usually only needed once, but only you know whether you'll need one again.",
    regenerate: null,
  },
  "pnpm.store": {
    rule: "pnpm.store",
    name: "pnpm store",
    category: ["package_caches", "Package caches"],
    explain: "pnpm's package store, which pnpm cleans itself without breaking projects.",
    regenerate: null,
  },
  "git.repository": {
    rule: "git.repository",
    name: "Git repository data",
    category: ["git", "Git"],
    explain: "A repository's history, which can't be recreated.",
    regenerate: null,
  },
};

/** Every item, in each section, largest first. */
export const ITEMS: Record<Section, Item[]> = {
  removable: [
    item(
      1,
      `${HOME}\\code\\web\\node_modules`,
      "javascript.node_modules",
      "regenerable",
      812 * MIB,
      {
        disposal: "delete",
        preselected: true,
      },
    ),
    item(2, `${HOME}\\code\\engine\\target`, "rust.target", "regenerable", 2400 * MIB, {
      disposal: "delete",
      preselected: true,
    }),
    item(
      3,
      `${HOME}\\code\\site\\node_modules`,
      "javascript.node_modules",
      "regenerable",
      340 * MIB,
      {
        disposal: "delete",
        preselected: false,
        concerns: [{ message: "The project has uncommitted changes", blocks: false }],
      },
    ),
  ],
  review: [
    item(4, `${HOME}\\Downloads\\setup-old-tool.exe`, "downloads.installers", "review", 96 * MIB, {
      disposal: "quarantine",
      preselected: false,
      daysAgo: 210,
    }),
  ],
  advice: [
    item(5, `${HOME}\\AppData\\Local\\pnpm\\store`, "pnpm.store", "protected", 1800 * MIB, {
      command: "pnpm store prune",
    }),
  ],
  kept: [
    item(6, `${HOME}\\code\\notes\\.git`, "git.repository", "protected", 12 * MIB, {
      concerns: [{ message: "Git tracks it, or files inside it", blocks: true }],
    }),
  ],
};

function item(
  id: number,
  path: string,
  rule: string,
  risk: Item["risk"],
  onDisk: number,
  more: {
    disposal?: Item["disposal"];
    preselected?: boolean;
    concerns?: Item["concerns"];
    command?: string;
    daysAgo?: number;
  },
): Item {
  return {
    id,
    path,
    rule,
    risk,
    size: bytes(onDisk),
    files: Math.round(onDisk / 20_000),
    modified: Math.floor(Date.now() / 1000) - (more.daysAgo ?? 3) * DAY,
    concerns: more.concerns ?? [],
    disposal: more.disposal ?? null,
    command: more.command ?? null,
    preselected: more.preselected ?? false,
  };
}

/** Results as a scan with `scan` as its ID would give them, holding `items`. */
export function results(scan: number, items: Record<Section, Item[]> = ITEMS): Results {
  const sections = (Object.keys(items) as Section[]).map((section) => {
    const listed = items[section];
    const rules = new Map<string, RuleTotal>();
    for (const found of listed) {
      const total = rules.get(found.rule) ?? {
        rule: found.rule,
        name: RULES[found.rule]?.name ?? found.rule,
        items: 0,
        size: bytes(0),
      };
      total.items += 1;
      total.size = bytes(total.size.onDisk + found.size.onDisk);
      rules.set(found.rule, total);
    }
    return {
      section,
      items: listed.length,
      size: bytes(listed.reduce((sum, found) => sum + found.size.onDisk, 0)),
      rules: [...rules.values()].sort((a, b) => b.size.onDisk - a.size.onDisk),
    };
  });
  return {
    scan,
    roots: [HOME],
    entries: 482_113,
    seconds: 6.4,
    cancelled: false,
    unreadable: 2,
    sections,
    removed: bytes(0),
    preselected: items.removable
      .filter((found) => found.preselected)
      .flatMap((found) =>
        found.disposal ? [{ id: found.id, size: found.size, disposal: found.disposal }] : [],
      ),
    categories: categories([...items.removable, ...items.review]),
  };
}

function categories(removable: Item[]): CategoryTotal[] {
  const totals = new Map<string, CategoryTotal>();
  for (const found of removable) {
    const [category, name] = RULES[found.rule]?.category ?? [found.rule, found.rule];
    const total = totals.get(category) ?? { category, name, items: 0, size: bytes(0) };
    total.items += 1;
    total.size = bytes(total.size.onDisk + found.size.onDisk);
    totals.set(category, total);
  }
  return [...totals.values()].sort((a, b) => b.size.onDisk - a.size.onDisk);
}

/** A folder in the sample tree: its own files, its folders, or the item a rule found there. */
interface Shape {
  name: string;
  item?: number;
  files?: number;
  onDisk?: number;
  children?: Shape[];
}

const TREE: Shape = {
  name: HOME,
  files: 8,
  onDisk: 2 * MIB,
  children: [
    {
      name: "code",
      children: [
        { name: "web", files: 40, onDisk: 30 * MIB, children: [{ name: "node_modules", item: 1 }] },
        { name: "engine", files: 120, onDisk: 90 * MIB, children: [{ name: "target", item: 2 }] },
        {
          name: "site",
          files: 25,
          onDisk: 12 * MIB,
          children: [{ name: "node_modules", item: 3 }],
        },
        { name: "notes", files: 300, onDisk: 40 * MIB, children: [{ name: ".git", item: 6 }] },
      ],
    },
    {
      name: "Downloads",
      files: 14,
      onDisk: 700 * MIB,
      children: [{ name: "setup-old-tool.exe", item: 4 }],
    },
    {
      name: "AppData",
      children: [
        {
          name: "Local",
          files: 2000,
          onDisk: 1400 * MIB,
          children: [{ name: "pnpm", children: [{ name: "store", item: 5 }] }],
        },
      ],
    },
    { name: "Videos", files: 30, onDisk: 9000 * MIB },
  ],
};

interface Node {
  shape: Shape;
  path: string;
  parent: number | null;
  children: number[];
}

const NODES: Node[] = [];
(function index(shape: Shape, path: string, parent: number | null): number {
  const at = NODES.push({ shape, path, parent, children: [] }) - 1;
  for (const child of shape.children ?? []) {
    NODES[at]?.children.push(index(child, `${path}\\${child.name}`, at));
  }
  return at;
})(TREE, HOME, null);

function find(id: number): Item | undefined {
  return Object.values(ITEMS)
    .flat()
    .find((item) => item.id === id);
}

function onDisk(at: number): number {
  const node = NODES[at] as Node;
  if (node.shape.item !== undefined) {
    return find(node.shape.item)?.size.onDisk ?? 0;
  }
  return (node.shape.onDisk ?? 0) + node.children.reduce((sum, child) => sum + onDisk(child), 0);
}

function files(at: number): number {
  const node = NODES[at] as Node;
  if (node.shape.item !== undefined) {
    return find(node.shape.item)?.files ?? 0;
  }
  return (node.shape.files ?? 0) + node.children.reduce((sum, child) => sum + files(child), 0);
}

/** What's removable at or under `at`: the items still in `items`, in `counted` if any. */
function removable(at: number, items: Record<Section, Item[]>, counted: string[]): number {
  const node = NODES[at] as Node;
  const id = node.shape.item;
  if (id !== undefined) {
    const found = [...items.removable, ...items.review].find((item) => item.id === id);
    const category = found && RULES[found.rule]?.category[0];
    return found && (counted.length === 0 || counted.includes(category ?? ""))
      ? found.size.onDisk
      : 0;
  }
  return node.children.reduce((sum, child) => sum + removable(child, items, counted), 0);
}

function entry(at: number, items: Record<Section, Item[]>, counted: string[]): FolderEntry {
  const node = NODES[at] as Node;
  const id = node.shape.item;
  const found =
    id === undefined
      ? undefined
      : Object.values(items)
          .flat()
          .find((item) => item.id === id);
  const usage = onDisk(at);
  return {
    node: at,
    name: node.parent === null ? node.path : node.shape.name,
    path: node.path,
    kind: id === undefined ? "folder" : "item",
    usage: { logical: usage, onDisk: usage },
    files: files(at),
    removable: removable(at, items, counted),
    incomplete: false,
    hasChildren: node.children.length > 0,
    item: found
      ? {
          id: found.id,
          ruleName: RULES[found.rule]?.name ?? found.rule,
          risk: found.risk,
          command: found.command,
        }
      : null,
  };
}

/** The sample tree's folder `node`, or its root, as the `folder` command gives it. */
export function folder(
  node: number | null,
  limit: number,
  counted: string[],
  items: Record<Section, Item[]> = ITEMS,
): FolderView {
  if (node === null) {
    return {
      trail: [],
      entries: [entry(0, items, counted)],
      hidden: 0,
      hiddenOnDisk: 0,
      ownFiles: 0,
      ownUsage: { logical: 0, onDisk: 0 },
      ownRemovable: 0,
    };
  }
  const at = NODES[node];
  if (!at) {
    throw `The scan has no folder ${node}`;
  }
  const trail: FolderEntry[] = [];
  for (let up: number | null = node; up !== null; up = NODES[up]?.parent ?? null) {
    trail.unshift(entry(up, items, counted));
  }
  const children = at.children
    .map((child) => entry(child, items, counted))
    .sort((a, b) => b.usage.onDisk - a.usage.onDisk);
  const shown = children.slice(0, limit);
  const own = at.shape.item === undefined ? (at.shape.onDisk ?? 0) : 0;
  return {
    trail,
    entries: shown,
    hidden: children.length - shown.length,
    hiddenOnDisk: children.slice(limit).reduce((sum, child) => sum + child.usage.onDisk, 0),
    ownFiles: at.shape.item === undefined ? (at.shape.files ?? 0) : 0,
    ownUsage: { logical: own, onDisk: own },
    ownRemovable: 0,
  };
}

export function details(found: Item, section: Section): Details {
  const rule = RULES[found.rule];
  return {
    item: found,
    section,
    ruleName: rule?.name ?? found.rule,
    explain: rule?.explain ?? "",
    regenerate: rule?.regenerate ?? null,
    evidence: found.rule === "javascript.node_modules" ? ["Next to package.json"] : [],
  };
}

export const HOME_FOLDER = HOME;

const NOW = Math.floor(Date.now() / 1000);

export const QUARANTINE: QuarantineView = {
  entries: [
    {
      id: "q-2025-1",
      original: `${HOME}\\Downloads\\driver-pack.zip`,
      rule: "downloads.installers",
      ruleName: "Installers in Downloads",
      size: bytes(640 * MIB),
      at: NOW - 40 * DAY,
    },
    {
      id: "q-2026-7",
      original: `${HOME}\\Downloads\\old-setup.msi`,
      rule: "downloads.installers",
      ruleName: "Installers in Downloads",
      size: bytes(220 * MIB),
      at: NOW - 2 * DAY,
    },
  ],
  unreadable: [],
};

const NONE = { items: 0, size: bytes(0) };

export const HISTORY: HistoryView = {
  runs: [
    {
      journal: `${HOME}\\AppData\\Local\\cleanse\\journal\\run-3.jsonl`,
      at: NOW - 2 * DAY,
      command: "clean",
      items: 4,
      deleted: { items: 2, size: bytes(3212 * MIB) },
      quarantined: { items: 1, size: bytes(220 * MIB) },
      restored: 0,
      purged: NONE,
      skipped: 1,
      failed: 0,
      cutShort: false,
    },
    {
      journal: `${HOME}\\AppData\\Local\\cleanse\\journal\\run-2.jsonl`,
      at: NOW - 30 * DAY,
      command: "purge",
      items: 1,
      deleted: NONE,
      quarantined: NONE,
      restored: 0,
      purged: { items: 1, size: bytes(90 * MIB) },
      skipped: 0,
      failed: 0,
      cutShort: false,
    },
    {
      journal: `${HOME}\\AppData\\Local\\cleanse\\journal\\run-1.jsonl`,
      at: NOW - 41 * DAY,
      command: "clean",
      items: 3,
      deleted: { items: 1, size: bytes(400 * MIB) },
      quarantined: { items: 1, size: bytes(640 * MIB) },
      restored: 0,
      purged: NONE,
      skipped: 0,
      failed: 0,
      cutShort: true,
    },
  ],
  leftovers: [
    {
      original: `${HOME}\\code\\old-app\\node_modules`,
      tombstone: `${HOME}\\code\\old-app\\.cleanse-removing-node_modules`,
      rule: "javascript.node_modules",
    },
  ],
  unreadable: [],
};

export const SETTINGS: SettingsView = {
  version: "0.0.0-preview",
  os: "windows",
  arch: "x86_64",
  elevated: false,
  alwaysElevated: false,
  path: `${HOME}\\AppData\\Roaming\\cleanse\\config.toml`,
  exists: true,
  exclude: [`${HOME}\\code\\keep`],
  disabledRules: ["browsers.edge_cache"],
  largeFileSize: 1024 * MIB,
  cloudSyncDetection: "Windows Cloud Files API (OneDrive, iCloud, Dropbox, and other providers)",
  folders: [
    { name: "Home", path: HOME },
    { name: "Documents", path: `${HOME}\\Documents` },
    { name: "Downloads", path: `${HOME}\\Downloads` },
    { name: "Cache", path: `${HOME}\\AppData\\Local` },
    { name: "App data", path: `${HOME}\\AppData\\Local` },
    { name: "Settings", path: `${HOME}\\AppData\\Roaming` },
    { name: "Temp", path: `${HOME}\\AppData\\Local\\Temp` },
  ],
};

export const RULE_VIEWS: RuleView[] = [
  {
    id: "javascript.node_modules",
    name: "Node.js dependencies",
    category: "Dependencies",
    risk: "regenerable",
    explain: "Installed packages, which the project's package manager installs again.",
    regenerate: "Run npm install, or your package manager's install, in the project.",
    advise: null,
    matches: {
      kind: "projectArtifact",
      names: ["node_modules"],
      markers: ["package.json"],
      contains: [],
      gitIgnored: false,
    },
    owner: null,
    idleDays: null,
    tests: 3,
  },
  {
    id: "pnpm.store",
    name: "pnpm store",
    category: "Package caches",
    risk: "protected",
    explain: "pnpm's package store, which pnpm cleans itself without breaking projects.",
    regenerate: null,
    advise: "pnpm store prune",
    matches: { kind: "knownLocation", locations: ["DataLocal/pnpm/store"], entry: "folder" },
    owner: null,
    idleDays: null,
    tests: 1,
  },
  {
    id: "temp.files",
    name: "Temporary files",
    category: "Temporary files",
    risk: "safe",
    explain: "Files programs leave in the temp folder and don't need once they've finished.",
    regenerate: null,
    advise: null,
    matches: { kind: "knownLocation", locations: ["Temp/*"], entry: "filesAndFolders" },
    owner: null,
    idleDays: 7,
    tests: 2,
  },
  {
    id: "files.large",
    name: "Large files",
    category: "Large files",
    risk: "review",
    explain: "Files this large are worth a look, but only you know whether you still need them.",
    regenerate: null,
    advise: null,
    matches: { kind: "largeFile", size: 1024 * MIB },
    owner: null,
    idleDays: null,
    tests: 2,
  },
];
