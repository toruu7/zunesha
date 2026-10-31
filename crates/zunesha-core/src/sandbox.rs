//! Workspace containment and log redaction.
//!
//! `current_dir` is not a security boundary: a child process can still open
//! absolute paths, follow a symlink out of the workspace, or write through
//! `..`. This module adds the guard rails Zunesha can enforce portably:
//!
//! * **Workspace validation** — every symlink in the workspace must resolve
//!   inside the workspace. A tracked symlink pointing at `/etc` or the user's
//!   home directory is rejected before the agent starts.
//! * **Best-effort OS sandbox** — on Linux, when `bwrap` (bubblewrap) is
//!   usable, the agent runs with the host filesystem read-only and only the
//!   workspace writable. This is the strong mode. When `bwrap` is unavailable
//!   the worker still validates the workspace and pins the cwd, but the process
//!   is not a hard jail; `ZOU_SANDBOX=off` disables even the warning.
//! * **Secret redaction** — common `KEY=value`, `Authorization: Bearer …`, and
//!   token-shaped values are redacted before logs or final output are persisted
//!   or streamed to a client.
//!
//! The design is intentionally a policy layer, not a promise that a hostile
//! local process is fully contained. The long-term path is to add Docker /
//! microVM / remote providers behind the same interface.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::error::{Error, Result};

/// How strongly to contain the agent process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// Validate the workspace and use `bwrap` when it is available.
    Guard,
    /// Do not add an OS sandbox. Workspace validation still runs.
    Off,
}

impl SandboxMode {
    /// Read `ZOU_SANDBOX` (`guard` by default, `off` to opt out).
    pub fn from_env() -> Self {
        match std::env::var("ZOU_SANDBOX")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "off" | "0" | "false" | "none" => Self::Off,
            _ => Self::Guard,
        }
    }

    /// Stable label used in logs and diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Guard => "guard",
            Self::Off => "off",
        }
    }
}

/// The containment policy for one workspace.
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    workspace: PathBuf,
    mode: SandboxMode,
}

impl SandboxPolicy {
    /// Bind a policy to `workspace`.
    ///
    /// The path is canonicalized so later symlink checks compare real paths.
    pub fn new(workspace: impl Into<PathBuf>, mode: SandboxMode) -> Result<Self> {
        let workspace = workspace.into();
        let workspace = workspace.canonicalize().map_err(|e| {
            Error::SandboxViolation(format!(
                "workspace {} cannot be resolved: {e}",
                workspace.display()
            ))
        })?;
        if !workspace.is_dir() {
            return Err(Error::SandboxViolation(format!(
                "workspace is not a directory: {}",
                workspace.display()
            )));
        }
        Ok(Self { workspace, mode })
    }

    /// The canonical workspace root.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The configured mode.
    pub fn mode(&self) -> SandboxMode {
        self.mode
    }

    /// Reject symlinks whose real target escapes the workspace.
    ///
    /// `.git` is skipped: git's internal links are its own business, and a
    /// workspace's `.git` is a fresh local repository, not the user's.
    pub fn validate(&self) -> Result<()> {
        validate_tree(&self.workspace, &self.workspace)
    }

    /// Wrap an agent command for execution.
    ///
    /// In [`SandboxMode::Guard`] on Linux, if `bwrap` is usable, the returned
    /// program is `bwrap` and the host filesystem is read-only except for the
    /// workspace (and a private `/tmp`). Otherwise the command is returned
    /// unchanged; the caller still pins the cwd and validates symlinks.
    pub fn wrap(&self, program: &str, args: &[String]) -> Result<(String, Vec<String>)> {
        if self.mode == SandboxMode::Off || !bwrap_usable() {
            return Ok((program.to_string(), args.to_vec()));
        }

        let ws = self.workspace.to_string_lossy().into_owned();
        let mut wrapped = vec![
            "--die-with-parent".to_string(),
            "--new-session".to_string(),
            "--unshare-pid".to_string(),
            "--unshare-ipc".to_string(),
            "--unshare-uts".to_string(),
            "--ro-bind".to_string(),
            "/".to_string(),
            "/".to_string(),
            "--bind".to_string(),
            ws.clone(),
            ws.clone(),
            "--tmpfs".to_string(),
            "/tmp".to_string(),
            "--proc".to_string(),
            "/proc".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
        ];

        // Agent CLIs need to update their own cache/session/config files.
        // Bind only the known state directories, not the whole home directory:
        // the rest of the host stays read-only. `--bind-try` is a no-op when a
        // directory does not exist.
        for dir in agent_state_dirs() {
            wrapped.push("--bind-try".to_string());
            wrapped.push(dir.clone());
            wrapped.push(dir);
        }

        wrapped.push("--chdir".to_string());
        wrapped.push(ws);
        wrapped.push("--".to_string());
        wrapped.push(program.to_string());
        wrapped.extend(args.iter().cloned());
        Ok(("bwrap".to_string(), wrapped))
    }
}

