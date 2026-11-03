import { useEffect, useRef } from "react";
import { Button } from "react-aria-components";
import type { Handled } from "../bindings/Handled";
import { bytes, items } from "../format";

export type Done = "restore" | "purge" | "finish";

interface Props {
  done: Done;
  handled: Handled;
  onDismiss: () => void;
}

/** What restoring, purging, or finishing did, in the CLI's words, and why anything was left. */
export function HandledReport({ done, handled, onDismiss }: Props) {
  const heading = useRef<HTMLHeadingElement>(null);

  useEffect(() => {
    heading.current?.focus();
  }, []);

  const freed = handled.done.size.onDisk;
  const count = items(handled.done.items);
  let summary: string;
  let failed: string;
  switch (done) {
    case "restore":
      summary = `Restored ${count}.`;
      failed = "Couldn't restore";
      break;
    case "purge":
      summary = freed > 0 ? `Purged ${count}, freeing ${bytes(freed)}.` : `Purged ${count}.`;
      failed = "Couldn't purge";
      break;
    case "finish":
      summary = `Removed what was left of ${count}, freeing ${bytes(freed)}.`;
      failed = "Couldn't finish";
      break;
  }

  return (
    <section className="card report" aria-labelledby="handled-heading">
      <h2 id="handled-heading" ref={heading} tabIndex={-1}>
        {summary}
      </h2>
      {handled.failed.length > 0 && (
        <>
          <h3>
            {failed}: {items(handled.failed.length)}
          </h3>
          <ul className="problems">
            {handled.failed.map((problem) => (
              <li key={problem.path}>
                <span className="path">{problem.path}</span>
                <span className="item-note warns">{problem.reason}</span>
              </li>
            ))}
          </ul>
        </>
      )}
      {handled.journal && (
        <p className="muted">
          The journal of this run is <span className="path">{handled.journal}</span>
        </p>
      )}
      <Button className="button" onPress={onDismiss}>
        Done
      </Button>
    </section>
  );
}
