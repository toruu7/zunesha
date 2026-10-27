//! User-wide configuration for the `zou` CLI.
//!
//! The only setting today is the OpenCode model used by the default agent
//! command. It lives outside any target repository so choosing a model once
//! applies to every project:
//!
//! ```text
//! ${XDG_CONFIG_HOME:-~/.config}/zou/config.json
//! ```
//!
//! `ZOU_CONFIG_FILE` overrides the path (useful for tests and for keeping an
//! isolated setup), and `ZOU_AGENT_CMD` still takes precedence over the saved
//! model — an explicit environment override always wins.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Directory name below the user's config root.
pub const CONFIG_DIR: &str = "zou";

/// Config file name below [`CONFIG_DIR`].
pub const CONFIG_FILE: &str = "config.json";

/// Environment variable that overrides the config file path.
pub const CONFIG_PATH_ENV: &str = "ZOU_CONFIG_FILE";

/// Model ids are passed as a single argv element, but keeping them to a
/// conservative character set prevents accidental whitespace/newline values
/// from making a saved setting unusable or confusing.
const MAX_MODEL_LEN: usize = 256;

/// User-wide Zunesha settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    /// Agent-related settings (`zou model` writes here).
    #[serde(default)]
    pub agent: AgentConfig,
}

/// Settings for the coding agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentConfig {
    /// OpenCode model id (`provider/model`) passed via `--model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Config {
    /// Resolve the config file path.
    ///
    /// Resolution order:
    /// 1. `ZOU_CONFIG_FILE` when non-empty (explicit path).
    /// 2. `$XDG_CONFIG_HOME/zou/config.json` when `XDG_CONFIG_HOME` is set.
    /// 3. `$HOME/.config/zou/config.json`.
    /// 4. `%USERPROFILE%\.config\zou\config.json` on Windows-like shells where
    ///    only `USERPROFILE` is set.
    pub fn path() -> Result<PathBuf> {
        if let Some(path) = std::env::var_os(CONFIG_PATH_ENV) {
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }

        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".config"))
            })
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".config"))
            })
            .ok_or_else(|| {
                Error::Config(format!(
                    "could not locate a config directory; set HOME or XDG_CONFIG_HOME \
                     (or {CONFIG_PATH_ENV})"
                ))
            })?;

        Ok(base.join(CONFIG_DIR).join(CONFIG_FILE))
    }

    /// Load the user-wide config. A missing file is an empty config.
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path()?)
    }

    /// Load a config from an explicit path (tests, embedding).
    ///
    /// A missing file is not an error: it means "no saved settings".
    /// Malformed JSON or an invalid model id is reported instead of being
    /// silently ignored.
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Ok(Self::default());
        }

        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("could not read {}: {e}", path.display())))?;
        let mut config: Self = serde_json::from_str(&text)
            .map_err(|e| Error::Config(format!("{} is not valid JSON: {e}", path.display())))?;

        if let Some(model) = config.agent.model.as_mut() {
            *model = normalize_model(model)?;
        }
        Ok(config)
    }

    /// Save this config to an explicit path, creating its parent directory.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    Error::Config(format!(
                        "could not create config directory {}: {e}",
                        parent.display()
                    ))
                })?;
            }
        }

        let mut json = serde_json::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("could not encode config: {e}")))?;
        json.push('\n');
        std::fs::write(path, json)
            .map_err(|e| Error::Config(format!("could not write {}: {e}", path.display())))
    }

    /// Save the selected model user-wide and return the config path.
    ///
    /// This intentionally starts from a fresh config: selecting a model is
    /// also the recovery path for a hand-edited/corrupt config file.
    pub fn set_model(model: &str) -> Result<PathBuf> {
        let model = normalize_model(model)?;
        let path = Self::path()?;
        let config = Self {
            agent: AgentConfig { model: Some(model) },
        };
        config.save_to(&path)?;
        Ok(path)
    }

    /// Remove the saved model file, returning the path that was cleared.
    ///
    /// The config currently contains only the model setting; removing the
    /// file is the clean way to forget it (and also repairs a corrupt file).
    pub fn clear_model() -> Result<PathBuf> {
        let path = Self::path()?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(path),
            Err(e) => Err(Error::Config(format!(
                "could not remove {}: {e}",
                path.display()
            ))),
        }
    }

    /// The saved model id, if any.
    pub fn model(&self) -> Option<&str> {
        self.agent.model.as_deref()
    }
}

/// Trim and validate an OpenCode model id, returning the stored form.
///
/// OpenCode ids are `provider/model` (optionally with a `#variant`). We only
/// reject values that cannot be a single argument or are obviously not a
/// model id; the authoritative list still comes from `opencode models`.
pub fn normalize_model(model: &str) -> Result<String> {
    let model = model.trim();
    if model.is_empty() {
        return Err(Error::Config("model id must not be empty".into()));
    }
    if model.len() > MAX_MODEL_LEN {
        return Err(Error::Config(format!(
            "model id is too long (maximum {MAX_MODEL_LEN} bytes)"
        )));
    }
    if model.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(Error::Config(format!(
            "invalid model id `{model}`: whitespace and control characters are not allowed"
        )));
    }
    if !model.contains('/') {
        return Err(Error::Config(format!(
            "invalid model id `{model}`: expected `provider/model` (see `opencode models`)"
        )));
    }
    Ok(model.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join(CONFIG_FILE);
        (tmp, path)
    }

    #[test]
    fn missing_file_loads_as_empty_config() {
        let (_tmp, path) = temp_config();
        let config = Config::load_from(&path).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.model(), None);
    }

    #[test]
    fn model_round_trips_and_creates_parent_directory() {
        let (_tmp, path) = temp_config();
        let config = Config {
            agent: AgentConfig {
                model: Some("opencode/deepseek-v4.1-flash-free".into()),
            },
        };
        config.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.model(), Some("opencode/deepseek-v4.1-flash-free"));

        let json = std::fs::read_to_string(&path).unwrap();
        assert!(json.contains("\"model\""));
        assert!(json.ends_with('\n'));
    }

    #[test]
    fn empty_or_malformed_model_is_rejected() {
        assert!(normalize_model("").is_err());
        assert!(normalize_model("   ").is_err());
        assert!(normalize_model("opencode/has space").is_err());
        assert!(normalize_model("opencode/has\nnewline").is_err());
        assert!(normalize_model("no-provider").is_err());
        assert_eq!(
            normalize_model(" opencode/big-pickle ").unwrap(),
            "opencode/big-pickle"
        );
    }

    #[test]
    fn malformed_json_is_an_explicit_error() {
        let (_tmp, path) = temp_config();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();

        let err = Config::load_from(&path).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("not valid JSON"));
    }

    #[test]
    fn invalid_saved_model_is_rejected_on_load() {
        let (_tmp, path) = temp_config();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"agent":{"model":"not-a-model"}}"#).unwrap();

        let err = Config::load_from(&path).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("provider/model"));
    }
}
