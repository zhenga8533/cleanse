import { useEffect, useRef } from "react";
import type { Results } from "../bindings/Results";
import { bytes, count, items, seconds, size } from "../format";
import { RuleGroup } from "./RuleGroup";
import { SECTIONS } from "./sections";
import type { Chosen } from "./selection";

interface Props {
  results: Results;
  version: number;
  chosen: Chosen;
  onChoose: (chosen: Chosen) => void;
  onShow: (item: number) => void;
}

/** What a scan found, section by section, each grouped by rule. */
export function ResultsView({ results, version, chosen, onChoose, onShow }: Props) {
  const scanned = useRef<HTMLParagraphElement>(null);

  // Focus was on the scan's Cancel button, which is gone now, so it moves to what was found.
  // biome-ignore lint/correctness/useExhaustiveDependencies: each new scan moves focus once.
  useEffect(() => {
    scanned.current?.focus();
  }, [results.scan]);

  const found = results.sections.some((section) => section.items > 0);
  return (
    <div className="results">
      <p ref={scanned} tabIndex={-1} className="scanned">
        Scanned <span className="path">{results.roots.join(", ")}</span>: {count(results.entries)}{" "}
        entries in {seconds(results.seconds)}
      </p>
      {results.cancelled && (
        <p className="notice warning">The scan was cancelled, so these results are partial.</p>
      )}
      {results.unreadable > 0 && (
        <p className="muted">
          {results.unreadable === 1 ? "1 folder" : `${count(results.unreadable)} folders`} couldn't
          be read, so anything inside them was skipped.
        </p>
      )}
      {results.removed.onDisk > 0 && (
        <p className="muted">Removed since this scan: {bytes(results.removed.onDisk)}.</p>
      )}
      {!found && <p>Nothing the rules recognize was found.</p>}
      {results.sections
        .filter((section) => section.items > 0)
        .map((section) => (
          <section
            key={section.section}
            className="result-section"
            aria-labelledby={`section-${section.section}`}
          >
            <h2 id={`section-${section.section}`}>
              {SECTIONS[section.section].title}: {items(section.items)}, {size(section.size)}
            </h2>
            <p className="muted">{SECTIONS[section.section].about}</p>
            <div className="groups">
              {section.rules.map((rule) => (
                <RuleGroup
                  key={rule.rule}
                  scan={results.scan}
                  section={section.section}
                  rule={rule}
                  version={version}
                  chosen={chosen}
                  onChoose={onChoose}
                  onShow={onShow}
                />
              ))}
            </div>
          </section>
        ))}
    </div>
  );
}
