use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use clap::builder::{PossibleValuesParser, TypedValueParser};
use cleanse_engine::{Engine, Progress, RuleSet, Scan, save_plan};
use cleanse_model::{Candidate, Category, Concern, PlanItem, Risk, RuleId, SavedPlan, Size};
use serde::Serialize;

use crate::{format, now};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Folders to scan. Defaults to your home folder.
    paths: Vec<PathBuf>,
    /// Print the results as JSON.
    #[arg(long)]
    json: bool,
    /// List every item that can be removed or needs your review, not just the largest.
    #[arg(long)]
    all: bool,
    /// Also list everything that's kept, and why.
    #[arg(long)]
    show_protected: bool,
    /// Save what can be removed to a plan file, to review and then remove with `cleanse apply`.
    #[arg(long, value_name = "FILE")]
    save: Option<PathBuf>,
    #[command(flatten)]
    categories: Categories,
}

/// `--category`, shared by the commands that scan.
#[derive(Debug, clap::Args)]
pub(crate) struct Categories {
    /// Only items in these categories, separated by commas, like `temp,logs`.
    #[arg(
        long = "category",
        value_name = "CATEGORY",
        value_delimiter = ',',
        value_parser = PossibleValuesParser::new(Category::ALL.map(Category::id))
            .try_map(|id| {
                Category::ALL
                    .into_iter()
                    .find(|category| category.id() == id)
                    .ok_or_else(|| format!("`{id}` isn't a category"))
            }),
    )]
    categories: Vec<Category>,
}

impl Categories {
    /// Leaves only the candidates in the chosen categories, or all of them if none were chosen.
    pub(crate) fn keep(&self, candidates: &mut Vec<Candidate>) {
        if !self.categories.is_empty() {
            candidates.retain(|candidate| self.categories.contains(&candidate.found.category));
        }
    }
}

/// How many of the largest removable items the summary lists.
const LARGEST: usize = 10;

pub fn run(engine: &Engine, args: &Args) -> crate::Result {
    let scan = scan(engine, &args.paths, &args.categories)?;
    if args.json {
        print_json(engine, &scan)?;
    } else {
        print_human(engine, &scan, args);
    }
    if let Some(path) = &args.save {
        save(&scan, path, args.json)?;
    }
    Ok(())
}

