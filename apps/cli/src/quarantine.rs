use std::error::Error;
use std::io;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Subcommand;
use cleanse_engine::{ApplyError, Engine, Handled, Listing, Probe};
use cleanse_model::{EntryOutcome, QuarantineEntry, Size};
use inquire::{Confirm, InquireError};

use crate::exit::{ABORTED, PARTIAL, USAGE};
use crate::scan::items;
use crate::{can_ask, format};

const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List what's in quarantine (the default)
    List {
        /// Print the list as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Delete quarantined items for good, freeing their space
    Purge(PurgeArgs),
}

#[derive(Debug, clap::Args)]
#[group(id = "which", required = true, args = ["ids", "all", "older_than"])]
struct PurgeArgs {
    /// IDs of the items to purge, as `cleanse quarantine` lists them.
    ids: Vec<String>,
    /// Purge everything in quarantine.
    #[arg(long)]
    all: bool,
    /// Purge what was quarantined at least this many days ago.
    #[arg(long, value_name = "DAYS")]
    older_than: Option<u64>,
    /// Purge without asking first.
    #[arg(long)]
    yes: bool,
}

#[derive(Debug, clap::Args)]
#[group(id = "which", required = true, args = ["ids", "all"])]
pub struct RestoreArgs {
    /// IDs of the items to put back, as `cleanse quarantine` lists them.
    ids: Vec<String>,
    /// Put back everything in quarantine.
    #[arg(long)]
    all: bool,
}

pub fn run(engine: &Engine, args: &Args) -> Result<ExitCode, Box<dyn Error>> {
    match &args.command {
        None => list(engine, false),
        Some(Command::List { json }) => list(engine, *json),
        Some(Command::Purge(args)) => purge(engine, args),
    }
}

fn list(engine: &Engine, json: bool) -> Result<ExitCode, Box<dyn Error>> {
    let listing = engine.quarantined()?;
    if json {
        serde_json::to_writer_pretty(io::stdout().lock(), &listing.entries)?;
        println!();
        return Ok(ExitCode::SUCCESS);
    }

    report_unreadable(&listing);
    if listing.entries.is_empty() {
        println!("Nothing is in quarantine.");
        return Ok(ExitCode::SUCCESS);
    }
    let total: Size = listing.entries.iter().map(|entry| entry.size).sum();
    println!(
        "{} in quarantine, {}. They take space until you purge them.\n",
        items(listing.entries.len()),
        format::bytes(total.on_disk)
    );
    let width = listing
        .entries
        .iter()
        .map(|entry| entry.id.len())
        .max()
        .unwrap_or(0);
    println!(
        "  {:<width$}  {:<12}  {:>10}  Was at",
        "ID", "Quarantined", "Size"
    );
    let now = now();
    for entry in &listing.entries {
        println!(
            "  {:<width$}  {:<12}  {:>10}  {}",
            entry.id,
            format::ago(now.saturating_sub(entry.at) / SECONDS_PER_DAY),
            format::bytes(entry.size.on_disk),
            entry.original.display()
        );
    }
    println!(
        "\n`cleanse restore ID` puts an item back; `cleanse quarantine purge ID` deletes it for \
         good."
    );
    Ok(ExitCode::SUCCESS)
}

fn purge(engine: &Engine, args: &PurgeArgs) -> Result<ExitCode, Box<dyn Error>> {
    if !args.yes && !can_ask() {
        eprintln!(
            "cleanse: purge asks before deleting, which needs a terminal. To purge without \
             asking, add --yes."
        );
        return Ok(ExitCode::from(USAGE));
    }
    if engine.system().is_elevated() {
        return Err(ApplyError::Elevated.into());
    }
    let listing = engine.quarantined()?;
    report_unreadable(&listing);
    let chosen = if let Some(days) = args.older_than {
        let now = now();
        listing
            .entries
            .into_iter()
            .filter(|entry| now.saturating_sub(entry.at) >= days * SECONDS_PER_DAY)
            .collect()
    } else {
        choose(listing, &args.ids, args.all)?
    };
    if chosen.is_empty() {
        println!("Nothing to purge.");
        return Ok(ExitCode::SUCCESS);
    }

    let total: Size = chosen.iter().map(|entry| entry.size).sum();
    if !args.yes {
        let question = format!(
            "Purge {} ({})?",
            items(chosen.len()),
            format::bytes(total.on_disk)
        );
        let answer = Confirm::new(&question)
            .with_default(false)
            .with_help_message("Purged items are deleted for good and can't be restored")
            .prompt();
        match answer {
            Ok(true) => {}
            Ok(false)
            | Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                println!("Nothing was purged.");
                return Ok(ExitCode::from(ABORTED));
            }
            Err(error) => return Err(error.into()),
        }
    }
    let handled = engine.purge(&chosen)?;
    Ok(report(&chosen, &handled, "Purged", "Couldn't purge"))
}

pub fn restore(engine: &Engine, args: &RestoreArgs) -> Result<ExitCode, Box<dyn Error>> {
    if engine.system().is_elevated() {
        return Err(ApplyError::Elevated.into());
    }
    let listing = engine.quarantined()?;
    report_unreadable(&listing);
    let chosen = choose(listing, &args.ids, args.all)?;
    if chosen.is_empty() {
        println!("Nothing is in quarantine.");
        return Ok(ExitCode::SUCCESS);
    }
    let handled = engine.restore(&chosen)?;
    Ok(report(&chosen, &handled, "Restored", "Couldn't restore"))
}

/// The entries named by `ids`, or all of them. An ID that isn't in quarantine is an error, so a
/// typo never quietly does less than asked.
fn choose(
    listing: Listing,
    ids: &[String],
    all: bool,
) -> Result<Vec<QuarantineEntry>, Box<dyn Error>> {
    if all {
        return Ok(listing.entries);
    }
    if let Some(unknown) = ids
        .iter()
        .find(|id| !listing.entries.iter().any(|entry| &entry.id == *id))
    {
        return Err(format!(
            "nothing in quarantine has the ID {unknown}. Run `cleanse quarantine` to list them."
        )
        .into());
    }
    Ok(listing
        .entries
        .into_iter()
        .filter(|entry| ids.contains(&entry.id))
        .collect())
}

fn report(entries: &[QuarantineEntry], handled: &Handled, done: &str, failed: &str) -> ExitCode {
    let mut freed = Size::default();
    let mut succeeded = 0;
    let mut failures = Vec::new();
    for (entry, outcome) in entries.iter().zip(&handled.outcomes) {
        match outcome {
            EntryOutcome::Restored => succeeded += 1,
            EntryOutcome::Purged { size } => {
                succeeded += 1;
                freed += *size;
            }
            EntryOutcome::Failed { error } => failures.push((entry, error)),
        }
    }

    if freed.on_disk > 0 {
        println!(
            "{done} {}, freeing {}.",
            items(succeeded),
            format::bytes(freed.on_disk)
        );
    } else {
        println!("{done} {}.", items(succeeded));
    }
    if !failures.is_empty() {
        println!("\n{failed}: {}", items(failures.len()));
        for (entry, error) in &failures {
            println!("  {} (was at {})", entry.id, entry.original.display());
            println!("{:>8}{error}", "! ");
        }
    }
    println!("\nThe journal of this run is {}", handled.journal.display());

    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(PARTIAL)
    }
}

fn report_unreadable(listing: &Listing) {
    for record in &listing.unreadable {
        eprintln!(
            "cleanse: couldn't read the quarantine record {}, so it isn't listed",
            record.display()
        );
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}
