import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

async function start(): Promise<void> {
  // Outside the app's window, in a plain browser, there's no engine to answer. Development builds
  // answer with sample data instead, so the UI can be reviewed there. Production builds leave this
  // out entirely.
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    const { installMocks } = await import("./dev/mocks");
    installMocks();
  }
  const root = document.getElementById("root");
  if (!root) {
    throw new Error("index.html has no #root element");
  }
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void start();
