//! Child-process environment hygiene.
//!
//! Zunesha is often launched from inside another coding agent (for example an
//! OpenCode terminal session). A child CLI that inherits that parent session's
//! marker variables may decide it is already attached to the *parent* project
//! and operate there, defeating workspace isolation. The same applies to a
//! planner or validation command that shells out to an agent.
//!
//! [`prepare`] pins the child's working directory and `PWD`/`INIT_CWD`, and
//! removes session markers that only describe the caller's agent session.
//! Credentials and provider configuration are intentionally left alone.

use std::ffi::OsStr;
use std::path::Path;

/// The small slice of `std`/`tokio` command APIs this module needs.
pub(crate) trait CommandEnv {
    fn set_working_dir(&mut self, dir: &Path);
    fn set_env(&mut self, key: &str, value: &OsStr);
    fn remove_env(&mut self, key: &OsStr);
}

impl CommandEnv for std::process::Command {
    fn set_working_dir(&mut self, dir: &Path) {
        self.current_dir(dir);
    }

    fn set_env(&mut self, key: &str, value: &OsStr) {
        self.env(key, value);
    }

    fn remove_env(&mut self, key: &OsStr) {
        self.env_remove(key);
    }
}

impl CommandEnv for tokio::process::Command {
    fn set_working_dir(&mut self, dir: &Path) {
        self.current_dir(dir);
    }

    fn set_env(&mut self, key: &str, value: &OsStr) {
        self.env(key, value);
    }

    fn remove_env(&mut self, key: &OsStr) {
        self.env_remove(key);
    }
}

/// Configure a child command to run in `dir` without inheriting a parent
/// agent session's project identity.
pub(crate) fn prepare<C: CommandEnv>(command: &mut C, dir: &Path) {
    command.set_working_dir(dir);

    // `current_dir` changes the OS cwd but leaves the inherited `PWD` alone;
    // Node-based CLIs often consult `PWD`/`INIT_CWD` when choosing a project.
    if let Ok(absolute) = dir.canonicalize() {
        command.set_env("PWD", absolute.as_os_str());
        command.set_env("INIT_CWD", absolute.as_os_str());
    }
    command.remove_env(OsStr::new("OLDPWD"));

    for (name, _) in std::env::vars_os() {
        if is_parent_agent_session(&name) {
            command.remove_env(&name);
        }
    }
}

/// Session markers set by common agent CLIs. Keep this list narrow: a child
/// agent still needs its own provider keys and user configuration.
fn is_parent_agent_session(name: &OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_uppercase();
    name.starts_with("OPENCODE") || name == "CLAUDECODE" || name.starts_with("CLAUDE_CODE_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_parent_agent_session_markers() {
        for marker in [
            "OPENCODE",
            "OPENCODE_TERMINAL",
            "OPENCODE_SESSION_ID",
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
        ] {
            assert!(is_parent_agent_session(OsStr::new(marker)), "{marker}");
        }
        for safe in ["PATH", "HOME", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"] {
            assert!(!is_parent_agent_session(OsStr::new(safe)), "{safe}");
        }
    }

    #[test]
    fn prepare_sets_pwd_and_drops_parent_session_markers() {
        let dir = tempfile::tempdir().unwrap();
        // Unique test marker: proves the filter removes arbitrary OPENCODE*
        // names rather than a hard-coded list.
        std::env::set_var("OPENCODE_ZOU_TEST_LEAK", "parent");
        std::env::set_var("CLAUDECODE", "1");

        let mut command = std::process::Command::new("sh");
        command.args([
            "-c",
            "printf '%s\\n' \"$PWD\"; \
             printf '%s\\n' \"${OPENCODE_ZOU_TEST_LEAK-unset}\"; \
             printf '%s\\n' \"${CLAUDECODE-unset}\"",
        ]);
        prepare(&mut command, dir.path());
        let out = command.output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);

        let expected = dir.path().canonicalize().unwrap();
        assert!(
            text.lines().next() == expected.to_str(),
            "PWD should be the child directory: {text:?}"
        );
        assert!(
            text.contains("unset"),
            "parent session marker leaked: {text:?}"
        );

        std::env::remove_var("OPENCODE_ZOU_TEST_LEAK");
        std::env::remove_var("CLAUDECODE");
    }
}
