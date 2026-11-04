//! Declarative cleanup rules: loading, validation, and matching.
//!
//! Rules are data. They recognize paths and describe what they found, and nothing in a rule can
//! run a command (ADR 0003).

mod matcher;
mod schema;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use cleanse_model::RuleId;
use serde::Deserialize;

pub use matcher::{Detection, Listed, Matcher};
pub(crate) use matcher::{base_folder, wildcard_match};
pub use schema::{Base, Entry, Location, Os, Rule, RuleTest, Scope, Target};

/// The rule files that ship with Cleanse, embedded at build time. A test checks that this list
/// matches the files in `rules/`.
const BUILTIN: &[(&str, &str)] = &[
    ("ai-agents.toml", include_str!("../rules/ai-agents.toml")),
    ("browsers.toml", include_str!("../rules/browsers.toml")),
    (
        "crash-dumps.toml",
        include_str!("../rules/crash-dumps.toml"),
    ),
    ("dotnet.toml", include_str!("../rules/dotnet.toml")),
    ("downloads.toml", include_str!("../rules/downloads.toml")),
    (
        "game-engines.toml",
        include_str!("../rules/game-engines.toml"),
    ),
    ("java.toml", include_str!("../rules/java.toml")),
    ("javascript.toml", include_str!("../rules/javascript.toml")),
    (
        "large-files.toml",
        include_str!("../rules/large-files.toml"),
    ),
    ("logs.toml", include_str!("../rules/logs.toml")),
    ("native.toml", include_str!("../rules/native.toml")),
    (
        "other-languages.toml",
        include_str!("../rules/other-languages.toml"),
    ),
    (
        "package-caches.toml",
        include_str!("../rules/package-caches.toml"),
    ),
    ("python.toml", include_str!("../rules/python.toml")),
    ("rust.toml", include_str!("../rules/rust.toml")),
    ("temp.toml", include_str!("../rules/temp.toml")),
    (
        "tool-caches.toml",
        include_str!("../rules/tool-caches.toml"),
    ),
];

/// The system rules that ship with Cleanse, which only system cleanup loads. A test checks that
/// this list matches the files in `rules/system/`.
const BUILTIN_SYSTEM: &[(&str, &str)] = &[
    (
        "system/crash-reports.toml",
        include_str!("../rules/system/crash-reports.toml"),
    ),
    (
        "system/logs.toml",
        include_str!("../rules/system/logs.toml"),
    ),
    (
        "system/temp.toml",
        include_str!("../rules/system/temp.toml"),
    ),
    (
        "system/updates.toml",
        include_str!("../rules/system/updates.toml"),
    ),
];

/// A validated collection of rules with unique ids, all for one [`Scope`].
#[derive(Clone, Debug)]
pub struct RuleSet {
    rules: Vec<Rule>,
    scope: Scope,
}

impl RuleSet {
    /// The rules that ship with Cleanse.
    pub fn builtin() -> Result<Self, RuleError> {
        Self::from_sources(BUILTIN.iter().copied())
    }

    /// The system rules that ship with Cleanse.
    pub fn builtin_system() -> Result<Self, RuleError> {
        Self::system_from_sources(BUILTIN_SYSTEM.iter().copied())
    }

    /// Parses and validates rule files given as `(file name, contents)` pairs. A system rule in
    /// them is an error.
    pub fn from_sources<'a>(
        sources: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, RuleError> {
        Self::load(Scope::User, sources)
    }

    /// Like [`RuleSet::from_sources`], for system rules. Any other rule is an error.
    pub fn system_from_sources<'a>(
        sources: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, RuleError> {
        Self::load(Scope::System, sources)
    }

    fn load<'a>(
        scope: Scope,
        sources: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, RuleError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RuleFile {
            rule: Vec<Rule>,
        }

        let mut rules: Vec<Rule> = Vec::new();
        let mut ids = HashSet::new();
        let mut locations = HashSet::new();
        for (file, contents) in sources {
            let parsed: RuleFile = toml::from_str(contents).map_err(|error| RuleError {
                file: file.to_owned(),
                message: error.to_string(),
            })?;
            for rule in parsed.rule {
                let fail = |message: String| RuleError {
                    file: file.to_owned(),
                    message: format!("rule `{}`: {message}", rule.id),
                };
                match (scope, rule.scope) {
                    (Scope::User, Scope::System) => {
                        return Err(fail(
                            "system rules are loaded only for system cleanup".into(),
                        ));
                    }
                    (Scope::System, Scope::User) => {
                        return Err(fail(
                            "only system rules, with `scope = \"system\"`, are loaded for system \
                             cleanup"
                                .into(),
                        ));
                    }
                    (Scope::User, Scope::User) | (Scope::System, Scope::System) => {}
                }
                rule.validate().map_err(fail)?;
                if !ids.insert(rule.id.clone()) {
                    return Err(fail("another rule already uses this id".into()));
                }
                // Every large file would match both, so which one it's under would be arbitrary.
                if matches!(rule.target, Target::LargeFile {})
                    && rules
                        .iter()
                        .any(|other| matches!(other.target, Target::LargeFile {}))
                {
                    return Err(fail("another rule is already a `large_file` rule".into()));
                }
                if let Target::KnownLocation {
                    locations: specs, ..
                } = &rule.target
                {
                    for spec in specs {
                        // Lowercased because Windows and macOS don't distinguish case.
                        let key = (spec.os, spec.base, spec.path.to_lowercase());
                        if !locations.insert(key) {
                            return Err(fail(format!(
                                "another rule already claims `{}` on {:?}",
                                spec.path, spec.os
                            )));
                        }
                    }
                }
                rules.push(rule);
            }
        }
        Ok(Self { rules, scope })
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn get(&self, id: &RuleId) -> Option<&Rule> {
        self.rules.iter().find(|rule| &rule.id == id)
    }

    /// The rules except those in `disabled`. Naming a rule that doesn't exist is an error, so a
    /// typo doesn't leave a rule on unnoticed.
    pub fn without(mut self, disabled: &[RuleId]) -> Result<Self, String> {
        if let Some(unknown) = disabled.iter().find(|id| self.get(id).is_none()) {
            return Err(format!(
                "`{unknown}` in `disabled_rules` isn't a rule. Run `cleanse rules` to list them."
            ));
        }
        self.rules.retain(|rule| !disabled.contains(&rule.id));
        Ok(self)
    }
}

/// A rule file that couldn't be loaded, and why.
#[derive(Debug)]
pub struct RuleError {
    pub file: String,
    pub message: String,
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

impl Error for RuleError {}
