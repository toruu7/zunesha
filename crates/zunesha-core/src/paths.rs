//! Filesystem layout of the `.zou` control directory.
//!
//! Everything the orchestrator owns lives in a single hidden folder at the
//! root of the target repository:
//!
//! ```text
//! <repo>/
//! └── .zou/
//!     ├── state.db     # SQLite: tasks, dependencies, metadata
//!     └── ws/          # one isolated workspace per task
//!         └── <task_id>/
//! ```

use std::path::{Path, PathBuf};

/// Name of the hidden control directory inside the target repository.
pub const ORCHESTRATOR_DIR: &str = ".zou";

/// SQLite database file inside the control directory.
pub const STATE_DB_FILE: &str = "state.db";

/// Directory holding one isolated workspace per in-flight task.
pub const WORKSPACES_DIR: &str = "ws";

/// Resolved paths for an orchestrator-managed repository.
///
/// Construct with [`ZuneshaPaths::new`] from a repository root, or
/// discover an existing installation with [`ZuneshaPaths::discover`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZuneshaPaths {
    root: PathBuf,
}

impl ZuneshaPaths {
    /// Build a path set rooted at `root` (the repository root).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Walk up from `start` looking for an existing `.zou/state.db`.
    ///
    /// Returns `None` when no orchestrator installation is found in any
    /// ancestor directory.
    pub fn discover(start: &Path) -> Option<Self> {
        let mut current = Some(start);
        while let Some(dir) = current {
            let candidate = Self::new(dir);
            if candidate.state_db().is_file() {
                return Some(candidate);
            }
            current = dir.parent();
        }
        None
    }

    /// The repository root this installation is anchored to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/.zou`
    pub fn dir(&self) -> PathBuf {
        self.root.join(ORCHESTRATOR_DIR)
    }

    /// `<root>/.zou/state.db`
    pub fn state_db(&self) -> PathBuf {
        self.dir().join(STATE_DB_FILE)
    }

    /// `<root>/.zou/ws`
    pub fn workspaces_dir(&self) -> PathBuf {
        self.dir().join(WORKSPACES_DIR)
    }

    /// `<root>/.zou/ws/<task_id>`
    pub fn workspace(&self, task_id: &str) -> PathBuf {
        self.workspaces_dir().join(task_id)
    }

    /// Whether an orchestrator installation exists at this root.
    pub fn is_initialized(&self) -> bool {
        self.state_db().is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_derived_from_root() {
        let paths = ZuneshaPaths::new("/tmp/repo");
        assert_eq!(paths.root(), Path::new("/tmp/repo"));
        assert_eq!(paths.dir(), PathBuf::from("/tmp/repo/.zou"));
        assert_eq!(paths.state_db(), PathBuf::from("/tmp/repo/.zou/state.db"));
        assert_eq!(paths.workspaces_dir(), PathBuf::from("/tmp/repo/.zou/ws"));
        assert_eq!(paths.workspace("t1"), PathBuf::from("/tmp/repo/.zou/ws/t1"));
    }

    #[test]
    fn discover_walks_up_ancestors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let nested = root.join("src").join("deep");
        std::fs::create_dir_all(&nested).unwrap();

        // Not initialized yet.
        assert!(ZuneshaPaths::discover(&nested).is_none());

        // Initialize at the root, then discover from the nested dir.
        std::fs::create_dir_all(root.join(ORCHESTRATOR_DIR)).unwrap();
        std::fs::write(root.join(ORCHESTRATOR_DIR).join(STATE_DB_FILE), b"").unwrap();

        let found = ZuneshaPaths::discover(&nested).expect("should discover");
        assert_eq!(found.root(), root);
        assert!(found.is_initialized());
    }
}
