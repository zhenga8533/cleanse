use std::env;
use std::path::Path;

fn main() {
    // Listing the app's commands makes each one refused unless a capability allows it, so the
    // web view can only call what `capabilities/` names.
    let commands = tauri_build::AppManifest::new().commands(&[
        "app_info",
        "folders",
        "pick_folders",
        "remove_folder",
        "scan",
        "cancel",
        "summary",
        "candidates",
        "explain",
        "folder",
        "clean",
        "quarantined",
        "restore",
        "purge",
        "history",
        "finish",
        "settings",
        "rules",
    ]);

    // Tauri's Windows code needs Common Controls v6, which a program only gets if its manifest
    // asks for it. tauri-build embeds such a manifest in the app alone, and test programs without
    // one exit before running (STATUS_ENTRYPOINT_NOT_FOUND). So the same manifest, tauri-build's
    // default, goes into every program this package links instead.
    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = Path::new(&env::var("CARGO_MANIFEST_DIR").expect("set by Cargo"))
            .join("windows-app-manifest.xml");
        println!("cargo::rerun-if-changed={}", manifest.display());
        println!("cargo::rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo::rustc-link-arg=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }

    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(commands)
            .windows_attributes(windows),
    )
    .expect("tauri-build failed");
}
