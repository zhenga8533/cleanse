use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use cleanse_engine::{Analysis, Engine, NodeId, NodeKind, SizeTree, removable_under};
use cleanse_model::{Candidate, Category, EntryKind, Risk, Size};
use serde::Serialize;

use crate::format;
use crate::scan::{
    JsonAdvice, Reporter, advice, advised, items, json_advice, print_advice, print_scanned,
};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Folders to analyze. Defaults to your home folder.
    paths: Vec<PathBuf>,
    /// How many levels of folders to show below each folder analyzed.
    #[arg(long, default_value_t = 2)]
    depth: usize,
    /// How many of the largest entries to show in each folder.
    #[arg(long, default_value_t = 10)]
    top: usize,
    /// Print the results as JSON.
    #[arg(long)]
    json: bool,
}

pub fn run(engine: &Engine, args: &Args) -> crate::Result {
    let roots = if args.paths.is_empty() {
        engine.default_roots()
    } else {
        args.paths.clone()
    };
    let reporter = Reporter::new();
    let analysis = engine.analyze(
        &roots,
        &|progress| reporter.scanning(progress),
        &AtomicBool::new(false),
    );
    reporter.clear();
    let analysis = analysis?;
    if args.json {
        print_json(engine, &analysis, args)
    } else {
        print_human(engine, &analysis, args);
        Ok(())
    }
}

/// One line of a folder's breakdown: a folder or item in it, or its own files together.
enum Entry {
    Node(NodeId),
    Files { count: u64, on_disk: u64 },
}

/// A folder's largest entries, up to `limit`, and how many were left out and how much they take.
fn largest(tree: &SizeTree, id: NodeId, limit: usize) -> (Vec<Entry>, usize, u64) {
    let node = tree.node(id);
    let mut entries: Vec<(u64, Entry)> = tree
        .children(id)
        .iter()
        .map(|&child| (tree.node(child).usage.on_disk, Entry::Node(child)))
        .collect();
    if node.own_files > 0 {
        let on_disk = node.own_usage.on_disk;
        entries.push((
            on_disk,
            Entry::Files {
                count: node.own_files,
                on_disk,
            },
        ));
    }
    entries.sort_by_key(|(on_disk, _)| Reverse(*on_disk));
    let hidden = entries.len().saturating_sub(limit);
    let hidden_size = entries[entries.len() - hidden..]
        .iter()
        .map(|(on_disk, _)| on_disk)
        .sum();
    entries.truncate(limit);
    (
        entries.into_iter().map(|(_, entry)| entry).collect(),
        hidden,
        hidden_size,
    )
}

/// How much can be removed at or inside `path`, or directly in it for its own files.
fn removable(candidates: &[Candidate], path: &Path, only_direct_files: bool) -> u64 {
    removable_under(candidates, path)
        .filter(|candidate| {
            !only_direct_files
                || candidate.found.kind == EntryKind::File
                    && candidate.found.path.parent() == Some(path)
        })
        .map(|candidate| candidate.found.size.on_disk)
        .sum()
}

/// The candidate for an item in the tree, which the scan has by path.
fn candidate_at<'s>(analysis: &'s Analysis, path: &Path) -> Option<&'s Candidate> {
    let candidates = &analysis.scan.candidates;
    candidates
        .binary_search_by(|candidate| candidate.found.path.as_path().cmp(path))
        .ok()
        .map(|at| &candidates[at])
}

/// What can be removed in an analysis, in all and by tier and category.
#[derive(Serialize)]
struct Removable {
    items: usize,
    size: Size,
    by_risk: BTreeMap<Risk, u64>,
    by_category: BTreeMap<Category, u64>,
}

