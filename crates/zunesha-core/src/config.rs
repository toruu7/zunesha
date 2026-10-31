//! User-wide configuration for the `zou` CLI.
//!
//! It stores the active agent provider (OpenCode or Google Antigravity) and a
//! model per provider, so switching back restores the previous selection. It
//! lives outside any target repository so choosing a model once applies to
//! every project:
//!
//! ```text
//! ${XDG_CONFIG_HOME:-~/.config}/zou/config.json
//! ```
//!
//! `ZOU_CONFIG_FILE` overrides the path (useful for tests and for keeping an
//! isolated setup), and `ZOU_AGENT_CMD` still takes precedence over the saved
//! model — an explicit environment override always wins.

use std::collections::BTreeMap;
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
    /// Agent-related settings (`zou agent` / `zou model` write here).
    #[serde(default)]
    pub agent: AgentConfig,
}

/// Which coding-agent CLI Zunesha should launch.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentProvider {
    /// OpenCode (`opencode run --format json`).
    #[default]
    Opencode,
    /// Google Antigravity CLI (`agy -p --output-format stream-json`).
    Antigravity,
}

impl AgentProvider {
    /// Parse a user-facing provider name.
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "opencode" | "open-code" | "oc" => Ok(Self::Opencode),
            "antigravity" | "agy" | "google" => Ok(Self::Antigravity),
            other => Err(Error::Config(format!(
                "unknown agent provider `{other}`; expected `opencode` or `antigravity`"
            ))),
        }
    }

    /// Stable config/CLI label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Opencode => "opencode",
            Self::Antigravity => "antigravity",
        }
    }

    /// The CLI executable used by this provider.
    pub fn binary(self) -> &'static str {
        match self {
            Self::Opencode => "opencode",
            Self::Antigravity => "agy",
        }
    }

    /// Human-readable product name.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Opencode => "OpenCode",
            Self::Antigravity => "Google Antigravity",
        }
    }
}

/// Settings for the coding agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentConfig {
    /// Which agent CLI to launch. Defaults to OpenCode for backwards
    /// compatibility with configs written before this field existed.
    #[serde(default)]
    pub provider: AgentProvider,
    /// Model id for the *active* provider, passed via `--model`. OpenCode
    /// uses `provider/model`; Antigravity uses a slug from `agy models` (for
    /// example `gemini-3.8-flash-medium`).
    ///
    /// Kept as a top-level field so older Zunesha versions can still read the
    /// config. New versions also write `models`, which preserves a selection
    /// for every provider independently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Saved model per provider (`opencode` / `antigravity`). Switching
    /// providers restores the matching entry instead of discarding the other
    /// provider's choice.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, String>,
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

        // Validate the active model and migrate it into the per-provider map.
        // Older configs only had `agent.model`; writing the map on the next
        // save is what makes provider switches non-destructive.
        if let Some(model) = config.agent.model.as_mut() {
            *model = normalize_agent_model(config.agent.provider, model)?;
            config
                .agent
                .models
                .insert(config.agent.provider.as_str().to_string(), model.clone());
        }
        for (provider, model) in config.agent.models.iter_mut() {
            if let Ok(provider) = AgentProvider::parse(provider) {
                *model = normalize_agent_model(provider, model)?;
            }
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

        // Keep the legacy top-level `model` in sync with the map even for
        // callers that construct `AgentConfig` directly (tests, embedding).
        let mut config = self.clone();
        if let Some(model) = config.agent.model.clone() {
            config
                .agent
                .models
                .entry(config.agent.provider.as_str().to_string())
                .or_insert(model);
        }

        let mut json = serde_json::to_string_pretty(&config)
            .map_err(|e| Error::Config(format!("could not encode config: {e}")))?;
        json.push('\n');
        std::fs::write(path, json)
            .map_err(|e| Error::Config(format!("could not write {}: {e}", path.display())))
    }

    /// Save the selected model user-wide and return the config path.
    ///
    /// The provider is preserved, so switching between OpenCode and
    /// Antigravity does not lose the other provider's selection accidentally.
    /// A malformed/corrupt config is treated as empty, which keeps a direct
    /// `zou model <id>` save usable as a recovery path.
    pub fn set_model(model: &str) -> Result<PathBuf> {
        let path = Self::path()?;
        let mut config = Self::load_from(&path).unwrap_or_default();
        let model = normalize_agent_model(config.agent.provider, model)?;
        config.agent.model = Some(model.clone());
        config
            .agent
            .models
            .insert(config.agent.provider.as_str().to_string(), model);
        config.save_to(&path)?;
        Ok(path)
    }

    /// Select the agent provider user-wide.
    ///
    /// Each provider keeps its own model in `agent.models`; switching back
    /// restores the previous choice. A legacy config with only `agent.model`
    /// is migrated on load. If the active model is incompatible with the new
    /// provider and no saved model exists for it, the model is cleared (the
    /// caller can tell the user to run `zou model`).
    pub fn set_provider(provider: AgentProvider) -> Result<PathBuf> {
        let path = Self::path()?;
        let mut config = Self::load_from(&path).unwrap_or_default();
        config.select_provider(provider);
        config.save_to(&path)?;
        Ok(path)
    }

    /// Switch the in-memory provider, restoring its saved model if present.
    pub fn select_provider(&mut self, provider: AgentProvider) {
        self.agent.provider = provider;
        if let Some(model) = self.agent.models.get(provider.as_str()).cloned() {
            self.agent.model = Some(model);
            return;
        }
        // No saved entry yet: keep the current model only when it is valid for
        // the new provider, and record it so a later switch back is lossless.
        match self.agent.model.clone() {
            Some(model) if normalize_agent_model(provider, &model).is_ok() => {
                self.agent
                    .models
                    .insert(provider.as_str().to_string(), model);
            }
            _ => self.agent.model = None,
        }
    }

    /// Forget the active provider's saved model while keeping the provider
    /// and any other provider's selection.
    pub fn clear_model() -> Result<PathBuf> {
        let path = Self::path()?;
        if !path.is_file() {
            return Ok(path);
        }
        let mut config = Self::load_from(&path).unwrap_or_default();
        config.agent.model = None;
        config.agent.models.remove(config.agent.provider.as_str());
        config.save_to(&path)?;
        Ok(path)
    }

    /// The active provider's saved model id, if any.
    pub fn model(&self) -> Option<&str> {
        self.agent.model.as_deref()
    }

    /// The saved model for `provider`, even when it is not active.
    pub fn model_for(&self, provider: AgentProvider) -> Option<&str> {
        self.agent
            .models
            .get(provider.as_str())
            .map(String::as_str)
            .or_else(|| {
                (self.agent.provider == provider)
                    .then_some(self.agent.model.as_deref())
                    .flatten()
            })
    }

    /// The selected agent provider.
    pub fn provider(&self) -> AgentProvider {
        self.agent.provider
    }
}

