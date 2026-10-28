# 0001: Rust core with a Tauri desktop app

**Status:** Accepted

## Context

Cleanse needs to scan millions of files quickly on Windows, macOS, and Linux, call low-level
platform APIs (file identity, reparse points, cloud placeholders), and ship both a scriptable CLI
and a polished, accessible desktop app. Bugs in this code can delete user data.

Options considered:

- **Rust + Tauri**: memory safety without a garbage collector, mature filesystem crates, and a
  desktop shell that uses the system web view.
- **Go + Wails**: simpler concurrency and faster compiles, but a less mature desktop story and
  weaker typing for the data model shared with the UI.
- **Electron**: largest ecosystem, but large installers and memory use, and the core would
  still need a native helper for platform APIs.
- **Native Rust GUIs (Slint, egui, iced)**: small binaries, but accessibility support is weaker
  than the web platform's, and Slint's licensing complicates a permissive project.

## Decision

Write the engine and CLI in Rust. Build the desktop app with Tauri v2 and a React + TypeScript
front-end using React Aria Components for accessible primitives. The desktop app links the
engine in-process.

## Consequences

- One language for the engine, CLI, and desktop backend, with types shared to the UI through
  generated TypeScript bindings.
- The UI inherits web accessibility (screen readers, keyboard navigation, high-contrast modes)
  through the system web view.
- Web views differ per OS (WebView2, WebKit, WebKitGTK), so the UI must be tested on all three.
- Rust raises the bar for contributors. Rules are kept as data so most contributions need no
  Rust ([ADR 0003](0003-declarative-rules.md)).
