use std::error::Error;
use std::process::ExitCode;

use cleanse_engine::{Action, ApplyError, Engine, Probe, Run};
use cleanse_model::{EntryOutcome, Outcome, Size};

use crate::exit::PARTIAL;
use crate::scan::items;
use crate::{SECONDS_PER_DAY, format, now};

/// How many runs are listed without `--all`.
const RECENT: usize = 20;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// List every run, not just the most recent.
    #[arg(long)]
    all: bool,
    /// Remove what's left of removals that didn't finish.
    #[arg(long)]
    finish: bool,
}

pub fn run(engine: &Engine, args: &Args) -> Result<ExitCode, Box<dyn Error>> {
    if args.finish {
        return finish(engine);
    }
    let history = engine.history()?;
    for journal in &history.unreadable {
        eprintln!("cleanse: couldn't read the journal {}", journal.display());
    }

    if history.runs.is_empty() {
        println!("Cleanse hasn't changed anything yet.");
    } else {
        let shown = if args.all {
            history.runs.len()
        } else {
            RECENT.min(history.runs.len())
        };
        let now = now();
        println!("Most recent first:");
        for run in history.runs.iter().rev().take(shown) {
            println!(
                "  {:<12}  {:<8}  {}",
                format::ago(now.saturating_sub(run.at) / SECONDS_PER_DAY),
                command(run.action),
                summary(run)
            );
        }
        let hidden = history.runs.len() - shown;
        if hidden > 0 {
            println!("  ...and {hidden} earlier. Add --all to list them all.");
        }
    }

    if !history.leftovers.is_empty() {
        println!(
            "\n{} didn't finish, so part of it is still on disk:",
            match history.leftovers.len() {
                1 => "1 removal".to_owned(),
                n => format!("{n} removals"),
            }
        );
        for leftover in &history.leftovers {
            println!(
                "  {} (was {})",
                leftover.tombstone.display(),
                leftover.original.display()
            );
        }
        println!("Run `cleanse history --finish` to remove what's left.");
    }
    Ok(ExitCode::SUCCESS)
}

fn finish(engine: &Engine) -> Result<ExitCode, Box<dyn Error>> {
    if engine.system().is_elevated() {
        return Err(ApplyError::Elevated.into());
    }
    let leftovers = engine.history()?.leftovers;
    if leftovers.is_empty() {
        println!("Nothing to finish.");
        return Ok(ExitCode::SUCCESS);
    }

    let applied = engine.finish(&leftovers)?;
    let mut freed = Size::default();
    let mut finished = 0;
    let mut problems = Vec::new();
    for (leftover, outcome) in leftovers.iter().zip(&applied.outcomes) {
        match outcome {
            Outcome::Deleted { size } => {
                finished += 1;
                freed += *size;
            }
            Outcome::Skipped { reason } => problems.push((leftover, reason.to_string())),
            Outcome::Failed { error } => problems.push((leftover, error.clone())),
            Outcome::Quarantined { .. } => {}
        }
    }
    println!(
        "Removed what was left of {}, freeing {}.",
        items(finished),
        format::bytes(freed.on_disk)
    );
    if !problems.is_empty() {
        println!("\nCouldn't finish: {}", items(problems.len()));
        for (leftover, why) in &problems {
            println!("  {}", leftover.tombstone.display());
            println!("{:>8}{why}", "! ");
        }
    }
    println!("\nThe journal of this run is {}", applied.journal.display());
    Ok(if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(PARTIAL)
    })
}

/// The command that made the run.
fn command(action: Action) -> &'static str {
    match action {
        Action::Apply => "clean",
        Action::Restore => "restore",
        Action::Purge => "purge",
        Action::Finish => "finish",
    }
}

/// What a run did, like "Deleted 3 (1.2 GiB), quarantined 1 (12 MiB), skipped 1".
fn summary(run: &Run) -> String {
    let mut deleted = (0, Size::default());
    let mut quarantined = (0, Size::default());
    let mut purged = (0, Size::default());
    let (mut restored, mut skipped, mut failed) = (0, 0, 0);
    for outcome in &run.outcomes {
        match outcome {
            Outcome::Deleted { size } => {
                deleted.0 += 1;
                deleted.1 += *size;
            }
            Outcome::Quarantined { size, .. } => {
                quarantined.0 += 1;
                quarantined.1 += *size;
            }
            Outcome::Skipped { .. } => skipped += 1,
            Outcome::Failed { .. } => failed += 1,
        }
    }
    for outcome in &run.entry_outcomes {
        match outcome {
            EntryOutcome::Restored => restored += 1,
            EntryOutcome::Purged { size } => {
                purged.0 += 1;
                purged.1 += *size;
            }
            EntryOutcome::Failed { .. } => failed += 1,
        }
    }

    let sized = |verb: &str, (count, size): (usize, Size)| {
        (count > 0).then(|| format!("{verb} {count} ({})", format::bytes(size.on_disk)))
    };
    let counted = |verb: &str, count: usize| (count > 0).then(|| format!("{verb} {count}"));
    let parts: Vec<String> = [
        sized("deleted", deleted),
        sized("quarantined", quarantined),
        counted("restored", restored),
        sized("purged", purged),
        counted("skipped", skipped),
        counted("failed", failed),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut summary = if parts.is_empty() {
        format!("nothing done, of {}", items(run.items))
    } else {
        parts.join(", ")
    };
    if !run.finished {
        summary.push_str(" (cut short)");
    }
    let mut chars = summary.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
