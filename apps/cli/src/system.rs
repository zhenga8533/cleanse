//! `cleanse system`: cleaning the operating system's own folders, as an administrator or root
//! (ADR 0010). It's a cleanup of its own, with its own rules, journal, and quarantine, and it
//! reads no settings or plan files.

use std::error::Error;
use std::process::ExitCode;

use clap::Subcommand;
use cleanse_engine::{Engine, Probe};

use crate::exit::USAGE;
use crate::{clean, explain, history, quarantine, rules, scan};

#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List what could be removed from the system's folders, and what's kept and why
    Scan(scan::Report),
    /// Choose what to remove from the system's folders, then remove it after checking each item
    /// again
    Clean(clean::Choosing),
    /// List what system cleanup quarantined, or purge it for good
    Quarantine(quarantine::Args),
    /// Put what system cleanup quarantined back where it was
    Restore(quarantine::RestoreArgs),
    /// Show what system cleanup has removed, restored, and purged
    History(history::Args),
    /// Explain whether a file or folder could be removed by system cleanup, and why
    Explain(explain::Args),
    /// List the system cleanup rules, or show one in detail
    Rules(rules::Args),
}

pub fn run(args: &Args) -> Result<ExitCode, Box<dyn Error>> {
    let engine = Engine::new_system()?;
    // Listing the rules reads nothing on this system, so it works anywhere.
    if !matches!(args.command, Command::Rules(_))
        && let Some(why) = unavailable(&engine)
    {
        eprintln!("cleanse: {why}");
        return Ok(ExitCode::from(USAGE));
    }
    match &args.command {
        Command::Scan(args) => scan::run_system(&engine, args).map(|()| ExitCode::SUCCESS),
        Command::Clean(args) => clean::run_system(&engine, args),
        Command::Quarantine(args) => quarantine::run(&engine, args),
        Command::Restore(args) => quarantine::restore(&engine, args),
        Command::History(args) => history::run(&engine, args),
        Command::Explain(args) => explain::run(&engine, args).map(|()| ExitCode::SUCCESS),
        Command::Rules(args) => rules::run(&engine, args).map(|()| ExitCode::SUCCESS),
    }
}

/// Why system cleanup can't run here, if it can't. Even a scan needs elevation: without it, the
/// owners of the system's files often can't be read, so the scan would keep everything for
/// reasons that aren't the real ones.
fn unavailable(engine: &Engine) -> Option<&'static str> {
    if !cfg!(any(windows, target_os = "linux")) {
        Some("system cleanup is only for Windows and Linux")
    } else if engine.system().is_elevated() {
        None
    } else if cfg!(windows) {
        Some(
            "system cleanup works in Windows' own folders, so it runs only as an administrator. \
             Open a terminal with Run as administrator, and run this again there.",
        )
    } else {
        Some(
            "system cleanup works in the system's own folders, so it runs only as root. Run \
             this again with sudo in front.",
        )
    }
}
