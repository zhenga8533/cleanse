import { useEffect, useState } from "react";
import { Button } from "react-aria-components";
import * as api from "../api";
import type { CleanReport } from "../bindings/CleanReport";
import type { Folders } from "../bindings/Folders";
import type { Results } from "../bindings/Results";
import type { ScanProgress } from "../bindings/ScanProgress";
import { count, items, size } from "../format";
import { Activity } from "./Activity";
import { CleanDialog } from "./CleanDialog";
import { CleanReportView } from "./CleanReportView";
import { FolderList } from "./FolderList";
import { ItemDetails } from "./ItemDetails";
import { ResultsView } from "./ResultsView";
import { type Chosen, total } from "./selection";

/** What's running, if anything. Only one scan or cleanup runs at a time. */
type Running =
  | { kind: "nothing" }
  | { kind: "scan"; progress: ScanProgress | null; cancelling: boolean }
  | { kind: "cleanup"; done: number; total: number; cancelling: boolean };

interface Props {
  /** Cleanse runs elevated, so it won't remove anything. */
  elevated: boolean;
  /** Told the scan whose results show, whenever they change. */
  onResults: (scan: number) => void;
}

/** Choose folders, scan them, look through what was found, and clean up. */
export function ScanScreen({ elevated, onResults }: Props) {
  const [folders, setFolders] = useState<Folders | null>(null);
  const [running, setRunning] = useState<Running>({ kind: "nothing" });
  const [results, setResults] = useState<Results | null>(null);
  const [version, setVersion] = useState(0);
  const [chosen, setChosen] = useState<Chosen>(new Map());
  const [shown, setShown] = useState<number | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [report, setReport] = useState<CleanReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  const fail = (error: unknown) => setError(String(error));

  useEffect(() => {
    api.folders().then(setFolders, (error: unknown) => setError(String(error)));
  }, []);

  async function scan(): Promise<void> {
    setError(null);
    setReport(null);
    setShown(null);
    setRunning({ kind: "scan", progress: null, cancelling: false });
    try {
      const found = await api.scan((progress) =>
        // Progress can arrive after the scan has finished, and is stale by then.
        setRunning((running) => (running.kind === "scan" ? { ...running, progress } : running)),
      );
      setResults(found);
      onResults(found.scan);
      setChosen(new Map(found.preselected.map((choice) => [choice.id, choice])));
      setVersion((version) => version + 1);
    } catch (error) {
      fail(error);
    } finally {
      setRunning({ kind: "nothing" });
    }
  }

  async function clean(): Promise<void> {
    if (!results) {
      return;
    }
    const ids = [...chosen.keys()];
    setConfirming(false);
    setError(null);
    setReport(null);
    setRunning({ kind: "cleanup", done: 0, total: ids.length, cancelling: false });
    try {
      setReport(
        await api.clean(results.scan, ids, (done) =>
          setRunning((running) => (running.kind === "cleanup" ? { ...running, done } : running)),
        ),
      );
      setChosen(new Map());
      setShown(null);
      setResults(await api.summary(results.scan));
      onResults(results.scan);
      setVersion((version) => version + 1);
    } catch (error) {
      fail(error);
    } finally {
      setRunning({ kind: "nothing" });
    }
  }

  function cancel(): void {
    setRunning((running) =>
      running.kind === "nothing" ? running : { ...running, cancelling: true },
    );
    api.cancel().catch(fail);
  }

  const busy = running.kind !== "nothing";
  const selected = total([...chosen.values()].map((choice) => choice.size));
  return (
    <div className={shown === null ? "scan" : "scan with-details"}>
      <div className="scan-main">
        <h1>Scan</h1>
        <p className="muted">Find what can be removed, and why each item is safe to remove.</p>
        {folders && (
          <FolderList folders={folders} isDisabled={busy} onChange={setFolders} onError={fail} />
        )}
        <div className="row">
          <Button
            className="button primary"
            isDisabled={busy || !folders || folders.folders.length === 0}
            onPress={() => void scan()}
          >
            {results ? "Scan again" : "Scan"}
          </Button>
        </div>
        {running.kind === "scan" && (
          <Activity
            label={scanLabel(running.progress)}
            done={
              running.progress?.kind === "assessing" && running.progress.total > 0
                ? running.progress.done / running.progress.total
                : null
            }
            onCancel={cancel}
            isCancelling={running.cancelling}
          />
        )}
        {running.kind === "cleanup" && (
          <Activity
            label={`Removing ${count(running.done)} of ${count(running.total)}`}
            done={running.total > 0 ? running.done / running.total : null}
            onCancel={cancel}
            isCancelling={running.cancelling}
          />
        )}
        {error && (
          <p role="alert" className="notice error">
            {error}
          </p>
        )}
        {report && <CleanReportView report={report} onDismiss={() => setReport(null)} />}
        {results && (
          <ResultsView
            results={results}
            version={version}
            chosen={chosen}
            onChoose={setChosen}
            onShow={setShown}
          />
        )}
      </div>
      {results && shown !== null && (
        <ItemDetails scan={results.scan} item={shown} onClose={() => setShown(null)} />
      )}
      {results && (
        <div className="clean-bar">
          <span>
            {chosen.size === 0
              ? "Nothing selected"
              : `Selected: ${items(chosen.size)}, ${size(selected)}`}
          </span>
          {elevated && <span className="muted">Cleaning is off while Cleanse runs elevated.</span>}
          <Button
            className="button primary"
            isDisabled={elevated || busy || chosen.size === 0}
            onPress={() => setConfirming(true)}
          >
            Clean…
          </Button>
        </div>
      )}
      <CleanDialog
        chosen={chosen}
        isOpen={confirming}
        onOpenChange={setConfirming}
        onConfirm={() => void clean()}
      />
    </div>
  );
}

function scanLabel(progress: ScanProgress | null): string {
  switch (progress?.kind) {
    case undefined:
      return "Starting the scan…";
    case "walking":
      return `Looking through folders: ${count(progress.entries)} entries`;
    case "assessing":
      return `Checking ${count(progress.done)} of ${count(progress.total)} matches`;
  }
}
