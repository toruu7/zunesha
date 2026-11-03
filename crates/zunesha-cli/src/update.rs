//! `zou update` — check the install's release tags and reinstall sequentially.
//!
//! When multiple newer release tags exist (e.g. v0.15.0 -> v0.17.0), the updater
//! steps through each intermediate version one-by-one so migrations and state
//! upgrades apply without jumping errors.

use std::io::{self, Write};
use std::path::PathBuf;

use zunesha_core::update::{
    binary_path, install_prefix, install_release, load_record, newer_releases, parse_tags,
    remote_tags, update_source, Version,
};
use zunesha_core::{Error, Result};

pub use zunesha_core::update::latest_release;

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

/// Run the sequential update flow.
pub fn run(options: Options) -> Result<()> {
    let current_str = env!("CARGO_PKG_VERSION");
    let current = Version::parse(current_str).ok_or_else(|| {
        Error::Config(format!("installed version `{current_str}` is not valid semver"))
    })?;

    let prefix = install_prefix(options.prefix.as_deref())?;
    let record = load_record(&prefix);
    let (source, release_base) = update_source(options.repo.as_deref(), record.as_ref())?;

    let tags = remote_tags(&source)?;
    let newer = newer_releases(current, &tags);
    if newer.is_empty() {
        println!("✔ up to date (v{current})");
        return Ok(());
    }

    let latest_tag = &newer.last().unwrap().1;
    println!(
        "Update available: v{current} → {latest_tag} ({} step(s) in sequence)",
        newer.len()
    );
    for (idx, (_, tag)) in newer.iter().enumerate() {
        println!("  [{}/{}] {tag}", idx + 1, newer.len());
    }

    if options.check {
        println!("  run `zou update` to install sequentially");
        return Ok(());
    }

    if !options.yes && !confirm("Update sequentially now?")? {
        println!("update skipped");
        return Ok(());
    }

    for (idx, (_, tag)) in newer.iter().enumerate() {
        println!("==> Step {}/{}: updating to {tag}…", idx + 1, newer.len());
        install_release(
            &source,
            tag,
            &prefix,
            options.no_dashboard,
            release_base.as_deref(),
        )?;
        println!("✔ installed {tag}");
    }

    println!("✔ successfully updated step-by-step from v{current} to {latest_tag}");
    println!("  binary : {}", binary_path(&prefix).display());
    println!("  note   : restart `zou start` if it is running");
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