impl Removable {
    fn of(candidates: &[Candidate]) -> Self {
        let removable: Vec<&Candidate> = candidates
            .iter()
            .filter(|candidate| candidate.risk() != Risk::Protected)
            .collect();
        let mut by_risk = BTreeMap::new();
        let mut by_category = BTreeMap::new();
        for candidate in &removable {
            let on_disk = candidate.found.size.on_disk;
            *by_risk.entry(candidate.risk()).or_default() += on_disk;
            *by_category.entry(candidate.found.category).or_default() += on_disk;
        }
        Self {
            items: removable.len(),
            size: format::total(&removable),
            by_risk,
            by_category,
        }
    }
}

fn print_human(engine: &Engine, analysis: &Analysis, args: &Args) {
    let tree = &analysis.tree;
    print_scanned(&analysis.scan);

    println!("\n{:>10}  {:>10}  Folder", "Size", "Removable");
    for &root in tree.roots() {
        print_node(engine, analysis, root, 0, args);
    }
    let incomplete = tree.roots().iter().any(|&root| tree.node(root).incomplete);
    if incomplete {
        println!("\n* Part of it couldn't be read or measured, so it takes more than shown.");
    }

    let removable = Removable::of(&analysis.scan.candidates);
    if removable.items == 0 {
        println!("\nNothing here can be removed.");
    } else {
        println!(
            "\nCan be removed: {}, {}",
            items(removable.items),
            format::size(&removable.size)
        );
        for (risk, on_disk) in removable.by_risk {
            println!("  {:<24} {:>10}", risk.to_string(), format::bytes(on_disk));
        }
        println!();
        let mut by_category: Vec<_> = removable.by_category.into_iter().collect();
        by_category.sort_by_key(|(_, on_disk)| Reverse(*on_disk));
        for (category, on_disk) in by_category {
            println!(
                "  {:<24} {:>10}",
                category.to_string(),
                format::bytes(on_disk)
            );
        }
    }
    print_advice(&advised(
        engine.rules(),
        &largest_first(&analysis.scan.candidates),
    ));
    if !analysis.scan.unreadable.is_empty() {
        println!(
            "\n{} couldn't be read, so what's inside isn't counted.",
            match analysis.scan.unreadable.len() {
                1 => "1 folder".to_owned(),
                n => format!("{} folders", format::count(n as u64)),
            }
        );
    }
    println!("\nNothing was removed. Run `cleanse clean` to choose what to remove.");
}

fn print_node(engine: &Engine, analysis: &Analysis, id: NodeId, depth: usize, args: &Args) {
    let tree = &analysis.tree;
    let node = tree.node(id);
    let path = tree.path(id);
    let mut label = if depth == 0 {
        path.display().to_string()
    } else {
        node.name.to_string_lossy().into_owned()
    };
    match node.kind {
        NodeKind::Folder => {}
        NodeKind::Item => {
            if let Some(candidate) = candidate_at(analysis, &path) {
                let rule = engine
                    .rules()
                    .get(&candidate.found.rule)
                    .map_or(candidate.found.rule.0.as_str(), |rule| &rule.name);
                label = match advice(engine.rules(), candidate) {
                    Some(command) => format!("{label} ({rule}, clean with `{command}`)"),
                    None => format!("{label} ({rule}, {})", candidate.risk()),
                };
            }
        }
        NodeKind::OtherFilesystem => label = format!("{label} (another filesystem, not measured)"),
    }
    if node.incomplete {
        label.push_str(" *");
    }
    print_row(
        node.usage.on_disk,
        removable(&analysis.scan.candidates, &path, false),
        depth,
        &label,
    );

    if depth >= args.depth {
        return;
    }
    let (entries, hidden, hidden_size) = largest(tree, id, args.top);
    for entry in entries {
        match entry {
            Entry::Node(child) => print_node(engine, analysis, child, depth + 1, args),
            Entry::Files { count, on_disk } => print_row(
                on_disk,
                removable(&analysis.scan.candidates, &path, true),
                depth + 1,
                &files(count),
            ),
        }
    }
    if hidden > 0 {
        println!(
            "{:>10}  {:>10}  {:indent$}...and {} more",
            format::bytes(hidden_size),
            "",
            "",
            format::count(hidden as u64),
            indent = (depth + 1) * 2
        );
    }
}

