use std::fmt;

use serde::{Deserialize, Serialize};

/// What kind of thing an item is, for grouping results. Every rule declares one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Installed project dependencies, like `node_modules` and virtual environments.
    Dependencies,
    /// Compiled output and build caches inside projects, like `target/` and `.next`.
    BuildOutput,
    /// Package-manager download caches, like npm's and pip's.
    PackageCaches,
    /// Caches kept by IDEs, compilers, and SDKs, like Xcode's `DerivedData`.
    ToolCaches,
    /// Files written by AI coding agents.
    AiAgents,
    /// Downloaded AI models.
    AiModels,
    /// Git worktrees and repository data.
    Git,
    /// Temporary files.
    Temp,
    /// Logs and crash dumps.
    Logs,
    /// Web browser caches.
    BrowserCaches,
    /// Files in the Downloads folder.
    Downloads,
    /// Unusually large files.
    LargeFiles,
}

impl Category {
    /// Every category, in the order they're declared.
    pub const ALL: [Self; 12] = [
        Self::Dependencies,
        Self::BuildOutput,
        Self::PackageCaches,
        Self::ToolCaches,
        Self::AiAgents,
        Self::AiModels,
        Self::Git,
        Self::Temp,
        Self::Logs,
        Self::BrowserCaches,
        Self::Downloads,
        Self::LargeFiles,
    ];

    /// The name rule files and JSON use, like `build_output`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Dependencies => "dependencies",
            Self::BuildOutput => "build_output",
            Self::PackageCaches => "package_caches",
            Self::ToolCaches => "tool_caches",
            Self::AiAgents => "ai_agents",
            Self::AiModels => "ai_models",
            Self::Git => "git",
            Self::Temp => "temp",
            Self::Logs => "logs",
            Self::BrowserCaches => "browser_caches",
            Self::Downloads => "downloads",
            Self::LargeFiles => "large_files",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Dependencies => "Dependencies",
            Self::BuildOutput => "Build output",
            Self::PackageCaches => "Package caches",
            Self::ToolCaches => "Tool caches",
            Self::AiAgents => "AI agents",
            Self::AiModels => "AI models",
            Self::Git => "Git",
            Self::Temp => "Temporary files",
            Self::Logs => "Logs",
            Self::BrowserCaches => "Browser caches",
            Self::Downloads => "Downloads",
            Self::LargeFiles => "Large files",
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_category_is_listed_once_under_its_serialized_name() {
        let distinct: HashSet<Category> = Category::ALL.into_iter().collect();

        assert_eq!(distinct.len(), Category::ALL.len());
        for category in Category::ALL {
            assert_eq!(serde_json::to_value(category).unwrap(), category.id());
        }
    }
}
