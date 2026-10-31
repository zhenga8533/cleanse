use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;

use cleanse_engine::{ApplyError, Engine, Probe, load_plan};

use crate::clean::{aborted, confirm, remove};
use crate::exit::USAGE;
use crate::scan::items;
use crate::{SECONDS_PER_DAY, can_ask, format, now};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// A plan saved by `cleanse scan --save`.
    plan: PathBuf,
    /// Remove the plan's selected items without asking.
    #[arg(long)]
    yes: bool,
}

pub fn run(engine: &Engine, args: &Args) -> Result<ExitCode, Box<dyn Error>> {
    if !args.yes && !can_ask() {
        eprintln!(
            "cleanse: apply asks before removing anything, which needs a terminal. To remove \
             the plan's selected items without asking, add --yes."
        );
        return Ok(ExitCode::from(USAGE));
    }
    if engine.system().is_elevated() {
        return Err(ApplyError::Elevated.into());
    }
    let saved = load_plan(&args.plan)
        .map_err(|error| format!("couldn't read the plan {}: {error}", args.plan.display()))?;

    let roots: Vec<String> = saved
        .roots
        .iter()
        .map(|root| root.display().to_string())
        .collect();
    println!(
        "This plan was saved {} from a scan of {}.",
        format::ago(now().saturating_sub(saved.saved_at) / SECONDS_PER_DAY),
        roots.join(", ")
    );
    let plan = saved.selected();
    let unselected = saved.items.len() - plan.items.len();
    if plan.items.is_empty() {
        println!(
            "Nothing in it is selected, so nothing was removed. To select an item, set its \
             \"selected\" to true."
        );
        return Ok(ExitCode::SUCCESS);
    }

    println!("Selected:");
    for item in &plan.items {
        println!(
            "  {:>10}  {:<11}  {}",
            format::bytes(item.size.on_disk),
            item.risk.to_string(),
            item.path.display()
        );
        for concern in &item.concerns {
            println!("{:>29}{concern}", "! ");
        }
    }
    if unselected > 0 {
        println!("Not selected, so kept: {}.", items(unselected));
    }
    println!("Each item is checked again first, and left alone if anything about it changed.");

    if !args.yes && !confirm(&plan.items)? {
        return Ok(aborted());
    }
    remove(engine, &plan)
}
