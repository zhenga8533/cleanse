//! Where space goes: a tree of folder sizes, built from the folders a scan read and the items
//! it measured.

#[cfg(test)]
mod tests;

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use cleanse_model::{Candidate, EntryKind, Risk};

use crate::scan::DirFiles;

/// How much space something takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub logical: u64,
    pub on_disk: u64,
}

impl Usage {
    fn add(&mut self, other: Self) {
        self.logical += other.logical;
        self.on_disk += other.on_disk;
    }
}

/// Folder sizes under a scan's roots, each folder's children largest first.
#[derive(Debug, Default)]
pub struct SizeTree {
    nodes: Vec<Node>,
    roots: Vec<NodeId>,
    /// The scan stopped early, so some folders weren't read or measured.
    pub partial: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(u32);

#[derive(Debug)]
pub struct Node {
    /// A root's full path, or the name inside its parent.
    pub name: Box<OsStr>,
    pub parent: Option<NodeId>,
    pub kind: NodeKind,
    /// Everything in it, including what's in its subfolders.
    pub usage: Usage,
    pub files: u64,
    /// The files directly in it, which have no nodes of their own.
    pub own_usage: Usage,
    pub own_files: u64,
    /// Part of it couldn't be read, so it's bigger than it shows.
    pub incomplete: bool,
    children: Vec<NodeId>,
}

impl Node {
    fn new(kind: NodeKind, usage: Usage, files: u64, incomplete: bool) -> Self {
        let folder = kind == NodeKind::Folder;
        Self {
            name: Box::default(),
            parent: None,
            kind,
            usage,
            files,
            own_usage: if folder { usage } else { Usage::default() },
            own_files: if folder { files } else { 0 },
            incomplete,
            children: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// A folder the scan read.
    Folder,
    /// A folder a rule recognized, measured as a whole rather than read folder by folder.
    Item,
    /// A folder on another filesystem, which scans don't enter, so its size isn't known.
    OtherFilesystem,
}

/// A folder that has a size without the scan having read it.
#[derive(Clone, Debug)]
pub struct Leaf {
    pub path: PathBuf,
    pub kind: NodeKind,
    pub usage: Usage,
    pub files: u64,
    pub incomplete: bool,
}

impl Leaf {
    /// A folder candidate, measured as a whole. File candidates are already in their folder's
    /// files, so they have no leaf.
    pub fn of(candidate: &Candidate) -> Option<Self> {
        (candidate.found.kind == EntryKind::Dir).then(|| Self {
            path: candidate.found.path.clone(),
            kind: NodeKind::Item,
            usage: Usage {
                logical: candidate.found.size.logical,
                on_disk: candidate.found.size.on_disk,
            },
            files: candidate.found.files,
            incomplete: candidate
                .concerns()
                .contains(&cleanse_model::Concern::Incomplete),
        })
    }

    pub fn other_filesystem(path: PathBuf) -> Self {
        Self {
            path,
            kind: NodeKind::OtherFilesystem,
            usage: Usage::default(),
            files: 0,
            incomplete: true,
        }
    }
}

impl SizeTree {
    /// Builds the tree for `roots` from the folders a walk read, with the files directly in
    /// each, and folders sized some other way. `unreadable` are what couldn't be read, which
    /// makes the folders around them incomplete.
    pub fn build(
        roots: &[PathBuf],
        folders: Vec<(PathBuf, DirFiles)>,
        leaves: Vec<Leaf>,
        unreadable: &[PathBuf],
        partial: bool,
    ) -> Self {
        let mut paths: Vec<PathBuf> = Vec::with_capacity(folders.len() + leaves.len());
        let mut nodes: Vec<Node> = Vec::with_capacity(paths.capacity());
        for (path, files) in folders {
            let usage = Usage {
                logical: files.logical,
                on_disk: files.on_disk,
            };
            paths.push(path);
            nodes.push(Node::new(
                NodeKind::Folder,
                usage,
                files.files,
                files.incomplete,
            ));
        }
        for leaf in leaves {
            paths.push(leaf.path);
            nodes.push(Node::new(
                leaf.kind,
                leaf.usage,
                leaf.files,
                leaf.incomplete,
            ));
        }
        // A root that couldn't be read still shows, empty and incomplete.
        let missing: Vec<PathBuf> = {
            let read: HashSet<&[u8]> = paths.iter().map(|path| key(path)).collect();
            roots
                .iter()
                .filter(|root| !read.contains(key(root)))
                .cloned()
                .collect()
        };
        for root in missing {
            paths.push(root);
            nodes.push(Node::new(NodeKind::Folder, Usage::default(), 0, true));
        }

        let index: HashMap<&[u8], u32> = paths
            .iter()
            .enumerate()
            .map(|(at, path)| {
                (
                    key(path),
                    u32::try_from(at).expect("fewer than 2^32 folders"),
                )
            })
            .collect();
        let is_root = |path: &Path| roots.iter().any(|root| root == path);
        let mut tree_roots = Vec::new();
        for (at, path) in paths.iter().enumerate() {
            let id = NodeId(u32::try_from(at).expect("fewer than 2^32 folders"));
            let parent = (!is_root(path))
                .then(|| path.parent().and_then(|parent| index.get(key(parent))))
                .flatten();
            let node = &mut nodes[at];
            if let Some(&parent) = parent {
                node.parent = Some(NodeId(parent));
                node.name = path.file_name().unwrap_or(path.as_os_str()).into();
            } else {
                // Every folder a walk reads is inside its root, so only roots have no parent.
                node.name = path.as_os_str().into();
                tree_roots.push(id);
            }
        }
        for path in unreadable {
            let around = path
                .ancestors()
                .find_map(|ancestor| index.get(key(ancestor)));
            if let Some(&at) = around {
                nodes[at as usize].incomplete = true;
            }
        }

        // Deepest first, so each node's total is complete before it's added to its parent's.
        let mut order: Vec<usize> = (0..nodes.len()).collect();
        order.sort_by_cached_key(|&at| Reverse(paths[at].components().count()));
        for at in order {
            let Some(NodeId(parent)) = nodes[at].parent else {
                continue;
            };
            let (usage, files, incomplete) =
                (nodes[at].usage, nodes[at].files, nodes[at].incomplete);
            let parent = &mut nodes[parent as usize];
            parent.usage.add(usage);
            parent.files += files;
            parent.incomplete |= incomplete;
            parent
                .children
                .push(NodeId(u32::try_from(at).expect("fewer than 2^32 folders")));
        }
        sort_largest_first(&mut nodes, &mut tree_roots);

        Self {
            nodes,
            roots: tree_roots,
            partial,
        }
    }

    /// The roots, largest first.
    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }

    /// The folders directly in `id`, largest first.
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.node(id).children
    }

    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut names = Vec::new();
        let mut at = Some(id);
        while let Some(id) = at {
            let node = self.node(id);
            names.push(&*node.name);
            at = node.parent;
        }
        names.iter().rev().collect()
    }
}

