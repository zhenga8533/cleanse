//! What system cleanup checks before putting something back or finishing a removal (ADR 0010).
//! Its journals and quarantine records are in a folder only Administrators and SYSTEM, or root,
//! can change, but where they point is checked anyway.

#[cfg(test)]
mod tests;

use std::path::Path;

use cleanse_core::rules::{Listed, Matcher, Os, RuleSet};
use cleanse_core::safety::{ProtectedPolicy, SystemRoots};
use cleanse_model::EntryKind;
use cleanse_platform::{Probe, SystemFolders};

/// The places system cleanup may restore an item to, or finish removing one from: where a system
/// rule names an item, directly inside a system folder that checked out.
pub(crate) struct Targets<'a> {
    matcher: Matcher<'a>,
    roots: SystemRoots,
    policy: ProtectedPolicy,
    probe: &'a dyn Probe,
}

impl<'a> Targets<'a> {
    pub(crate) fn new(rules: &'a RuleSet, folders: &SystemFolders, probe: &'a dyn Probe) -> Self {
        Self {
            matcher: Matcher::system(rules.rules(), folders, Os::current()),
            roots: SystemRoots::verify(rules, folders, probe),
            policy: ProtectedPolicy::for_system_cleanup(folders, probe),
            probe,
        }
    }

    /// Whether the `kind` of item at `path` is one.
    pub(crate) fn allow(&self, path: &Path, kind: EntryKind) -> bool {
        self.matcher
            .match_entry(path, Listed { kind, on_disk: 0 })
            .is_some()
            && self.roots.holds(path, self.probe)
            && self.policy.check(path, None, &[]).is_none()
    }
}
