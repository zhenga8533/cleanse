//! The `cleanse` command-line interface.

mod clean;
mod doctor;
mod explain;
mod format;
mod quarantine;
mod rules;
mod scan;

use std::error::Error;
use std::io::{self, IsTerminal};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use cleanse_engine::Engine;

type Result = std::result::Result<(), Box<dyn Error>>;

/// Exit codes besides success (0) and failure (1).
mod exit {
    /// The command line was fine, but it can't work this way, such as asking without a terminal.
    pub const USAGE: u8 = 2;
    /// Some of the items couldn't be removed, restored, or purged.
    pub const PARTIAL: u8 = 3;
    /// The person declined, so nothing was changed.
    pub const ABORTED: u8 = 4;
}

/// Whether there's a terminal to ask questions in.
fn can_ask() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

/// Find files you can safely remove, with an explanation for every item.
///
/// Scanning and explaining never remove anything. `clean` deletes only what you approve, after
/// checking each item again.
#[derive(Debug, Parser)]
#[command(name = "cleanse", version, about, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List what could be removed in some folders, and what's kept and why
    Scan(scan::Args),
    /// Choose what to remove, then remove it after checking each item again
    Clean(clean::Args),
    /// List what's in quarantine, or purge it for good
    Quarantine(quarantine::Args),
    /// Put quarantined items back where they were
    Restore(quarantine::RestoreArgs),
    /// Explain whether a file or folder could be removed, and why
    Explain(explain::Args),
    /// List the cleanup rules, or show one in detail
    Rules(rules::Args),
    /// Show what Cleanse knows about this system
    Doctor,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = Engine::new()
        .map_err(Into::into)
        .and_then(|engine| match &cli.command {
            Command::Scan(args) => scan::run(&engine, args).map(|()| ExitCode::SUCCESS),
            Command::Clean(args) => clean::run(&engine, args),
            Command::Quarantine(args) => quarantine::run(&engine, args),
            Command::Restore(args) => quarantine::restore(&engine, args),
            Command::Explain(args) => explain::run(&engine, args).map(|()| ExitCode::SUCCESS),
            Command::Rules(args) => rules::run(&engine, args).map(|()| ExitCode::SUCCESS),
            Command::Doctor => {
                doctor::run(&engine);
                Ok(ExitCode::SUCCESS)
            }
        });
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("cleanse: {error}");
            ExitCode::FAILURE
        }
    }
}
