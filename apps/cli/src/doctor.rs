use std::path::Path;

use cleanse_engine::{Engine, Probe, cloud_sync_detection};
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
        if system.is_elevated() { "yes" } else { "no" }
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
    Ok(())
}

fn print_json(engine: &Engine) -> crate::Result {
    #[derive(Serialize)]
    struct Report<'a> {
        version: &'static str,
        os: &'static str,
        arch: &'static str,
        elevated: bool,
        /// Every built-in rule, counting those turned off.
        rules: usize,
        cloud_sync_detection: &'static str,
        settings: Settings<'a>,
        folders: Folders,
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
    write_json(&Report {
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        elevated: engine.system().is_elevated(),
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
    })
}
