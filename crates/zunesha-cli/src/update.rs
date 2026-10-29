//! `zou update` — check the install's release tags and optionally reinstall.
//!
//! The installed binary carries its own Cargo version. Updates are published
//! as semver Git tags (`v1.2.3`); the updater compares the highest tag in the
//! source repository with `env!("CARGO_PKG_VERSION")` and asks before running
//! `install.sh` (or `install.ps1` on Windows) from that tag.
//!
//! The installer records where the binary came from in
//! `<prefix>/share/zou/install.json`, so an installed `zou` can find its
//! source checkout or remote. `--repo` and `ZOU_UPDATE_REPO` override it.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use zunesha_core::{Error, Result};

/// Used when no install record or override supplies a source.
pub const DEFAULT_REPO: &str = "https://github.com/toruu7/zunesha";

/// Environment variable that overrides the update source.
pub const UPDATE_REPO_ENV: &str = "ZOU_UPDATE_REPO";

/// Options for `zou update`.
#[derive(Debug, Default)]
pub struct Options {
    /// Source checkout or Git URL. Overrides the install record.
    pub repo: Option<String>,
    /// Install root. Defaults to the parent of the running binary's `bin/`.
    pub prefix: Option<PathBuf>,
    /// Only report whether an update exists.
    pub check: bool,
    /// Install without asking for confirmation.
    pub yes: bool,
    /// Pass `--no-dashboard` to the installer.
    pub no_dashboard: bool,
}

/// Metadata written by `install.sh` after a successful install.
#[derive(Debug, Default, Deserialize)]
struct InstallRecord {
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    remote: Option<String>,
    /// Prebuilt release feed, if the original install used one. Empty means
    /// the installation was source-built and updates should stay source.
    #[serde(default)]
    release_base: Option<String>,
}

/// A release version. Pre-release/build suffixes are ignored for comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    fn parse(text: &str) -> Option<Self> {
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

/// Run the update flow.
pub fn run(options: Options) -> Result<()> {
    let current = Version::parse(env!("CARGO_PKG_VERSION")).ok_or_else(|| {
        Error::Config(format!(
            "installed version `{}` is not valid semver",
            env!("CARGO_PKG_VERSION")
        ))
    })?;

    let prefix = install_prefix(options.prefix.as_deref())?;
    let record = load_record(&prefix);
    let (source, release_base) = update_source(options.repo.as_deref(), record.as_ref())?;

    let tags = remote_tags(&source)?;
    let Some((latest, tag)) = latest_release(&tags) else {
        return Err(Error::Config(format!(
            "no release tags found in {source}; expected tags like v{current}"
        )));
    };

    if latest <= current {
        println!("✔ up to date (v{current})");
        return Ok(());
    }

    println!("Update available: v{current} → {tag}");
    if options.check {
        println!("  run `zou update` to install it");
        return Ok(());
    }

    if !options.yes && !confirm("Update now?")? {
        println!("update skipped");
        return Ok(());
    }

    install(
        &source,
        &tag,
        &prefix,
        options.no_dashboard,
        release_base.as_deref(),
    )?;

    println!("✔ updated to {tag}");
    println!("  binary : {}", binary_path(&prefix).display());
    println!("  note   : restart `zou start` if it is running");
    Ok(())
}

/// Resolve the install prefix from `--prefix` or the running executable.
fn install_prefix(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(prefix) = explicit {
        return Ok(prefix.to_path_buf());
    }

    let exe = std::env::current_exe()
        .map_err(|e| Error::Config(format!("could not locate the running `zou` binary: {e}")))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    exe.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            Error::Config(format!(
                "could not infer the install prefix from {}; pass --prefix <DIR>",
                exe.display()
            ))
        })
}

/// Path to the installer's metadata file.
fn record_path(prefix: &Path) -> PathBuf {
    prefix.join("share").join("zou").join("install.json")
}

/// Path to the installed binary.
fn binary_path(prefix: &Path) -> PathBuf {
    prefix
        .join("bin")
        .join(if cfg!(windows) { "zou.exe" } else { "zou" })
}

/// Load the install record. A missing or malformed record is not fatal.
fn load_record(prefix: &Path) -> Option<InstallRecord> {
    let path = record_path(prefix);
    let text = fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&text) {
        Ok(record) => Some(record),
        Err(e) => {
            eprintln!(
                "warning: ignoring malformed install record {}: {e}",
                path.display()
            );
            None
        }
    }
}

