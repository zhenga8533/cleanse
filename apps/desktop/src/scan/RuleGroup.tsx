import { useEffect, useState } from "react";
import {
  Button,
  Checkbox,
  Disclosure,
  DisclosurePanel,
  GridList,
  GridListItem,
  Heading,
  type Key,
} from "react-aria-components";
import { candidates } from "../api";
import type { Item } from "../bindings/Item";
import type { RuleTotal } from "../bindings/RuleTotal";
import type { Section } from "../bindings/Section";
import { bytes, count, items, modified, now, size } from "../format";
import { removable, SECTIONS } from "./sections";
import { type Chosen, choose } from "./selection";

/** How many items a rule's list loads at a time. */
export const PAGE = 50;

interface Props {
  scan: number;
  section: Section;
  rule: RuleTotal;
  /** Changes whenever the results do, so an open list loads again. */
  version: number;
  chosen: Chosen;
  onChoose: (chosen: Chosen) => void;
  onShow: (item: number) => void;
}

/** A rule's items in one section, listed once it's opened. */
export function RuleGroup({ scan, section, rule, version, chosen, onChoose, onShow }: Props) {
  const [isExpanded, setExpanded] = useState(false);
  const [shown, setShown] = useState<{ items: Item[]; total: number } | null>(null);
  const [error, setError] = useState<string | null>(null);

  // biome-ignore lint/correctness/useExhaustiveDependencies: `version` says when to load again.
  useEffect(() => {
    if (!isExpanded) {
      return;
    }
    let current = true;
    setError(null);
    candidates(scan, section, rule.rule, 0, PAGE).then(
      (page) => current && setShown({ items: page.items, total: page.total }),
      (error: unknown) => current && setError(String(error)),
    );
    return () => {
      current = false;
    };
  }, [isExpanded, scan, section, rule.rule, version]);

  function showMore(): void {
    if (!shown) {
      return;
    }
    candidates(scan, section, rule.rule, shown.items.length, PAGE).then(
      (page) => setShown({ items: [...shown.items, ...page.items], total: page.total }),
      (error: unknown) => setError(String(error)),
    );
  }

  const selectable = removable(section);
  const at = now();
  return (
    <Disclosure className="group" isExpanded={isExpanded} onExpandedChange={setExpanded}>
      <Heading level={3} className="group-heading">
        <Button slot="trigger" className="group-trigger">
          <span className="chevron" aria-hidden="true" />
          <span className="group-name">{rule.name}</span>{" "}
          <span className="muted">
            {items(rule.items)}, {size(rule.size)}
          </span>
        </Button>
      </Heading>
      <DisclosurePanel className="group-panel">
        {error && (
          <p role="alert" className="notice error">
            {error}
          </p>
        )}
        {shown && (
          <>
            <GridList
              aria-label={`${rule.name}, ${SECTIONS[section].title}`}
              className="items"
              items={shown.items}
              selectionMode={selectable ? "multiple" : "none"}
              selectedKeys={shown.items
                .filter((item) => chosen.has(item.id))
                .map((item) => item.id)}
              onSelectionChange={(keys) => onChoose(choose(chosen, shown.items, keys))}
              // Where rows have checkboxes, React Aria runs a row's action only while nothing is
              // chosen, so a click would sometimes choose and sometimes open. There, a click
              // always chooses and each row's Details button opens it.
              {...(selectable ? {} : { onAction: (key: Key) => onShow(Number(key)) })}
            >
              {(item) => (
                <GridListItem id={item.id} textValue={item.path} className="item">
                  {selectable && (
                    <Checkbox slot="selection" className="checkbox">
                      <span className="box" aria-hidden="true" />
                    </Checkbox>
                  )}
                  <span className="item-size">{bytes(item.size.onDisk)}</span>{" "}
                  <span className="item-main">
                    <span className="path">{item.path}</span>
                    {section === "review" && item.modified !== null && (
                      <span className="muted"> {modified(item.modified, at)}</span>
                    )}
                    {item.command && (
                      <span className="item-note">
                        Run <code>{item.command}</code>
                      </span>
                    )}
                    {item.concerns[0] && (
                      <span
                        className={item.concerns[0].blocks ? "item-note keeps" : "item-note warns"}
                      >
                        {item.concerns[0].message}
                        {item.concerns.length > 1 &&
                          ` (and ${count(item.concerns.length - 1)} more)`}
                      </span>
                    )}
                  </span>
                  <Button className="button quiet" onPress={() => onShow(item.id)}>
                    Details
                  </Button>
                </GridListItem>
              )}
            </GridList>
            {shown.items.length < shown.total && (
              <Button className="button quiet" onPress={showMore}>
                Show {count(Math.min(PAGE, shown.total - shown.items.length))} more of{" "}
                {count(shown.total - shown.items.length)}
              </Button>
            )}
          </>
        )}
      </DisclosurePanel>
    </Disclosure>
  );
}
