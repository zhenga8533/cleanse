import { useEffect, useState } from "react";
import { Breadcrumb, Breadcrumbs, Button, Link, type SortDescriptor } from "react-aria-components";
import { folder, summary } from "../api";
import type { CategoryTotal } from "../bindings/CategoryTotal";
import type { FolderView } from "../bindings/FolderView";
import { bytes, count } from "../format";
import { CategoryFilter } from "./CategoryFilter";
import { FolderTable } from "./FolderTable";
import { rows } from "./rows";
import { type Tile, Treemap } from "./Treemap";

/** How many of a folder's largest entries are shown. */
export const LIMIT = 200;

/** The scan whose results show, and a count that changes whenever they do, such as after a cleanup. */
export interface Latest {
  scan: number;
  version: number;
}

interface Props {
  latest: Latest | null;
  onScan: () => void;
}

/** Where the space in the scanned folders goes, folder by folder, as `cleanse analyze` shows. */
export function DiskScreen({ latest, onScan }: Props) {
  // The folder shown, with the scan it's from: a new scan starts again from its roots.
  const [at, setAt] = useState<{ scan: number; node: number | null } | null>(null);
  const [view, setView] = useState<FolderView | null>(null);
  const [roots, setRoots] = useState<string[]>([]);
  const [totals, setTotals] = useState<CategoryTotal[]>([]);
  const [counted, setCounted] = useState<string[]>([]);
  const [sort, setSort] = useState<SortDescriptor>({ column: "size", direction: "descending" });
  const [error, setError] = useState<string | null>(null);

  const scan = latest?.scan ?? null;
  const version = latest?.version ?? 0;
  const node = at !== null && at.scan === scan ? at.node : null;

  // biome-ignore lint/correctness/useExhaustiveDependencies: `version` says when to read again.
  useEffect(() => {
    if (scan === null) {
      return;
    }
    let current = true;
    summary(scan).then(
      (results) => {
        if (current) {
          setRoots(results.roots);
          setTotals(results.categories);
        }
      },
      (error: unknown) => current && setError(String(error)),
    );
    return () => {
      current = false;
    };
  }, [scan, version]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: `version` says when to read again.
  useEffect(() => {
    if (scan === null) {
      return;
    }
    let current = true;
    setError(null);
    folder(scan, node, LIMIT, counted).then(
      (view) => {
        if (!current) {
          return;
        }
        // A scan of one folder starts inside it, rather than at a list of one.
        const only = view.trail.length === 0 && view.entries.length === 1 ? view.entries[0] : null;
        if (only?.hasChildren) {
          setAt({ scan, node: only.node });
        } else {
          setView(view);
        }
      },
      (error: unknown) => current && setError(String(error)),
    );
    return () => {
      current = false;
    };
  }, [scan, version, node, counted]);

  const heading = (
    <>
      <h1>Disk usage</h1>
      <p className="muted">See where space goes, folder by folder.</p>
    </>
  );
  if (scan === null) {
    return (
      <>
        {heading}
        <div className="card">
          <p>Scan first, and this shows where the space in the scanned folders goes.</p>
          <div className="row">
            <Button className="button primary" onPress={onScan}>
              Go to Scan
            </Button>
          </div>
        </div>
      </>
    );
  }

  const open = (node: number | null) => setAt({ scan, node });
  const here = view?.trail.at(-1);
  const name = here?.name ?? "the scanned folders";
  const listed = view ? rows(view) : [];
  const tiles: Tile[] = listed.map((row) => ({
    key: row.key,
    name: row.name,
    onDisk: row.onDisk,
    removable: row.removable,
    opens: row.opens,
  }));
  if (view && view.hidden > 0) {
    tiles.push({
      key: "rest",
      name: `${count(view.hidden)} more`,
      onDisk: view.hiddenOnDisk,
      removable: 0,
      opens: null,
      rest: true,
    });
  }
  const crumbs = [
    ...(roots.length > 1 ? [{ id: "roots", name: "Scanned folders" }] : []),
    ...(view?.trail ?? []).map((entry) => ({ id: String(entry.node), name: entry.name })),
  ];

  return (
    <>
      {heading}
      {error && (
        <p role="alert" className="notice error">
          {error}
        </p>
      )}
      {view && (
        <>
          <div className="row disk-bar">
            <nav aria-label="Folders">
              <Breadcrumbs
                className="crumbs"
                onAction={(key) => open(key === "roots" ? null : Number(key))}
              >
                {crumbs.map((crumb) => (
                  <Breadcrumb key={crumb.id} id={crumb.id} className="crumb">
                    <Link className="crumb-link">{crumb.name}</Link>
                  </Breadcrumb>
                ))}
              </Breadcrumbs>
            </nav>
            <CategoryFilter totals={totals} counted={counted} onChange={setCounted} />
          </div>
          {here && (
            <p className="muted">
              {bytes(here.usage.onDisk)} in {count(here.files)}{" "}
              {here.files === 1 ? "file" : "files"}
              {here.removable > 0 && `, ${bytes(here.removable)} of it removable`}
            </p>
          )}
          <Treemap
            label={`What takes the space in ${name}, drawn to scale. The table below lists the same entries.`}
            tiles={tiles}
            onOpen={open}
          />
          <FolderTable
            label={`What's in ${name}`}
            rows={listed}
            sort={sort}
            onSortChange={setSort}
            onOpen={open}
          />
          {view.hidden > 0 && (
            <p className="muted">
              …and {count(view.hidden)} smaller {view.hidden === 1 ? "entry" : "entries"},{" "}
              {bytes(view.hiddenOnDisk)}.
            </p>
          )}
          {listed.some((row) => row.incomplete) && (
            <p className="muted">* Part of it couldn't be read, so it takes more than shown.</p>
          )}
        </>
      )}
    </>
  );
}
