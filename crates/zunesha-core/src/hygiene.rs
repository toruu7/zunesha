//! Worktree hygiene, disk accounting, and the disk-space circuit breaker.
//!
//! Workspaces are valuable until their task is merged or explicitly pruned.
//! This module only deletes a workspace when the task is `DONE`, gone, or an
//! unreferenced directory is clearly orphaned. Running, review, and
//! needs-interaction worktrees are always protected.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::Result;
use crate::paths::ZuneshaPaths;
use crate::task::TaskStatus;
use crate::workspace::WorkspaceManager;

/// Disk accounting for the dashboard/TUI status bars.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskUsage {
    /// Bytes occupied by `.zou/ws`.
    pub workspace_bytes: u64,
    /// Bytes free on the filesystem containing `.zou/ws`, when detectable.
    pub free_bytes: Option<u64>,
    /// Human-readable workspace footprint.
    pub workspace_label: String,
    /// Human-readable free space, when detectable.
    pub free_label: Option<String>,
}

impl DiskUsage {
    /// Whether spawning another worktree would cross the configured floor.
    pub fn below_floor(&self, min_free_gb: f64) -> bool {
        if min_free_gb <= 0.0 {
            return false;
        }
        let Some(free) = self.free_bytes else {
            return false;
        };
        let floor = (min_free_gb * 1024.0 * 1024.0 * 1024.0) as u64;
        free < floor
    }
}

/// Measure `.zou/ws` and the free space on its filesystem.
pub fn disk_usage(paths: &ZuneshaPaths) -> DiskUsage {
    let ws = paths.workspaces_dir();
    let workspace_bytes = directory_size(&ws);
    let free_bytes = free_disk_bytes(&ws);
    DiskUsage {
        workspace_bytes,
        free_bytes,
        workspace_label: bytes_label(workspace_bytes),
        free_label: free_bytes.map(bytes_label),
    }
}

/// Recursively sum regular-file sizes, without following symlinks.
pub fn directory_size(root: &Path) -> u64 {
    fn walk(path: &Path, total: &mut u64, depth: usize) {
        if depth > 64 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.file_type() else {
                continue;
            };
            if meta.is_symlink() {
                continue;
            }
            if meta.is_dir() {
                walk(&entry.path(), total, depth + 1);
            } else if meta.is_file() {
                if let Ok(meta) = entry.metadata() {
                    *total = total.saturating_add(meta.len());
                }
            }
        }
    }
    let mut total = 0;
    walk(root, &mut total, 0);
    total
}

/// Query free bytes with `df -Pk` (portable across Linux and macOS). On
/// Windows, or when `df` is unavailable, return `None` and let the circuit
/// breaker fail open rather than blocking work.
fn free_disk_bytes(path: &Path) -> Option<u64> {
    if !path.exists() {
        return None;
    }
    #[cfg(unix)]
    {
        let output = std::process::Command::new("df")
            .arg("-Pk")
            .arg(path)
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().nth(1)?;
        let available_kb = line.split_whitespace().nth(3)?.parse::<u64>().ok()?;
        Some(available_kb.saturating_mul(1024))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn bytes_label(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Remove worktrees that are safe to discard: completed tasks, records whose
/// task no longer exists, and orphan directories with no live task.
///
/// `REVIEW`, `NEEDS_INTERACTION`, `FAILED_NEEDS_HUMAN`, `IN_PROGRESS`,
/// `EVALUATING`, `RESOLVING_CONFLICTS`, and `TODO` worktrees are protected
/// because they may contain the only copy of a human's in-flight work.
pub fn prune_workspaces(paths: &ZuneshaPaths, db: &Database) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    let mgr = WorkspaceManager::new(paths.clone());
    let records = db.list_workspaces()?;
    let recorded: std::collections::HashSet<String> = records
        .iter()
        .map(|record| record.task_id.clone())
        .collect();

    for record in records {
        let removable = match db.get_task(&record.task_id)? {
            None => true,
            Some(task) => task.status == TaskStatus::Done,
        };
        if removable {
            mgr.remove(db, &record.task_id)?;
            removed.push(record.task_id);
        }
    }

    // Orphan directories: no DB record. Keep a directory whose id still names a
    // live task; remove it only when the task is gone or DONE.
    if let Ok(entries) = std::fs::read_dir(paths.workspaces_dir()) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if recorded.contains(&name) {
                continue;
            }
            let removable = match db.get_task(&name)? {
                None => true,
                Some(task) => task.status == TaskStatus::Done,
            };
            if removable {
                remove_dir(&entry.path())?;
                removed.push(name);
            }
        }
    }

    removed.sort();
    removed.dedup();
    Ok(removed)
}

fn remove_dir(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

/// The `.zou/ws` path, useful for tests and callers that need to display it.
pub fn workspace_root(paths: &ZuneshaPaths) -> PathBuf {
    paths.workspaces_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_labels_are_readable() {
        assert_eq!(bytes_label(0), "0 B");
        assert_eq!(bytes_label(1024), "1.0 KiB");
        assert_eq!(bytes_label(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[test]
    fn floor_check_only_fails_when_free_space_is_known() {
        let usage = DiskUsage {
            workspace_bytes: 0,
            free_bytes: None,
            workspace_label: "0 B".into(),
            free_label: None,
        };
        assert!(!usage.below_floor(5.0));
        let low = DiskUsage {
            free_bytes: Some(1024),
            ..usage
        };
        assert!(low.below_floor(5.0));
    }
}