/// Puts each node's children, and the roots, largest first, and by name among equals, which
/// on Unix are common: small files each take a whole block.
fn sort_largest_first(nodes: &mut [Node], roots: &mut [NodeId]) {
    let mut children: Vec<Vec<NodeId>> = nodes
        .iter_mut()
        .map(|node| std::mem::take(&mut node.children))
        .collect();
    let order = |a: &NodeId, b: &NodeId| {
        let (a, b) = (&nodes[a.0 as usize], &nodes[b.0 as usize]);
        b.usage
            .on_disk
            .cmp(&a.usage.on_disk)
            .then_with(|| a.name.cmp(&b.name))
    };
    for list in &mut children {
        list.sort_by(order);
    }
    roots.sort_by(order);
    for (node, list) in nodes.iter_mut().zip(children) {
        node.children = list;
    }
}

/// The candidates at or inside `path` that can be removed. `candidates` must be sorted by path,
/// as a scan's are, which puts everything inside a folder right after it.
pub fn removable_under<'c, 'p>(
    candidates: &'c [Candidate],
    path: &'p Path,
) -> impl Iterator<Item = &'c Candidate> + use<'c, 'p> {
    let start = candidates.partition_point(|candidate| candidate.found.path.as_path() < path);
    candidates[start..]
        .iter()
        .take_while(move |candidate| candidate.found.path.starts_with(path))
        .filter(|candidate| candidate.risk() != Risk::Protected)
}

/// Paths as stored, compared exactly. The walk joins each name onto its folder's path, so a
/// parent's path is always the same bytes as its folder's.
fn key(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}
