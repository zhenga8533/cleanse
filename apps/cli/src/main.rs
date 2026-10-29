//! The `cleanse` command-line interface.

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
/// This version only scans and explains. It never removes anything.
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
            Command::Scan(args) => scan::run(&engine, args),
            Command::Explain(args) => explain::run(&engine, args),
            Command::Rules(args) => rules::run(&engine, args),
            Command::Doctor => {
                doctor::run(&engine);
                Ok(())
            }
        });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cleanse: {error}");
            ExitCode::FAILURE
        }
    }
}