/// Trim and validate an OpenCode model id, returning the stored form.
///
/// OpenCode ids are `provider/model` (optionally with a `#variant`). We only
/// reject values that cannot be a single argument or are obviously not a
/// model id; the authoritative list still comes from `opencode models`.
pub fn normalize_model(model: &str) -> Result<String> {
    normalize_agent_model(AgentProvider::Opencode, model)
}

/// Trim and validate a model id for a specific provider.
///
/// OpenCode requires `provider/model`; Antigravity uses plain slugs such as
/// `gemini-3.8-flash-medium`. Both reject whitespace/control characters so the
/// value is always safe as one argv element.
pub fn normalize_agent_model(provider: AgentProvider, model: &str) -> Result<String> {
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
    if provider == AgentProvider::Opencode && !model.contains('/') {
        return Err(Error::Config(format!(
            "invalid model id `{model}`: expected `provider/model` (see `opencode models`)"
        )));
    }
    if provider == AgentProvider::Antigravity && model.contains('/') {
        return Err(Error::Config(format!(
            "invalid model id `{model}`: expected a plain slug (see `agy models`)"
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
                provider: AgentProvider::Opencode,
                model: Some("opencode/deepseek-v4.1-flash-free".into()),
                models: Default::default(),
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
    fn antigravity_provider_and_slug_round_trip() {
        let (_tmp, path) = temp_config();
        let config = Config {
            agent: AgentConfig {
                provider: AgentProvider::Antigravity,
                model: Some("gemini-3.8-flash-medium".into()),
                models: Default::default(),
            },
        };
        config.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.provider(), AgentProvider::Antigravity);
        assert_eq!(loaded.model(), Some("gemini-3.8-flash-medium"));
    }

    #[test]
    fn switching_providers_preserves_each_saved_model() {
        let mut config = Config::default();
        config.agent.provider = AgentProvider::Opencode;
        config.agent.model = Some("opencode/deepseek-v4.1-flash-free".into());
        config.agent.models.insert(
            "opencode".into(),
            "opencode/deepseek-v4.1-flash-free".into(),
        );

        config.select_provider(AgentProvider::Antigravity);
        assert_eq!(config.model(), None);
        config.agent.model = Some("gemini-3.8-flash-medium".into());
        config
            .agent
            .models
            .insert("antigravity".into(), "gemini-3.8-flash-medium".into());

        config.select_provider(AgentProvider::Opencode);
        assert_eq!(config.model(), Some("opencode/deepseek-v4.1-flash-free"));
        assert_eq!(
            config.model_for(AgentProvider::Antigravity),
            Some("gemini-3.8-flash-medium")
        );
    }

    #[test]
    fn old_config_without_provider_defaults_to_opencode() {
        let (_tmp, path) = temp_config();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"agent":{"model":"opencode/big-pickle"}}"#).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.provider(), AgentProvider::Opencode);
        assert_eq!(loaded.model(), Some("opencode/big-pickle"));
        assert_eq!(
            loaded.model_for(AgentProvider::Opencode),
            Some("opencode/big-pickle"),
            "legacy model is migrated into the per-provider map"
        );
    }

    #[test]
    fn provider_specific_model_validation() {
        assert_eq!(
            normalize_agent_model(AgentProvider::Antigravity, " gemini-3.8-flash-medium ").unwrap(),
            "gemini-3.8-flash-medium"
        );
        assert!(normalize_agent_model(AgentProvider::Antigravity, "has space").is_err());
        assert!(normalize_agent_model(AgentProvider::Antigravity, "opencode/model").is_err());
        assert!(normalize_agent_model(AgentProvider::Opencode, "no-provider").is_err());
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
