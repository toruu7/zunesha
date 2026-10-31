//! Interactive terminal UI for a running Zunesha daemon.
//!
//! The TUI is deliberately a client, not a second orchestrator: it reads the
//! same REST snapshot and WebSocket stream as the browser dashboard, and sends
//! mutations through the same API. That keeps one owner for the worker pool and
//! SQLite state while giving the terminal a first-class control surface.
//!
//! The layout is intentionally conservative: a task list, a detail pane with
//! logs/output/diff/task tabs, a composer, and a status bar. Themes and keymaps
//! are configurable through `~/.config/zou/tui.toml` or `.zou/tui.toml`.

use std::collections::HashMap;
use std::future::Future;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use ratatui::{Frame, Terminal};
use serde::Deserialize;
use tokio::sync::mpsc;
use zunesha_api::events::{ApiEvent, ProjectInfo};
use zunesha_core::{AgentProvider, Config, Error, StreamKind, Task, TaskStatus};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const CONFIG_RELOAD_INTERVAL: Duration = Duration::from_secs(2);

/// Entry point for `zou tui`.
pub fn run(
    target: &Path,
    url: &str,
    start: bool,
    theme: Option<&str>,
    no_mouse: bool,
) -> zunesha_core::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Config(format!("failed to build TUI runtime: {e}")))?;
    runtime.block_on(run_async(target, url, start, theme, no_mouse))
}

async fn run_async(
    target: &Path,
    url: &str,
    start: bool,
    theme: Option<&str>,
    no_mouse: bool,
) -> zunesha_core::Result<()> {
    let api = ApiClient::new(url);

    if start && !api.health().await {
        start_daemon(target)?;
        let mut ready = false;
        for _ in 0..100 {
            if api.health().await {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !ready {
            return Err(Error::Config(format!(
                "daemon did not become ready at {url}; run `zou start` manually"
            )));
        }
    }

    let (config, config_path, config_error) = TuiConfig::load(target);
    let theme_override = theme.map(str::to_string);
    let theme = theme_override
        .as_deref()
        .and_then(Theme::builtin)
        .or_else(|| config.theme.as_deref().and_then(Theme::builtin))
        .unwrap_or_else(Theme::paper)
        .with_colors(&config.colors);

    let mouse = !no_mouse && config.mouse;
    let mut app = App::new(api.clone(), config, config_path, theme, theme_override);
    if let Some(error) = config_error {
        app.status = error;
    }

    match api.tasks().await {
        Ok(response) => app.apply_tasks(response),
        Err(error) => app.status = format!("daemon unavailable: {error}"),
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_ws(api, tx.clone());

    install_panic_hook();
    let mut terminal = TerminalGuard::enter(mouse)?;
    let mut events = EventStream::new();
    let result = event_loop(&mut terminal.terminal, &mut app, &mut events, &mut rx, tx).await;
    drop(terminal);
    result
}

fn start_daemon(target: &Path) -> zunesha_core::Result<()> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::Config(format!("cannot locate the zou binary: {e}")))?;
    std::process::Command::new(exe)
        .arg("--path")
        .arg(target)
        .arg("start")
        .arg("--no-resume")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::Config(format!("could not start the daemon: {e}")))?;
    Ok(())
}

fn install_panic_hook() {
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let mut out = io::stdout();
        let _ = execute!(
            out,
            LeaveAlternateScreen,
            DisableMouseCapture,
            crossterm::cursor::Show
        );
        old_hook(info);
    }));
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    mouse: bool,
}

impl TerminalGuard {
    fn enter(mouse: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        if mouse {
            execute!(stdout, EnableMouseCapture)?;
        }
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { terminal, mouse })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let out = self.terminal.backend_mut();
        let _ = execute!(out, LeaveAlternateScreen, DisableMouseCapture);
        let _ = self.terminal.show_cursor();
        let _ = self.mouse;
    }
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    events: &mut EventStream,
    rx: &mut mpsc::UnboundedReceiver<AppMsg>,
    tx: mpsc::UnboundedSender<AppMsg>,
) -> zunesha_core::Result<()> {
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    loop {
        terminal.draw(|frame| draw(frame, app))?;
        if app.should_quit {
            break;
        }

        tokio::select! {
            event = events.next() => {
                match event {
                    Some(Ok(event)) => app.handle_event(event, &tx),
                    Some(Err(error)) => {
                        app.status = format!("terminal error: {error}");
                        break;
                    }
                    None => break,
                }
            }
            message = rx.recv() => {
                if let Some(message) = message {
                    app.handle_msg(message, &tx);
                }
            }
            _ = ticker.tick() => {
                app.tick = app.tick.wrapping_add(1);
                app.maybe_reload_config();
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP + WebSocket client
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ApiClient {
    base: String,
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct TasksResponse {
    project: ProjectInfo,
    tasks: Vec<Task>,
}

impl ApiClient {
    fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn ws_url(&self) -> String {
        let base = self.base.trim_end_matches('/');
        if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{rest}/ws")
        } else if let Some(rest) = base.strip_prefix("http://") {
            format!("ws://{rest}/ws")
        } else {
            format!("ws://{base}/ws")
        }
    }

    async fn health(&self) -> bool {
        self.http
            .get(self.endpoint("/health"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    }

    async fn tasks(&self) -> Result<TasksResponse, String> {
        let response = self
            .http
            .get(self.endpoint("/api/tasks"))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        decode_json(response).await
    }

    async fn post(&self, path: &str, body: Option<serde_json::Value>) -> Result<String, String> {
        let mut request = self.http.post(self.endpoint(path));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        let status = response.status();
        let text = response.text().await.map_err(|e| e.to_string())?;
        if status.is_success() {
            Ok(text)
        } else {
            Err(format!("HTTP {status}: {}", text.trim()))
        }
    }

    async fn plan(&self, prompt: &str, attachments: &[PathBuf]) -> Result<String, String> {
        let uploads = attachment_payload(attachments)?;
        self.post(
            "/api/plan",
            Some(serde_json::json!({ "prompt": prompt, "attachments": uploads })),
        )
        .await
    }

    async fn run_ready(&self) -> Result<String, String> {
        self.post("/api/run", None).await
    }

    async fn run_one(&self, task_id: &str) -> Result<String, String> {
        self.post(&format!("/api/tasks/{task_id}/run"), None).await
    }

    async fn approve(&self, task_id: &str) -> Result<String, String> {
        self.post(&format!("/api/tasks/{task_id}/approve"), None)
            .await
    }

    async fn reject(&self, task_id: &str, feedback: &str) -> Result<String, String> {
        self.post(
            &format!("/api/tasks/{task_id}/reject"),
            Some(serde_json::json!({ "feedback": feedback })),
        )
        .await
    }

    async fn cancel(&self, task_id: &str) -> Result<String, String> {
        self.post(&format!("/api/tasks/{task_id}/cancel"), None)
            .await
    }

    async fn delete_task(&self, task_id: &str) -> Result<String, String> {
        self.delete(&format!("/api/tasks/{task_id}")).await
    }

    async fn delete(&self, path: &str) -> Result<String, String> {
        let response = self
            .http
            .delete(self.endpoint(path))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = response.status();
        let text = response.text().await.map_err(|e| e.to_string())?;
        if status.is_success() {
            Ok(text)
        } else {
            Err(format!("HTTP {status}: {}", text.trim()))
        }
    }

    async fn recover(&self, task_id: Option<&str>) -> Result<String, String> {
        let body = match task_id {
            Some(task_id) => serde_json::json!({ "task_id": task_id }),
            None => serde_json::json!({}),
        };
        self.post("/api/recover", Some(body)).await
    }
}

async fn decode_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, String> {
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {}", text.trim()));
    }
    serde_json::from_str(&text).map_err(|e| format!("invalid API response: {e}"))
}

fn spawn_ws(api: ApiClient, tx: mpsc::UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        loop {
            match tokio_tungstenite::connect_async(api.ws_url()).await {
                Ok((mut socket, _)) => {
                    let _ = tx.send(AppMsg::Ws(WsMsg::Connected));
                    while let Some(message) = socket.next().await {
                        match message {
                            Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                                if let Ok(event) = serde_json::from_str::<ApiEvent>(&text) {
                                    let _ = tx.send(AppMsg::Ws(WsMsg::Event(event)));
                                }
                            }
                            Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => break,
                            Ok(_) => {}
                            Err(error) => {
                                let _ = tx.send(AppMsg::Ws(WsMsg::Disconnected(error.to_string())));
                                break;
                            }
                        }
                    }
                    let _ = tx.send(AppMsg::Ws(WsMsg::Disconnected("socket closed".into())));
                }
                Err(error) => {
                    let _ = tx.send(AppMsg::Ws(WsMsg::Disconnected(error.to_string())));
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

enum AppMsg {
    Ws(WsMsg),
    Snapshot(Result<TasksResponse, String>),
    Action {
        label: String,
        result: Result<String, String>,
    },
    Models(Result<Vec<String>, String>),
}

enum WsMsg {
    Connected,
    Disconnected(String),
    Event(ApiEvent),
}

// ---------------------------------------------------------------------------
// Configuration, themes, and keymaps
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
struct TuiConfig {
    theme: Option<String>,
    mouse: bool,
    follow_logs: bool,
    layout: String,
    statusbar: Vec<String>,
    keys: HashMap<String, String>,
    colors: HashMap<String, String>,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: None,
            mouse: true,
            follow_logs: true,
            layout: "auto".into(),
            statusbar: vec![
                "project".into(),
                "connection".into(),
                "running".into(),
                "review".into(),
                "theme".into(),
            ],
            keys: HashMap::new(),
            colors: HashMap::new(),
        }
    }
}

impl TuiConfig {
    fn load(target: &Path) -> (Self, Option<PathBuf>, Option<String>) {
        let mut path = config_path();
        let project_path = target.join(".zou").join("tui.toml");
        if project_path.is_file() {
            path = Some(project_path);
        }
        let Some(path) = path else {
            return (Self::default(), None, None);
        };
        if !path.is_file() {
            return (Self::default(), None, None);
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Self>(&text) {
                Ok(config) => (config, Some(path), None),
                Err(error) => (
                    Self::default(),
                    Some(path.clone()),
                    Some(format!("config {}: {error}", path.display())),
                ),
            },
            Err(error) => (
                Self::default(),
                Some(path.clone()),
                Some(format!("config {}: {error}", path.display())),
            ),
        }
    }

    fn mtime(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok())
    }
}

fn config_path() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(home).join("zou").join("tui.toml"));
    }
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("zou")
            .join("tui.toml")
    })
}

