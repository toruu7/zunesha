//! Project-local orchestration settings (`.zou/config.toml`).
//!
//! The user-wide [`crate::Config`] chooses the agent provider and model; this
//! file controls how *this repository* is isolated, verified, and recovered.
//! Every field has a conservative default, so an existing installation keeps
//! working without a config file.
//!
//! Example:
//!
//! ```toml
//! verify_cmd = "cargo check --workspace && cargo test"
//! max_verify_retries = 2
//! bootstrap_copy = [".env", ".env.local"]
//! bootstrap_cmd = "npm ci --ignore-scripts"
//! fallback_agents = ["antigravity"]
//! rate_limit_cooldown_secs = 60
//! auto_prune = true
//! min_free_disk_gb = 5
//! idle_timeout_secs = 900
//! isolated_caches = true
//! opencode_guardrails = true
//! opencode_steps = 80
//! acceptance_check = false
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Project-local settings loaded from `.zou/config.toml` (or `config.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    /// Backup agent commands/providers tried after the primary hits a quota
    /// wall. Entries may be provider names (`opencode`, `antigravity`) or full
    /// command lines honoring `{prompt}` / `{workspace}`.
    pub fallback_agents: Vec<String>,
    /// Delay before switching to a fallback agent (or retrying after a quota
    /// reset). Zero means switch immediately.
    pub rate_limit_cooldown_secs: u64,
    /// Test/lint command run in the worktree after the agent exits 0. When
    /// unset, `ZOU_EVAL_CMD` and then auto-detection are used.
    pub verify_cmd: Option<String>,
    /// Self-healing cycles after a failed `verify_cmd` before parking the task.
    pub max_verify_retries: u32,
    /// Untracked files copied from the repository root into each fresh
    /// worktree (for example `.env`, `.env.local`, local certs).
    pub bootstrap_copy: Vec<String>,
    /// Command run once inside a fresh worktree after the snapshot copy and
    /// before the baseline commit (for example dependency installation).
    pub bootstrap_cmd: Option<String>,
    /// Remove a task's worktree automatically after a clean approval. When
    /// false the workspace remains available for inspection.
    pub auto_prune: bool,
    /// Do not spawn new worktrees when free disk space falls below this value.
    /// `0` disables the circuit breaker.
    pub min_free_disk_gb: f64,
    /// Kill an agent after this many seconds with no stdout and no workspace
    /// file activity. `0` disables idle detection.
    pub idle_timeout_secs: u64,
    /// Isolate build/linter caches and temporary files inside each worktree.
    pub isolated_caches: bool,
    /// Merge a workspace-local `.opencode/opencode.json` with the external
    /// directory jail, step cap, and doom-loop prompt.
    pub opencode_guardrails: bool,
    /// Soft step cap written into the OpenCode config. OpenCode emits a
    /// progress summary before the hard provider token limit.
    pub opencode_steps: u32,
    /// Run a lightweight acceptance/diff sanity check before marking REVIEW.
    /// A non-empty diff is required when this is enabled; the result is
    /// surfaced as `requires_attention` rather than silently approving.
    pub acceptance_check: bool,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            fallback_agents: Vec::new(),
            rate_limit_cooldown_secs: 0,
            verify_cmd: None,
            max_verify_retries: 2,
            bootstrap_copy: Vec::new(),
            bootstrap_cmd: None,
            auto_prune: true,
            min_free_disk_gb: 5.0,
            idle_timeout_secs: 900,
            isolated_caches: true,
            opencode_guardrails: true,
            opencode_steps: 80,
            acceptance_check: false,
        }
    }
}

impl ProjectConfig {
    /// The canonical config path for a repository root.
    pub fn path(root: &Path) -> PathBuf {
        root.join(".zou").join("config.toml")
    }

    /// Load `.zou/config.toml`, falling back to `.zou/config.json` for older
    /// hand-written setups. A missing file yields defaults; a malformed file is
    /// an error so a typo does not silently disable the verification gate.
    pub fn load(root: &Path) -> Result<Self> {
        let toml_path = Self::path(root);
        if toml_path.is_file() {
            let text = std::fs::read_to_string(&toml_path)?;
            let mut config: Self = toml::from_str(&text)
                .map_err(|e| Error::Config(format!("{}: {e}", toml_path.display())))?;
            config.normalize();
            return Ok(config);
        }

        let json_path = root.join(".zou").join("config.json");
        if json_path.is_file() {
            let text = std::fs::read_to_string(&json_path)?;
            let mut config: Self = serde_json::from_str(&text)
                .map_err(|e| Error::Config(format!("{}: {e}", json_path.display())))?;
            config.normalize();
            return Ok(config);
        }

        Ok(Self::default())
    }

    /// Load without failing a worker on a malformed optional file.
    pub fn load_or_default(root: &Path) -> Self {
        Self::load(root).unwrap_or_else(|error| {
            eprintln!("warning: ignoring project config: {error}");
            Self::default()
        })
    }

    /// Clamp values that would make the orchestrator unsafe or unusable.
    fn normalize(&mut self) {
        if self.max_verify_retries > 20 {
            self.max_verify_retries = 20;
        }
        if self.opencode_steps == 0 {
            self.opencode_steps = 80;
        }
        if !self.min_free_disk_gb.is_finite() || self.min_free_disk_gb < 0.0 {
            self.min_free_disk_gb = 0.0;
        }
        self.bootstrap_copy.retain(|path| is_safe_relative(path));
        self.fallback_agents
            .retain(|agent| !agent.trim().is_empty());
    }

    /// Whether a fallback is configured.
    pub fn has_fallbacks(&self) -> bool {
        !self.fallback_agents.is_empty()
    }
}

/// Reject absolute paths and parent traversal for bootstrap copies.
pub fn is_safe_relative(path: &str) -> bool {
    let path = Path::new(path.trim());
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe_and_verification_is_bounded() {
        let config = ProjectConfig::default();
        assert_eq!(config.max_verify_retries, 2);
        assert!(config.auto_prune);
        assert_eq!(config.min_free_disk_gb, 5.0);
        assert_eq!(config.idle_timeout_secs, 900);
        assert!(config.isolated_caches);
        assert!(config.opencode_guardrails);
    }

    #[test]
    fn loads_toml_and_rejects_unsafe_bootstrap_paths() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".zou")).unwrap();
        std::fs::write(
            ProjectConfig::path(tmp.path()),
            r#"
verify_cmd = "cargo test"
max_verify_retries = 4
bootstrap_copy = [".env", "../outside", "/etc/passwd"]
auto_prune = false
min_free_disk_gb = 2.5
idle_timeout_secs = 30
"#,
        )
        .unwrap();
        let config = ProjectConfig::load(tmp.path()).unwrap();
        assert_eq!(config.verify_cmd.as_deref(), Some("cargo test"));
        assert_eq!(config.max_verify_retries, 4);
        assert_eq!(config.bootstrap_copy, vec![".env"]);
        assert!(!config.auto_prune);
        assert_eq!(config.min_free_disk_gb, 2.5);
        assert_eq!(config.idle_timeout_secs, 30);
    }

    #[test]
    fn missing_config_is_default_and_bad_toml_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            ProjectConfig::load(tmp.path()).unwrap(),
            ProjectConfig::default()
        );
        std::fs::create_dir_all(tmp.path().join(".zou")).unwrap();
        std::fs::write(ProjectConfig::path(tmp.path()), "not = [valid").unwrap();
        assert!(ProjectConfig::load(tmp.path()).is_err());
    }
}
