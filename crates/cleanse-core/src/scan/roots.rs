use std::io;
use std::path::{self, PathBuf};

use crate::safety::key;

/// The folders to walk for `roots`: absolute, without a trailing separator, and without any
/// that's inside another or the same folder spelled differently, which would be walked twice.
/// The first spelling given is kept.
pub fn distinct_roots(roots: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut kept: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for root in roots {
        // Rebuilding from components drops a trailing separator, so the walk's paths below it
        // have it as their parent exactly.
        let root: PathBuf = path::absolute(root)?.components().collect();
        let root_key = key(&root);
        if kept.iter().any(|(_, other)| root_key.starts_with(other)) {
            continue;
        }
        kept.retain(|(_, other)| !other.starts_with(&root_key));
        kept.push((root, root_key));
    }
    Ok(kept.into_iter().map(|(root, _)| root).collect())
}