#[derive(Clone, Debug)]
struct Theme {
    name: String,
    bg: Color,
    fg: Color,
    muted: Color,
    accent: Color,
    signal: Color,
    success: Color,
    danger: Color,
    border: Color,
    highlight_bg: Color,
    highlight_fg: Color,
}

impl Theme {
    fn names() -> &'static [&'static str] {
        &["paper", "midnight", "solarized", "high-contrast", "mono"]
    }

    fn builtin(name: &str) -> Option<Self> {
        match name {
            "paper" => Some(Self {
                name: "paper".into(),
                bg: Color::Rgb(243, 235, 221),
                fg: Color::Rgb(21, 21, 21),
                muted: Color::Rgb(102, 96, 86),
                accent: Color::Rgb(214, 59, 46),
                signal: Color::Rgb(242, 193, 78),
                success: Color::Rgb(47, 125, 79),
                danger: Color::Rgb(179, 38, 30),
                border: Color::Rgb(21, 21, 21),
                highlight_bg: Color::Rgb(21, 21, 21),
                highlight_fg: Color::Rgb(243, 235, 221),
            }),
            "midnight" => Some(Self {
                name: "midnight".into(),
                bg: Color::Rgb(15, 18, 28),
                fg: Color::Rgb(226, 232, 240),
                muted: Color::Rgb(126, 139, 158),
                accent: Color::Rgb(96, 165, 250),
                signal: Color::Rgb(250, 204, 21),
                success: Color::Rgb(74, 222, 128),
                danger: Color::Rgb(248, 113, 113),
                border: Color::Rgb(71, 85, 105),
                highlight_bg: Color::Rgb(30, 64, 175),
                highlight_fg: Color::Rgb(239, 246, 255),
            }),
            "solarized" => Some(Self {
                name: "solarized".into(),
                bg: Color::Rgb(0, 43, 54),
                fg: Color::Rgb(147, 161, 161),
                muted: Color::Rgb(88, 110, 117),
                accent: Color::Rgb(38, 139, 210),
                signal: Color::Rgb(181, 137, 0),
                success: Color::Rgb(133, 153, 0),
                danger: Color::Rgb(220, 50, 47),
                border: Color::Rgb(88, 110, 117),
                highlight_bg: Color::Rgb(7, 54, 66),
                highlight_fg: Color::Rgb(253, 246, 227),
            }),
            "high-contrast" => Some(Self {
                name: "high-contrast".into(),
                bg: Color::Black,
                fg: Color::White,
                muted: Color::Gray,
                accent: Color::Yellow,
                signal: Color::Cyan,
                success: Color::Green,
                danger: Color::Red,
                border: Color::White,
                highlight_bg: Color::White,
                highlight_fg: Color::Black,
            }),
            "mono" => Some(Self {
                name: "mono".into(),
                bg: Color::Reset,
                fg: Color::White,
                muted: Color::Gray,
                accent: Color::White,
                signal: Color::Gray,
                success: Color::White,
                danger: Color::White,
                border: Color::Gray,
                highlight_bg: Color::Gray,
                highlight_fg: Color::Black,
            }),
            _ => None,
        }
    }

    fn with_colors(mut self, colors: &HashMap<String, String>) -> Self {
        for (key, value) in colors {
            let Some(color) = parse_color(value) else {
                continue;
            };
            match key.as_str() {
                "bg" => self.bg = color,
                "fg" => self.fg = color,
                "muted" => self.muted = color,
                "accent" => self.accent = color,
                "signal" => self.signal = color,
                "success" => self.success = color,
                "danger" => self.danger = color,
                "border" => self.border = color,
                "highlight_bg" => self.highlight_bg = color,
                "highlight_fg" => self.highlight_fg = color,
                _ => {}
            }
        }
        self
    }

    fn next(&self) -> Self {
        let names = Self::names();
        let index = names
            .iter()
            .position(|name| *name == self.name)
            .map(|index| (index + 1) % names.len())
            .unwrap_or(0);
        Self::builtin(names[index]).unwrap_or_else(Self::paper)
    }
}

impl Theme {
    fn paper() -> Self {
        Self::builtin("paper").expect("paper theme")
    }
}

fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim().trim_start_matches('#');
    if value.len() != 6 {
        return None;
    }
    let red = u8::from_str_radix(&value[0..2], 16).ok()?;
    let green = u8::from_str_radix(&value[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&value[4..6], 16).ok()?;
    Some(Color::Rgb(red, green, blue))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Quit,
    Help,
    Plan,
    Attach,
    RunAll,
    RunOne,
    Approve,
    Reject,
    Cancel,
    Recover,
    Delete,
    Refresh,
    Logs,
    Output,
    Diff,
    Task,
    Theme,
    Model,
    Filter,
    Command,
    Up,
    Down,
    PageUp,
    PageDown,
    Enter,
    Escape,
    Tab,
}

impl Action {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "quit" => Self::Quit,
            "help" => Self::Help,
            "plan" => Self::Plan,
            "attach" => Self::Attach,
            "run" => Self::RunAll,
            "run_one" => Self::RunOne,
            "approve" => Self::Approve,
            "reject" => Self::Reject,
            "cancel" => Self::Cancel,
            "recover" => Self::Recover,
            "delete" | "remove" => Self::Delete,
            "refresh" => Self::Refresh,
            "logs" => Self::Logs,
            "output" => Self::Output,
            "diff" => Self::Diff,
            "task" => Self::Task,
            "theme" => Self::Theme,
            "model" => Self::Model,
            "filter" => Self::Filter,
            "command" => Self::Command,
            "up" => Self::Up,
            "down" => Self::Down,
            "page_up" => Self::PageUp,
            "page_down" => Self::PageDown,
            "enter" => Self::Enter,
            "escape" => Self::Escape,
            "tab" => Self::Tab,
            _ => return None,
        })
    }
}

