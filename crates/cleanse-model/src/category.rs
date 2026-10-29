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
