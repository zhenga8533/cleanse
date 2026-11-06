use std::cmp::Reverse;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use cleanse_engine::{Applied, Engine, Scan, paced};
use cleanse_model::{Candidate, Disposal, Outcome, Plan, PlanItem, Risk, Size};
use inquire::{Confirm, InquireError, MultiSelect};
use serde::Serialize;

use crate::exit::{ABORTED, PARTIAL, USAGE};
use crate::scan::{self, Reporter, items};
use crate::{can_ask, command, format, now, write_json};

/// How many items the selection list shows at once.
const PAGE_SIZE: usize = 15;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Folders to clean. Defaults to your home folder.
    paths: Vec<PathBuf>,
    #[command(flatten)]
    choice: Choosing,
}

/// How the items to remove are chosen, which `cleanse system clean` shares. System cleanup
/// always cleans the system's folders.
#[derive(Debug, clap::Args)]
pub(crate) struct Choosing {
    /// Delete the preselected items up to --max-risk without asking. Items with warnings are
    /// never preselected.
    #[arg(long, requires = "max_risk")]
    yes: bool,
    /// The highest tier that --yes and --dry-run select.
    #[arg(long, value_enum)]
    max_risk: Option<MaxRisk>,
    /// List what --yes would delete, and delete nothing.
    #[arg(long, conflicts_with = "yes")]
    dry_run: bool,
    /// Print what --dry-run lists as JSON.
    #[arg(long, requires = "dry_run")]
    json: bool,
    #[command(flatten)]
    categories: scan::Categories,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum MaxRisk {
    Safe,
    Regenerable,
}

impl From<MaxRisk> for Risk {
    fn from(max: MaxRisk) -> Self {
        match max {
            MaxRisk::Safe => Self::Safe,
            MaxRisk::Regenerable => Self::Regenerable,
        }
    }
}

pub fn run(engine: &Engine, args: &Args) -> Result<ExitCode, Box<dyn Error>> {
    clean(engine, &args.paths, &args.choice)
}

/// Cleans the system's folders, for `cleanse system clean`.
pub(crate) fn run_system(engine: &Engine, args: &Choosing) -> Result<ExitCode, Box<dyn Error>> {
    clean(engine, &[], args)
}

fn clean(engine: &Engine, paths: &[PathBuf], args: &Choosing) -> Result<ExitCode, Box<dyn Error>> {
    let asking = !args.yes && !args.dry_run;
    // Checked before scanning, so nobody waits through a scan only to be turned away.
    if asking && !can_ask() {
        eprintln!(
            "cleanse: clean asks which items to delete, which needs a terminal. To delete the \
             preselected items without asking, add --yes and --max-risk."
        );
        return Ok(ExitCode::from(USAGE));
    }
    if !args.dry_run {
        engine.check_can_change()?;
    }

    let scan = scan::scan(engine, paths, &args.categories)?;
    if args.json {
        let max = args.max_risk.map_or(Risk::Regenerable, Risk::from);
        print_dry_run(&scan, &preselected(&removable(&scan), max))?;
        return Ok(ExitCode::SUCCESS);
    }
    scan::print_scanned(&scan);
    let removable = removable(&scan);
    if removable.is_empty() {
        println!("Nothing can be removed.");
        return Ok(ExitCode::SUCCESS);
    }

    let selected = if asking {
        match choose(&removable)? {
            Some(chosen) => chosen,
            None => return Ok(aborted()),
        }
    } else {
        let max = args.max_risk.map_or(Risk::Regenerable, Risk::from);
        preselected(&removable, max)
    };
    if selected.is_empty() {
        println!("Nothing was selected, so nothing was removed.");
        return Ok(ExitCode::SUCCESS);
    }

    let total = format::size(&format::total(&selected));
    if args.dry_run {
        list(&selected);
        println!(
            "\n--yes would delete {} ({total}). Nothing was removed.",
            items(selected.len())
        );
        return Ok(ExitCode::SUCCESS);
    }
    let plan = Plan {
        roots: scan.roots.clone(),
        items: selected
            .iter()
            .filter_map(|candidate| PlanItem::approve(candidate))
            .collect(),
    };
    if asking && !confirm(&plan.items)? {
        return Ok(aborted());
    }
    remove(engine, &plan)
}

/// Removes the plan's items, showing progress, and reports what happened to each.
pub(crate) fn remove(engine: &Engine, plan: &Plan) -> Result<ExitCode, Box<dyn Error>> {
    let reporter = Reporter::new();
    let total = plan.items.len();
    let applied = paced(
        Reporter::INTERVAL,
        |(done, current): (usize, Option<String>)| {
            reporter.removing(done, total, current.as_deref());
        },
        |report| {
            engine.apply(
                plan,
                &|removal| report((removal.done, removal.current.map(name))),
                &AtomicBool::new(false),
            )
        },
    );
    reporter.clear();
    Ok(report(engine, plan, &applied?))
}

/// What a progress line calls an item: its own name.
fn name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// What can be removed, largest first: everything but what's protected.
fn removable(scan: &Scan) -> Vec<&Candidate> {
    let mut removable: Vec<&Candidate> = scan
        .candidates
        .iter()
        .filter(|candidate| candidate.risk() < Risk::Protected)
        .collect();
    removable.sort_by_key(|candidate| Reverse(candidate.found.size.on_disk));
    removable
}

/// The items selected by default, up to `max`: those without warnings, and never Review items.
fn preselected<'c>(candidates: &[&'c Candidate], max: Risk) -> Vec<&'c Candidate> {
    candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.preselected() && candidate.risk() <= max)
        .collect()
}

