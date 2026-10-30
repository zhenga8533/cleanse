use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use cleanse_engine::{Engine, Progress, Scan};
use cleanse_model::{Candidate, Risk, RuleId};
use serde::Serialize;

use crate::format;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Folders to scan. Defaults to your home folder.
    paths: Vec<PathBuf>,
    /// Print the results as JSON.
    #[arg(long)]
    json: bool,
    /// List every item that can be removed, not just the largest.
    #[arg(long)]
    all: bool,
    /// Also list everything that's kept, and why.
    #[arg(long)]
    show_protected: bool,
}

/// How many of the largest removable items the summary lists.
const LARGEST: usize = 10;

pub fn run(engine: &Engine, args: &Args) -> crate::Result {
    let scan = scan(engine, &args.paths)?;
    if args.json {
        print_json(&scan)
    } else {
        print_human(engine, &scan, args);
        Ok(())
    }
}

/// Scans `paths`, or the home folder if there are none, showing progress.
pub(crate) fn scan(engine: &Engine, paths: &[PathBuf]) -> io::Result<Scan> {
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
    scan
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

    fn scanning(&self, progress: Progress) {
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
    let kept = tier(&[Risk::Protected]);

    let limit = if args.all { usize::MAX } else { LARGEST };
    print_section(engine, "Can be removed", &removable, limit);
    // Every review item needs a decision, so they're always listed.
    print_section(engine, "Needs your review", &review, usize::MAX);
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
    println!("\nNothing was removed. Run `cleanse clean` to choose what to remove.");
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

    let mut by_rule: BTreeMap<&RuleId, Vec<&Candidate>> = BTreeMap::new();
    for candidate in candidates {
        by_rule
            .entry(&candidate.found.rule)
            .or_default()
            .push(candidate);
    }
    if by_rule.len() > 1 {
        let mut rules: Vec<_> = by_rule.into_iter().collect();
        rules.sort_by_key(|(_, group)| Reverse(format::total(group).on_disk));
        for (rule, group) in rules {
            let name = engine
                .rules()
                .get(rule)
                .map_or(rule.0.as_str(), |rule| &rule.name);
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
    let hidden = candidates.len() - shown;
    if hidden > 0 {
        let noun = if hidden == 1 { "item" } else { "items" };
        println!(
            "  ...and {} smaller {noun}. Add --all to list every item.",
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

fn print_json(scan: &Scan) -> crate::Result {
    #[derive(Serialize)]
    struct Output<'a> {
        roots: Vec<String>,
        entries: u64,
        elapsed_seconds: f64,
        cancelled: bool,
        unreadable: Vec<String>,
        candidates: &'a [Candidate],
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
    };
    let stdout = io::stdout().lock();
    serde_json::to_writer_pretty(stdout, &output)?;
    println!();
    Ok(())
}