/// Validate a path stays beneath `root`.
///
/// This is useful for callers that accept a path from a task, prompt, or API.
/// Existing paths are canonicalized; a missing final component is resolved
/// through its nearest existing parent so `workspace/new-file` is still
/// checked.
pub fn ensure_within(root: &Path, candidate: &Path) -> Result<PathBuf> {
    let root = root.canonicalize().map_err(|e| {
        Error::SandboxViolation(format!(
            "sandbox root {} cannot be resolved: {e}",
            root.display()
        ))
    })?;
    let resolved = resolve_lenient(candidate)?;
    if resolved == root || resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(Error::SandboxViolation(format!(
            "path escapes the workspace: {} (workspace {})",
            candidate.display(),
            root.display()
        )))
    }
}

fn resolve_lenient(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path.canonicalize().map_err(|e| {
            Error::SandboxViolation(format!("cannot resolve {}: {e}", path.display()))
        });
    }

    let mut missing = Vec::new();
    let mut current = path;
    loop {
        if current.exists() {
            let mut resolved = current.canonicalize().map_err(|e| {
                Error::SandboxViolation(format!("cannot resolve {}: {e}", current.display()))
            })?;
            for component in missing.iter().rev() {
                resolved.push(component);
            }
            return Ok(resolved);
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                current = parent;
            }
            _ => {
                return Err(Error::SandboxViolation(format!(
                    "cannot resolve {}",
                    path.display()
                )))
            }
        }
    }
}

fn validate_tree(root: &Path, dir: &Path) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        Error::SandboxViolation(format!("cannot read workspace {}: {e}", dir.display()))
    })?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            let target = std::fs::canonicalize(&path).map_err(|e| {
                Error::SandboxViolation(format!(
                    "workspace symlink {} cannot be resolved: {e}",
                    path.display()
                ))
            })?;
            if !target.starts_with(root) {
                return Err(Error::SandboxViolation(format!(
                    "workspace symlink escapes the workspace: {} -> {}",
                    path.display(),
                    target.display()
                )));
            }
        } else if metadata.is_dir() {
            validate_tree(root, &path)?;
        }
    }
    Ok(())
}

/// Known agent cache/session directories that must stay writable.
///
/// Keep this list narrow. Binding the entire home directory would let a
/// compromised agent rewrite shell profiles, SSH keys, or other projects.
fn agent_state_dirs() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    [
        ".cache/opencode",
        ".local/share/opencode",
        ".config/opencode",
        ".cache/antigravity",
        ".local/share/antigravity",
        ".config/antigravity",
        ".gemini/antigravity-cli",
    ]
    .into_iter()
    .map(|relative| home.join(relative).to_string_lossy().into_owned())
    .collect()
}