const DEFAULT_KEYS: &[(&str, &str)] = &[
    ("quit", "q"),
    ("help", "?"),
    ("plan", "p"),
    ("attach", "A"),
    ("run", "r"),
    ("run_one", "R"),
    ("approve", "a"),
    ("reject", "x"),
    ("cancel", "C"),
    ("recover", "c"),
    ("delete", "D"),
    ("refresh", "g"),
    ("logs", "l"),
    ("output", "o"),
    ("diff", "d"),
    ("task", "i"),
    ("theme", "t"),
    ("model", "m"),
    ("filter", "f"),
    ("command", ":"),
    ("up", "k"),
    ("down", "j"),
    ("page_up", "pageup"),
    ("page_down", "pagedown"),
    ("enter", "enter"),
    ("escape", "esc"),
    ("tab", "tab"),
];

fn build_keymap(config: &TuiConfig) -> (HashMap<String, Action>, Vec<String>) {
    let mut map = HashMap::new();
    let mut errors = Vec::new();
    for (name, spec) in DEFAULT_KEYS {
        if let (Some(action), Some(key)) = (Action::from_name(name), normalize_key_spec(spec)) {
            map.insert(key, action);
        }
    }
    map.entry("delete".into()).or_insert(Action::Delete);
    for (name, spec) in &config.keys {
        let Some(action) = Action::from_name(name) else {
            errors.push(format!("unknown key action `{name}`"));
            continue;
        };
        let Some(key) = normalize_key_spec(spec) else {
            errors.push(format!("invalid key `{spec}` for `{name}`"));
            continue;
        };
        map.insert(key, action);
    }
    (map, errors)
}

fn normalize_key_spec(spec: &str) -> Option<String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let (mods, key) = match spec.rsplit_once('+') {
        Some((mods, key)) => (mods, key),
        None => ("", spec),
    };
    if key.is_empty() {
        return None;
    }
    let key = if key.chars().count() == 1 {
        key.to_string()
    } else {
        key.to_ascii_lowercase()
    };
    let key = match key.as_str() {
        "escape" => "esc".into(),
        "return" => "enter".into(),
        "space" => " ".into(),
        "pgup" => "pageup".into(),
        "pgdn" => "pagedown".into(),
        "del" => "delete".into(),
        other => other.to_string(),
    };
    let mut prefix = String::new();
    for modifier in mods.split('+').filter(|part| !part.is_empty()) {
        match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => prefix.push_str("ctrl+"),
            "alt" | "meta" => prefix.push_str("alt+"),
            "shift" => {}
            _other => return None,
        }
    }
    Some(format!("{prefix}{key}"))
}

fn key_id(key: &KeyEvent) -> Option<String> {
    let mut prefix = String::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        prefix.push_str("ctrl+");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        prefix.push_str("alt+");
    }
    let name = match key.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => "backtab".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::Insert => "insert".into(),
        KeyCode::Null => return None,
        KeyCode::F(number) => format!("f{number}"),
        _ => return None,
    };
    Some(format!("{prefix}{name}"))
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct LogLine {
    seq: usize,
    stream: StreamKind,
    line: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetailTab {
    Logs,
    Output,
    Diff,
    Task,
}

impl DetailTab {
    fn index(self) -> usize {
        match self {
            Self::Logs => 0,
            Self::Output => 1,
            Self::Diff => 2,
            Self::Task => 3,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Logs => Self::Output,
            Self::Output => Self::Diff,
            Self::Diff => Self::Task,
            Self::Task => Self::Logs,
        }
    }
}

#[derive(Clone, Debug)]
enum InputMode {
    Plan(String),
    Attach(String),
    Reject(String),
    Filter(String),
    Command(String),
}

impl InputMode {
    fn title(&self) -> &'static str {
        match self {
            Self::Plan(_) => "plan a feature",
            Self::Attach(_) => "attach a file",
            Self::Reject(_) => "reject with feedback",
            Self::Filter(_) => "filter tasks",
            Self::Command(_) => "command",
        }
    }

    fn prompt(&self) -> &'static str {
        match self {
            Self::Plan(_) => "prompt",
            Self::Attach(_) => "path",
            Self::Reject(_) => "feedback",
            Self::Filter(_) => "text",
            Self::Command(_) => ":",
        }
    }

    fn text_mut(&mut self) -> &mut String {
        match self {
            Self::Plan(text)
            | Self::Attach(text)
            | Self::Reject(text)
            | Self::Filter(text)
            | Self::Command(text) => text,
        }
    }

    fn text(&self) -> &str {
        match self {
            Self::Plan(text)
            | Self::Attach(text)
            | Self::Reject(text)
            | Self::Filter(text)
            | Self::Command(text) => text,
        }
    }
}

#[derive(Clone, Debug)]
enum ConfirmAction {
    RunAll,
    RunOne(String),
    Approve(String),
    Cancel(String),
    Recover(Option<String>),
    Delete(String),
}

#[derive(Clone, Debug)]
struct ModelPicker {
    models: Vec<String>,
    current: Option<String>,
    selected: usize,
    loading: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Connection {
    Connected,
    Connecting,
    Disconnected,
}

struct App {
    api: ApiClient,
    project: Option<ProjectInfo>,
    tasks: Vec<Task>,
    logs: HashMap<String, Vec<LogLine>>,
    selected: usize,
    detail_tab: DetailTab,
    connection: Connection,
    status: String,
    attachments: Vec<PathBuf>,
    input: Option<InputMode>,
    confirm: Option<ConfirmAction>,
    model_picker: Option<ModelPicker>,
    help: bool,
    filter: String,
    theme: Theme,
    theme_override: Option<String>,
    config: TuiConfig,
    config_path: Option<PathBuf>,
    config_mtime: Option<SystemTime>,
    last_config_check: Instant,
    keys: HashMap<String, Action>,
    follow_logs: bool,
    log_scroll: u16,
    tick: u64,
    should_quit: bool,
}

impl App {
    fn new(
        api: ApiClient,
        config: TuiConfig,
        config_path: Option<PathBuf>,
        theme: Theme,
        theme_override: Option<String>,
    ) -> Self {
        let (keys, key_errors) = build_keymap(&config);
        let mut status = String::new();
        if !key_errors.is_empty() {
            status = format!("config: {}", key_errors.join("; "));
        }
        let config_mtime = config_path.as_deref().and_then(TuiConfig::mtime);
        let follow_logs = config.follow_logs;
        Self {
            api,
            project: None,
            tasks: Vec::new(),
            logs: HashMap::new(),
            selected: 0,
            detail_tab: DetailTab::Logs,
            connection: Connection::Connecting,
            status,
            attachments: Vec::new(),
            input: None,
            confirm: None,
            model_picker: None,
            help: false,
            filter: String::new(),
            theme,
            theme_override,
            config,
            config_path,
            config_mtime,
            last_config_check: Instant::now(),
            keys,
            follow_logs,
            log_scroll: 0,
            tick: 0,
            should_quit: false,
        }
    }

    fn apply_tasks(&mut self, response: TasksResponse) {
        self.project = Some(response.project);
        self.tasks = response.tasks;
        self.logs.clear();
        for task in &self.tasks {
            self.logs
                .insert(task.id.clone(), split_logs(&task.worker_logs));
        }
        self.clamp_selection();
    }

    fn apply_event(&mut self, event: ApiEvent) {
        match event {
            ApiEvent::Snapshot { project, tasks, .. } => {
                self.project = Some(project);
                self.tasks = tasks;
                self.logs.clear();
                for task in &self.tasks {
                    self.logs
                        .insert(task.id.clone(), split_logs(&task.worker_logs));
                }
                self.clamp_selection();
            }
            ApiEvent::TaskUpdated { task } => {
                if let Some(existing) = self.tasks.iter_mut().find(|item| item.id == task.id) {
                    *existing = task.clone();
                } else {
                    self.tasks.push(task.clone());
                }
                let persisted = split_logs(&task.worker_logs);
                let existing = self.logs.remove(&task.id).unwrap_or_default();
                self.logs.insert(task.id, merge_logs(existing, persisted));
                self.clamp_selection();
            }
            ApiEvent::Log {
                task_id,
                stream,
                line,
                seq,
            } => {
                let lines = self.logs.entry(task_id).or_default();
                let entry = LogLine { seq, stream, line };
                if seq < lines.len() {
                    lines[seq] = entry;
                } else {
                    while lines.len() < seq {
                        lines.push(LogLine {
                            seq: lines.len(),
                            stream: StreamKind::Stdout,
                            line: String::new(),
                        });
                    }
                    lines.push(entry);
                }
                if self.follow_logs {
                    self.log_scroll = u16::MAX;
                }
            }
        }
    }

