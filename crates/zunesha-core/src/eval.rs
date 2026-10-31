//! Project validation for the Phase 8 self-healing loop.
//!
//! When an agent exits 0, the task moves to `EVALUATING` and the project's
//! test/lint command runs inside the task's workspace. This module owns:
//!
//! * [`EvalCommand`] – *which* command to run (explicit via `ZOU_EVAL_CMD`,
//!   else auto-detected from the workspace's files).
//! * [`run_evaluation`] – spawning it, streaming its output into the worker's
//!   log channel (prefixed `[eval]`), and capturing a copy for the fix prompt.
//! * [`build_fix_description`] / [`strip_validation_section`] – rewriting the
//!   task description so the next agent attempt sees the failure, and cleaning
//!   it back up once the code passes.
//!
//! A failure sends the task back to `IN_PROGRESS` with the captured output
//! appended to its description; three failures park it in `FAILED_NEEDS_HUMAN`.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::control::RunControl;
use crate::worker::{split_command_line, StreamKind};

/// Default wall-clock limit for one evaluation run.
pub const DEFAULT_EVAL_TIMEOUT_SECS: u64 = 600;

/// Cap on the failure output appended to a task description (chars).
///
/// Test suites can print megabytes; the fix prompt only needs the tail where
/// compilers and test harnesses put their summaries.
const MAX_PROMPT_OUTPUT: usize = 8192;

/// Heading of the section [`build_fix_description`] manages.
const VALIDATION_HEADING: &str = "## Validation failure";

/// The validation command for a workspace.
///
/// Resolution order:
/// 1. `ZOU_EVAL_CMD` (parsed at pool construction, shell-style quoting) —
///    e.g. `ZOU_EVAL_CMD="sh -c 'cargo test && npm test'"`.
/// 2. Auto-detection from the workspace files ([`EvalCommand::detect`]).
///
/// With no explicit command and nothing detectable, evaluation is skipped and
/// the task goes straight to `REVIEW` — there is nothing to run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvalCommand {
    /// Parsed argv from `ZOU_EVAL_CMD`, when set. `{workspace}` may appear
    /// in any element; the subprocess always runs with the workspace as its
    /// working directory regardless.
    argv: Option<Vec<String>>,
}

impl EvalCommand {
    /// Read `ZOU_EVAL_CMD` (call once at pool construction, mirroring
    /// [`crate::worker::AgentCommand::default`]'s `ZOU_AGENT_CMD` handling).
    ///
    /// An unset or empty variable means auto-detect; an unparsable command
    /// line warns and falls back to auto-detect rather than silently running
    /// the wrong validator.
    pub fn from_env() -> Self {
        match std::env::var("ZOU_EVAL_CMD") {
            Ok(spec) if !spec.trim().is_empty() => match split_command_line(&spec) {
                Ok(argv) if !argv.is_empty() => Self { argv: Some(argv) },
                Ok(_) | Err(_) => {
                    eprintln!(
                        "warning: ignoring ZOU_EVAL_CMD (invalid command line); auto-detecting"
                    );
                    Self::default()
                }
            },
            _ => Self::default(),
        }
    }

    /// An explicit command, for tests and programmatic configuration.
    /// Falls back to auto-detect on an unparsable spec.
    pub fn fixed(spec: &str) -> Self {
        match split_command_line(spec) {
            Ok(argv) if !argv.is_empty() => Self { argv: Some(argv) },
            _ => Self::default(),
        }
    }

    /// Resolve the command to run for `workspace`.
    ///
    /// Returns `None` when there is nothing to evaluate. `{workspace}` is
    /// substituted into every argument.
    pub fn resolve(&self, workspace: &Path) -> Option<Vec<String>> {
        let argv = self.argv.clone().or_else(|| Self::detect(workspace))?;
        let ws = workspace.display().to_string();
        Some(
            argv.into_iter()
                .map(|a| a.replace("{workspace}", &ws))
                .collect(),
        )
    }

