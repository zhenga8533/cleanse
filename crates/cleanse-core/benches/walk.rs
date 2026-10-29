//! Times the directory walker on a real tree. Read-only.
//!
//! ```text
//! cargo xtask gen-tree <dir>                                   # optional synthetic tree
//! cargo bench -p cleanse-core --bench walk -- <dir> [--runs <n>]
//! ```
//!
//! One untimed warm-up run fills the OS cache first, so the timed runs measure warm-cache walks.

#![allow(
    clippy::cast_precision_loss,
    reason = "throughput figures are approximate"
)]

use std::env;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use cleanse_core::scan::{Descend, Entry, walk};
use cleanse_model::EntryKind;

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

fn run_once(root: &Path) -> io::Result<(Duration, Counts)> {
    let totals = Totals::default();
    let start = Instant::now();
    let report = walk(root, |entry| {
        totals.add(entry);
        Descend::Yes
    })?;
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
        Ok(()) => ExitCode::SUCCESS,
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

fn benchmark(root: &Path, runs: usize) -> io::Result<()> {
    let threads = thread::available_parallelism().map_or(1, NonZeroUsize::get);
    println!(
        "root: {}\nthreads: {threads}, timed runs: {runs}\n",
        root.display()
    );

    let (_, counts) = run_once(root)?;
    let mut times = Vec::with_capacity(runs);
    for run in 0..runs {
        let (elapsed, found) = run_once(root)?;
        if found != counts {
            println!(
                "note: run {run} saw a different tree (did something write to it?): {found:?}"
            );
        }
        times.push(elapsed);
    }
    times.sort();
    let median = times[times.len() / 2];

    println!(
        "{:>9} {:>9} {:>9} {:>14}",
        "min", "median", "max", "entries/s"
    );
    println!(
        "{:>9} {:>9} {:>9} {:>14}",
        seconds(times[0]),
        seconds(median),
        seconds(times[times.len() - 1]),
        thousands(counts.entries() as f64 / median.as_secs_f64()),
    );
    println!(
        "\n{} entries ({} files, {} dirs, {} links, {} other), {} MiB in files, \
         {} unreadable, {} on other filesystems",
        counts.entries(),
        counts.files,
        counts.dirs,
        counts.links,
        counts.other,
        counts.bytes / (1024 * 1024),
        counts.errors,
        counts.other_filesystems,
    );
    Ok(())
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