    fn visible_tasks(&self) -> Vec<&Task> {
        let needle = self.filter.trim().to_lowercase();
        self.tasks
            .iter()
            .filter(|task| {
                needle.is_empty()
                    || task.id.to_lowercase().contains(&needle)
                    || task.title.to_lowercase().contains(&needle)
                    || status_label(task.status).to_lowercase().contains(&needle)
            })
            .collect()
    }

    fn selected_task(&self) -> Option<Task> {
        self.visible_tasks()
            .get(self.selected)
            .map(|task| (*task).clone())
    }

    fn clamp_selection(&mut self) {
        let count = self.visible_tasks().len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    fn running_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| is_active(task.status))
            .count()
    }

    fn count(&self, status: TaskStatus) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.status == status)
            .count()
    }

    fn maybe_reload_config(&mut self) {
        if self.last_config_check.elapsed() < CONFIG_RELOAD_INTERVAL {
            return;
        }
        self.last_config_check = Instant::now();
        let Some(path) = self.config_path.clone() else {
            return;
        };
        let mtime = TuiConfig::mtime(&path);
        if mtime == self.config_mtime {
            return;
        }
        self.config_mtime = mtime;
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                self.status = format!("config {}: {error}", path.display());
                return;
            }
        };
        let config: TuiConfig = match toml::from_str(&text) {
            Ok(config) => config,
            Err(error) => {
                self.status = format!("config {}: {error}", path.display());
                return;
            }
        };
        let (keys, errors) = build_keymap(&config);
        self.keys = keys;
        self.follow_logs = config.follow_logs;
        if let Some(name) = self.theme_override.as_deref() {
            if let Some(theme) = Theme::builtin(name) {
                self.theme = theme.with_colors(&config.colors);
            }
        } else if let Some(theme) = config.theme.as_deref().and_then(Theme::builtin) {
            self.theme = theme.with_colors(&config.colors);
        } else {
            self.theme = self.theme.clone().with_colors(&config.colors);
        }
        self.config = config;
        self.status = if errors.is_empty() {
            "config reloaded".into()
        } else {
            format!("config reloaded; {}", errors.join("; "))
        };
    }

    fn handle_event(&mut self, event: Event, tx: &mpsc::UnboundedSender<AppMsg>) {
        match event {
            Event::Key(key)
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
            {
                self.handle_key(key, tx);
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => self.scroll_logs(-3),
                MouseEventKind::ScrollDown => self.scroll_logs(3),
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent, tx: &mpsc::UnboundedSender<AppMsg>) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if self.help {
            self.help = false;
            return;
        }

        if self.confirm.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(action) = self.confirm.take() {
                        self.execute_confirm(action, tx);
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm = None;
                    self.status = "cancelled".into();
                }
                _ => {}
            }
            return;
        }

        if self.input.is_some() {
            self.handle_input_key(key, tx);
            return;
        }

        if self.model_picker.is_some() {
            self.handle_model_key(key);
            return;
        }

        let Some(action) = key_id(&key).and_then(|id| self.keys.get(&id).copied()) else {
            return;
        };

        match action {
            Action::Quit => self.should_quit = true,
            Action::Help => self.help = true,
            Action::Plan => {
                self.input = Some(InputMode::Plan(String::new()));
            }
            Action::Attach => {
                self.input = Some(InputMode::Attach(String::new()));
            }
            Action::RunAll => {
                self.confirm = Some(ConfirmAction::RunAll);
            }
            Action::RunOne => {
                if let Some(task) = self.selected_task() {
                    if task.status == TaskStatus::Todo {
                        self.confirm = Some(ConfirmAction::RunOne(task.id));
                    } else {
                        self.status = format!(
                            "{} is {}; only TODO tasks can run",
                            task.id,
                            status_label(task.status)
                        );
                    }
                }
            }
            Action::Approve => {
                if let Some(task) = self.selected_task() {
                    if task.status == TaskStatus::Review {
                        self.confirm = Some(ConfirmAction::Approve(task.id));
                    } else {
                        self.status = format!(
                            "{} is {}; only REVIEW tasks can be approved",
                            task.id,
                            status_label(task.status)
                        );
                    }
                }
            }
            Action::Reject => {
                if let Some(task) = self.selected_task() {
                    if matches!(
                        task.status,
                        TaskStatus::Review | TaskStatus::FailedNeedsHuman
                    ) {
                        self.input = Some(InputMode::Reject(String::new()));
                    } else {
                        self.status = format!(
                            "{} is {}; only REVIEW/FAILED tasks can be rejected",
                            task.id,
                            status_label(task.status)
                        );
                    }
                }
            }
            Action::Cancel => {
                if let Some(task) = self.selected_task() {
                    if is_active(task.status) {
                        self.confirm = Some(ConfirmAction::Cancel(task.id));
                    } else {
                        self.status = format!(
                            "{} is {}; only running tasks can be cancelled (press D or :delete to remove)",
                            task.id,
                            status_label(task.status)
                        );
                    }
                }
            }
            Action::Delete => {
                if let Some(task) = self.selected_task() {
                    self.confirm = Some(ConfirmAction::Delete(task.id));
                }
            }
            Action::Recover => {
                let task_id = self.selected_task().map(|task| task.id);
                self.confirm = Some(ConfirmAction::Recover(task_id));
            }
            Action::Refresh => self.spawn_refresh(tx),
            Action::Logs => self.detail_tab = DetailTab::Logs,
            Action::Output => self.detail_tab = DetailTab::Output,
            Action::Diff => self.detail_tab = DetailTab::Diff,
            Action::Task => self.detail_tab = DetailTab::Task,
            Action::Theme => {
                self.theme = self.theme.next().with_colors(&self.config.colors);
                self.status = format!("theme: {}", self.theme.name);
            }
            Action::Model => self.open_model_picker(tx),
            Action::Filter => {
                self.input = Some(InputMode::Filter(self.filter.clone()));
            }
            Action::Command => {
                self.input = Some(InputMode::Command(String::new()));
            }
            Action::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.follow_logs = true;
            }
            Action::Down => {
                let count = self.visible_tasks().len();
                if self.selected + 1 < count {
                    self.selected += 1;
                }
                self.follow_logs = true;
            }
            Action::PageUp => self.scroll_logs(-10),
            Action::PageDown => self.scroll_logs(10),
            Action::Enter => {
                if self.selected_task().is_some() {
                    self.detail_tab = DetailTab::Logs;
                    self.follow_logs = true;
                }
            }
            Action::Escape => {
                self.filter.clear();
                self.clamp_selection();
                self.follow_logs = true;
            }
            Action::Tab => self.detail_tab = self.detail_tab.next(),
        }
    }

    fn open_model_picker(&mut self, tx: &mpsc::UnboundedSender<AppMsg>) {
        let current = Config::load()
            .ok()
            .and_then(|config| config.model().map(str::to_string));
        self.model_picker = Some(ModelPicker {
            models: Vec::new(),
            current,
            selected: 0,
            loading: true,
        });
        self.status = "loading models…".into();
        spawn_models(tx);
    }

    fn handle_model_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.model_picker = None;
                self.status = "model selection cancelled".into();
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(picker) = self.model_picker.as_mut() {
                    picker.selected = picker.selected.saturating_sub(1);
                }
                return;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(picker) = self.model_picker.as_mut() {
                    if picker.selected + 1 < picker.models.len() {
                        picker.selected += 1;
                    }
                }
                return;
            }
            KeyCode::Enter => {}
            _ => return,
        }

        let Some(model) = self
            .model_picker
            .as_ref()
            .and_then(|picker| picker.models.get(picker.selected).cloned())
        else {
            return;
        };
        match Config::set_model(&model) {
            Ok(path) => {
                self.status = format!(
                    "saved model {model}; restart `zou start` to use it ({})",
                    path.display()
                );
                self.model_picker = None;
            }
            Err(error) => self.status = format!("model: {error}"),
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent, tx: &mpsc::UnboundedSender<AppMsg>) {
        match key.code {
            KeyCode::Esc => {
                self.input = None;
            }
            KeyCode::Enter => {
                if let Some(mode) = self.input.take() {
                    self.submit_input(mode, tx);
                }
            }
            KeyCode::Backspace => {
                if let Some(mode) = self.input.as_mut() {
                    mode.text_mut().pop();
                }
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(mode) = self.input.as_mut() {
                    mode.text_mut().push(c);
                }
            }
            _ => {}
        }
    }

    fn submit_input(&mut self, mode: InputMode, tx: &mpsc::UnboundedSender<AppMsg>) {
        match mode {
            InputMode::Plan(text) => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    self.status = "plan cancelled: empty prompt".into();
                    return;
                }
                let api = self.api.clone();
                let attachments = self.attachments.clone();
                self.status = if attachments.is_empty() {
                    "planning…".into()
                } else {
                    format!("planning with {} attachment(s)…", attachments.len())
                };
                spawn_action(
                    tx,
                    "plan",
                    async move { api.plan(&text, &attachments).await },
                );
            }
            InputMode::Attach(text) => self.attach_path(&text),
            InputMode::Reject(feedback) => {
                let Some(task) = self.selected_task() else {
                    self.status = "no task selected".into();
                    return;
                };
                let api = self.api.clone();
                self.status = format!("rejecting {}…", task.id);
                spawn_action(tx, "reject", async move {
                    api.reject(&task.id, &feedback).await
                });
            }
            InputMode::Filter(text) => {
                self.filter = text;
                self.selected = 0;
                self.clamp_selection();
                self.status = if self.filter.is_empty() {
                    "filter cleared".into()
                } else {
                    format!("filter: {}", self.filter)
                };
            }
            InputMode::Command(command) => self.run_command(&command, tx),
        }
    }

    fn attach_path(&mut self, text: &str) {
        let path = PathBuf::from(text.trim());
        if text.trim().is_empty() {
            self.status = "attach cancelled: empty path".into();
        } else if !path.is_file() {
            self.status = format!("not a file: {}", path.display());
        } else if self.attachments.len() >= zunesha_core::MAX_ATTACHMENTS {
            self.status = format!(
                "attachment limit reached (maximum {})",
                zunesha_core::MAX_ATTACHMENTS
            );
        } else if std::fs::metadata(&path)
            .is_ok_and(|meta| meta.len() > zunesha_core::MAX_ATTACHMENT_BYTES as u64)
        {
            self.status = format!(
                "attachment too large: {} (maximum {} bytes)",
                path.display(),
                zunesha_core::MAX_ATTACHMENT_BYTES
            );
        } else {
            self.attachments.push(path.clone());
            self.status = format!(
                "attached {} ({} total)",
                path.display(),
                self.attachments.len()
            );
        }
    }

    fn run_command(&mut self, command: &str, tx: &mpsc::UnboundedSender<AppMsg>) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        let (name, argument) = match command.split_once(char::is_whitespace) {
            Some((name, argument)) => (name, argument.trim()),
            None => (command, ""),
        };
        match name {
            "q" | "quit" => self.should_quit = true,
            "help" | "?" => self.help = true,
            "plan" => self.input = Some(InputMode::Plan(argument.to_string())),
            "attach" => {
                if argument.is_empty() {
                    self.input = Some(InputMode::Attach(String::new()));
                } else {
                    self.attach_path(argument);
                }
            }
            "attachments" => {
                self.status = if self.attachments.is_empty() {
                    "no attachments".into()
                } else {
                    format!(
                        "attachments: {}",
                        self.attachments
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
            }
            "clear-attachments" | "clear_attachments" => {
                self.attachments.clear();
                self.status = "attachments cleared".into();
            }
            "run" => self.confirm = Some(ConfirmAction::RunAll),
            "run-one" | "run_one" => {
                if let Some(task) = self.selected_task() {
                    self.confirm = Some(ConfirmAction::RunOne(task.id));
                }
            }
            "approve" => {
                if let Some(task) = self.selected_task() {
                    self.confirm = Some(ConfirmAction::Approve(task.id));
                }
            }
            "reject" => {
                self.input = Some(InputMode::Reject(argument.to_string()));
            }
            "cancel" => {
                if let Some(task) = self.selected_task() {
                    self.confirm = Some(ConfirmAction::Cancel(task.id));
                }
            }
            "delete" | "remove" | "rm" => {
                let target_id = if argument.is_empty() {
                    self.selected_task().map(|task| task.id)
                } else {
                    Some(argument.to_string())
                };
                if let Some(task_id) = target_id {
                    self.confirm = Some(ConfirmAction::Delete(task_id));
                } else {
                    self.status = "no task selected to delete".into();
                }
            }
            "recover" => {
                let task_id = self.selected_task().map(|task| task.id);
                self.confirm = Some(ConfirmAction::Recover(task_id));
            }
            "refresh" => self.spawn_refresh(tx),
            "filter" => {
                self.filter = argument.to_string();
                self.selected = 0;
                self.clamp_selection();
            }
            "logs" => self.detail_tab = DetailTab::Logs,
            "output" => self.detail_tab = DetailTab::Output,
            "diff" => self.detail_tab = DetailTab::Diff,
            "task" => self.detail_tab = DetailTab::Task,
            "model" => self.open_model_picker(tx),
            "theme" => {
                if argument.is_empty() {
                    self.theme = self.theme.next().with_colors(&self.config.colors);
                } else if let Some(theme) = Theme::builtin(argument) {
                    self.theme = theme.with_colors(&self.config.colors);
                } else {
                    self.status = format!("unknown theme `{argument}`");
                    return;
                }
                self.status = format!("theme: {}", self.theme.name);
            }
            other => self.status = format!("unknown command `{other}`"),
        }
    }

    fn execute_confirm(&mut self, action: ConfirmAction, tx: &mpsc::UnboundedSender<AppMsg>) {
        match action {
            ConfirmAction::RunAll => {
                let api = self.api.clone();
                self.status = "starting ready tasks…".into();
                spawn_action(tx, "run", async move { api.run_ready().await });
            }
            ConfirmAction::RunOne(task_id) => {
                let api = self.api.clone();
                self.status = format!("starting {task_id}…");
                spawn_action(tx, "run", async move { api.run_one(&task_id).await });
            }
            ConfirmAction::Approve(task_id) => {
                let api = self.api.clone();
                self.status = format!("approving {task_id}…");
                spawn_action(tx, "approve", async move { api.approve(&task_id).await });
            }
            ConfirmAction::Cancel(task_id) => {
                let api = self.api.clone();
                self.status = format!("cancelling {task_id}…");
                spawn_action(tx, "cancel", async move { api.cancel(&task_id).await });
            }
            ConfirmAction::Recover(task_id) => {
                let api = self.api.clone();
                self.status = match &task_id {
                    Some(task_id) => format!("recovering {task_id}…"),
                    None => "recovering stuck tasks…".into(),
                };
                spawn_action(tx, "recover", async move {
                    api.recover(task_id.as_deref()).await
                });
            }
            ConfirmAction::Delete(task_id) => {
                let api = self.api.clone();
                self.status = format!("deleting {task_id}…");
                spawn_action(tx, "delete", async move { api.delete_task(&task_id).await });
            }
        }
    }

    fn spawn_refresh(&mut self, tx: &mpsc::UnboundedSender<AppMsg>) {
        let api = self.api.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(AppMsg::Snapshot(api.tasks().await));
        });
    }

    fn scroll_logs(&mut self, delta: i32) {
        if self.detail_tab != DetailTab::Logs {
            return;
        }
        self.follow_logs = false;
        let current = self.log_scroll as i32;
        self.log_scroll = (current + delta).max(0) as u16;
    }

    fn handle_msg(&mut self, message: AppMsg, tx: &mpsc::UnboundedSender<AppMsg>) {
        match message {
            AppMsg::Ws(WsMsg::Connected) => {
                self.connection = Connection::Connected;
                self.status = "connected".into();
            }
            AppMsg::Ws(WsMsg::Disconnected(error)) => {
                self.connection = Connection::Disconnected;
                self.status = format!("disconnected: {error}; retrying");
            }
            AppMsg::Ws(WsMsg::Event(event)) => {
                self.apply_event(event);
                if self.connection != Connection::Connected {
                    self.connection = Connection::Connected;
                }
            }
            AppMsg::Snapshot(Ok(response)) => {
                self.apply_tasks(response);
                self.status = "refreshed".into();
            }
            AppMsg::Snapshot(Err(error)) => {
                self.status = format!("refresh failed: {error}");
            }
            AppMsg::Action { label, result } => match result {
                Ok(_) => {
                    if label == "plan" {
                        self.attachments.clear();
                    }
                    self.status = format!("{label}: ok");
                    self.spawn_refresh(tx);
                }
                Err(error) => {
                    self.status = format!("{label}: {error}");
                }
            },
            AppMsg::Models(result) => match result {
                Ok(models) if models.is_empty() => {
                    self.status =
                        "model discovery returned no models; check the active agent's auth".into();
                    self.model_picker = None;
                }
                Ok(models) => {
                    let current = self
                        .model_picker
                        .as_ref()
                        .and_then(|picker| picker.current.clone());
                    let selected = current
                        .as_ref()
                        .and_then(|current| models.iter().position(|model| model == current))
                        .unwrap_or(0);
                    if let Some(picker) = self.model_picker.as_mut() {
                        picker.models = models;
                        picker.selected = selected;
                        picker.loading = false;
                    }
                    self.status = "select a model; enter saves, esc cancels".into();
                }
                Err(error) => {
                    self.status = format!("models: {error}");
                    self.model_picker = None;
                }
            },
        }
    }
}