fn largest_first(candidates: &[Candidate]) -> Vec<&Candidate> {
    let mut sorted: Vec<&Candidate> = candidates.iter().collect();
    sorted.sort_by_key(|candidate| Reverse(candidate.found.size.on_disk));
    sorted
}

fn print_row(on_disk: u64, removable: u64, depth: usize, label: &str) {
    let removable = if removable == 0 {
        "-".to_owned()
    } else {
        format::bytes(removable)
    };
    println!(
        "{:>10}  {removable:>10}  {:indent$}{label}",
        format::bytes(on_disk),
        "",
        indent = depth * 2
    );
}

fn files(count: u64) -> String {
    match count {
        1 => "(1 file)".to_owned(),
        n => format!("({} files)", format::count(n)),
    }
}

#[derive(Serialize)]
struct JsonFolder {
    path: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule: Option<String>,
    size: JsonUsage,
    files: u64,
    removable: u64,
    incomplete: bool,
    /// The files directly in it.
    own: JsonUsage,
    own_files: u64,
    /// Its largest folders, up to `--top`, and down to `--depth`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<JsonFolder>,
    /// How many folders `children` leaves out, and how much they take.
    #[serde(skip_serializing_if = "Option::is_none")]
    more: Option<JsonMore>,
}

#[derive(Serialize)]
struct JsonUsage {
    logical: u64,
    on_disk: u64,
}

#[derive(Serialize)]
struct JsonMore {
    folders: usize,
    on_disk: u64,
}

fn json_folder(analysis: &Analysis, id: NodeId, depth: usize, args: &Args) -> JsonFolder {
    let tree = &analysis.tree;
    let node = tree.node(id);
    let path = tree.path(id);
    let kind = match node.kind {
        NodeKind::Folder => "folder",
        NodeKind::Item => "item",
        NodeKind::OtherFilesystem => "other_filesystem",
    };
    let rule = (node.kind == NodeKind::Item)
        .then(|| candidate_at(analysis, &path))
        .flatten()
        .map(|candidate| candidate.found.rule.0.clone());
    let mut children = tree.children(id).to_vec();
    let mut more = None;
    if depth >= args.depth {
        children.clear();
    } else if children.len() > args.top {
        let hidden = children.split_off(args.top);
        more = Some(JsonMore {
            folders: hidden.len(),
            on_disk: hidden.iter().map(|&id| tree.node(id).usage.on_disk).sum(),
        });
    }
    JsonFolder {
        path: path.to_string_lossy().into_owned(),
        kind,
        rule,
        size: JsonUsage {
            logical: node.usage.logical,
            on_disk: node.usage.on_disk,
        },
        files: node.files,
        removable: removable(&analysis.scan.candidates, &path, false),
        incomplete: node.incomplete,
        own: JsonUsage {
            logical: node.own_usage.logical,
            on_disk: node.own_usage.on_disk,
        },
        own_files: node.own_files,
        children: children
            .into_iter()
            .map(|child| json_folder(analysis, child, depth + 1, args))
            .collect(),
        more,
    }
}

fn print_json(engine: &Engine, analysis: &Analysis, args: &Args) -> crate::Result {
    #[derive(Serialize)]
    struct Output<'a> {
        roots: Vec<String>,
        entries: u64,
        elapsed_seconds: f64,
        cancelled: bool,
        unreadable: Vec<String>,
        folders: Vec<JsonFolder>,
        removable: Removable,
        /// What's best cleaned by its own tool, largest first, with the command for each.
        advice: Vec<JsonAdvice<'a>>,
    }

    let scan = &analysis.scan;
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
        folders: analysis
            .tree
            .roots()
            .iter()
            .map(|&root| json_folder(analysis, root, 0, args))
            .collect(),
        removable: Removable::of(&scan.candidates),
        advice: json_advice(&advised(engine.rules(), &largest_first(&scan.candidates))),
    };
    serde_json::to_writer_pretty(io::stdout().lock(), &output)?;
    println!();
    Ok(())
}
