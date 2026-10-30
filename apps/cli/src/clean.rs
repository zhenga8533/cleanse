use std::cmp::Reverse;
use std::error::Error;
use std::fmt;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use cleanse_engine::{Applied, ApplyError, Engine, Probe, Scan};
use cleanse_model::{Candidate, Outcome, Plan, PlanItem, Risk, Size};
use inquire::{Confirm, InquireError, MultiSelect};

use crate::format;
use crate::scan::{self, Reporter, items};

/// The command line was fine, but it can't work this way, such as asking without a terminal.
const USAGE: u8 = 2;
/// Some approved items couldn't be removed.
const PARTIAL: u8 = 3;
/// The person declined, so nothing was removed.
const ABORTED: u8 = 4;

/// How many items the selection list shows at once.
const PAGE_SIZE: usize = 15;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Folders to clean. Defaults to your home folder.
    paths: Vec<PathBuf>,
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
    let asking = !args.yes && !args.dry_run;
    // Checked before scanning, so nobody waits through a scan only to be turned away.
    if asking && !(io::stdin().is_terminal() && io::stderr().is_terminal()) {
        eprintln!(
            "cleanse: clean asks which items to delete, which needs a terminal. To delete the \
             preselected items without asking, add --yes and --max-risk."
        );
        return Ok(ExitCode::from(USAGE));
    }
    if !args.dry_run && engine.system().is_elevated() {
        return Err(ApplyError::Elevated.into());
    }

    let scan = scan::scan(engine, &args.paths)?;
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
    if asking && !confirm(selected.len(), &total)? {
        return Ok(aborted());
    }

    let plan = Plan {
        roots: scan.roots.clone(),
        items: selected
            .iter()
            .filter_map(|candidate| PlanItem::approve(candidate))
            .collect(),
    };
    let reporter = Reporter::new();
    let count = plan.items.len();
    let applied = engine.apply(
        &plan,
        &|done| reporter.removing(done, count),
        &AtomicBool::new(false),
    );
    reporter.clear();
    Ok(report(&plan, &applied?))
}

/// What can be deleted, largest first. Review items are left out because they go to quarantine,
/// which isn't available yet.
fn removable(scan: &Scan) -> Vec<&Candidate> {
    let mut removable: Vec<&Candidate> = scan
        .candidates
        .iter()
        .filter(|candidate| candidate.risk() <= Risk::Regenerable)
        .collect();
    removable.sort_by_key(|candidate| Reverse(candidate.found.size.on_disk));
    removable
}

/// The items selected by default, up to `max`: those without warnings.
fn preselected<'c>(candidates: &[&'c Candidate], max: Risk) -> Vec<&'c Candidate> {
    candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.preselected() && candidate.risk() <= max)
        .collect()
}

/// One line of the selection list.
struct Choice<'c>(&'c Candidate);

impl fmt::Display for Choice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let candidate = self.0;
        write!(
            f,
            "{:>10}  {:<11}  {}",
            format::bytes(candidate.found.size.on_disk),
            candidate.risk().to_string(),
            candidate.found.path.display()
        )?;
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
        "{} can be deleted ({}). Items with warnings aren't selected.",
        items(removable.len()),
        format::size(&format::total(removable))
    );
    let choices = removable.iter().map(|candidate| Choice(candidate)).collect();
    let chosen = MultiSelect::new("Which items should be deleted?", choices)
        .with_default(&defaults)
        .with_page_size(PAGE_SIZE)
        .prompt();
    match chosen {
        Ok(chosen) => Ok(Some(chosen.into_iter().map(|choice| choice.0).collect())),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(error) => Err(error),
    }
}

fn confirm(count: usize, total: &str) -> Result<bool, InquireError> {
    let answer = Confirm::new(&format!("Delete {} ({total})?", items(count)))
        .with_default(false)
        .with_help_message(
            "Deleted items can't be restored, but they can be rebuilt, such as with npm install",
        )
        .prompt();
    match answer {
        Ok(answer) => Ok(answer),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(false),
        Err(error) => Err(error),
    }
}

fn aborted() -> ExitCode {
    println!("Nothing was removed.");
    ExitCode::from(ABORTED)
}

fn list(candidates: &[&Candidate]) {
    for candidate in candidates {
        println!("  {}", Choice(candidate));
    }
}

/// Prints what happened to each item, and whether all of them were dealt with.
fn report(plan: &Plan, applied: &Applied) -> ExitCode {
    let mut deleted = 0;
    let mut freed = Size::default();
    let mut skipped = Vec::new();
    let mut failed = Vec::new();
    for (item, outcome) in plan.items.iter().zip(&applied.outcomes) {
        match outcome {
            Outcome::Deleted { size } => {
                deleted += 1;
                freed += *size;
            }
            Outcome::Skipped { reason } => skipped.push((&item.path, reason.to_string())),
            Outcome::Failed { error } => failed.push((&item.path, error.clone())),
        }
    }

    println!("Deleted {}, {}.", items(deleted), format::bytes(freed.on_disk));
    for (title, list) in [("Skipped", &skipped), ("Couldn't delete", &failed)] {
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
        let all = [&safe, &regenerable, &warned];

        let paths = |selected: Vec<&Candidate>| -> Vec<PathBuf> {
            selected
                .into_iter()
                .map(|candidate| candidate.found.path.clone())
                .collect()
        };

        assert_eq!(paths(preselected(&all, Risk::Safe)), [PathBuf::from("Safe")]);
        assert_eq!(
            paths(preselected(&all, Risk::Regenerable)),
            [PathBuf::from("Safe"), PathBuf::from("Regenerable")]
        );
    }
}