fn spawn_action<F>(tx: &mpsc::UnboundedSender<AppMsg>, label: &'static str, future: F)
where
    F: Future<Output = Result<String, String>> + Send + 'static,
{
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = future.await;
        let _ = tx.send(AppMsg::Action {
            label: label.to_string(),
            result,
        });
    });
}

/// Read TUI attachments and base64-encode them for the REST plan request.
fn attachment_payload(paths: &[PathBuf]) -> Result<Vec<serde_json::Value>, String> {
    use base64::Engine as _;
    // Reuse the CLI reader so the TUI enforces the same count/size limits
    // before reading a huge file into memory.
    let uploads = crate::read_attachments(paths).map_err(|error| error.to_string())?;
    Ok(uploads
        .into_iter()
        .map(|upload| {
            serde_json::json!({
                "name": upload.name,
                "media_type": upload.media_type,
                "data": base64::engine::general_purpose::STANDARD.encode(upload.bytes),
            })
        })
        .collect())
}

fn spawn_models(tx: &mpsc::UnboundedSender<AppMsg>) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(|| -> Result<Vec<String>, String> {
            let provider = Config::load()
                .map(|config| config.provider())
                .unwrap_or(AgentProvider::Opencode);
            let binary = provider.binary();
            let output = std::process::Command::new(binary)
                .arg("models")
                .output()
                .map_err(|error| {
                    format!(
                        "could not run `{binary} models`: {error}. Install {binary} or use ZOU_AGENT_CMD."
                    )
                })?;
            if !output.status.success() {
                let detail = String::from_utf8_lossy(&output.stderr);
                let detail = detail.trim();
                let detail = if detail.is_empty() {
                    format!("exit status {}", output.status)
                } else {
                    detail.lines().next().unwrap_or(detail).to_string()
                };
                return Err(format!("`{binary} models` failed: {detail}"));
            }
            Ok(parse_models(
                provider,
                &String::from_utf8_lossy(&output.stdout),
            ))
        })
        .await
        .map_err(|error| format!("model discovery task failed: {error}"))
        .and_then(|result| result);
        let _ = tx.send(AppMsg::Models(result));
    });
}

