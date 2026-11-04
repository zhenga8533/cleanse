import { Fragment, useEffect, useRef, useState } from "react";
import { Button, Cell, Column, Row, Table, TableBody, TableHeader } from "react-aria-components";
import { rules as loadRules, settings as loadSettings } from "../api";
import type { RuleMatch } from "../bindings/RuleMatch";
import type { RuleView } from "../bindings/RuleView";
import type { SettingsView } from "../bindings/SettingsView";
import { bytes } from "../format";
import { RISKS } from "../scan/sections";

/** How Cleanse is set up here, as `cleanse doctor` reports it, and its rules, as `cleanse rules` lists them. */
export function SettingsScreen() {
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [rules, setRules] = useState<RuleView[]>([]);
  const [shown, setShown] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    loadSettings().then(setSettings, (error: unknown) => setError(String(error)));
    loadRules().then(setRules, (error: unknown) => setError(String(error)));
  }, []);

  const rule = rules.find((rule) => rule.id === shown);
  return (
    <div className={rule ? "split with-details" : "split"}>
      <div className="split-main">
        <h1>Settings</h1>
        <p className="muted">Your settings file, the folders Cleanse knows, and its rules.</p>
        {error && (
          <p role="alert" className="notice error">
            {error}
          </p>
        )}
        {settings && <Doctor settings={settings} rules={rules.length} />}
        {rules.length > 0 && (
          <section className="result-section" aria-labelledby="rules-heading">
            <h2 id="rules-heading">Rules</h2>
            <p className="muted">
              What Cleanse recognizes. Each says why what it matches is safe to remove.
            </p>
            <RuleTable rules={rules} onShow={setShown} />
          </section>
        )}
      </div>
      {rule && <RuleDetails rule={rule} onClose={() => setShown(null)} />}
    </div>
  );
}

function Doctor({ settings, rules }: { settings: SettingsView; rules: number }) {
  let file: string;
  if (settings.path === null) {
    file = "None: this system has no folder for app settings";
  } else if (settings.exists) {
    file = settings.path;
  } else {
    file = `${settings.path} (not created, so the defaults apply)`;
  }
  return (
    <section className="card" aria-labelledby="doctor-heading">
      <h2 id="doctor-heading">
        Cleanse {settings.version} on {settings.os} ({settings.arch})
      </h2>
      <dl className="facts">
        <dt>Running as administrator or root</dt>
        <dd>{settings.elevated ? "Yes" : "No"}</dd>
        <dt>Rules</dt>
        <dd>
          {settings.disabledRules.length === 0
            ? `${rules} built in`
            : `${rules + settings.disabledRules.length} built in, ${settings.disabledRules.length} turned off in your settings`}
        </dd>
        <dt>Cloud sync detection</dt>
        <dd>{settings.cloudSyncDetection}</dd>
        <dt>Settings file</dt>
        <dd className="path">{file}</dd>
        {settings.disabledRules.length > 0 && (
          <>
            <dt>Turned off</dt>
            <dd>{settings.disabledRules.join(", ")}</dd>
          </>
        )}
        {settings.exclude.length > 0 && (
          <>
            <dt>Excluded</dt>
            <dd>
              <ul>
                {settings.exclude.map((folder) => (
                  <li key={folder} className="path">
                    {folder}
                  </li>
                ))}
              </ul>
            </dd>
          </>
        )}
        <dt>Large files</dt>
        <dd>{bytes(settings.largeFileSize)} or more on disk</dd>
        {settings.folders.map((folder) => (
          <Fragment key={folder.name}>
            <dt>{folder.name} folder</dt>
            <dd className="path">{folder.path ?? "Not defined"}</dd>
          </Fragment>
        ))}
      </dl>
      <p className="muted">
        To change these, edit the settings file. Cleanse reads it when it starts.
      </p>
    </section>
  );
}

