use std::path::{Path, PathBuf};

use cleanse_model::{
    Candidate, CandidateId, Category, Concern, EntryKind, FileIdentity, Match, Risk, RuleId, Size,
};

use super::{Leaf, NodeId, NodeKind, SizeTree, Usage, removable_under};
use crate::scan::DirFiles;

fn path(parts: &[&str]) -> PathBuf {
    parts.iter().collect()
}

/// A folder the walk read, with `files` files of `bytes` bytes each directly in it.
fn folder(parts: &[&str], files: u64, bytes: u64) -> (PathBuf, DirFiles) {
    (
        path(parts),
        DirFiles {
            files,
            logical: files * bytes,
            on_disk: files * bytes,
            incomplete: false,
        },
    )
}

fn item(parts: &[&str], bytes: u64) -> Leaf {
    Leaf {
        path: path(parts),
        kind: NodeKind::Item,
        usage: Usage {
            logical: bytes,
            on_disk: bytes,
        },
        files: 1,
        incomplete: false,
    }
}

fn named(tree: &SizeTree, ids: &[NodeId]) -> Vec<(String, u64)> {
    ids.iter()
        .map(|&id| {
            let node = tree.node(id);
            (node.name.to_string_lossy().into_owned(), node.usage.on_disk)
        })
        .collect()
}

fn child(tree: &SizeTree, parent: NodeId, name: &str) -> NodeId {
    *tree
        .children(parent)
        .iter()
        .find(|&&id| tree.node(id).name.as_ref() == Path::new(name).as_os_str())
        .unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn totals_add_up_through_the_tree_largest_first() {
    let tree = SizeTree::build(
        &[path(&["root"])],
        vec![
            folder(&["root", "c"], 1, 5),
            folder(&["root", "a", "b"], 2, 500),
            folder(&["root"], 1, 10),
            folder(&["root", "a"], 1, 100),
        ],
        Vec::new(),
        &[],
        false,
    );

    let [root] = tree.roots() else {
        panic!("{:?}", tree.roots())
    };
    let node = tree.node(*root);
    assert_eq!(node.name.as_ref(), Path::new("root").as_os_str());
    assert_eq!((node.usage.on_disk, node.files), (1115, 5));
    assert_eq!((node.own_usage.on_disk, node.own_files), (10, 1));
    assert_eq!(
        named(&tree, tree.children(*root)),
        [("a".to_owned(), 1100), ("c".to_owned(), 5)]
    );
    let a = child(&tree, *root, "a");
    assert_eq!(tree.path(child(&tree, a, "b")), path(&["root", "a", "b"]));
    assert!(!node.incomplete && !tree.partial);
}

#[test]
fn measured_items_and_other_filesystems_are_leaves() {
    let mut mount = Leaf::other_filesystem(path(&["root", "mnt"]));
    mount.usage = Usage::default();
    let tree = SizeTree::build(
        &[path(&["root"])],
        vec![folder(&["root"], 0, 0), folder(&["root", "web"], 1, 10)],
        vec![item(&["root", "web", "node_modules"], 4000), mount],
        &[],
        false,
    );

    let root = tree.roots()[0];
    let web = child(&tree, root, "web");
    assert_eq!(tree.node(web).usage.on_disk, 4010);
    assert_eq!(tree.node(web).own_usage.on_disk, 10);
    let node_modules = tree.node(child(&tree, web, "node_modules"));
    assert_eq!(node_modules.kind, NodeKind::Item);
    assert_eq!(node_modules.own_files, 0);
    let mnt = tree.node(child(&tree, root, "mnt"));
    assert_eq!(mnt.kind, NodeKind::OtherFilesystem);
    // Its size isn't known, so neither is the root's.
    assert!(mnt.incomplete && tree.node(root).incomplete);
    assert!(!tree.node(web).incomplete);
}

#[test]
fn what_couldnt_be_read_makes_the_folders_around_it_incomplete() {
    let tree = SizeTree::build(
        &[path(&["root"])],
        vec![
            folder(&["root"], 0, 0),
            folder(&["root", "a"], 0, 0),
            folder(&["root", "b"], 0, 0),
        ],
        Vec::new(),
        &[path(&["root", "a", "locked"])],
        true,
    );

    let root = tree.roots()[0];
    assert!(tree.node(child(&tree, root, "a")).incomplete);
    assert!(!tree.node(child(&tree, root, "b")).incomplete);
    assert!(tree.node(root).incomplete);
    assert!(tree.partial);
}

#[test]
fn several_roots_are_separate_trees_and_unread_ones_still_show() {
    let tree = SizeTree::build(
        &[path(&["small"]), path(&["big"]), path(&["missing"])],
        vec![folder(&["small"], 1, 5), folder(&["big"], 1, 50)],
        Vec::new(),
        &[],
        false,
    );

    assert_eq!(
        named(&tree, tree.roots()),
        [
            ("big".to_owned(), 50),
            ("small".to_owned(), 5),
            ("missing".to_owned(), 0)
        ]
    );
    let missing = tree.node(tree.roots()[2]);
    assert!(missing.incomplete);
}

fn candidate(parts: &[&str], risk: Risk, concerns: Vec<Concern>) -> Candidate {
    Candidate::assess(
        CandidateId(0),
        Match {
            path: path(parts),
            kind: EntryKind::Dir,
            rule: RuleId("test.rule".into()),
            category: Category::BuildOutput,
            declared_risk: risk,
            size: Size::default(),
            files: 0,
            evidence: Vec::new(),
        },
        Some(FileIdentity { volume: 1, file: 1 }),
        concerns,
    )
}

#[test]
fn finds_what_can_be_removed_under_a_folder() {
    // Sorted by path, as a scan's are.
    let candidates = [
        candidate(&["root", "a", "node_modules"], Risk::Regenerable, vec![]),
        candidate(&["root", "a", "target"], Risk::Safe, vec![]),
        candidate(&["root", "a", "x"], Risk::Safe, vec![Concern::TrackedByGit]),
        candidate(&["root", "ab", "target"], Risk::Safe, vec![]),
        candidate(&["root", "b", "target"], Risk::Safe, vec![]),
    ];

    let under = |parts: &[&str]| -> Vec<PathBuf> {
        removable_under(&candidates, &path(parts))
            .map(|candidate| candidate.found.path.clone())
            .collect()
    };

    assert_eq!(
        under(&["root", "a"]),
        [
            path(&["root", "a", "node_modules"]),
            path(&["root", "a", "target"])
        ]
    );
    assert_eq!(under(&["root"]).len(), 4);
    assert_eq!(
        under(&["root", "a", "target"]),
        [path(&["root", "a", "target"])]
    );
    assert_eq!(under(&["root", "c"]), Vec::<PathBuf>::new());
}
