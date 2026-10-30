//! The `cleanse` command-line interface.

mod clean;
mod doctor;
mod explain;
mod format;
mod rules;
mod scan;

use std::error::Error;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use cleanse_engine::Engine;

type Result = std::result::Result<(), Box<dyn Error>>;

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
    /// Choose what to delete, then delete it after checking each item again
    Clean(clean::Args),
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