    /// Auto-detection: the first matching rule wins.
    ///
    /// * `package.json` with a `test` script → `npm test --silent`
    /// * `Cargo.toml` → `cargo test --quiet`
    /// * `Makefile` with a `test:` target → `make test`
    ///
    /// Mixed repos (both Rust and JS) should set `ZOU_EVAL_CMD` explicitly
    /// (e.g. `sh -c 'cargo test && npm test'`) — the detector runs exactly one
    /// command by design, to keep the validation step predictable.
    pub fn detect(workspace: &Path) -> Option<Vec<String>> {
        // npm: only when a test script actually exists (`npm test` fails
        // without one, which would fail every task for the wrong reason).
        let pkg = workspace.join("package.json");
        if pkg.is_file() {
            if let Ok(text) = std::fs::read_to_string(&pkg) {
                let has_test = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| v.get("scripts").cloned())
                    .is_some_and(|scripts| scripts.get("test").is_some());
                if has_test {
                    return Some(vec!["npm".into(), "test".into(), "--silent".into()]);
                }
            }
        }
        // Rust: a manifest means at least `cargo check`-level validation;
        // `cargo test` compiles and runs the tests.
        if workspace.join("Cargo.toml").is_file() {
            return Some(vec!["cargo".into(), "test".into(), "--quiet".into()]);
        }
        // Make: a `test:` target is the project's stated entry point.
        let makefile = workspace.join("Makefile");
        if makefile.is_file() {
            if let Ok(text) = std::fs::read_to_string(&makefile) {
                let has_test_target = text.lines().any(|line| line.starts_with("test:"));
                if has_test_target {
                    return Some(vec!["make".into(), "test".into()]);
                }
            }
        }
        None
    }
}

/// Wall-clock limit for one evaluation, from `ZOU_EVAL_TIMEOUT_SECS`
/// (default [`DEFAULT_EVAL_TIMEOUT_SECS`]).
pub fn timeout_from_env() -> Duration {
    let secs = std::env::var("ZOU_EVAL_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_EVAL_TIMEOUT_SECS);
    Duration::from_secs(secs.max(1))
}

/// Result of one evaluation run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalOutcome {
    pub success: bool,
    /// Combined stdout+stderr, tail-capped; empty on a clean pass.
    pub output: String,
    /// True when a human cancelled the run while validation was executing.
    /// The worker treats this as an interruption, not a failed validation, so
    /// it does not consume the self-healing retry budget.
    pub cancelled: bool,
}

/// Run the validation command inside the workspace.
///
/// Output is streamed into `tx` (prefixed `[eval]`) so the UI shows the test
/// run live, and captured raw for [`build_fix_description`]. A spawn error or
/// a timeout is a *failure outcome*, not an error: the loop must count it as a
/// failed validation, not crash the worker.
pub async fn run_evaluation(
    argv: &[String],
    workspace: &Path,
    timeout: Duration,
    tx: &mpsc::UnboundedSender<(StreamKind, String)>,
) -> EvalOutcome {
    run_evaluation_with_control(argv, workspace, timeout, tx, None).await
}