fn parse_models(provider: AgentProvider, output: &str) -> Vec<String> {
    let mut models = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        let model = match provider {
            AgentProvider::Opencode => {
                if line.is_empty() || !line.contains('/') || line.chars().any(char::is_whitespace) {
                    continue;
                }
                line
            }
            AgentProvider::Antigravity => match line.split_whitespace().next() {
                Some(model) if !model.is_empty() => model,
                _ => continue,
            },
        };
        if !models.iter().any(|existing| existing == model) {
            models.push(model.to_string());
        }
    }
    models
}

fn split_logs(text: &str) -> Vec<LogLine> {
    text.lines()
        .enumerate()
        .map(|(seq, line)| LogLine {
            seq,
            stream: StreamKind::Stdout,
            line: line.to_string(),
        })
        .collect()
}

fn merge_logs(existing: Vec<LogLine>, persisted: Vec<LogLine>) -> Vec<LogLine> {
    if persisted.is_empty() {
        return existing;
    }
    let mut merged = persisted;
    for line in existing {
        if line.seq >= merged.len() {
            merged.push(line);
        } else if merged[line.seq].line != line.line {
            let seq = line.seq;
            merged[seq] = line;
        }
    }
    merged
}

fn is_active(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::InProgress | TaskStatus::Evaluating | TaskStatus::ResolvingConflicts
    )
}

fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Backlog => "BACKLOG",
        TaskStatus::Todo => "TODO",
        TaskStatus::InProgress => "IN_PROGRESS",
        TaskStatus::Evaluating => "EVALUATING",
        TaskStatus::Review => "REVIEW",
        TaskStatus::ResolvingConflicts => "RESOLVING",
        TaskStatus::Done => "DONE",
        TaskStatus::FailedNeedsHuman => "FAILED",
    }
}

fn status_color(theme: &Theme, status: TaskStatus) -> Color {
    match status {
        TaskStatus::Backlog => theme.muted,
        TaskStatus::Todo => theme.fg,
        TaskStatus::InProgress | TaskStatus::Evaluating | TaskStatus::ResolvingConflicts => {
            theme.signal
        }
        TaskStatus::Review => theme.accent,
        TaskStatus::Done => theme.success,
        TaskStatus::FailedNeedsHuman => theme.danger,
    }
}

fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else if c != '\r' {
            output.push(c);
        }
    }
    output
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(app.theme.bg).fg(app.theme.fg)),
        area,
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, app, chunks[0]);
    draw_body(frame, app, chunks[1]);
    draw_footer(frame, app, chunks[2]);

    if app.help {
        draw_help(frame, app, area);
    }
    if app.confirm.is_some() {
        draw_confirm(frame, app, area);
    }
    if app.input.is_some() {
        draw_input(frame, app, area);
    }
    if app.model_picker.is_some() {
        draw_model_picker(frame, app, area);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let project = app
        .project
        .as_ref()
        .map(|project| project.name.as_str())
        .unwrap_or("connecting…");
    let root = app
        .project
        .as_ref()
        .map(|project| project.root.as_str())
        .unwrap_or("");
    let connection = match app.connection {
        Connection::Connected => "connected",
        Connection::Connecting => "connecting",
        Connection::Disconnected => "disconnected",
    };
    let connection_color = match app.connection {
        Connection::Connected => app.theme.success,
        Connection::Connecting => app.theme.signal,
        Connection::Disconnected => app.theme.danger,
    };
    let spinner = if app.running_count() > 0 {
        SPINNER[(app.tick as usize / 5) % SPINNER.len()]
    } else {
        "·"
    };

    let first = Line::from(vec![
        Span::styled(
            " zou ",
            Style::default()
                .fg(app.theme.highlight_fg)
                .bg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            project,
            Style::default()
                .fg(app.theme.fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(root, Style::default().fg(app.theme.muted)),
        Span::raw("  "),
        Span::styled(
            format!("{spinner} {} running", app.running_count()),
            Style::default().fg(app.theme.signal),
        ),
        Span::raw("  "),
        Span::styled(connection, Style::default().fg(connection_color)),
    ]);

    let mut counts = Vec::new();
    for status in TaskStatus::ALL {
        let count = app.count(status);
        if count > 0
            || matches!(
                status,
                TaskStatus::Todo | TaskStatus::InProgress | TaskStatus::Review
            )
        {
            counts.push(Span::styled(
                format!("{} {}", status_label(status), count),
                Style::default().fg(status_color(&app.theme, status)),
            ));
            counts.push(Span::raw("  "));
        }
    }
    if !app.filter.is_empty() {
        counts.push(Span::styled(
            format!("filter: {}", app.filter),
            Style::default().fg(app.theme.accent),
        ));
    }
    let second = Line::from(counts);

    let header = Paragraph::new(Text::from(vec![first, second]))
        .style(Style::default().bg(app.theme.bg).fg(app.theme.fg));
    frame.render_widget(header, area);
}

fn draw_body(frame: &mut Frame, app: &mut App, area: Rect) {
    let stack = app.config.layout == "stack" || (app.config.layout != "split" && area.width < 100);
    if stack {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(area);
        draw_task_list(frame, app, chunks[0]);
        draw_detail(frame, app, chunks[1]);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
            .split(area);
        draw_task_list(frame, app, chunks[0]);
        draw_detail(frame, app, chunks[1]);
    }
}

fn draw_task_list(frame: &mut Frame, app: &App, area: Rect) {
    let tasks = app.visible_tasks();
    let items: Vec<ListItem> = tasks
        .iter()
        .map(|task| {
            let mut spans = vec![Span::styled(
                format!("{:<11}", status_label(task.status)),
                Style::default()
                    .fg(status_color(&app.theme, task.status))
                    .add_modifier(Modifier::BOLD),
            )];
            if is_active(task.status) {
                spans.push(Span::styled(
                    format!("{} ", SPINNER[(app.tick as usize / 5) % SPINNER.len()]),
                    Style::default().fg(app.theme.signal),
                ));
            } else {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                task.id.clone(),
                Style::default().fg(app.theme.muted),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::raw(task.title.clone()));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(format!(" tasks ({}) ", tasks.len()));
    if items.is_empty() {
        let empty = Paragraph::new("no tasks match")
            .style(Style::default().fg(app.theme.muted))
            .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(app.theme.highlight_bg)
                .fg(app.theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(app.selected.min(tasks.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(task) = app.selected_task() else {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.border))
            .title(" detail ");
        let empty = Paragraph::new("select a task")
            .style(Style::default().fg(app.theme.muted))
            .block(block);
        frame.render_widget(empty, area);
        return;
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(format!(" {} — {} ", task.id, task.title));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);
    let tabs = Tabs::new(vec!["logs", "output", "diff", "task"])
        .select(app.detail_tab.index())
        .style(Style::default().fg(app.theme.muted))
        .highlight_style(
            Style::default()
                .fg(app.theme.highlight_fg)
                .bg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, chunks[0]);

    match app.detail_tab {
        DetailTab::Logs => draw_logs(frame, app, chunks[1], &task),
        DetailTab::Output => draw_output(
            frame,
            chunks[1],
            &task.agent_output,
            "no agent output captured",
            app,
        ),
        DetailTab::Diff => draw_diff(frame, app, chunks[1], &task.diff),
        DetailTab::Task => {
            let text = format!(
                "id       : {}\nstatus   : {}\nretries  : {}\ncreated  : {}\nupdated  : {}\n\n{}",
                task.id,
                status_label(task.status),
                task.retry_count,
                task.created_at,
                task.updated_at,
                task.description
            );
            draw_text(frame, chunks[1], &text, "no description", app);
        }
    }
}

fn draw_logs(frame: &mut Frame, app: &mut App, area: Rect, task: &Task) {
    let logs = app.logs.get(&task.id).cloned().unwrap_or_default();
    if logs.is_empty() {
        let empty = Paragraph::new(if is_active(task.status) {
            "waiting for logs…"
        } else {
            "no logs captured"
        })
        .style(Style::default().fg(app.theme.muted));
        frame.render_widget(empty, area);
        return;
    }

    let height = area.height as usize;
    let total = logs.len();
    let max_scroll = total.saturating_sub(height).min(u16::MAX as usize) as u16;
    if app.follow_logs || app.log_scroll > max_scroll {
        app.log_scroll = max_scroll;
    }

    let lines: Vec<Line> = logs
        .iter()
        .map(|line| {
            let color = match line.stream {
                StreamKind::Stdout => app.theme.fg,
                StreamKind::Stderr => app.theme.danger,
            };
            Line::from(Span::styled(
                strip_ansi(&line.line),
                Style::default().fg(color),
            ))
        })
        .collect();
    let paragraph = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .scroll((app.log_scroll, 0));
    frame.render_widget(paragraph, area);
}

fn draw_text(frame: &mut Frame, area: Rect, text: &str, empty: &str, app: &App) {
    let content = if text.trim().is_empty() { empty } else { text };
    let paragraph = Paragraph::new(content)
        .style(Style::default().fg(if text.trim().is_empty() {
            app.theme.muted
        } else {
            app.theme.fg
        }))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Render the agent's final answer as lightweight terminal Markdown: headings,
/// bullets, quotes, and fenced code get distinct styles while the raw Markdown
/// stays intact (important when the answer includes code or tables).
fn draw_output(frame: &mut Frame, area: Rect, text: &str, empty: &str, app: &App) {
    if text.trim().is_empty() {
        draw_text(frame, area, "", empty, app);
        return;
    }
    let mut lines = Vec::new();
    let mut in_fence = false;
    for raw in text.lines() {
        let line = strip_ansi(raw);
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(app.theme.muted),
            )));
        } else if in_fence {
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(app.theme.muted),
            )));
        } else if trimmed.starts_with('#') {
            lines.push(Line::from(Span::styled(
                line,
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )));
        } else if trimmed.starts_with('>') {
            lines.push(Line::from(Span::styled(
                line,
                Style::default()
                    .fg(app.theme.muted)
                    .add_modifier(Modifier::ITALIC),
            )));
        } else if trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("+ ")
            || trimmed
                .split_once(". ")
                .is_some_and(|(prefix, _)| prefix.chars().all(|c| c.is_ascii_digit()))
        {
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(app.theme.fg),
            )));
        } else if trimmed.starts_with("---") || trimmed.starts_with("***") {
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(app.theme.muted),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(app.theme.fg),
            )));
        }
    }
    let paragraph = Paragraph::new(Text::from(lines))
        .style(Style::default().fg(app.theme.fg))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn draw_diff(frame: &mut Frame, app: &App, area: Rect, diff: &str) {
    if diff.trim().is_empty() {
        draw_text(frame, area, "", "no diff captured", app);
        return;
    }
    let lines: Vec<Line> = diff
        .lines()
        .map(|line| {
            let color = if line.starts_with("+++") || line.starts_with("---") {
                app.theme.muted
            } else if line.starts_with('+') {
                app.theme.success
            } else if line.starts_with('-') {
                app.theme.danger
            } else if line.starts_with("@@") {
                app.theme.signal
            } else {
                app.theme.fg
            };
            Line::from(Span::styled(line.to_string(), Style::default().fg(color)))
        })
        .collect();
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let mut parts = Vec::new();
    if !app.status.is_empty() {
        parts.push(app.status.clone());
    }
    for item in &app.config.statusbar {
        match item.as_str() {
            "project" => {
                if let Some(project) = &app.project {
                    parts.push(project.name.clone());
                }
            }
            "connection" => parts.push(
                match app.connection {
                    Connection::Connected => "connected",
                    Connection::Connecting => "connecting",
                    Connection::Disconnected => "disconnected",
                }
                .into(),
            ),
            "running" => parts.push(format!("{} running", app.running_count())),
            "review" => parts.push(format!("{} review", app.count(TaskStatus::Review))),
            "theme" => parts.push(format!("theme {}", app.theme.name)),
            "filter" if !app.filter.is_empty() => parts.push(format!("filter {}", app.filter)),
            "counts" => parts.push(format!("{} tasks", app.tasks.len())),
            _ => {}
        }
    }
    if parts.is_empty() {
        parts.push("q quit  p plan  A attach  r run  a approve  x reject  c recover".into());
    }
    parts.push("? help".into());
    let paragraph = Paragraph::new(parts.join("  ·  "))
        .style(Style::default().fg(app.theme.muted).bg(app.theme.bg))
        .alignment(Alignment::Left);
    frame.render_widget(paragraph, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let Some(input) = app.input.as_ref() else {
        return;
    };
    let popup = centered_rect(70, 5, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.accent))
        .title(format!(" {} ", input.title()))
        .style(Style::default().bg(app.theme.bg).fg(app.theme.fg));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let prompt = match input {
        InputMode::Plan(_) if !app.attachments.is_empty() => {
            format!("prompt ({} attached): ", app.attachments.len())
        }
        _ => format!("{}: ", input.prompt()),
    };
    let text = Line::from(vec![
        Span::styled(prompt, Style::default().fg(app.theme.muted)),
        Span::styled(input.text(), Style::default().fg(app.theme.fg)),
        Span::styled("█", Style::default().fg(app.theme.accent)),
    ]);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().bg(app.theme.bg)),
        inner,
    );
}

fn draw_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let Some(confirm) = app.confirm.as_ref() else {
        return;
    };
    let message = match confirm {
        ConfirmAction::RunAll => "run every ready (TODO) task?".to_string(),
        ConfirmAction::RunOne(task_id) => format!("run {task_id}?"),
        ConfirmAction::Approve(task_id) => format!("approve {task_id} and merge its diff?"),
        ConfirmAction::Cancel(task_id) => {
            format!("cancel {task_id}? the agent process tree will be killed")
        }
        ConfirmAction::Recover(Some(task_id)) => format!("release {task_id} back to TODO?"),
        ConfirmAction::Recover(None) => "release every stuck task back to TODO?".to_string(),
        ConfirmAction::Delete(task_id) => {
            format!("delete {task_id}? this permanently removes the task and its workspace")
        }
    };
    let popup = centered_rect(60, 5, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.signal))
        .title(" confirm ")
        .style(Style::default().bg(app.theme.bg).fg(app.theme.fg));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(format!("{message}\n\n[y] yes   [n/esc] cancel"))
            .style(Style::default().fg(app.theme.fg).bg(app.theme.bg))
            .alignment(Alignment::Center),
        inner,
    );
}

