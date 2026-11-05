use std::path::Path;

use cleanse_engine::{Elevation, Engine, Probe, SystemFolders, cloud_sync_detection};
use cleanse_model::RuleId;
use serde::Serialize;

use crate::{format, write_json};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Print the report as JSON.
    #[arg(long)]
    json: bool,
}

pub fn run(engine: &Engine, args: &Args) -> crate::Result {
    if args.json {
        return print_json(engine);
    }
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
        match system.elevation() {
            Elevation::None => "no",
            Elevation::Elevated => "yes, so cleaning your own files is off",
            Elevation::AlwaysElevated => {
                "yes, as everything this account runs is, as with UAC off, so cleaning your own \
                 files still works"
            }
        }
    );
    let config = engine.config();
    let disabled = config.disabled_rules.len();
    if disabled == 0 {
        println!("Rules: {} built in", engine.rules().rules().len());
    } else {
        println!(
            "Rules: {} built in, {disabled} turned off in your settings",
            engine.rules().rules().len() + disabled
        );
    }
    println!("Cloud sync detection: {}", cloud_sync_detection());
    match engine.config_path() {
        Some(path) if path.exists() => println!("Settings: {}", path.display()),
        Some(path) => println!("Settings: {} (not created)", path.display()),
        None => println!("Settings: none; this system has no folder for app settings"),
    }
    for rule in &config.disabled_rules {
        println!("  turned off   {rule}");
    }
    for folder in &config.exclude {
        println!("  excluded     {}", folder.display());
    }
    println!(
        "Large files: {} or more on disk",
        format::bytes(config.large_file_size)
    );
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

    let system_folders = SystemFolders::current()?;
    println!("System cleanup: {}", system_cleanup());
    // Each OS has only some of them, so the others aren't listed.
    for (name, path) in [
        ("Windows", system_folders.system_root.clone()),
        ("ProgramData", system_folders.program_data.clone()),
        ("var", system_folders.var.clone()),
        ("Its data", system_folders.cleanse_data()),
    ] {
        if let Some(path) = path {
            println!("  {name:<12} {}", path.display());
        }
    }
    Ok(())
}

/// How to run system cleanup here, if it runs here at all.
fn system_cleanup() -> &'static str {
    if cfg!(windows) {
        "`cleanse system`, from a terminal opened with Run as administrator"
    } else if cfg!(target_os = "linux") {
        "`sudo cleanse system`"
    } else {
        "only on Windows and Linux"
    }
}

fn print_json(engine: &Engine) -> crate::Result {
    #[derive(Serialize)]
    struct Report<'a> {
        version: &'static str,
        os: &'static str,
        arch: &'static str,
        elevated: bool,
        /// Everything the account runs is elevated, as on Windows with UAC off, so cleaning its
        /// own files still works.
        always_elevated: bool,
        /// Every built-in rule, counting those turned off.
        rules: usize,
        cloud_sync_detection: &'static str,
        settings: Settings<'a>,
        folders: Folders,
        system_cleanup: SystemCleanup,
    }

    /// `None` for folders the system doesn't have, and macOS has none of them.
    #[derive(Serialize)]
    struct SystemCleanup {
        /// Whether system cleanup runs on this operating system, when elevated.
        available: bool,
        system_root: Option<String>,
        program_data: Option<String>,
        var: Option<String>,
        /// Where system cleanup keeps its journal and quarantine.
        data: Option<String>,
    }

    #[derive(Serialize)]
    struct Settings<'a> {
        /// `None` if the system has no folder for app settings.
        path: Option<String>,
        exists: bool,
        exclude: Vec<String>,
        disabled_rules: &'a [RuleId],
        large_file_size: u64,
    }

    /// `None` for folders the system doesn't define.
    #[derive(Serialize)]
    struct Folders {
        home: Option<String>,
        documents: Option<String>,
        downloads: Option<String>,
        cache: Option<String>,
        data_local: Option<String>,
        config: Option<String>,
        temp: String,
    }

    let lossy = |path: &Path| path.to_string_lossy().into_owned();
    let folders = engine.system().known_folders();
    let config = engine.config();
    let system_folders = SystemFolders::current()?;
    write_json(&Report {
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        elevated: engine.system().is_elevated(),
        always_elevated: engine.system().elevation() == Elevation::AlwaysElevated,
        rules: engine.rules().rules().len() + config.disabled_rules.len(),
        cloud_sync_detection: cloud_sync_detection(),
        settings: Settings {
            path: engine.config_path().map(lossy),
            exists: engine.config_path().is_some_and(Path::exists),
            exclude: config.exclude.iter().map(|path| lossy(path)).collect(),
            disabled_rules: &config.disabled_rules,
            large_file_size: config.large_file_size,
        },
        folders: Folders {
            home: folders.home.as_deref().map(lossy),
            documents: folders.documents.as_deref().map(lossy),
            downloads: folders.downloads.as_deref().map(lossy),
            cache: folders.cache.as_deref().map(lossy),
            data_local: folders.data_local.as_deref().map(lossy),
            config: folders.config.as_deref().map(lossy),
            temp: lossy(&folders.temp),
        },
        system_cleanup: SystemCleanup {
            available: cfg!(any(windows, target_os = "linux")),
            system_root: system_folders.system_root.as_deref().map(lossy),
            program_data: system_folders.program_data.as_deref().map(lossy),
            var: system_folders.var.as_deref().map(lossy),
            data: system_folders.cleanse_data().as_deref().map(lossy),
        },
    })
}