/// Like [`run_evaluation`], but also stops the validation process tree when
/// `control` is cancelled. The worker uses this so cancelling a task while its
/// tests are running does not wait for the eval timeout.
pub async fn run_evaluation_with_control(
    argv: &[String],
    workspace: &Path,
    timeout: Duration,
    tx: &mpsc::UnboundedSender<(StreamKind, String)>,
    control: Option<&RunControl>,
) -> EvalOutcome {
    let Some((program, args)) = argv.split_first() else {
        return EvalOutcome {
            success: false,
            output: "evaluation command is empty".to_string(),
            cancelled: false,
        };
    };

    let mut command = Command::new(program);
    command
        .args(args)
        .env("CI", "1")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    crate::child_env::prepare(&mut command, workspace);
    // The test command often spawns its own children (a shell, a test runner,
    // a watcher). Put it in its own process group so a timeout can kill the
    // whole tree instead of leaving a grandchild holding the pipe open.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            return EvalOutcome {
                success: false,
                output: format!("failed to run test command `{program}`: {e}"),
                cancelled: false,
            }
        }
    };

    let captured: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let out_reader: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>> =
        child.stdout.take().map(|r| Box::new(r) as _);
    let err_reader: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>> =
        child.stderr.take().map(|r| Box::new(r) as _);
    let mut readers = Vec::new();
    for (stream, reader) in [
        (StreamKind::Stdout, out_reader),
        (StreamKind::Stderr, err_reader),
    ] {
        let Some(reader) = reader else { continue };
        let tx = tx.clone();
        let captured = captured.clone();
        readers.push(tokio::spawn(read_eval_lines(reader, stream, tx, captured)));
    }

    let mut timed_out = false;
    let mut cancelled = false;
    let mut wait_error = None;
    // Box the future so it can be dropped before `terminate_tree` needs the
    // mutable borrow of `child` again.
    let mut wait = Box::pin(child.wait());
    let status = tokio::select! {
        _ = wait_for_cancel(control) => {
            cancelled = true;
            None
        }
        result = tokio::time::timeout(timeout, &mut wait) => match result {
            Ok(Ok(status)) => Some(status),
            Ok(Err(error)) => {
                wait_error = Some(error);
                None
            }
            Err(_) => {
                timed_out = true;
                None
            }
        },
    };
    drop(wait);
    if cancelled || timed_out || wait_error.is_some() {
        if cancelled {
            let _ = tx.send((
                StreamKind::Stderr,
                "[zou] run cancelled by user; stopping the validation process tree".to_string(),
            ));
        }
        terminate_tree(&mut child).await;
    }
    if let Some(error) = wait_error {
        return EvalOutcome {
            success: false,
            output: format!("failed to wait on test command: {error}"),
            cancelled: false,
        };
    }
    for reader in readers {
        let _ = reader.await;
    }

    let passed = !timed_out && !cancelled && status.is_some_and(|s| s.success());
    let output = if passed {
        // Nothing to report on a clean run: the fix prompt only ever sees
        // failures.
        String::new()
    } else {
        let mut guard = captured.lock().expect("eval capture poisoned");
        let mut text = std::mem::take(&mut *guard);
        if cancelled {
            let tail = tail_of(&text, MAX_PROMPT_OUTPUT);
            text = format!(
                "evaluation was cancelled by a human and its process tree was stopped. \
                 Output so far:\n{tail}"
            );
        } else if timed_out {
            let tail = tail_of(&text, MAX_PROMPT_OUTPUT);
            text = format!(
                "evaluation timed out after {}s and was killed. Output so far:\n{tail}",
                timeout.as_secs()
            );
        }
        text
    };

    EvalOutcome {
        success: passed,
        output,
        cancelled,
    }
}

async fn wait_for_cancel(control: Option<&RunControl>) {
    match control {
        Some(control) => control.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

/// Kill an evaluation command and its descendants.
async fn terminate_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let group = format!("-{pid}");
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &group])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &group])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// Read one eval stream: forward `[eval]`-prefixed lines to the UI channel,
/// and append the raw line to the shared capture buffer.
async fn read_eval_lines<R>(
    reader: R,
    stream: StreamKind,
    tx: mpsc::UnboundedSender<(StreamKind, String)>,
    captured: Arc<Mutex<String>>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Ok(mut guard) = captured.lock() {
            guard.push_str(&line);
            guard.push('\n');
        }
        if tx.send((stream, format!("[eval] {line}"))).is_err() {
            break; // collector gone (run ending)
        }
    }
}

/// Last `max` chars of `text` (with a truncation marker when cut).
fn tail_of(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().skip(text.chars().count() - max).collect();
    format!("[…truncated…]\n{cut}")
}