/// Saves the scan as a plan, and says how to apply it: on stderr when stdout holds JSON.
fn save(scan: &Scan, path: &Path, json: bool) -> crate::Result {
    let plan = SavedPlan::new(&scan.roots, &scan.candidates, now());
    save_plan(path, &plan)
        .map_err(|error| format!("couldn't save the plan to {}: {error}", path.display()))?;

    let selected = plan.items.iter().filter(|saved| saved.selected).count();
    let approvable = scan
        .candidates
        .iter()
        .filter(|candidate| PlanItem::approve(candidate).is_some())
        .count();
    let say = |line: String| {
        if json {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    };
    if !json {
        println!();
    }
    say(format!(
        "Saved a plan of {}, {selected} selected, to {}. Review it, then run `cleanse apply {}` \
         to remove the selected items.",
        items(plan.items.len()),
        path.display(),
        path.display()
    ));
    let left_out = approvable - plan.items.len();
    if left_out > 0 {
        say(format!(
            "{} left out, because their paths aren't valid Unicode and the plan can't hold them \
             exactly.",
            items(left_out)
        ));
    }
    Ok(())
}

/// Scans `paths`, or the home folder if there are none, showing progress, and keeps the
/// candidates in `categories`.
pub(crate) fn scan(
    engine: &Engine,
    paths: &[PathBuf],
    categories: &Categories,
) -> io::Result<Scan> {
    let roots = if paths.is_empty() {
        engine.default_roots()
    } else {
        paths.to_vec()
    };
    let reporter = Reporter::new();
    let scan = engine.scan(
        &roots,
        &|progress| reporter.scanning(progress),
        &AtomicBool::new(false),
    );
    reporter.clear();
    let mut scan = scan?;
    categories.keep(&mut scan.candidates);
    Ok(scan)
}

/// Shows progress on one line of stderr, at most a few times a second, and only when stderr is
/// a terminal so piped output stays clean.
pub(crate) struct Reporter {
    last: Option<Mutex<Instant>>,
}

impl Reporter {
    const INTERVAL: Duration = Duration::from_millis(100);

    pub(crate) fn new() -> Self {
        let interactive = io::stderr().is_terminal();
        Self {
            last: interactive.then(|| Mutex::new(Instant::now())),
        }
    }

    pub(crate) fn scanning(&self, progress: Progress) {
        self.show(|| match progress {
            Progress::Walking { entries } => {
                format!("Scanning: {} entries", format::count(entries))
            }
            Progress::Assessing { done, total } => format!("Checking {done} of {total} matches"),
        });
    }

    pub(crate) fn removing(&self, done: usize, total: usize) {
        self.show(|| format!("Removing {done} of {total}"));
    }

    fn show(&self, message: impl FnOnce() -> String) {
        let Some(last) = &self.last else { return };
        let mut last = last.lock().unwrap_or_else(PoisonError::into_inner);
        if last.elapsed() < Self::INTERVAL {
            return;
        }
        *last = Instant::now();
        eprint!("\r{:<60}", message());
        // Progress is best-effort; a failed flush only delays the next update.
        let _ = io::stderr().flush();
    }

    pub(crate) fn clear(&self) {
        if self.last.is_some() {
            eprint!("\r{:<60}\r", "");
        }
    }
}

/// The first lines of a scan's report: what was scanned, how much, and how long it took.
pub(crate) fn print_scanned(scan: &Scan) {
    let roots: Vec<String> = scan
        .roots
        .iter()
        .map(|root| root.display().to_string())
        .collect();
    println!(
        "Scanned {}: {} entries in {}",
        roots.join(", "),
        format::count(scan.entries),
        format::seconds(scan.elapsed)
    );
    if scan.cancelled {
        println!("The scan was cancelled, so these results are partial.");
    }
}

fn print_human(engine: &Engine, scan: &Scan, args: &Args) {
    print_scanned(scan);

    let mut by_size: Vec<&Candidate> = scan.candidates.iter().collect();
    by_size.sort_by_key(|candidate| Reverse(candidate.found.size.on_disk));
    let tier = |wanted: &[Risk]| -> Vec<&Candidate> {
        by_size
            .iter()
            .copied()
            .filter(|candidate| wanted.contains(&candidate.risk()))
            .collect()
    };
    let removable = tier(&[Risk::Safe, Risk::Regenerable]);
    let review = tier(&[Risk::Review]);
    let mut kept = tier(&[Risk::Protected]);
    let advised = advised(engine.rules(), &by_size);
    kept.retain(|candidate| advice(engine.rules(), candidate).is_none());

    let limit = if args.all { usize::MAX } else { LARGEST };
    print_section(engine, "Can be removed", &removable, limit);
    print_review(engine, &review, limit, now());
    print_advice(&advised);
    if args.show_protected {
        print_section(engine, "Kept", &kept, usize::MAX);
    } else if !kept.is_empty() {
        println!(
            "\nKept: {} ({}). Add --show-protected to see them and why.",
            items(kept.len()),
            format::bytes(format::total(&kept).on_disk)
        );
    }
    if !scan.unreadable.is_empty() {
        println!(
            "\n{} couldn't be read, so anything inside them was skipped.",
            match scan.unreadable.len() {
                1 => "1 folder".to_owned(),
                n => format!("{} folders", format::count(n as u64)),
            }
        );
    }
    if args.save.is_some() {
        println!("\nNothing was removed.");
    } else {
        println!("\nNothing was removed. Run `cleanse clean` to choose what to remove.");
    }
}

/// Prints a section's totals per rule, then its items largest first, up to `limit`.
fn print_section(engine: &Engine, title: &str, candidates: &[&Candidate], limit: usize) {
    if candidates.is_empty() {
        return;
    }
    println!(
        "\n{title}: {}, {}",
        items(candidates.len()),
        format::size(&format::total(candidates))
    );

    let rules = by_rule(engine, candidates);
    if rules.len() > 1 {
        for (name, group) in rules {
            println!(
                "  {name:<36} {:>10}  {}",
                format::bytes(format::total(&group).on_disk),
                items(group.len())
            );
        }
        println!();
    }

    let shown = candidates.len().min(limit);
    for candidate in &candidates[..shown] {
        println!(
            "  {:>10}  {:<12} {}",
            format::bytes(candidate.found.size.on_disk),
            candidate.risk().to_string(),
            candidate.found.path.display()
        );
        for concern in candidate.concerns() {
            println!("{:>27}{concern}", "! ");
        }
    }
    print_hidden("  ", candidates.len() - shown);
}

/// Prints the items that need a decision under their rules, largest first, with how long ago
/// each was modified to help decide. Each rule lists up to `limit` items, so many of one kind,
/// like installers, don't hide the others.
fn print_review(engine: &Engine, candidates: &[&Candidate], limit: usize, now: u64) {
    if candidates.is_empty() {
        return;
    }
    println!(
        "\nNeeds your review: {}, {}",
        items(candidates.len()),
        format::size(&format::total(candidates))
    );
    for (name, group) in by_rule(engine, candidates) {
        println!(
            "\n  {name}: {}, {}",
            items(group.len()),
            format::bytes(format::total(&group).on_disk)
        );
        let shown = group.len().min(limit);
        for candidate in &group[..shown] {
            let modified = candidate
                .found
                .modified
                .map(|at| format!("  {}", format::modified(at, now)))
                .unwrap_or_default();
            println!(
                "    {:>10}  {}{modified}",
                format::bytes(candidate.found.size.on_disk),
                candidate.found.path.display()
            );
            for concern in candidate.concerns() {
                println!("{:>16}{concern}", "! ");
            }
        }
        print_hidden("    ", group.len() - shown);
    }
}

/// The candidates grouped by rule, with each rule's name, the largest group first. Each group
/// keeps the candidates' order.
fn by_rule<'c>(engine: &Engine, candidates: &[&'c Candidate]) -> Vec<(String, Vec<&'c Candidate>)> {
    let mut by_rule: BTreeMap<&RuleId, Vec<&Candidate>> = BTreeMap::new();
    for candidate in candidates {
        by_rule
            .entry(&candidate.found.rule)
            .or_default()
            .push(candidate);
    }
    let mut rules: Vec<_> = by_rule
        .into_iter()
        .map(|(rule, group)| {
            let name = engine
                .rules()
                .get(rule)
                .map_or_else(|| rule.0.clone(), |rule| rule.name.clone());
            (name, group)
        })
        .collect();
    rules.sort_by_key(|(_, group)| Reverse(format::total(group).on_disk));
    rules
}