/// Whether `bwrap` exists and can bind the root read-only in this environment.
///
/// Cached: probing spawns a short-lived process and should happen at most once.
fn bwrap_usable() -> bool {
    static USABLE: OnceLock<bool> = OnceLock::new();
    *USABLE.get_or_init(|| {
        // Probe the namespaces and mounts the real wrapper uses. A host can
        // allow a bare read-only bind while denying `--unshare-pid`; treating
        // that as "usable" would make every agent spawn fail.
        Command::new("bwrap")
            .args([
                "--die-with-parent",
                "--new-session",
                "--unshare-pid",
                "--unshare-ipc",
                "--unshare-uts",
                "--ro-bind",
                "/",
                "/",
                "--tmpfs",
                "/tmp",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--",
                "/bin/true",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

/// Redact common secret shapes from one log/output line.
///
/// This is intentionally conservative: it catches `NAME=value`,
/// `NAME: value` for secret-looking names, and bearer tokens. It is a
/// best-effort safety net, not a substitute for not putting secrets in the
/// workspace in the first place.
pub fn redact_secrets(line: &str) -> String {
    let mut result = line.to_string();

    // Authorization headers and bearer tokens. Handle bearer first so the
    // header name itself is not mistaken for the credential.
    let lower = result.to_ascii_lowercase();
    if let Some(pos) = lower.find("bearer ") {
        let start = pos + "bearer ".len();
        let end = result[start..]
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .map(|offset| start + offset)
            .unwrap_or(result.len());
        if end > start {
            result.replace_range(start..end, "[REDACTED]");
        }
    } else if lower.contains("authorization:") {
        result = redact_after_marker(&result, "authorization:", " ");
    }

    // KEY=value / KEY: value for secret-looking names.
    for name in [
        "api_key",
        "apikey",
        "access_token",
        "refresh_token",
        "token",
        "secret",
        "password",
        "passwd",
        "private_key",
        "client_secret",
    ] {
        result = redact_assignment(&result, name);
    }

    result
}

fn redact_assignment(line: &str, name: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let Some(pos) = lower.find(name) else {
        return line.to_string();
    };
    // Only redact when the name is followed by a separator, so "tokenizer"
    // does not match "token".
    let after_name = pos + name.len();
    let rest = &line[after_name..];
    let Some(separator) = rest.find(['=', ':']) else {
        return line.to_string();
    };
    let value_start = after_name + separator + 1;
    let value = line[value_start..].trim_start();
    let leading = line[value_start..].len() - value.len();
    let start = value_start + leading;
    let end = value
        .find(|c: char| c.is_whitespace() || c == ',' || c == '"' || c == '\'')
        .map(|offset| start + offset)
        .unwrap_or(line.len());
    if end <= start {
        return line.to_string();
    }
    let mut out = line.to_string();
    out.replace_range(start..end, "[REDACTED]");
    out
}

fn redact_after_marker(line: &str, marker: &str, separator: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let Some(pos) = lower.find(marker) else {
        return line.to_string();
    };
    let value_start = pos + marker.len();
    let rest = &line[value_start..];
    let Some(offset) = rest.find(separator) else {
        return line.to_string();
    };
    let start = value_start + offset + separator.len();
    let value = line[start..].trim_start();
    let leading = line[start..].len() - value.len();
    let start = start + leading;
    let end = value
        .find(|c: char| c.is_whitespace() || c == ',' || c == '"' || c == '\'')
        .map(|offset| start + offset)
        .unwrap_or(line.len());
    if end <= start {
        return line.to_string();
    }
    let mut out = line.to_string();
    out.replace_range(start..end, "[REDACTED]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_normal_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("app.rs"), "fn main() {}\n").unwrap();
        let policy = SandboxPolicy::new(tmp.path(), SandboxMode::Off).unwrap();
        policy.validate().unwrap();
    }

    #[test]
    fn rejects_a_symlink_that_escapes_the_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "do not read\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("escape")).unwrap();

        let policy = SandboxPolicy::new(workspace.path(), SandboxMode::Off).unwrap();
        let err = policy.validate().unwrap_err();
        assert!(
            matches!(err, Error::SandboxViolation(ref message) if message.contains("escapes")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn allows_a_symlink_inside_the_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("src")).unwrap();
        std::fs::write(workspace.path().join("src/app.rs"), "fn main() {}\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("src", workspace.path().join("linked-src")).unwrap();

        let policy = SandboxPolicy::new(workspace.path(), SandboxMode::Off).unwrap();
        policy.validate().unwrap();
    }

    #[test]
    fn ensure_within_rejects_dotdot_and_absolute_escapes() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("src")).unwrap();
        std::fs::write(workspace.path().join("src/app.rs"), "").unwrap();

        assert!(ensure_within(workspace.path(), &workspace.path().join("src/app.rs")).is_ok());
        assert!(ensure_within(workspace.path(), &workspace.path().join("src/new.rs")).is_ok());
        assert!(ensure_within(workspace.path(), Path::new("/etc/passwd")).is_err());
        assert!(ensure_within(workspace.path(), &workspace.path().join("..")).is_err());
    }

    #[test]
    fn off_mode_does_not_rewrite_the_command() {
        let workspace = tempfile::tempdir().unwrap();
        let policy = SandboxPolicy::new(workspace.path(), SandboxMode::Off).unwrap();
        let (program, args) = policy
            .wrap("sh", &["-c".to_string(), "true".to_string()])
            .unwrap();
        assert_eq!(program, "sh");
        assert_eq!(args, vec!["-c".to_string(), "true".to_string()]);
    }

    #[test]
    fn redacts_secret_assignments_and_bearer_tokens() {
        assert_eq!(
            redact_secrets("API_KEY=sk-live-123456"),
            "API_KEY=[REDACTED]"
        );
        assert_eq!(redact_secrets("password: hunter2"), "password: [REDACTED]");
        assert_eq!(
            redact_secrets("Authorization: Bearer abc.def.ghi"),
            "Authorization: Bearer [REDACTED]"
        );
        assert_eq!(
            redact_secrets("the tokenizer is fine"),
            "the tokenizer is fine"
        );
    }
}