fn draw_model_picker(frame: &mut Frame, app: &App, area: Rect) {
    let Some(picker) = app.model_picker.as_ref() else {
        return;
    };
    let popup = centered_rect(74, 22, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.accent))
        .title(" model ")
        .style(Style::default().bg(app.theme.bg).fg(app.theme.fg));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    if picker.loading {
        frame.render_widget(
            Paragraph::new("loading `opencode models`…")
                .style(Style::default().fg(app.theme.muted)),
            chunks[0],
        );
    } else if picker.models.is_empty() {
        frame.render_widget(
            Paragraph::new("no models returned by `opencode models`")
                .style(Style::default().fg(app.theme.danger)),
            chunks[0],
        );
    } else {
        let items: Vec<ListItem> = picker
            .models
            .iter()
            .enumerate()
            .map(|(index, model)| {
                let current = picker.current.as_deref() == Some(model.as_str());
                let marker = if current { "●" } else { " " };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{marker} {:>2}. ", index + 1),
                        Style::default().fg(if current {
                            app.theme.success
                        } else {
                            app.theme.muted
                        }),
                    ),
                    Span::styled(
                        model.clone(),
                        Style::default().fg(if current {
                            app.theme.success
                        } else {
                            app.theme.fg
                        }),
                    ),
                ]))
            })
            .collect();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .bg(app.theme.highlight_bg)
                    .fg(app.theme.highlight_fg)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");
        let mut state =
            ListState::default().with_selected(Some(picker.selected.min(picker.models.len() - 1)));
        frame.render_stateful_widget(list, chunks[0], &mut state);
    }

    let hint = if picker.loading {
        "please wait…"
    } else {
        "j/k move   enter save   esc cancel   (restart `zou start` to apply)"
    };
    frame.render_widget(
        Paragraph::new(hint)
            .style(Style::default().fg(app.theme.muted))
            .alignment(Alignment::Center),
        chunks[1],
    );
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let popup = centered_rect(76, 18, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.accent))
        .title(" keys ")
        .style(Style::default().bg(app.theme.bg).fg(app.theme.fg));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let mut lines = vec![
        Line::from("navigation: j/k or ↑/↓ select   tab detail tab   enter logs"),
        Line::from("actions:    p plan   A attach   r run all   R run selected   C cancel   D delete"),
        Line::from("            a approve   x reject   c recover   g refresh"),
        Line::from("views:      l logs   o output   d diff   i task"),
        Line::from("config:     t theme   m model   f filter   : command palette"),
        Line::from("logs:       pageup/pagedown scroll   esc clear filter"),
        Line::from(""),
        Line::from("commands:   :run  :approve  :reject <text>  :cancel  :delete  :recover"),
        Line::from("            :plan <prompt>  :attach <path>  :clear-attachments"),
        Line::from("            :filter <text>  :theme <name>"),
        Line::from(""),
        Line::from("config:     ~/.config/zou/tui.toml or .zou/tui.toml"),
        Line::from("themes:     paper, midnight, solarized, high-contrast, mono"),
        Line::from(""),
        Line::from("press any key to close"),
    ];
    if !app.status.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            app.status.clone(),
            Style::default().fg(app.theme.muted),
        )));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::default().bg(app.theme.bg).fg(app.theme.fg))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let width = area
        .width
        .saturating_mul(percent_x)
        .saturating_div(100)
        .max(1);
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn sample_task(id: &str, status: TaskStatus, title: &str) -> Task {
        Task {
            id: id.into(),
            title: title.into(),
            description: "A task description".into(),
            status,
            retry_count: 0,
            worker_logs: "line one\nline two".into(),
            agent_output: "final output".into(),
            diff: "+added\n-removed".into(),
            created_at: "2026-10-28 00:00:00".into(),
            updated_at: "2026-10-28 00:00:01".into(),
        }
    }

    fn sample_app() -> App {
        let mut app = App::new(
            ApiClient::new("http://127.0.0.1:1"),
            TuiConfig::default(),
            None,
            Theme::paper(),
            None,
        );
        app.apply_tasks(TasksResponse {
            project: ProjectInfo {
                name: "demo".into(),
                root: "/tmp/demo".into(),
            },
            tasks: vec![
                sample_task("task-1", TaskStatus::InProgress, "Build the TUI"),
                sample_task("task-2", TaskStatus::Review, "Review the diff"),
            ],
        });
        app
    }

    #[test]
    fn renders_task_list_and_detail_without_panicking() {
        let mut app = sample_app();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(content.contains("tasks"));
        assert!(content.contains("Build the TUI"));
        assert!(content.contains("IN_PROGRESS"));
    }

    #[test]
    fn key_specs_normalize_and_override_defaults() {
        assert_eq!(normalize_key_spec("ctrl+r").as_deref(), Some("ctrl+r"));
        assert_eq!(normalize_key_spec("R").as_deref(), Some("R"));
        assert_eq!(normalize_key_spec("?").as_deref(), Some("?"));
        assert_eq!(normalize_key_spec("pageup").as_deref(), Some("pageup"));
        assert!(normalize_key_spec("hyper+x").is_none());

        let mut config = TuiConfig::default();
        config.keys.insert("quit".into(), "ctrl+q".into());
        let (keys, errors) = build_keymap(&config);
        assert!(errors.is_empty());
        assert_eq!(keys.get("ctrl+q"), Some(&Action::Quit));
        assert_eq!(keys.get("D"), Some(&Action::Delete));
        assert_eq!(keys.get("delete"), Some(&Action::Delete));
    }

    #[test]
    fn config_parses_theme_keymap_layout_statusbar_and_colors() {
        let config: TuiConfig = toml::from_str(
            r##"
theme = "midnight"
mouse = false
follow_logs = false
layout = "stack"
statusbar = ["project", "running", "theme"]

[keys]
quit = "ctrl+q"
run_one = "ctrl+r"

[colors]
accent = "#ffb000"
success = "#00dd88"
"##,
        )
        .unwrap();
        assert_eq!(config.theme.as_deref(), Some("midnight"));
        assert!(!config.mouse);
        assert!(!config.follow_logs);
        assert_eq!(config.layout, "stack");
        assert_eq!(
            config.statusbar,
            vec![
                "project".to_string(),
                "running".to_string(),
                "theme".to_string()
            ]
        );

        let (keys, errors) = build_keymap(&config);
        assert!(errors.is_empty());
        assert_eq!(keys.get("ctrl+q"), Some(&Action::Quit));
        assert_eq!(keys.get("ctrl+r"), Some(&Action::RunOne));

        let theme = Theme::builtin("midnight")
            .unwrap()
            .with_colors(&config.colors);
        assert_eq!(theme.accent, Color::Rgb(255, 176, 0));
        assert_eq!(theme.success, Color::Rgb(0, 221, 136));
    }

    #[test]
    fn merges_persisted_logs_without_duplicating_live_lines() {
        let existing = vec![
            LogLine {
                seq: 0,
                stream: StreamKind::Stdout,
                line: "one".into(),
            },
            LogLine {
                seq: 1,
                stream: StreamKind::Stdout,
                line: "live".into(),
            },
        ];
        let persisted = vec![LogLine {
            seq: 0,
            stream: StreamKind::Stdout,
            line: "one".into(),
        }];
        let merged = merge_logs(existing, persisted);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].line, "live");
    }

    #[test]
    fn event_log_appends_and_updates_task() {
        let mut app = sample_app();
        app.apply_event(ApiEvent::Log {
            task_id: "task-1".into(),
            stream: StreamKind::Stderr,
            line: "oops".into(),
            seq: 2,
        });
        assert_eq!(app.logs.get("task-1").map(Vec::len), Some(3));
        assert_eq!(app.logs["task-1"][2].line, "oops");

        app.apply_event(ApiEvent::TaskUpdated {
            task: sample_task("task-2", TaskStatus::Done, "Review the diff"),
        });
        assert_eq!(app.tasks[1].status, TaskStatus::Done);
    }
}
