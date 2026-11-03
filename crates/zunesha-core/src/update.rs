//! Core updater logic for Zunesha.
//!
//! Checks release tags, resolves sequential upgrade paths, and installs
//! updates step-by-step so intermediate migrations and state changes are
//! applied without breaking jumps.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::error::{Error, Result};

/// Default repository used when no install record or override is present.
pub const DEFAULT_REPO: &str = "https://github.com/toruu7/zunesha";

/// Official docs site URL.
pub const DOCS_URL: &str = "https://zunesha-docs.vercel.app";

/// Environment variable that overrides the update source repository.
pub const UPDATE_REPO_ENV: &str = "ZOU_UPDATE_REPO";

/// Metadata written by the installer after a successful install.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct InstallRecord {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub release_base: Option<String>,
    #[serde(default)]
    pub installed_at: Option<String>,
}

/// A parsed semver release version (`major.minor.patch`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let core = text.split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Result of an update check.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateCheck {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    /// Sequential versions to apply one-by-one from current to latest.
    pub pending_updates: Vec<String>,
    pub docs_url: String,
    pub repo_url: String,
}

/// Options when executing an update.
#[derive(Clone, Debug, Default)]
pub struct UpdateOptions {
    pub repo: Option<String>,
    pub prefix: Option<PathBuf>,
    pub no_dashboard: bool,
}

/// Check if a newer version is available and list sequential update steps.
pub fn check_updates(repo_override: Option<&str>, prefix: Option<&Path>) -> Result<UpdateCheck> {
    let current_str = env!("CARGO_PKG_VERSION");
    let current = Version::parse(current_str).ok_or_else(|| {
        Error::Config(format!("installed version `{current_str}` is not valid semver"))
    })?;

    let prefix_buf = match prefix {
        Some(p) => p.to_path_buf(),
        None => install_prefix(None)?,
    };

    let record = load_record(&prefix_buf);
    let (source, _release_base) = update_source(repo_override, record.as_ref())?;

    let tags = match remote_tags(&source) {
        Ok(t) => t,
        Err(_e) => {
            // If offline or network unavailable, report up to date gracefully
            return Ok(UpdateCheck {
                current_version: current_str.to_string(),
                latest_version: current_str.to_string(),
                update_available: false,
                pending_updates: Vec::new(),
                docs_url: DOCS_URL.to_string(),
                repo_url: DEFAULT_REPO.to_string(),
            });
        }
    };

    let newer = newer_releases(current, &tags);
    let update_available = !newer.is_empty();
    let latest_version = if let Some(last) = newer.last() {
        last.0.to_string()
    } else {
        current_str.to_string()
    };
    let pending_updates = newer.into_iter().map(|(_, tag)| tag).collect();

    Ok(UpdateCheck {
        current_version: current_str.to_string(),
        latest_version,
        update_available,
        pending_updates,
        docs_url: DOCS_URL.to_string(),
        repo_url: source,
    })
}

/// Filter and sort tags strictly newer than `current` in ascending semver order.
pub fn newer_releases(current: Version, tags: &[String]) -> Vec<(Version, String)> {
    let mut newer: Vec<_> = tags
        .iter()
        .filter_map(|tag| Version::parse(tag).map(|v| (v, tag.clone())))
        .filter(|(v, _)| *v > current)
        .collect();

    // Ascending order: step through v0.15 -> v0.16 -> v0.17 sequentially
    newer.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    newer.dedup_by(|a, b| a.0 == b.0);
    newer
}

/// Highest semver release tag among all tags.
pub fn latest_release(tags: &[String]) -> Option<(Version, String)> {
    tags.iter()
        .filter_map(|tag| Version::parse(tag).map(|version| (version, tag.clone())))
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)))
}

