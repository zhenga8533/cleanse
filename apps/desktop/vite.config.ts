import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  // Tauri loads the dev server from this fixed address (`devUrl` in tauri.conf.json) and prints
  // its own output in the same terminal.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    // The oldest web views Tauri supports: WebView2 on Windows and WebKit on macOS 10.15.
    target: ["es2021", "chrome105", "safari15"],
  },
  test: {
    environment: "jsdom",
    setupFiles: ["src/test/setup.ts"],
  },
});