/// The command that cleans `candidate` better than removing it would, from its rule. An item a
/// safety check keeps for another reason, such as a folder excluded in the settings, has none,
/// and is listed as kept with that reason instead.
pub(crate) fn advice<'r>(rules: &'r RuleSet, candidate: &Candidate) -> Option<&'r str> {
    if candidate.concerns().iter().any(Concern::blocks) {
        return None;
    }
    rules.get(&candidate.found.rule)?.advise.as_deref()
}

/// The candidates with [`advice`], in the order given, each with its command.
pub(crate) fn advised<'c, 'r>(
    rules: &'r RuleSet,
    candidates: &[&'c Candidate],
) -> Vec<(&'c Candidate, &'r str)> {
    candidates
        .iter()
        .filter_map(|&candidate| Some((candidate, advice(rules, candidate)?)))
        .collect()
}

/// Prints what Cleanse leaves to other tools, and the command for each.
pub(crate) fn print_advice(advised: &[(&Candidate, &str)]) {
    if advised.is_empty() {
        return;
    }
    let total: u64 = advised
        .iter()
        .map(|(candidate, _)| candidate.found.size.on_disk)
        .sum();
    println!(
        "\nBest cleaned by their own tools: {}, {}",
        items(advised.len()),
        format::bytes(total)
    );
    for (candidate, command) in advised {
        println!(
            "  {:>10}  {}",
            format::bytes(candidate.found.size.on_disk),
            candidate.found.path.display()
        );
        println!("{:14}Run `{command}`", "");
    }
}

/// [`advised`] items, as `--json` gives them.
#[derive(Serialize)]
pub(crate) struct JsonAdvice<'a> {
    path: String,
    rule: &'a RuleId,
    size: Size,
    command: &'a str,
}