/// One line of the selection list. Review items say how long ago they were modified, at `now`,
/// to help decide.
struct Choice<'c> {
    candidate: &'c Candidate,
    now: u64,
}

impl fmt::Display for Choice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let candidate = self.candidate;
        write!(
            f,
            "{:>10}  {:<11}  {}",
            format::bytes(candidate.found.size.on_disk),
            candidate.risk().to_string(),
            candidate.found.path.display()
        )?;
        if candidate.risk() == Risk::Review
            && let Some(at) = candidate.found.modified
        {
            write!(f, "  {}", format::modified(at, self.now))?;
        }
        if let Some(concern) = candidate.concerns().first() {
            write!(f, "  ! {concern}")?;
        }
        Ok(())
    }
}

/// Asks which items to delete, starting from the preselected ones. `None` means the person
/// backed out.
fn choose<'c>(removable: &[&'c Candidate]) -> Result<Option<Vec<&'c Candidate>>, InquireError> {
    let defaults: Vec<usize> = (0..removable.len())
        .filter(|&index| removable[index].preselected())
        .collect();
    println!(
        "{} can be removed ({}). Items with warnings, and Review items, which go to quarantine, \
         aren't selected.",
        items(removable.len()),
        format::size(&format::total(removable))
    );
    let now = now();
    let choices = removable
        .iter()
        .map(|candidate| Choice { candidate, now })
        .collect();
    let chosen = MultiSelect::new("Which items should be removed?", choices)
        .with_default(&defaults)
        .with_page_size(PAGE_SIZE)
        .prompt();
    match chosen {
        Ok(chosen) => Ok(Some(
            chosen.into_iter().map(|choice| choice.candidate).collect(),
        )),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn confirm(selected: &[PlanItem]) -> Result<bool, InquireError> {
    let (quarantined, deleted): (Vec<&PlanItem>, Vec<&PlanItem>) = selected
        .iter()
        .partition(|item| item.disposal == Disposal::Quarantine);
    let amount = |group: &[&PlanItem]| {
        let total: Size = group.iter().map(|item| item.size).sum();
        format!("{} ({})", items(group.len()), format::size(&total))
    };
    let question = match (deleted.is_empty(), quarantined.is_empty()) {
        (false, true) => format!("Delete {}?", amount(&deleted)),
        (true, false) => format!("Quarantine {}?", amount(&quarantined)),
        _ => format!(
            "Delete {} and quarantine {}?",
            amount(&deleted),
            amount(&quarantined)
        ),
    };
    let answer = Confirm::new(&question)
        .with_default(false)
        .with_help_message(
            "Deleted items can't be restored, but they can be rebuilt, such as with npm \
             install. Quarantined items can be restored until they're purged.",
        )
        .prompt();
    match answer {
        Ok(answer) => Ok(answer),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(false),
        Err(error) => Err(error),
    }
}

pub(crate) fn aborted() -> ExitCode {
    println!("Nothing was removed.");
    ExitCode::from(ABORTED)
}

/// What `--yes` would delete, largest first, as JSON.
fn print_dry_run(scan: &Scan, selected: &[&Candidate]) -> crate::Result {
    #[derive(Serialize)]
    struct Output<'a> {
        roots: Vec<String>,
        /// The scan was cancelled, so the results are partial.
        cancelled: bool,
        /// What `--yes` would delete.
        items: &'a [&'a Candidate],
        size: Size,
    }

    write_json(&Output {
        roots: scan
            .roots
            .iter()
            .map(|root| root.to_string_lossy().into_owned())
            .collect(),
        cancelled: scan.cancelled,
        items: selected,
        size: format::total(selected),
    })
}

