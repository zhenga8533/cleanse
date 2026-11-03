import { useEffect, useRef } from "react";
import { Button } from "react-aria-components";
import type { CleanReport } from "../bindings/CleanReport";
import { bytes, items } from "../format";

interface Props {
  report: CleanReport;
  onDismiss: () => void;
}

/** What a cleanup did, and why anything was left. */
export function CleanReportView({ report, onDismiss }: Props) {
  const heading = useRef<HTMLHeadingElement>(null);

  useEffect(() => {
    heading.current?.focus();
  }, []);

  return (
    <section className="card report" aria-labelledby="report-heading">
      <h2 id="report-heading" ref={heading} tabIndex={-1}>
        Cleanup finished
      </h2>
      <p>
        Deleted {items(report.deleted.items)}, {bytes(report.deleted.size.onDisk)}.
      </p>
      {report.quarantined.items > 0 && (
        <p>
          Quarantined {items(report.quarantined.items)}, {bytes(report.quarantined.size.onDisk)}.
          They can be restored from Quarantine, and take space until you purge them.
        </p>
      )}
      {(
        [
          ["Skipped", report.skipped],
          ["Couldn't remove", report.failed],
        ] as const
      ).map(
        ([title, problems]) =>
          problems.length > 0 && (
            <div key={title}>
              <h3>
                {title}: {items(problems.length)}
              </h3>
              <ul className="problems">
                {problems.map((problem) => (
                  <li key={problem.path}>
                    <span className="path">{problem.path}</span>
                    <span className="item-note warns">{problem.reason}</span>
                  </li>
                ))}
              </ul>
            </div>
          ),
      )}
      {report.journal && (
        <p className="muted">
          The journal of this cleanup is <span className="path">{report.journal}</span>
        </p>
      )}
      <Button className="button" onPress={onDismiss}>
        Done
      </Button>
    </section>
  );
}
