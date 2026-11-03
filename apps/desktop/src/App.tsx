import { useEffect, useState } from "react";
import { Tab, TabList, TabPanel, Tabs } from "react-aria-components";
import { appInfo } from "./api";
import type { AppInfo } from "./bindings/AppInfo";
import { ScanScreen } from "./scan/ScanScreen";

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
      <Tabs orientation="vertical" className="layout">
        <TabList aria-label="Sections" className="nav">
          {SECTIONS.map((section) => (
            <Tab key={section.id} id={section.id}>
              {section.title}
            </Tab>
          ))}
        </TabList>
        {SECTIONS.map((section) =>
          section.id === "scan" && startup.state === "ready" ? (
            // Kept while another section shows, so its results are still there on coming back.
            <TabPanel key={section.id} id={section.id} className="panel" shouldForceMount>
              <ScanScreen elevated={startup.info.elevated} />
            </TabPanel>
          ) : (
            <TabPanel key={section.id} id={section.id} className="panel">
              <h1>{section.title}</h1>
              <p className="muted">{section.description}</p>
            </TabPanel>
          ),
        )}
      </Tabs>
    </div>
  );
}