/// Pick where to check for tags and where the installer should clone from.
///
/// The local checkout path is useful for finding the `origin` remote, but the
/// remote is a better install source: it is reachable from any directory and
/// is what the tag check already authenticated against.
fn update_source(
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
        .and_then(|record| record.repo.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let explicit_source = explicit.is_some();
    let repo = explicit
        .or(recorded_repo)
        .unwrap_or_else(|| DEFAULT_REPO.to_string());
    let repo = normalize_source(&repo)?;
    let explicit_local = explicit_source && Path::new(&repo).is_dir();

    let recorded_remote = record
        .and_then(|record| record.remote.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    // An explicit source wins; otherwise use the remote recorded at install
    // time (the local checkout may have moved or be gone). An explicitly
    // passed local checkout is used as-is so it still works without network.
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

    // Reuse the original prebuilt feed for a normal update. An explicit
    // --repo may point at a different fork, so build from that source instead
    // of downloading releases from the old project.
    let release_base = if explicit_source {
        None
    } else {
        record
            .and_then(|record| record.release_base.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };

    Ok((source, release_base))
}

/// Make a relative local checkout absolute so it survives a temp-dir clone.
fn normalize_source(repo: &str) -> Result<String> {
    let path = Path::new(repo);
    if !path.exists() {
        return Ok(repo.to_string());
    }
    path.canonicalize()
        .map(|path| path.display().to_string())
        .map_err(|e| Error::Config(format!("could not resolve update source {repo}: {e}")))
}

/// Find the `origin` remote for a local checkout, otherwise use it as-is.
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

/// List release tags from a local or remote Git repository.
fn remote_tags(source: &str) -> Result<Vec<String>> {
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

/// Parse `git ls-remote --tags --refs` output into bare tag names.
fn parse_tags(output: &str) -> Vec<String> {
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

/// Highest semver release tag. Unparseable tags are ignored.
fn latest_release(tags: &[String]) -> Option<(Version, String)> {
    tags.iter()
        .filter_map(|tag| Version::parse(tag).map(|version| (version, tag.clone())))
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)))
}

/// Ask the user a yes/no question. EOF means no.
fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    io::stdout().flush()?;

    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
        return Ok(false);
    }
    let answer = input.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

/// Clone the release tag, then delegate to its `install.sh`.
fn install(
    source: &str,
    tag: &str,
    prefix: &Path,
    no_dashboard: bool,
    release_base: Option<&str>,
) -> Result<()> {
    let temp = TempDir::new()?;
    let checkout = temp.path().join("src");

    println!("  source : {source}");
    println!("  cloning {tag}…");
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

        println!("  installing with {}", installer.display());
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

        println!("  installing with {}", installer.display());
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
            "installer exited with {status}; the previous binary was left in place"
        )));
    }
    Ok(())
}

/// A small self-cleaning temp directory.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!("zou-update-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&path).map_err(|e| {
            Error::Config(format!(
                "could not create temporary directory {}: {e}",
                path.display()
            ))
        })?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::{latest_release, parse_tags, Version};

    #[test]
    fn version_parsing_ignores_v_prefix_and_suffixes() {
        assert_eq!(
            Version::parse("v0.2.0"),
            Some(Version {
                major: 0,
                minor: 2,
                patch: 0
            })
        );
        assert_eq!(
            Version::parse("1.2.3"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(
            Version::parse("1.2.3-rc.1"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(Version::parse("1.2"), None);
        assert_eq!(Version::parse("not-a-version"), None);
    }

    #[test]
    fn latest_release_orders_numerically_not_lexicographically() {
        let tags = vec![
            "v0.9.0".to_string(),
            "v0.10.0".to_string(),
            "not-a-tag".to_string(),
            "v0.2.0".to_string(),
        ];
        assert_eq!(
            latest_release(&tags),
            Some((
                Version {
                    major: 0,
                    minor: 10,
                    patch: 0
                },
                "v0.10.0".to_string()
            ))
        );
    }

    #[test]
    fn parse_tags_keeps_only_tag_refs() {
        let output = "\
abc\trefs/tags/v0.1.0\n\
def\trefs/tags/v0.2.0^{}\n\
ghi\trefs/heads/master\n";
        assert_eq!(parse_tags(output), vec!["v0.1.0", "v0.2.0^{}"]);
    }
}
