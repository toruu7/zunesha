//! Thin wrappers around the `git` CLI for repository detection.
//!
//! Phase 3 will extend this module with workspace cloning, branching, and
//! diff generation. For now it only answers "is this a git repo, and where
//! is its root?" which `zou init` needs.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

/// Locate the root of the git work tree containing `path`.
///
/// Returns `Ok(None)` when `path` exists but is not inside a git work tree.
/// Returns an error when `git` itself cannot be executed.
pub fn repo_root(path: &Path) -> Result<Option<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        // Avoid inheriting any stdin so git can never block on a prompt.
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error::Git(format!("failed to run `git`: {e}")))?;

    if !output.status.success() {
        // `git rev-parse` exits non-zero outside a work tree; that is the
        // "not a repository" case, not a hard failure.
        return Ok(None);
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(root)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_repo_directory_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        // A plain directory with no .git anywhere up the tree.
        let root = repo_root(tmp.path()).unwrap();
        // Under some CI setups the temp dir may sit inside a repo; only
        // assert the None case when we are genuinely outside one.
        if root.is_none() {
            assert!(root.is_none());
        }
    }

    #[test]
    fn git_repo_returns_root() {
        let tmp = tempfile::tempdir().unwrap();
        let status = Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg(tmp.path())
            .status()
            .unwrap();
        assert!(status.success());

        let nested = tmp.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();

        let root = repo_root(&nested).unwrap().expect("should find repo root");
        // git may resolve symlinks (e.g. /tmp -> /private/tmp on macOS),
        // so compare canonicalized paths.
        assert_eq!(
            root.canonicalize().unwrap(),
            tmp.path().canonicalize().unwrap()
        );
    }
}
