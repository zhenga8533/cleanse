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

pub use matcher::{Detection, Matcher};
pub use schema::{Base, Location, Os, Rule, RuleTest, Target};

/// The rule files that ship with Cleanse, embedded at build time. A test checks that this list
/// matches the files in `rules/`.
const BUILTIN: &[(&str, &str)] = &[
    ("dotnet.toml", include_str!("../rules/dotnet.toml")),
    (
        "game-engines.toml",
        include_str!("../rules/game-engines.toml"),
    ),
    ("java.toml", include_str!("../rules/java.toml")),
    ("javascript.toml", include_str!("../rules/javascript.toml")),
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
    (
        "tool-caches.toml",
        include_str!("../rules/tool-caches.toml"),
    ),
];

/// A validated collection of rules with unique ids.
#[derive(Clone, Debug)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

impl RuleSet {
    /// The rules that ship with Cleanse.
    pub fn builtin() -> Result<Self, RuleError> {
        Self::from_sources(BUILTIN.iter().copied())
    }

    /// Parses and validates rule files given as `(file name, contents)` pairs.
    pub fn from_sources<'a>(
        sources: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, RuleError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RuleFile {
            rule: Vec<Rule>,
        }

        let mut rules = Vec::new();
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
                rule.validate().map_err(fail)?;
                if !ids.insert(rule.id.clone()) {
                    return Err(fail("another rule already uses this id".into()));
                }
                if let Target::KnownLocation { locations: specs } = &rule.target {
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
        Ok(Self { rules })
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
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