/// Rewrite the description for the next agent attempt (Phase 8).
///
/// Replaces any previous `## Validation failure` section (so only the latest
/// failure is in the prompt) and appends the captured output with the
/// blueprint's wording. The output is tail-capped: test suites can be huge and
/// the summary at the end is what the agent needs.
pub fn build_fix_description(current: &str, output: &str) -> String {
    let base = strip_validation_section(current);
    let output = tail_of(output.trim(), MAX_PROMPT_OUTPUT);
    let mut next = base.trim_end().to_string();
    if !next.is_empty() {
        next.push_str("\n\n");
    }
    next.push_str(VALIDATION_HEADING);
    next.push_str("\nThe code failed validation with these errors:\n\n```\n");
    next.push_str(&output);
    next.push_str("\n```\n\nPlease fix it.");
    next
}

/// Remove the `## Validation failure` section (everything from its heading to
/// the next `## ` heading or the end of the description).
///
/// Used on a successful validation so the reviewer sees the clean task text,
/// and before appending a fresh failure section.
pub fn strip_validation_section(description: &str) -> String {
    let Some(start) = description.find(VALIDATION_HEADING) else {
        return description.to_string();
    };
    // Walk past the heading to the next top-level section, if any.
    let after = &description[start + VALIDATION_HEADING.len()..];
    let rest = match after.find("\n## ") {
        Some(offset) => &after[offset + 1..],
        None => "",
    };
    let mut out = description[..start].trim_end().to_string();
    if !rest.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(rest);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(files: &[(&str, &str)]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for (name, content) in files {
            std::fs::write(tmp.path().join(name), content).unwrap();
        }
        tmp
    }

    #[test]
    fn detect_prefers_npm_then_cargo_then_make() {
        // package.json with a test script.
        let tmp = scratch(&[("package.json", r#"{"name":"x","scripts":{"test":"jest"}}"#)]);
        assert_eq!(
            EvalCommand::detect(tmp.path()).unwrap(),
            vec!["npm", "test", "--silent"]
        );

        // package.json WITHOUT a test script falls through to Cargo.toml.
        let tmp = scratch(&[
            ("package.json", r#"{"name":"x"}"#),
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ]);
        assert_eq!(
            EvalCommand::detect(tmp.path()).unwrap(),
            vec!["cargo", "test", "--quiet"]
        );

        // Makefile with a test target.
        let tmp = scratch(&[("Makefile", "build:\n\techo b\n\ntest:\n\tmake test\n")]);
        assert_eq!(
            EvalCommand::detect(tmp.path()).unwrap(),
            vec!["make", "test"]
        );

        // Nothing detectable.
        let tmp = scratch(&[("main.rs", "fn main() {}")]);
        assert!(EvalCommand::detect(tmp.path()).is_none());
    }

    #[test]
    fn fixed_spec_resolves_with_workspace_substitution() {
        let cmd = EvalCommand::fixed("sh -c 'cd {workspace} && ./check.sh'");
        let argv = cmd.resolve(Path::new("/tmp/ws-1")).unwrap();
        assert_eq!(argv, vec!["sh", "-c", "cd /tmp/ws-1 && ./check.sh"]);
    }

    #[tokio::test]
    async fn run_evaluation_reports_success_and_captures_nothing() {
        let tmp = scratch(&[]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let outcome = run_evaluation(
            &["sh".into(), "-c".into(), "echo noisy-but-ok".into()],
            tmp.path(),
            Duration::from_secs(5),
            &tx,
        )
        .await;
        assert!(outcome.success);
        assert!(outcome.output.is_empty());

        // The output still streamed to the UI, prefixed.
        drop(tx);
        let mut lines = Vec::new();
        while let Ok((_, line)) = rx.try_recv() {
            lines.push(line);
        }
        assert_eq!(lines, vec!["[eval] noisy-but-ok"]);
    }

    #[tokio::test]
    async fn run_evaluation_captures_failure_output_from_both_streams() {
        let tmp = scratch(&[]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let outcome = run_evaluation(
            &[
                "sh".into(),
                "-c".into(),
                "echo to-stdout; echo to-stderr >&2; exit 1".into(),
            ],
            tmp.path(),
            Duration::from_secs(5),
            &tx,
        )
        .await;
        assert!(!outcome.success);
        assert!(outcome.output.contains("to-stdout"), "{}", outcome.output);
        assert!(outcome.output.contains("to-stderr"), "{}", outcome.output);
    }

    #[tokio::test]
    async fn run_evaluation_treats_a_spawn_error_as_failure() {
        let tmp = scratch(&[]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let outcome = run_evaluation(
            &["definitely-not-a-real-binary-xyz".into()],
            tmp.path(),
            Duration::from_secs(5),
            &tx,
        )
        .await;
        assert!(!outcome.success);
        assert!(outcome.output.contains("failed to run test command"));
    }

    #[tokio::test]
    async fn run_evaluation_kills_a_hanging_command_at_the_timeout() {
        let tmp = scratch(&[]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let started = std::time::Instant::now();
        let outcome = run_evaluation(
            &["sh".into(), "-c".into(), "echo starting; sleep 30".into()],
            tmp.path(),
            Duration::from_millis(200),
            &tx,
        )
        .await;
        assert!(!outcome.success);
        assert!(outcome.output.contains("timed out"), "{}", outcome.output);
        assert!(outcome.output.contains("starting"), "partial output kept");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the sleep must be killed, not waited out"
        );
    }

    #[tokio::test]
    async fn run_evaluation_can_be_cancelled_and_kills_the_tree() {
        let tmp = scratch(&[]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let control = RunControl::new();
        let canceller = control.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            canceller.cancel();
        });

        let started = std::time::Instant::now();
        let outcome = run_evaluation_with_control(
            &["sh".into(), "-c".into(), "echo starting; sleep 30".into()],
            tmp.path(),
            Duration::from_secs(30),
            &tx,
            Some(&control),
        )
        .await;

        assert!(!outcome.success);
        assert!(outcome.cancelled);
        assert!(outcome.output.contains("cancelled"), "{}", outcome.output);
        assert!(outcome.output.contains("starting"), "partial output kept");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the sleep must be killed, not waited out"
        );
    }

    #[test]
    fn fix_description_appends_the_errors_with_blueprint_wording() {
        let out = build_fix_description("Original text", "assertion failed: x == 1");
        assert!(out.starts_with("Original text"));
        assert!(out.contains("The code failed validation with these errors:"));
        assert!(out.contains("assertion failed: x == 1"));
        assert!(out.contains("Please fix it."));
    }

    #[test]
    fn fix_description_replaces_the_previous_failure_section() {
        let once = build_fix_description("Text", "first error");
        let twice = build_fix_description(&once, "second error");
        assert!(twice.contains("second error"));
        assert!(
            !twice.contains("first error"),
            "only the latest failure stays in the prompt: {twice}"
        );
        assert_eq!(twice.matches("## Validation failure").count(), 1);
        assert!(twice.starts_with("Text"), "original text preserved");
    }

    #[test]
    fn strip_removes_the_failure_section_and_keeps_surrounding_text() {
        let desc = "Intro paragraph\n\n## Validation failure\nThe code failed…\n\nPlease fix it.\n\n## Reviewer feedback\nnits";
        let stripped = strip_validation_section(desc);
        assert!(stripped.starts_with("Intro paragraph"));
        assert!(stripped.ends_with("## Reviewer feedback\nnits"));
        assert!(!stripped.contains("Validation failure"));
        // A description without the section is unchanged.
        assert_eq!(strip_validation_section("plain"), "plain");
    }

    #[test]
    fn fix_description_caps_huge_output_keeping_the_tail() {
        let huge = "x".repeat(MAX_PROMPT_OUTPUT * 2);
        let out = build_fix_description("t", &huge);
        assert!(out.len() < MAX_PROMPT_OUTPUT + 500, "tail-capped");
        assert!(out.contains("[…truncated…]"));
    }
}