fn list(candidates: &[&Candidate]) {
    let now = now();
    for candidate in candidates {
        println!("  {}", Choice { candidate, now });
    }
}

/// Prints what happened to each item, and whether all of them were dealt with.
fn report(engine: &Engine, plan: &Plan, applied: &Applied) -> ExitCode {
    let mut deleted = 0;
    let mut freed = Size::default();
    let mut quarantined = 0;
    let mut held = Size::default();
    let mut skipped = Vec::new();
    let mut failed = Vec::new();
    for (item, outcome) in plan.items.iter().zip(&applied.outcomes) {
        match outcome {
            Outcome::Deleted { size } => {
                deleted += 1;
                freed += *size;
            }
            Outcome::Quarantined { size, .. } => {
                quarantined += 1;
                held += *size;
            }
            Outcome::Skipped { reason } => skipped.push((&item.path, reason.to_string())),
            Outcome::Failed { error } => failed.push((&item.path, error.clone())),
        }
    }

    println!(
        "Deleted {}, {}.",
        items(deleted),
        format::bytes(freed.on_disk)
    );
    if quarantined > 0 {
        println!(
            "Quarantined {}, {}. Run `{}` to see or restore them; they take space until you \
             purge them.",
            items(quarantined),
            format::bytes(held.on_disk),
            command(engine, "quarantine")
        );
    }
    for (title, list) in [("Skipped", &skipped), ("Couldn't remove", &failed)] {
        if list.is_empty() {
            continue;
        }
        println!("\n{title}: {}", items(list.len()));
        for (path, why) in list {
            println!("  {}", path.display());
            println!("{:>8}{why}", "! ");
        }
    }
    println!("\nThe journal of this run is {}", applied.journal.display());

    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(PARTIAL)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use cleanse_model::{
        Candidate, CandidateId, Category, Concern, EntryKind, FileIdentity, Match, Risk, RuleId,
        Size,
    };

    use super::preselected;

    fn candidate(risk: Risk, concerns: Vec<Concern>) -> Candidate {
        let found = Match {
            path: PathBuf::from(format!("{risk}")),
            kind: EntryKind::Dir,
            rule: RuleId("test.rule".into()),
            category: Category::Dependencies,
            declared_risk: risk,
            size: Size::default(),
            files: 0,
            modified: None,
            evidence: Vec::new(),
        };
        let identity = FileIdentity { volume: 1, file: 1 };
        Candidate::assess(CandidateId(0), found, Some(identity), concerns)
    }

    #[test]
    fn preselects_items_without_warnings_up_to_the_highest_tier() {
        let safe = candidate(Risk::Safe, vec![]);
        let regenerable = candidate(Risk::Regenerable, vec![]);
        let warned = candidate(Risk::Regenerable, vec![Concern::UncommittedChanges]);
        let review = candidate(Risk::Review, vec![]);
        let all = [&safe, &regenerable, &warned, &review];

        let paths = |selected: Vec<&Candidate>| -> Vec<PathBuf> {
            selected
                .into_iter()
                .map(|candidate| candidate.found.path.clone())
                .collect()
        };

        assert_eq!(
            paths(preselected(&all, Risk::Safe)),
            [PathBuf::from("Safe")]
        );
        assert_eq!(
            paths(preselected(&all, Risk::Regenerable)),
            [PathBuf::from("Safe"), PathBuf::from("Regenerable")]
        );
    }
}
