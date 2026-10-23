//! Implementation of `zou init`.
//!
//! Responsibilities:
//! 1. Verify the target is inside a git work tree (unless `--force`).
//! 2. Create the `.zou/` control directory.
//! 3. Bootstrap the SQLite database at `.zou/state.db`.
//! 4. Ensure `.zou/` is git-ignored so orchestrator state never
//!    pollutes the user's repository.

use std::path::{Path, PathBuf};

use crate::db::Database;
use crate::error::{Error, Result};
use crate::git;
use crate::paths::ZuneshaPaths;

/// Line added to the repository's `.gitignore`.
const GITIGNORE_ENTRY: &str = ".zou/";

/// Report returned by a successful [`init_project`] call, for the CLI to print.
#[derive(Debug, Clone)]
pub struct InitReport {
    /// Where the `.zou` directory was created.
    pub zou_dir: PathBuf,
    /// Path of the freshly bootstrapped SQLite database.
    pub state_db: PathBuf,
    /// Path of the (empty) task workspace directory.
    pub workspaces_dir: PathBuf,
    /// Root of the git work tree the installation is anchored to.
    pub repo_root: PathBuf,
    /// Whether `.zou/` was appended to `.gitignore`.
    pub gitignore_updated: bool,
    /// Whether an installation already existed (init is idempotent).
    pub already_initialized: bool,
}

/// Initialize the orchestrator inside the repository containing `target`.
///
/// `target` is usually the current working directory. The `.zou`
/// directory is always created at the *repo root* (not necessarily at
/// `target`) so state is shared no matter which subdirectory the user runs
/// the command from.
pub fn init_project(target: &Path, force: bool) -> Result<InitReport> {
    if !target.exists() {
        return Err(Error::PathNotFound {
            path: target.to_path_buf(),
        });
    }

    // 1. Git detection.
    let repo_root = git::repo_root(target)?;
    let repo_root = match repo_root {
        Some(root) => root,
        None if force => target.to_path_buf(),
        None => {
            return Err(Error::NotAGitRepository {
                path: target.to_path_buf(),
            })
        }
    };

    let paths = ZuneshaPaths::new(&repo_root);
    let already_initialized = paths.is_initialized();

    // 2. Control directory + workspace directory.
    std::fs::create_dir_all(paths.dir())?;
    std::fs::create_dir_all(paths.workspaces_dir())?;

    // 3. Bootstrap the database (creates it and applies the schema).
    let _db = Database::open(&paths.state_db())?;

    // 4. Keep our state out of the user's git status.
    let gitignore_updated = ensure_gitignore(&repo_root)?;

    Ok(InitReport {
        zou_dir: paths.dir(),
        state_db: paths.state_db(),
        workspaces_dir: paths.workspaces_dir(),
        repo_root,
        gitignore_updated,
        already_initialized,
    })
}

/// Append `.zou/` to the repo's `.gitignore` if it is not already
/// listed. Creates the file when missing. Returns whether the file changed.
fn ensure_gitignore(repo_root: &Path) -> Result<bool> {
    let gitignore = repo_root.join(".gitignore");
    let entry = GITIGNORE_ENTRY.trim_end_matches('/');

    if let Ok(existing) = std::fs::read_to_string(&gitignore) {
        let already_present = existing.lines().any(|line| {
            let line = line.trim();
            line == entry || line == GITIGNORE_ENTRY || line == format!("/{entry}")
        });
        if already_present {
            return Ok(false);
        }
        let mut updated = existing;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(GITIGNORE_ENTRY);
        updated.push('\n');
        std::fs::write(&gitignore, updated)?;
        return Ok(true);
    }

    // No .gitignore (or unreadable) — create a fresh one.
    std::fs::write(&gitignore, format!("{GITIGNORE_ENTRY}\n"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskStatus;

    /// Create a scratch git repository in a temp dir.
    fn scratch_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg(tmp.path())
            .status()
            .unwrap();
        assert!(status.success(), "git init should succeed in tests");
        tmp
    }

    #[test]
    fn init_inside_git_repo_creates_layout_and_db() {
        let repo = scratch_repo();
        let report = init_project(repo.path(), false).unwrap();

        assert!(report.zou_dir.is_dir());
        assert!(report.state_db.is_file());
        assert!(report.workspaces_dir.is_dir());
        assert_eq!(report.repo_root, repo.path().canonicalize().unwrap());
        assert!(!report.already_initialized);
    }

    #[test]
    fn init_without_git_repo_fails_unless_forced() {
        let tmp = tempfile::tempdir().unwrap();
        let err = init_project(tmp.path(), false).unwrap_err();
        assert!(matches!(err, Error::NotAGitRepository { .. }));

        // With --force it succeeds and anchors at the target itself.
        let report = init_project(tmp.path(), true).unwrap();
        assert!(report.state_db.is_file());
    }

    #[test]
    fn init_is_idempotent() {
        let repo = scratch_repo();
        let first = init_project(repo.path(), false).unwrap();
        assert!(!first.already_initialized);

        let second = init_project(repo.path(), false).unwrap();
        assert!(second.already_initialized);
        assert_eq!(first.state_db, second.state_db);
    }

    #[test]
    fn init_from_subdirectory_anchors_at_repo_root() {
        let repo = scratch_repo();
        let nested = repo.path().join("src").join("deep");
        std::fs::create_dir_all(&nested).unwrap();

        let report = init_project(&nested, false).unwrap();
        assert_eq!(report.repo_root, repo.path().canonicalize().unwrap());
        assert!(report.zou_dir.starts_with(repo.path()));
    }

    #[test]
    fn gitignore_is_appended_once() {
        let repo = scratch_repo();
        let report = init_project(repo.path(), false).unwrap();
        assert!(report.gitignore_updated);

        let contents = std::fs::read_to_string(repo.path().join(".gitignore")).unwrap();
        assert!(contents.contains(".zou/"));

        // Second init should not duplicate the entry.
        let again = init_project(repo.path(), false).unwrap();
        assert!(!again.gitignore_updated);
        let contents = std::fs::read_to_string(repo.path().join(".gitignore")).unwrap();
        assert_eq!(contents.matches(".zou/").count(), 1);
    }

    #[test]
    fn existing_gitignore_entries_are_respected() {
        let repo = scratch_repo();
        // Pre-existing ignore file using a leading slash form.
        std::fs::write(repo.path().join(".gitignore"), "/.zou\n").unwrap();

        let report = init_project(repo.path(), false).unwrap();
        assert!(
            !report.gitignore_updated,
            "should detect the leading-slash form"
        );
    }

    #[test]
    fn init_missing_path_errors() {
        let err = init_project(Path::new("/definitely/not/a/real/path"), false).unwrap_err();
        assert!(matches!(err, Error::PathNotFound { .. }));
    }

    #[test]
    fn database_is_usable_after_init() {
        let repo = scratch_repo();
        let report = init_project(repo.path(), false).unwrap();
        let db = Database::open(&report.state_db).unwrap();
        assert_eq!(db.task_count().unwrap(), 0);
        // Sanity: statuses are all parseable, proving enum/db agreement.
        for s in TaskStatus::ALL {
            assert_eq!(s.as_str().parse::<TaskStatus>().unwrap(), s);
        }
    }
}
