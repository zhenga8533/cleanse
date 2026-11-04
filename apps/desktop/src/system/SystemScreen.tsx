import { useEffect, useState } from "react";
import { Tab, TabList, TabPanel, Tabs } from "react-aria-components";
import { SYSTEM_CLEANUP, systemRules } from "../api";
import type { RuleView } from "../bindings/RuleView";
import type { SystemStatus } from "../bindings/SystemStatus";
import { CleanupContext } from "../cleanup";
import { HistoryScreen } from "../recovery/HistoryScreen";
import { QuarantineScreen } from "../recovery/QuarantineScreen";
import { ScanScreen } from "../scan/ScanScreen";
import { RuleDetails, RuleTable } from "../settings/SettingsScreen";

/**
 * System cleanup: the operating system's own folders, which a separate part of Cleanse cleans as
 * an administrator or root (ADR 0010), through the same screens as the user's own files. Nothing
 * here asks for administrator rights until it's needed: a scan, or opening the quarantine or the
 * history, which only an administrator can read.
 */
export function SystemScreen({ status }: { status: SystemStatus }) {
  if (status.unavailable !== null) {
    return (
      <>
        <h1>System</h1>
        <p className="notice warning">System cleanup isn't available here: {status.unavailable}</p>
        <p>
          The command line can still do it: <code>cleanse system scan</code>, in an administrator
          terminal on Windows or with <code>sudo</code> on Linux.
        </p>
      </>
    );
  }
  return (
    <CleanupContext value={SYSTEM_CLEANUP}>
      <Tabs className="system">
        <TabList aria-label="System cleanup" className="subnav">
          <Tab id="scan">Scan</Tab>
          <Tab id="quarantine">Quarantine</Tab>
          <Tab id="history">History</Tab>
          <Tab id="rules">Rules</Tab>
        </TabList>
        <p className="muted system-note">
          A separate part of Cleanse cleans the system's folders as an administrator. It asks for
          administrator rights when it starts, and again after ten minutes without use.
        </p>
        {/* The scan is kept while another part shows, so its results are still there. The app
            won't run elevated for system cleanup, so these screens never are. */}
        <TabPanel id="scan" className="subpanel" shouldForceMount>
          <ScanScreen elevated={false} />
        </TabPanel>
        <TabPanel id="quarantine" className="subpanel">
          <QuarantineScreen elevated={false} />
        </TabPanel>
        <TabPanel id="history" className="subpanel">
          <HistoryScreen elevated={false} />
        </TabPanel>
        <TabPanel id="rules" className="subpanel">
          <SystemRules />
        </TabPanel>
      </Tabs>
    </CleanupContext>
  );
}

/** System cleanup's rules, as `cleanse system rules` lists them. */
function SystemRules() {
  const [rules, setRules] = useState<RuleView[]>([]);
  const [shown, setShown] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    systemRules().then(setRules, (error: unknown) => setError(String(error)));
  }, []);

  const rule = rules.find((rule) => rule.id === shown);
  return (
    <div className={rule ? "split with-details" : "split"}>
      <div className="split-main">
        <h1>System rules</h1>
        <p className="muted">
          What system cleanup recognizes, on Windows and Linux. Each says why what it matches is
          safe to remove.
        </p>
        {error && (
          <p role="alert" className="notice error">
            {error}
          </p>
        )}
        {rules.length > 0 && <RuleTable rules={rules} onShow={setShown} />}
      </div>
      {rule && <RuleDetails rule={rule} onClose={() => setShown(null)} />}
    </div>
  );
}