pub(crate) fn json_advice<'a>(advised: &[(&'a Candidate, &'a str)]) -> Vec<JsonAdvice<'a>> {
    advised
        .iter()
        .map(|&(candidate, command)| JsonAdvice {
            path: candidate.found.path.to_string_lossy().into_owned(),
            rule: &candidate.found.rule,
            size: candidate.found.size,
            command,
        })
        .collect()
}

fn print_hidden(indent: &str, hidden: usize) {
    if hidden > 0 {
        let noun = if hidden == 1 { "item" } else { "items" };
        println!(
            "{indent}...and {} smaller {noun}. Add --all to list every item.",
            format::count(hidden as u64)
        );
    }
}

pub(crate) fn items(count: usize) -> String {
    match count {
        1 => "1 item".to_owned(),
        n => format!("{} items", format::count(n as u64)),
    }
}

fn print_json(engine: &Engine, scan: &Scan) -> crate::Result {
    #[derive(Serialize)]
    struct Output<'a> {
        roots: Vec<String>,
        entries: u64,
        elapsed_seconds: f64,
        cancelled: bool,
        unreadable: Vec<String>,
        candidates: &'a [Candidate],
        /// The candidates their own tools clean better, with the command for each.
        advice: Vec<JsonAdvice<'a>>,
    }

    let lossy = |paths: &[PathBuf]| -> Vec<String> {
        paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    };
    let output = Output {
        roots: lossy(&scan.roots),
        entries: scan.entries,
        elapsed_seconds: scan.elapsed.as_secs_f64(),
        cancelled: scan.cancelled,
        unreadable: lossy(&scan.unreadable),
        candidates: &scan.candidates,
        advice: json_advice(&advised(
            engine.rules(),
            &scan.candidates.iter().collect::<Vec<_>>(),
        )),
    };
    let stdout = io::stdout().lock();
    serde_json::to_writer_pretty(stdout, &output)?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use cleanse_engine::RuleSet;
    use cleanse_model::{
        Candidate, CandidateId, Category, Concern, EntryKind, FileIdentity, Match, Risk, RuleId,
        Size,
    };

    use super::advised;

    fn candidate(rule: &str, risk: Risk, concerns: Vec<Concern>) -> Candidate {
        let found = Match {
            path: PathBuf::from(rule),
            kind: EntryKind::Dir,
            rule: RuleId(rule.into()),
            category: Category::PackageCaches,
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
    fn advises_only_what_nothing_else_keeps() {
        let rules = RuleSet::builtin().unwrap();
        let store = candidate("pnpm.store", Risk::Protected, vec![]);
        let shared = candidate(
            "go.module_cache",
            Risk::Protected,
            vec![Concern::SharedData],
        );
        let excluded = candidate(
            "go.module_cache",
            Risk::Protected,
            vec![Concern::ProtectedLocation {
                what: "a folder excluded in your settings".into(),
            }],
        );
        let removable = candidate("javascript.node_modules", Risk::Regenerable, vec![]);

        let advised = advised(&rules, &[&store, &shared, &excluded, &removable]);

        let commands: Vec<&str> = advised.iter().map(|(_, command)| *command).collect();
        assert_eq!(commands, ["pnpm store prune", "go clean -modcache"]);
    }
}
