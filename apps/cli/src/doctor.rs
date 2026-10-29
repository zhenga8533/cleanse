use std::path::Path;

use cleanse_engine::{Engine, Probe};

pub fn run(engine: &Engine) {
    let system = engine.system();
    let folders = system.known_folders();
    println!(
        "Cleanse {} on {} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!(
        "Running as administrator or root: {}",
        if system.is_elevated() { "yes" } else { "no" }
    );
    println!("Rules: {} built in", engine.rules().rules().len());
    println!("Cloud sync detection: {}", cloud_detection());
    println!("Folders:");
    let folder = |name: &str, path: Option<&Path>| match path {
        Some(path) => println!("  {name:<12} {}", path.display()),
        None => println!("  {name:<12} (not defined)"),
    };
    folder("Home", folders.home.as_deref());
    folder("Documents", folders.documents.as_deref());
    folder("Downloads", folders.downloads.as_deref());
    folder("Cache", folders.cache.as_deref());
    folder("App data", folders.data_local.as_deref());
    folder("Settings", folders.config.as_deref());
    folder("Temp", Some(&folders.temp));
}

fn cloud_detection() -> &'static str {
    if cfg!(windows) {
        "Windows Cloud Files API (OneDrive, iCloud, Dropbox, and other providers)"
    } else if cfg!(target_os = "macos") {
        "by location (~/Library/CloudStorage and iCloud Drive)"
    } else {
        "not available; synced folders aren't recognized on Linux yet"
    }
}