/// Apply all pending updates step-by-step in sequence.
pub fn apply_sequential_updates(options: &UpdateOptions) -> Result<Vec<String>> {
    let current_str = env!("CARGO_PKG_VERSION");
    let current = Version::parse(current_str).ok_or_else(|| {
        Error::Config(format!("installed version `{current_str}` is not valid semver"))
    })?;

    let prefix = match &options.prefix {
        Some(p) => p.clone(),
        None => install_prefix(None)?,
    };

    let record = load_record(&prefix);
    let (source, release_base) = update_source(options.repo.as_deref(), record.as_ref())?;

    let tags = remote_tags(&source)?;
    let newer = newer_releases(current, &tags);
    if newer.is_empty() {
        return Ok(Vec::new());
    }

    let mut completed = Vec::new();
    for (idx, (_ver, tag)) in newer.iter().enumerate() {
        println!("==> Step {}/{}: updating to {tag}…", idx + 1, newer.len());
        install_release(
            &source,
            tag,
            &prefix,
            options.no_dashboard,
            release_base.as_deref(),
        )?;
        completed.push(tag.clone());
    }

    Ok(completed)
}

/// Locate the install prefix from an explicit option or executable path.
pub fn install_prefix(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(prefix) = explicit {
        return Ok(prefix.to_path_buf());
    }

    let exe = std::env::current_exe()
        .map_err(|e| Error::Config(format!("could not locate running binary: {e}")))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    exe.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            Error::Config(format!(
                "could not infer install prefix from {}; pass --prefix <DIR>",
                exe.display()
            ))
        })
}

/// Path to install record file.
pub fn record_path(prefix: &Path) -> PathBuf {
    prefix.join("share").join("zou").join("install.json")
}

/// Path to installed binary.
pub fn binary_path(prefix: &Path) -> PathBuf {
    prefix
        .join("bin")
        .join(if cfg!(windows) { "zou.exe" } else { "zou" })
}

/// Load the install record if present.
pub fn load_record(prefix: &Path) -> Option<InstallRecord> {
    let path = record_path(prefix);
    let text = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Resolve update source repository and release base feed.
pub fn update_source(
    explicit: Option<&str>,
    record: Option<&InstallRecord>,
) -> Result<(String, Option<String>)> {
    let explicit = explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var(UPDATE_REPO_ENV)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        });

    let recorded_repo = record
        .and_then(|r| r.repo.as_deref())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    let explicit_source = explicit.is_some();
    let repo = explicit
        .or(recorded_repo)
        .unwrap_or_else(|| DEFAULT_REPO.to_string());
    let repo = normalize_source(&repo)?;
    let explicit_local = explicit_source && Path::new(&repo).is_dir();

    let recorded_remote = record
        .and_then(|r| r.remote.as_deref())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    let source = if explicit_source {
        if explicit_local {
            repo.clone()
        } else {
            remote_for(&repo)
        }
    } else {
        recorded_remote.unwrap_or_else(|| remote_for(&repo))
    };

    if source.trim().is_empty() {
        return Err(Error::Config(
            "update source is empty; pass --repo <URL|PATH>".into(),
        ));
    }

    let release_base = if explicit_source {
        None
    } else {
        record
            .and_then(|r| r.release_base.as_deref())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };

    Ok((source, release_base))
}

fn normalize_source(repo: &str) -> Result<String> {
    let path = Path::new(repo);
    if !path.exists() {
        return Ok(repo.to_string());
    }
    path.canonicalize()
        .map(|path| path.display().to_string())
        .map_err(|e| Error::Config(format!("could not resolve update source {repo}: {e}")))
}

fn remote_for(repo: &str) -> String {
    let path = Path::new(repo);
    if path.is_dir() {
        if let Ok(output) = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["remote", "get-url", "origin"])
            .output()
        {
            if output.status.success() {
                let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !remote.is_empty() {
                    return remote;
                }
            }
        }
    }
    repo.to_string()
}

