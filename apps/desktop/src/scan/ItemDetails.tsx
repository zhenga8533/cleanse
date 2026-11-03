import { useEffect, useRef, useState } from "react";
import { Button } from "react-aria-components";
import { explain } from "../api";
import type { Details } from "../bindings/Details";
import { ago, count, now, size } from "../format";
import { RISKS, SECTIONS } from "./sections";

interface Props {
  scan: number;
  item: number;
  onClose: () => void;
}

/** Everything known about one item, as `cleanse explain` gives it. */
export function ItemDetails({ scan, item, onClose }: Props) {
  const [details, setDetails] = useState<Details | null>(null);
  const [error, setError] = useState<string | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const opener = useRef<Element | null>(null);

  useEffect(() => {
    let current = true;
    opener.current = document.activeElement;
    setDetails(null);
    setError(null);
    explain(scan, item).then(
      (details) => current && setDetails(details),
      (error: unknown) => current && setError(String(error)),
    );
    return () => {
      current = false;
    };
  }, [scan, item]);

  // The panel opens from a list, so focus follows it there.
  useEffect(() => {
    if (details) {
      heading.current?.focus();
    }
  }, [details]);

  // Back to what opened it, rather than the start of the page.
  function close(): void {
    const back = opener.current;
    onClose();
    if (back instanceof HTMLElement) {
      back.focus();
    }
  }

  return (
    <aside className="details" aria-labelledby="details-heading">
      <div className="details-header">
        <h2 id="details-heading" ref={heading} tabIndex={-1}>
          {details ? name(details.item.path) : "Details"}
        </h2>
        <Button className="button quiet" onPress={close}>
          Close
        </Button>
      </div>
      {error && (
        <p role="alert" className="notice error">
          {error}
        </p>
      )}
      {details && (
        <dl>
          <dt>Path</dt>
          <dd className="path">{details.item.path}</dd>
          <dt>Rule</dt>
          <dd>
            {details.ruleName} <span className="muted">({details.item.rule})</span>
          </dd>
          <dt>Tier</dt>
          <dd>
            {RISKS[details.item.risk]}, {SECTIONS[details.section].verdict}
          </dd>
          <dt>Why</dt>
          <dd>{details.explain}</dd>
          {details.regenerate && (
            <>
              <dt>Comes back</dt>
              <dd>{details.regenerate}</dd>
            </>
          )}
          {details.item.command && (
            <>
              <dt>Clean with</dt>
              <dd>
                <code>{details.item.command}</code>
              </dd>
            </>
          )}
          <dt>Size</dt>
          <dd>
            {size(details.item.size)} in {count(details.item.files)}{" "}
            {details.item.files === 1 ? "file" : "files"}
          </dd>
          {details.item.modified !== null && (
            <>
              <dt>Modified</dt>
              <dd>{ago(details.item.modified, now())}</dd>
            </>
          )}
          {details.evidence.length > 0 && (
            <>
              <dt>Evidence</dt>
              <dd>
                <ul>
                  {details.evidence.map((evidence) => (
                    <li key={evidence}>{evidence}</li>
                  ))}
                </ul>
              </dd>
            </>
          )}
          <dt>Concerns</dt>
          <dd>
            {details.item.concerns.length === 0 ? (
              "None"
            ) : (
              <ul>
                {details.item.concerns.map((concern) => (
                  <li key={concern.message} className={concern.blocks ? "keeps" : "warns"}>
                    {concern.message}{" "}
                    <span className="tag">{concern.blocks ? "keeps it" : "warning"}</span>
                  </li>
                ))}
              </ul>
            )}
          </dd>
        </dl>
      )}
    </aside>
  );
}

/** The last part of a path, whichever separator it uses. */
function name(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}
