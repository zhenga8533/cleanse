// Made-up results, for reviewing the UI in a browser and for tests. Nothing here is real.

import type { Details } from "../bindings/Details";
import type { Item } from "../bindings/Item";
import type { Results } from "../bindings/Results";
import type { RuleTotal } from "../bindings/RuleTotal";
import type { Section } from "../bindings/Section";
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
  explain: string;
  regenerate: string | null;
}

const RULES: Record<string, Rule> = {
  "javascript.node_modules": {
    rule: "javascript.node_modules",
    name: "Node.js dependencies",
    explain: "Installed packages, which the project's package manager installs again.",
    regenerate: "Run npm install, or your package manager's install, in the project.",
  },
  "rust.target": {
    rule: "rust.target",
    name: "Rust build output",
    explain: "Compiled output, which Cargo builds again.",
    regenerate: "Run cargo build in the project.",
  },
  "downloads.installers": {
    rule: "downloads.installers",
    name: "Installers in Downloads",
    explain:
      "Installers are usually only needed once, but only you know whether you'll need one again.",
    regenerate: null,
  },
  "pnpm.store": {
    rule: "pnpm.store",
    name: "pnpm store",
    explain: "pnpm's package store, which pnpm cleans itself without breaking projects.",
    regenerate: null,
  },
  "git.repository": {
    rule: "git.repository",
    name: "Git repository data",
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
