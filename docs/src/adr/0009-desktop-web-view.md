# 0009: The desktop app's web view can only name what the engine showed it

**Status:** Accepted

## Context

The desktop app shows Cleanse's results in a web view, which runs a few hundred npm packages
and displays text taken from the user's files, such as folder names. If a compromised package,
or content that slipped past the escaping, could call into the app with a path, it could direct
removals anywhere the user's account can write. The web view also comes with powers of its own
through Tauri: reading files, spawning processes, opening windows, and loading other sites.

## Decision

The web view gets nothing but a small set of the app's own commands, and none of them takes a
path.

- **Commands are refused unless a capability allows them.** `src-tauri/build.rs` lists the
  app's commands for tauri-build's app manifest, which makes each one deny-by-default, and the
  single capability, `src-tauri/capabilities/main.json`, allows exactly those, for the app's own
  window and content. It allows nothing from Tauri's core (events, paths, windows) and no plugin,
  so the web view can't read files, run programs, open windows, or listen for events. Progress
  comes back over channels, which don't need a permission.
- **Removals go by ID.** The commands go through the engine's `Session`, which names candidates,
  folders, and quarantine entries by IDs it gave out and refuses IDs from an earlier scan.
  Cleaning re-checks every item as `cleanse clean` does, so the web view can only choose among
  what the engine already decided may be removed.
- **Scans cover the folders the person picked.** A scan covers the folders chosen in the
  system's folder picker, which the Rust side opens, or else the home folder. The web view can
  ask for the picker but can't name a folder, so it can't widen a scan to somewhere the person
  didn't choose. The picker comes from `rfd` directly rather than Tauri's dialog plugin, which
  would bring in Tauri's file system plugin too.
- **The web view shows only the app.** The content security policy is `default-src 'self'`, so
  no remote or inline scripts run and nothing is fetched except through the app's own IPC.
  Inline styles are allowed, since React Aria positions popovers with them. The window refuses
  to navigate anywhere else and refuses to open new windows.
- **No updater** until builds are signed (M5). Tauri's updater needs an HTTP client, which
  cargo-deny bans ([ADR 0005](0005-no-network.md)).

- **Messages go through Tauri's isolation pattern.** Every IPC message from the page passes
  through a separate iframe, `apps/desktop/isolation/`, which has no dependencies and encrypts
  it with a key Tauri makes at launch; Tauri refuses messages that didn't come that way. The
  iframe's hook passes messages on unchanged, since the capability already limits what can be
  called and the commands check their arguments in Rust. It's where a check of messages goes if
  one is ever needed.

## Consequences

- A new command is added in three places: the handler in `src-tauri/src/lib.rs`, the list in
  `build.rs`, and the capability. Tests on Tauri's mock runtime check that the app's commands
  answer, and that Tauri's own commands, plugins, and other pages are refused.
- Anything that needs the operating system goes through a Rust command. If it changes files or
  starts a program, like showing an item in Explorer or Finder, it belongs in `cleanse-exec`
  ([ADR 0002](0002-single-mutator.md)).
- Debug builds can open WebView2's debugging port for inspecting the app while developing, when
  `CLEANSE_WEBVIEW_DEBUG_PORT` is set. Release builds don't have that code.
