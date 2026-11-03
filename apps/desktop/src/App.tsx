import { useEffect, useState } from "react";
import { type Key, Tab, TabList, TabPanel, Tabs } from "react-aria-components";
import { appInfo } from "./api";
import type { AppInfo } from "./bindings/AppInfo";
import { DiskScreen, type Latest } from "./disk/DiskScreen";
import { HistoryScreen } from "./recovery/HistoryScreen";
import { QuarantineScreen } from "./recovery/QuarantineScreen";
import { ScanScreen } from "./scan/ScanScreen";
import { SettingsScreen } from "./settings/SettingsScreen";

const SECTIONS = [
  {
    id: "scan",
    title: "Scan",
    description: "Find what can be removed, and why each item is safe to remove.",
  },
  {
    id: "disk",
    title: "Disk usage",
    description: "See where space goes, folder by folder.",
  },
  {
    id: "quarantine",
    title: "Quarantine",
    description: "Put back, or delete for good, what cleanups set aside.",
  },
  {
    id: "history",
    title: "History",
    description: "What Cleanse has removed, restored, and purged.",
  },
  {
    id: "settings",
    title: "Settings",
    description: "Your settings file, the folders Cleanse knows, and its rules.",
  },
] as const;

type Startup =
  | { state: "loading" }
  | { state: "ready"; info: AppInfo }
  | { state: "failed"; error: string };

export function App() {
  const [startup, setStartup] = useState<Startup>({ state: "loading" });
  const [section, setSection] = useState<Key>("scan");
  const [latest, setLatest] = useState<Latest | null>(null);

  useEffect(() => {
    appInfo().then(
      (info) => setStartup({ state: "ready", info }),
      (error: unknown) => setStartup({ state: "failed", error: String(error) }),
    );
  }, []);

  return (
    <div className="app">
      <header className="app-header">
        <span className="app-name">Cleanse</span>
        {startup.state === "ready" && <span className="app-version">{startup.info.version}</span>}
      </header>
      {startup.state === "failed" && (
        <p role="alert" className="banner banner-error">
          Cleanse couldn't start: {startup.error}
        </p>
      )}
      {startup.state === "ready" && startup.info.elevated && (
        <p role="alert" className="banner banner-warning">
          Cleanse is running as an administrator or root, so it won't change anything: that could
          remove files your own account can't. Open it again without elevation to clean up.
        </p>
      )}
      <Tabs
        orientation="vertical"
        className="layout"
        selectedKey={section}
        onSelectionChange={setSection}
      >
        <TabList aria-label="Sections" className="nav">
          {SECTIONS.map((section) => (
            <Tab key={section.id} id={section.id}>
              {section.title}
            </Tab>
          ))}
        </TabList>
        {/* Scan and Disk usage are kept while another section shows, so what they show is still
            there on coming back. */}
        {startup.state === "ready" && (
          <TabPanel id="scan" className="panel" shouldForceMount>
            <ScanScreen
              elevated={startup.info.elevated}
              onResults={(scan) =>
                setLatest((latest) => ({ scan, version: (latest?.version ?? 0) + 1 }))
              }
            />
          </TabPanel>
        )}
        {startup.state === "ready" && (
          <TabPanel id="disk" className="panel" shouldForceMount>
            <DiskScreen latest={latest} onScan={() => setSection("scan")} />
          </TabPanel>
        )}
        {/* The rest read what's on disk afresh each time they're opened. */}
        {startup.state === "ready" && (
          <TabPanel id="quarantine" className="panel">
            <QuarantineScreen elevated={startup.info.elevated} />
          </TabPanel>
        )}
        {startup.state === "ready" && (
          <TabPanel id="history" className="panel">
            <HistoryScreen elevated={startup.info.elevated} />
          </TabPanel>
        )}
        {startup.state === "ready" && (
          <TabPanel id="settings" className="panel">
            <SettingsScreen />
          </TabPanel>
        )}
        {startup.state !== "ready" &&
          SECTIONS.map((section) => (
            <TabPanel key={section.id} id={section.id} className="panel">
              <h1>{section.title}</h1>
              <p className="muted">{section.description}</p>
            </TabPanel>
          ))}
      </Tabs>
    </div>
  );
}
