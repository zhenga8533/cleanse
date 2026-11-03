import { useEffect, useState } from "react";
import {
  Button,
  Cell,
  Checkbox,
  Column,
  Dialog,
  Heading,
  Modal,
  ModalOverlay,
  Row,
  type Selection,
  Table,
  TableBody,
  TableHeader,
} from "react-aria-components";
import { purge, quarantined, restore } from "../api";
import type { Handled } from "../bindings/Handled";
import type { QuarantineView } from "../bindings/QuarantineView";
import { ago, bytes, items, now } from "../format";
import { total } from "../scan/selection";
import { type Done, HandledReport } from "./HandledReport";

interface Props {
  /** Cleanse runs elevated, so it won't change anything. */
  elevated: boolean;
}

/** What cleanups set aside, to put back or delete for good, as `cleanse quarantine` does. */
export function QuarantineScreen({ elevated }: Props) {
  const [listing, setListing] = useState<QuarantineView | null>(null);
  const [selected, setSelected] = useState<Selection>(new Set());
  const [running, setRunning] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [report, setReport] = useState<{ done: Done; handled: Handled } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => quarantined().then(setListing, (error: unknown) => setError(String(error)));

  // biome-ignore lint/correctness/useExhaustiveDependencies: loads once, when the section opens.
  useEffect(() => {
    void load();
  }, []);

  const entries = listing?.entries ?? [];
  const ids = selected === "all" ? entries.map((entry) => entry.id) : [...selected].map(String);
  const chosen = entries.filter((entry) => ids.includes(entry.id));
  const chosenSize = total(chosen.map((entry) => entry.size));

  async function handle(done: Done): Promise<void> {
    setConfirming(false);
    setError(null);
    setReport(null);
    setRunning(true);
    try {
      const handled = await (done === "restore" ? restore(ids) : purge(ids));
      setReport({ done, handled });
      setSelected(new Set());
    } catch (error) {
      setError(String(error));
    } finally {
      setRunning(false);
      void load();
    }
  }

  const at = now();
  const allSize = total(entries.map((entry) => entry.size));
  return (
    <>
      <h1>Quarantine</h1>
      <p className="muted">Put back, or delete for good, what cleanups set aside.</p>
      {error && (
        <p role="alert" className="notice error">
          {error}
        </p>
      )}
      {listing?.unreadable.map((record) => (
        <p key={record} className="notice warning">
          Couldn't read the quarantine record <span className="path">{record}</span>, so it isn't
          listed.
        </p>
      ))}
      {report && (
        <HandledReport
          done={report.done}
          handled={report.handled}
          onDismiss={() => setReport(null)}
        />
      )}
      {listing && entries.length === 0 && <p>Nothing is in quarantine.</p>}
      {entries.length > 0 && (
        <>
          <p>
            {items(entries.length)} in quarantine, {bytes(allSize.onDisk)}. They take space until
            you purge them.
          </p>
          <Table
            aria-label="What's in quarantine"
            className="table"
            selectionMode="multiple"
            selectedKeys={selected}
            onSelectionChange={setSelected}
          >
            <TableHeader>
              <Column className="column checkbox-column">
                <Checkbox slot="selection" className="checkbox">
                  <span className="box" aria-hidden="true" />
                </Checkbox>
              </Column>
              <Column isRowHeader className="column">
                Was at
              </Column>
              <Column className="column">Rule</Column>
              <Column className="column">Quarantined</Column>
              <Column className="column number">Size</Column>
            </TableHeader>
            <TableBody items={entries}>
              {(entry) => (
                <Row id={entry.id} className="table-row">
                  <Cell className="cell">
                    <Checkbox slot="selection" className="checkbox">
                      <span className="box" aria-hidden="true" />
                    </Checkbox>
                  </Cell>
                  <Cell className="cell">
                    <span className="path">{entry.original}</span>
                  </Cell>
                  <Cell className="cell">{entry.ruleName}</Cell>
                  <Cell className="cell">{ago(entry.at, at)}</Cell>
                  <Cell className="cell number">{bytes(entry.size.onDisk)}</Cell>
                </Row>
              )}
            </TableBody>
          </Table>
          <div className="clean-bar">
            <span>
              {chosen.length === 0
                ? "Nothing selected"
                : `Selected: ${items(chosen.length)}, ${bytes(chosenSize.onDisk)}`}
            </span>
            {elevated && (
              <span className="muted">
                Restoring and purging are off while Cleanse runs elevated.
              </span>
            )}
            <span className="actions">
              <Button
                className="button"
                isDisabled={elevated || running || chosen.length === 0}
                onPress={() => void handle("restore")}
              >
                Restore
              </Button>
              <Button
                className="button danger"
                isDisabled={elevated || running || chosen.length === 0}
                onPress={() => setConfirming(true)}
              >
                Purge…
              </Button>
            </span>
          </div>
        </>
      )}
      <ModalOverlay
        isOpen={confirming}
        onOpenChange={setConfirming}
        isDismissable
        className="overlay"
      >
        <Modal className="modal">
          <Dialog role="alertdialog" className="dialog">
            {({ close }) => (
              <>
                <Heading slot="title">
                  Purge {items(chosen.length)} ({bytes(chosenSize.onDisk)})?
                </Heading>
                <p>Purged items are deleted for good and can't be restored.</p>
                <div className="dialog-buttons">
                  <Button className="button" onPress={close} autoFocus>
                    Cancel
                  </Button>
                  <Button className="button danger" onPress={() => void handle("purge")}>
                    Purge
                  </Button>
                </div>
              </>
            )}
          </Dialog>
        </Modal>
      </ModalOverlay>
    </>
  );
}