/// Retrieve release tags from git remote.
pub fn remote_tags(source: &str) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["ls-remote", "--tags", "--refs", source])
        .output()
        .map_err(|e| Error::Config(format!("could not run git to check for updates: {e}")))?;

    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("git ls-remote failed");
        return Err(Error::Config(format!(
            "could not check {source} for updates: {detail}"
        )));
    }

    Ok(parse_tags(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse `git ls-remote --tags --refs` output.
pub fn parse_tags(output: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let _object = fields.next();
        let Some(reference) = fields.next() else {
            continue;
        };
        if let Some(tag) = reference.strip_prefix("refs/tags/") {
            tags.push(tag.to_string());
        }
    }
    tags
}

/// Clone and install a single release tag.
pub fn install_release(
    source: &str,
    tag: &str,
    prefix: &Path,
    no_dashboard: bool,
    release_base: Option<&str>,
) -> Result<()> {
    let temp = TempDir::new().map_err(|e| Error::Config(format!("temp dir error: {e}")))?;
    let checkout = temp.path().join("src");

    let mut clone = Command::new("git");
    clone.args(["-c", "advice.detachedHead=false", "clone", "--quiet"]);
    if !Path::new(source).is_dir() {
        clone.args(["--depth", "1"]);
    }
    let status = clone
        .args(["--branch", tag, source])
        .arg(&checkout)
        .status()
        .map_err(|e| Error::Config(format!("could not run git clone: {e}")))?;
    if !status.success() {
        return Err(Error::Config(format!(
            "could not clone {tag} from {source} (is the tag pushed?)"
        )));
    }

    #[cfg(windows)]
    let status = {
        let installer = checkout.join("install.ps1");
        if !installer.is_file() {
            return Err(Error::Config(format!(
                "release {tag} has no install.ps1 at its root"
            )));
        }

        let mut command = Command::new("powershell");
        command
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&installer)
            .args(["-Repo", source, "-Version", tag])
            .arg("-Prefix")
            .arg(prefix)
            .args(["-CargoProfile", "release"]);
        if let Some(release_base) = release_base {
            command.args(["-ReleaseBase", release_base]);
        } else {
            command.arg("-Source");
        }
        if no_dashboard {
            command.arg("-NoDashboard");
        }
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        command
            .status()
            .map_err(|e| Error::Config(format!("could not run the installer: {e}")))?
    };

    #[cfg(not(windows))]
    let status = {
        let installer = checkout.join("install.sh");
        if !installer.is_file() {
            return Err(Error::Config(format!(
                "release {tag} has no install.sh at its root"
            )));
        }

        let mut command = Command::new("sh");
        command.arg(&installer).args(["--repo", source]);
        if let Some(release_base) = release_base {
            command.args(["--release-base", release_base]);
        } else {
            command.arg("--source");
        }
        command
            .args(["--version", tag])
            .arg("--prefix")
            .arg(prefix)
            .args(["--profile", "release"])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        if no_dashboard {
            command.arg("--no-dashboard");
        }
        command
            .status()
            .map_err(|e| Error::Config(format!("could not run the installer: {e}")))?
    };

    if !status.success() {
        return Err(Error::Config(format!(
            "installer failed for {tag} (exit code: {})",
            status.code().unwrap_or(-1)
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing_and_ordering() {
        let v1 = Version::parse("v0.15.0").unwrap();
        let v2 = Version::parse("0.16.0").unwrap();
        let v3 = Version::parse("v0.17.0-beta.1").unwrap();
        assert!(v1 < v2);
        assert!(v2 < v3);
        assert_eq!(v1.to_string(), "0.15.0");
    }

    #[test]
    fn newer_releases_orders_sequentially_without_skipping() {
        let current = Version::parse("0.15.0").unwrap();
        let tags = vec![
            "v0.14.0".to_string(),
            "v0.15.0".to_string(),
            "v0.17.0".to_string(),
            "v0.16.0".to_string(),
            "v0.17.0".to_string(), // duplicate
            "unrelated-tag".to_string(),
        ];

        let newer = newer_releases(current, &tags);
        assert_eq!(newer.len(), 2);
        assert_eq!(newer[0].1, "v0.16.0");
        assert_eq!(newer[1].1, "v0.17.0");
    }

    #[test]
    fn parse_tags_strips_refs_prefix() {
        let sample = "commit1 refs/tags/v0.1.0\ncommit2 refs/tags/v0.2.0\n";
        assert_eq!(parse_tags(sample), vec!["v0.1.0", "v0.2.0"]);
    }
}
