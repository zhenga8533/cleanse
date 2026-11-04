import { useEffect, useState } from "react";
import { Button, Cell, Column, Row, Table, TableBody, TableHeader } from "react-aria-components";
import type { Amount } from "../bindings/Amount";
import type { Handled } from "../bindings/Handled";
import type { HistoryView } from "../bindings/HistoryView";
import type { RunView } from "../bindings/RunView";
import { useCleanup } from "../cleanup";
import { ago, bytes, count, items, now } from "../format";
import { HandledReport } from "./HandledReport";

/** How many runs show before asking for the rest, as `cleanse history` lists without `--all`. */
const RECENT = 20;

interface Props {
  /** Cleanse runs elevated, so it won't change anything. */
  elevated: boolean;
}

/** What Cleanse has removed, restored, and purged, as `cleanse history` lists it. */
export function HistoryScreen({ elevated }: Props) {
  const { kind, finish, history } = useCleanup();
  const [view, setView] = useState<HistoryView | null>(null);
  const [all, setAll] = useState(false);
  const [running, setRunning] = useState(false);
  const [report, setReport] = useState<Handled | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => history().then(setView, (error: unknown) => setError(String(error)));

  // biome-ignore lint/correctness/useExhaustiveDependencies: loads once, when the section opens.
  useEffect(() => {
    void load();
  }, []);

  async function finishLeftovers(): Promise<void> {
    setError(null);
    setReport(null);
    setRunning(true);
    try {
      setReport(await finish());
    } catch (error) {
      setError(String(error));
    } finally {
      setRunning(false);
      void load();
    }
  }

  const runs = view?.runs ?? [];
  const shown = all ? runs : runs.slice(0, RECENT);
  const at = now();
  return (
    <>
      <h1>{kind === "user" ? "History" : "System history"}</h1>
      <p className="muted">
        What {kind === "user" ? "Cleanse" : "system cleanup"} has removed, restored, and purged.
      </p>
      {error && (
        <p role="alert" className="notice error">
          {error}
        </p>
      )}
      {view?.unreadable.map((journal) => (
        <p key={journal} className="notice warning">
          Couldn't read the journal <span className="path">{journal}</span>.
        </p>
      ))}
      {report && <HandledReport done="finish" handled={report} onDismiss={() => setReport(null)} />}
      {view && view.leftovers.length > 0 && (
        <section className="card" aria-labelledby="leftovers-heading">
          <h2 id="leftovers-heading">
            {view.leftovers.length === 1 ? "1 removal" : `${count(view.leftovers.length)} removals`}{" "}
            didn't finish, so part of it is still on disk
          </h2>
          <ul className="problems">
            {view.leftovers.map((leftover) => (
              <li key={leftover.tombstone}>
                <span className="path">{leftover.tombstone}</span>
                <span className="muted">
                  Was <span className="path">{leftover.original}</span>
                </span>
              </li>
            ))}
          </ul>
          <div className="row">
            <Button
              className="button primary"
              isDisabled={elevated || running}
              onPress={() => void finishLeftovers()}
            >
              Remove what's left
            </Button>
            {elevated && (
              <span className="muted">Finishing is off while Cleanse runs elevated.</span>
            )}
          </div>
        </section>
      )}
      {view && runs.length === 0 && <p>Cleanse hasn't changed anything yet.</p>}
      {runs.length > 0 && (
        <Table aria-label="Runs, most recent first" className="table">
          <TableHeader>
            <Column isRowHeader className="column">
              When
            </Column>
            <Column className="column">Command</Column>
            <Column className="column">What it did</Column>
          </TableHeader>
          <TableBody
            items={shown.map((run, index) => ({ ...run, key: `${run.journal}#${index}` }))}
          >
            {(run) => (
              <Row id={run.key} className="table-row">
                <Cell className="cell">{ago(run.at, at)}</Cell>
                <Cell className="cell">{run.command}</Cell>
                <Cell className="cell">{summary(run)}</Cell>
              </Row>
            )}
          </TableBody>
        </Table>
      )}
      {runs.length > shown.length && (
        <div className="row">
          <Button className="button quiet" onPress={() => setAll(true)}>
            Show {count(runs.length - shown.length)} earlier
          </Button>
        </div>
      )}
    </>
  );
}

/** What a run did, like `Deleted 3 (1.2 GiB), quarantined 1 (12.0 MiB), skipped 1`, as the CLI says it. */
export function summary(run: RunView): string {
  const sized = (verb: string, amount: Amount) =>
    amount.items > 0 ? `${verb} ${amount.items} (${bytes(amount.size.onDisk)})` : null;
  const counted = (verb: string, n: number) => (n > 0 ? `${verb} ${n}` : null);
  const parts = [
    sized("deleted", run.deleted),
    sized("quarantined", run.quarantined),
    counted("restored", run.restored),
    sized("purged", run.purged),
    counted("skipped", run.skipped),
    counted("failed", run.failed),
  ].filter((part) => part !== null);
  let text = parts.length > 0 ? parts.join(", ") : `nothing done, of ${items(run.items)}`;
  if (run.cutShort) {
    text += " (cut short)";
  }
  return text.charAt(0).toUpperCase() + text.slice(1);
}