/** Rules by name, category, and tier, each of which opens its details. */
export function RuleTable({ rules, onShow }: { rules: RuleView[]; onShow: (id: string) => void }) {
  return (
    <Table aria-label="Rules" className="table" onRowAction={(key) => onShow(String(key))}>
      <TableHeader>
        <Column isRowHeader className="column">
          Name
        </Column>
        <Column className="column">Category</Column>
        <Column className="column">Tier</Column>
      </TableHeader>
      <TableBody items={rules}>
        {(rule) => (
          <Row id={rule.id} className="table-row opens">
            <Cell className="cell">{rule.name}</Cell>
            <Cell className="cell">{rule.category}</Cell>
            <Cell className="cell">{RISKS[rule.risk]}</Cell>
          </Row>
        )}
      </TableBody>
    </Table>
  );
}

export function RuleDetails({ rule, onClose }: { rule: RuleView; onClose: () => void }) {
  const heading = useRef<HTMLHeadingElement>(null);
  const opener = useRef<Element | null>(null);

  // biome-ignore lint/correctness/useExhaustiveDependencies: each rule opened moves focus once.
  useEffect(() => {
    opener.current ??= document.activeElement;
    heading.current?.focus();
  }, [rule.id]);

  function close(): void {
    const back = opener.current;
    onClose();
    if (back instanceof HTMLElement) {
      back.focus();
    }
  }

  return (
    <aside className="details" aria-labelledby="rule-heading">
      <div className="details-header">
        <h2 id="rule-heading" ref={heading} tabIndex={-1}>
          {rule.name}
        </h2>
        <Button className="button quiet" onPress={close}>
          Close
        </Button>
      </div>
      <dl>
        <dt>ID</dt>
        <dd>
          <code>{rule.id}</code>
        </dd>
        <dt>Category</dt>
        <dd>{rule.category}</dd>
        <dt>Tier</dt>
        <dd>{RISKS[rule.risk]}</dd>
        <dt>Why</dt>
        <dd>{rule.explain}</dd>
        {rule.regenerate && (
          <>
            <dt>Comes back</dt>
            <dd>{rule.regenerate}</dd>
          </>
        )}
        {rule.advise && (
          <>
            <dt>Clean with</dt>
            <dd>
              <code>{rule.advise}</code>
            </dd>
          </>
        )}
        <dt>Matches</dt>
        <dd>
          {matches(rule.matches).map((line) => (
            <div key={line}>{line}</div>
          ))}
        </dd>
        {rule.owner && (
          <>
            <dt>Kept while</dt>
            <dd>{rule.owner} is running</dd>
          </>
        )}
        {rule.idleDays !== null && (
          <>
            <dt>Kept until</dt>
            <dd>Unchanged for {rule.idleDays} days, and since the computer last started</dd>
          </>
        )}
        <dt>Tests</dt>
        <dd>{rule.tests}</dd>
      </dl>
    </aside>
  );
}

/** What a rule matches, a line at a time, as `cleanse rules show` says it. */
export function matches(match: RuleMatch): string[] {
  switch (match.kind) {
    case "projectArtifact":
      return [
        `a folder named ${match.names.join(", ")}`,
        ...(match.markers.length > 0 ? [`next to any of ${match.markers.join(", ")}`] : []),
        ...(match.contains.length > 0 ? [`containing ${match.contains.join(", ")}`] : []),
        ...(match.gitIgnored ? ["that Git ignores"] : []),
      ];
    case "knownLocation": {
      if (match.locations.length === 0) {
        return ["nothing on this operating system"];
      }
      const what = { folder: "", files: "files at ", filesAndFolders: "files and folders at " }[
        match.entry
      ];
      return [`${what}${match.locations.join(", ")}`];
    }
    case "largeFile":
      return [
        `files taking ${bytes(match.size)} or more on disk (large_file_size in your settings),`,
        "outside the folders apps keep their data in",
      ];
  }
}
