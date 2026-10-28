//! The `cleanse` command-line interface.

use clap::Parser;

/// Find and safely remove reclaimable files, with an explanation for every item.
#[derive(Debug, Parser)]
#[command(name = "cleanse", version, about, arg_required_else_help = true)]
struct Cli {}

fn main() {
    Cli::parse();
}
