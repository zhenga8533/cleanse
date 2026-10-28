//! Times the directory walkers on a real tree. Read-only.
//!
//! ```text
//! cargo xtask gen-tree <dir>                                   # optional synthetic tree
//! cargo bench -p cleanse-core --bench walk -- <dir> [--runs <n>]
//! ```
//!
//! Each walker gets one untimed warm-up run so both start from the same OS cache state, then the
//! timed runs alternate between walkers so drift (background I/O, thermal throttling) affects
//! them equally.

#![allow(
    clippy::cast_precision_loss,
    reason = "throughput figures are approximate"
)]

use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use cleanse_core::scan::{Entry, EntryKind, WalkReport, walk_dua, walk_parallel};

type Walk = fn(&Path, &Totals) -> io::Result<WalkReport>;

const WALKERS: [(&str, Walk); 2] = [
    ("std + rayon", |root, totals| {
        walk_parallel(root, |entry| totals.add(entry))
    }),
    ("dua-core", |root, totals| {
        walk_dua(root, |entry| totals.add(entry))
    }),
];

#[derive(Default)]
struct Totals {
    files: AtomicU64,
    dirs: AtomicU64,
    links: AtomicU64,
    other: AtomicU64,
    bytes: AtomicU64,
}

impl Totals {
    fn add(&self, entry: &Entry<'_>) {
        let counter = match entry.kind {
            EntryKind::File => {
                self.bytes.fetch_add(entry.len, Ordering::Relaxed);
                &self.files
            }
            EntryKind::Dir => &self.dirs,
            EntryKind::Link => &self.links,
            EntryKind::Other => &self.other,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Counts {
    files: u64,
    dirs: u64,
    links: u64,
    other: u64,
    bytes: u64,
    errors: usize,
    other_filesystems: usize,
}

impl Counts {
    fn entries(&self) -> u64 {
        self.files + self.dirs + self.links + self.other
    }
}

fn run_once(walk: Walk, root: &Path) -> io::Result<(Duration, Counts)> {
    let totals = Totals::default();
    let start = Instant::now();
    let report = walk(root, &totals)?;
    let elapsed = start.elapsed();
    let counts = Counts {
        files: totals.files.into_inner(),
        dirs: totals.dirs.into_inner(),
        links: totals.links.into_inner(),
        other: totals.other.into_inner(),
        bytes: totals.bytes.into_inner(),
        errors: report.errors.len(),
        other_filesystems: report.other_filesystems.len(),
    };
    Ok((elapsed, counts))
}

fn main() -> ExitCode {
    let (root, runs) = match parse_args() {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!(
                "{message}\n\nusage: cargo bench -p cleanse-core --bench walk -- <dir> [--runs <n>]"
            );
            return ExitCode::from(2);
        }
    };
    match benchmark(&root, runs) {
        Ok(consistent) if consistent => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("walk failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args() -> Result<(PathBuf, usize), String> {
    let mut root = None;
    let mut runs = 5;
    // `cargo bench` appends `--bench`; it means nothing to a custom harness.
    let mut args = env::args().skip(1).filter(|arg| arg != "--bench");
    while let Some(arg) = args.next() {
        if arg == "--runs" {
            let value = args.next().ok_or("--runs needs a value")?;
            runs = value
                .parse()
                .ok()
                .filter(|runs| *runs > 0)
                .ok_or(format!("invalid run count `{value}`"))?;
        } else if root.is_none() {
            root = Some(PathBuf::from(arg));
        } else {
            return Err(format!("unexpected argument `{arg}`"));
        }
    }
    Ok((root.ok_or("missing <dir>")?, runs))
}

/// Returns whether both walkers found the same entries in their back-to-back warm-up runs.
/// Later runs may legitimately differ if something writes to the tree during the benchmark.
fn benchmark(root: &Path, runs: usize) -> io::Result<bool> {
    let threads = thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    println!(
        "root: {}\nthreads: {threads}, timed runs per walker: {runs}\n",
        root.display()
    );

    let mut times: [Vec<Duration>; 2] = Default::default();
    let mut counts: [Option<Counts>; 2] = [None, None];
    for (index, (_, walk)) in WALKERS.iter().enumerate() {
        counts[index] = Some(run_once(*walk, root)?.1);
    }
    for run in 0..runs {
        let order = if run % 2 == 0 { [0, 1] } else { [1, 0] };
        for index in order {
            let (name, walk) = WALKERS[index];
            let (elapsed, found) = run_once(walk, root)?;
            if counts[index] != Some(found) {
                println!(
                    "note: {name} saw a different tree on run {run} (did something write to it?): \
                     {found:?}"
                );
            }
            times[index].push(elapsed);
        }
    }

    println!(
        "{:<12} {:>9} {:>9} {:>9} {:>14}",
        "walker", "min", "median", "max", "entries/s"
    );
    for (index, (name, _)) in WALKERS.iter().enumerate() {
        let times = &mut times[index];
        times.sort();
        let median = times[times.len() / 2];
        let entries = counts[index].map_or(0, |counts| counts.entries());
        println!(
            "{name:<12} {:>9} {:>9} {:>9} {:>14}",
            seconds(times[0]),
            seconds(median),
            seconds(times[times.len() - 1]),
            thousands(entries as f64 / median.as_secs_f64()),
        );
    }

    println!();
    for (index, (name, _)) in WALKERS.iter().enumerate() {
        if let Some(c) = counts[index] {
            println!(
                "{name}: {} entries ({} files, {} dirs, {} links, {} other), {} MiB in files, \
                 {} unreadable, {} on other filesystems",
                c.entries(),
                c.files,
                c.dirs,
                c.links,
                c.other,
                c.bytes / (1024 * 1024),
                c.errors,
                c.other_filesystems,
            );
        }
    }
    let consistent = counts[0] == counts[1];
    if !consistent {
        println!("WARNING: the walkers disagree about what's in the tree");
    }
    Ok(consistent)
}

fn seconds(duration: Duration) -> String {
    format!("{:.3}s", duration.as_secs_f64())
}

fn thousands(value: f64) -> String {
    let digits = format!("{value:.0}");
    let mut grouped = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}
