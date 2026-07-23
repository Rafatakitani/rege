//! Full-screen ratatui app: fixed layout (header line, scrolling chat, pinned
//! AGENTES dashboard, input line, status line). Porta `legacy/lib/regente/tui.rb` +
//! `screen.rb` + `dashboard.rb`. No streaming yet — Enter just echoes the
//! turn into chat as a placeholder until the master driver lands.

use crate::config::Config;
use crate::driver;
use crate::playbook;
use crate::sessions::{self, SessionRec};
use crate::stream;
use crate::theme::{self, Role};
use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Terminal;
use std::io::Stdout;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum AgentState {
    Running,
    Done,
    Failed,
    Unknown,
}

impl AgentState {
    fn icon(self) -> &'static str {
        match self {
            AgentState::Running => "●",
            AgentState::Done => "✓",
            AgentState::Failed => "✗",
            AgentState::Unknown => "○",
        }
    }

    fn label(self) -> &'static str {
        match self {
            AgentState::Running => "running",
            AgentState::Done => "done",
            AgentState::Failed => "failed",
            AgentState::Unknown => "unknown",
        }
    }

    fn role(self) -> Role {
        match self {
            AgentState::Running => Role::Warn,
            AgentState::Done => Role::Accent,
            AgentState::Failed => Role::Fail,
            AgentState::Unknown => Role::Dim,
        }
    }
}

struct AgentRow {
    name: String,
    state: AgentState,
    last: String,
}

#[derive(Clone, Copy)]
enum ChatRole {
    User,
    Assistant,
    Tool,
    Info,
    Error,
}

impl ChatRole {
    fn role(self) -> Role {
        match self {
            ChatRole::User => Role::Accent,
            ChatRole::Assistant => Role::Text,
            ChatRole::Tool => Role::Dim,
            ChatRole::Info => Role::Dim,
            ChatRole::Error => Role::Fail,
        }
    }
}

struct ChatMsg {
    role: ChatRole,
    text: String,
}

/// UI mode: normal chat input, or the interactive theme picker overlay.
enum Mode {
    Normal,
    ThemePicker { cursor: usize },
    ResumePicker { cursor: usize },
}

struct App {
    theme: String,
    mode: Mode,
    master: String,
    master_cli: String,
    master_model: Option<String>,
    playbook_prompt: String,
    repo: String,
    chat: Vec<ChatMsg>,
    input: String,
    agents: Vec<AgentRow>,
    logdir: PathBuf,
    master_session: String,
    buddy: Option<crate::buddy::Buddy>,
    buddy_tick: usize,
    session_id: Option<String>,
    rx: Option<Receiver<stream::Event>>,
    sessions_path: PathBuf,
    session_recorded: bool,
    pending_title: Option<String>,
    resume_list: Vec<SessionRec>,
    auto_copy: bool,
    row_text: Vec<String>,
    selection_start: Option<(u16, u16)>,
    selection_end: Option<(u16, u16)>,
    flash: Option<(String, Instant)>,
}

impl App {
    fn new(config: &Config, repo: &str) -> App {
        let theme = config.ui.get("theme").cloned().unwrap_or_else(|| theme::DEFAULT.to_string());
        let master = match &config.master.model {
            Some(m) => format!("{}/{}", config.master.cli, m),
            None => config.master.cli.clone(),
        };
        App {
            theme,
            mode: Mode::Normal,
            master,
            master_cli: config.master.cli.clone(),
            master_model: config.master.model.clone(),
            playbook_prompt: playbook::prompt(config),
            repo: repo.to_string(),
            chat: vec![ChatMsg {
                role: ChatRole::Info,
                text: "Regente pronto. Digite uma tarefa. /help lista comandos, /quit sai.".into(),
            }],
            input: String::new(),
            agents: Vec::new(),
            logdir: std::env::temp_dir().join("regente-logs"),
            master_session: "regente-master".into(),
            buddy: None,
            buddy_tick: 0,
            session_id: None,
            rx: None,
            sessions_path: sessions::default_path(),
            session_recorded: false,
            pending_title: None,
            resume_list: Vec::new(),
            auto_copy: config.ui.get("auto_copy").map(|v| v != "false").unwrap_or(true),
            row_text: Vec::new(),
            selection_start: None,
            selection_end: None,
            flash: None,
        }
    }

    /// True once the user has sent at least one message — controls whether
    /// the wordmark banner still shows atop the chat.
    fn conversation_started(&self) -> bool {
        self.chat.iter().any(|m| matches!(m.role, ChatRole::User))
    }

    fn push(&mut self, role: ChatRole, text: impl Into<String>) {
        self.chat.push(ChatMsg { role, text: text.into() });
    }

    fn refresh_agents(&mut self) {
        self.agents = list_agents(&self.logdir, &self.master_session);
    }

    /// Theme currently rendered — the picker's cursor previews live, so this
    /// diverges from `self.theme` only while `Mode::ThemePicker` is open.
    fn active_theme(&self) -> &str {
        match self.mode {
            Mode::ThemePicker { cursor } => theme::names()[cursor],
            Mode::Normal | Mode::ResumePicker { .. } => &self.theme,
        }
    }

    fn open_theme_picker(&mut self) {
        let cursor = theme::names().iter().position(|n| *n == self.theme).unwrap_or(0);
        self.mode = Mode::ThemePicker { cursor };
    }

    fn picker_move(&mut self, delta: isize) {
        if let Mode::ThemePicker { cursor } = self.mode {
            let names = theme::names();
            let next = (cursor as isize + delta).clamp(0, names.len() as isize - 1);
            self.mode = Mode::ThemePicker { cursor: next as usize };
        }
    }

    fn picker_confirm(&mut self) {
        if let Mode::ThemePicker { cursor } = self.mode {
            let name = theme::names()[cursor].to_string();
            self.theme = name.clone();
            if let Err(e) = save_theme_to_config(&name) {
                self.push(ChatRole::Error, format!("erro ao salvar tema: {e}"));
            }
            self.mode = Mode::Normal;
        }
    }

    fn picker_cancel(&mut self) {
        self.mode = Mode::Normal;
    }

    fn open_resume_picker(&mut self) {
        self.resume_list = sessions::recent(&self.sessions_path, 12);
        self.mode = Mode::ResumePicker { cursor: 0 };
    }

    fn resume_picker_move(&mut self, delta: isize) {
        if let Mode::ResumePicker { cursor } = self.mode {
            if self.resume_list.is_empty() {
                return;
            }
            let next = (cursor as isize + delta).clamp(0, self.resume_list.len() as isize - 1);
            self.mode = Mode::ResumePicker { cursor: next as usize };
        }
    }

    fn resume_picker_confirm(&mut self) {
        if let Mode::ResumePicker { cursor } = self.mode {
            if let Some(rec) = self.resume_list.get(cursor) {
                let title = rec.title.clone();
                self.session_id = Some(rec.id.clone());
                self.push(ChatRole::Info, format!("retomando sessão: {title}"));
            }
            self.mode = Mode::Normal;
        }
    }

    fn resume_picker_cancel(&mut self) {
        self.mode = Mode::Normal;
    }

    fn mouse_down(&mut self, col: u16, row: u16) {
        self.selection_start = Some((col, row));
        self.selection_end = Some((col, row));
    }

    fn mouse_drag(&mut self, col: u16, row: u16) {
        if self.selection_start.is_some() {
            self.selection_end = Some((col, row));
        }
    }

    fn mouse_up(&mut self, col: u16, row: u16) {
        self.selection_end = Some((col, row));
        self.finalize_selection();
    }

    /// Extracts the selected text from `row_text` and, if `auto_copy` is on,
    /// copies it via OSC52 and shows a transient status-bar message.
    fn finalize_selection(&mut self) {
        let (start, end) = match (self.selection_start.take(), self.selection_end.take()) {
            (Some(s), Some(e)) => (s, e),
            _ => return,
        };
        let text = extract_selection(&self.row_text, start, end);
        if text.is_empty() || !self.auto_copy {
            return;
        }
        copy_to_clipboard(&text);
        self.flash = Some((format!("copiado {} chars · /config desativa", text.chars().count()), Instant::now()));
    }

    fn submit(&mut self) {
        let line = self.input.trim().to_string();
        self.input.clear();
        if line.is_empty() {
            return;
        }
        if line.starts_with('/') {
            self.dispatch(&line);
            return;
        }
        if self.pending_title.is_none() {
            self.pending_title = Some(truncate_title(&line));
        }
        self.push(ChatRole::User, line.clone());
        self.spawn_turn(line);
    }

    /// Fires `driver::spawn_turn` on a background thread and stores the
    /// receiver; the event loop drains it every tick via `drain_stream`.
    fn spawn_turn(&mut self, task: String) {
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let cli = self.master_cli.clone();
        let model = self.master_model.clone();
        let repo = self.repo.clone();
        let playbook = self.playbook_prompt.clone();
        let session_id = self.session_id.clone();
        std::thread::spawn(move || {
            driver::spawn_turn(&cli, model.as_deref(), &repo, Some(&playbook), &task, session_id, tx);
        });
    }

    /// Drains any pending stream events without blocking, mapping each to a
    /// chat line (or updating `session_id` on Ready / closing `rx` on Done).
    fn drain_stream(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for event in events {
            self.handle_stream_event(event);
        }
        if disconnected {
            self.rx = None;
        }
    }

    fn handle_stream_event(&mut self, event: stream::Event) {
        match event {
            stream::Event::Ready { session_id } => {
                if !self.session_recorded {
                    let title = self.pending_title.clone().unwrap_or_else(|| "(sem título)".to_string());
                    sessions::add(
                        &self.sessions_path,
                        SessionRec { id: session_id.clone(), title, repo: self.repo.clone(), ts: sessions::now_ts() },
                    );
                    self.session_recorded = true;
                }
                self.session_id = Some(session_id);
            }
            stream::Event::Text(text) => self.push(ChatRole::Assistant, text),
            stream::Event::Tool { name, .. } => self.push(ChatRole::Tool, name),
            stream::Event::ToolResult(text) => self.push(ChatRole::Info, text),
            stream::Event::Done { cost } => {
                let line = match cost {
                    Some(c) => format!("— ${c:.4}"),
                    None => "— concluído".to_string(),
                };
                self.push(ChatRole::Info, line);
                self.rx = None;
            }
        }
    }

    fn dispatch(&mut self, line: &str) {
        let mut parts = line.split_whitespace();
        match parts.next().unwrap_or("") {
            "/quit" | "/q" => {}
            "/help" | "/?" => {
                self.push(ChatRole::Info, "comandos:");
                self.push(ChatRole::Info, "  /help            esta lista");
                self.push(ChatRole::Info, "  /theme [nome]    seletor de tema (sem arg abre picker)");
                self.push(ChatRole::Info, "  /model [nome]    troca modelo do mestre (sem arg mostra atual)");
                self.push(ChatRole::Info, "  /config          mostra config efetiva");
                self.push(ChatRole::Info, "  /resume          retoma sessão anterior");
                self.push(ChatRole::Info, "  /agents          lista agentes ativos");
                self.push(ChatRole::Info, "  /buddy [pet]     bicho de estimação");
                self.push(ChatRole::Info, "  /quit            sai (ou exit/quit/:q)");
            }
            "/model" => match parts.next() {
                None => {
                    let cur = self.master_model.clone().unwrap_or_else(|| "(default do CLI)".into());
                    let cli = self.master_cli.clone();
                    self.push(ChatRole::Info, format!("mestre: {cli} · modelo: {cur}"));
                    self.push(ChatRole::Info, "troca com: /model <nome>  (ex: /model opus, /model sonnet)");
                }
                Some(name) => {
                    self.master_model = Some(name.to_string());
                    self.master = format!("{}/{}", self.master_cli, name);
                    self.push(ChatRole::Info, format!("modelo do mestre: {name}"));
                }
            },
            "/config" => {
                let cur = self.master_model.clone().unwrap_or_else(|| "(default)".into());
                let cli = self.master_cli.clone();
                let theme = self.theme.clone();
                let auto = self.auto_copy;
                let repo = self.repo.clone();
                let sess = self.sessions_path.display().to_string();
                self.push(ChatRole::Info, "config efetiva:");
                self.push(ChatRole::Info, format!("  mestre       {cli} / {cur}"));
                self.push(ChatRole::Info, format!("  tema         {theme}"));
                self.push(ChatRole::Info, format!("  auto_copy    {auto}"));
                self.push(ChatRole::Info, format!("  repo         {repo}"));
                self.push(ChatRole::Info, format!("  sessões      {sess}"));
                self.push(ChatRole::Info, "edite ~/.config/regente/config.yml ou .regente.yml no projeto");
            }
            "/theme" => match parts.next() {
                None => self.open_theme_picker(),
                Some(name) if theme::exists(name) => {
                    self.theme = name.to_string();
                }
                Some(name) => {
                    self.push(ChatRole::Error, format!("tema inexistente: {name}"));
                }
            },
            "/resume" => self.open_resume_picker(),
            "/agents" => {
                if self.agents.is_empty() {
                    self.push(ChatRole::Info, "nenhum agente ativo");
                } else {
                    let names: Vec<String> = self
                        .agents
                        .iter()
                        .map(|a| format!("{} ({})", a.name, a.state.label()))
                        .collect();
                    self.push(ChatRole::Info, names.join(", "));
                }
            }
            "/buddy" => match parts.next() {
                None => {
                    let seed = buddy_seed();
                    self.buddy = Some(crate::buddy::Buddy::hatch(&seed));
                }
                Some("pet") => {
                    if let Some(buddy) = self.buddy.as_mut() {
                        let reaction = buddy.pet();
                        self.push(ChatRole::Assistant, reaction);
                    } else {
                        self.push(ChatRole::Info, "/buddy primeiro");
                    }
                }
                Some(other) => {
                    self.push(ChatRole::Error, format!("subcomando desconhecido: {other}"));
                }
            },
            other => {
                self.push(ChatRole::Error, format!("comando desconhecido: {other}"));
            }
        }
    }
}

/// Truncates a session title candidate to ~60 chars, appending "..." when cut.
fn truncate_title(text: &str) -> String {
    const MAX: usize = 60;
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let truncated: String = text.chars().take(MAX).collect();
    format!("{truncated}...")
}

/// Extracts selected text from `row_text` (absolute-row text captured during
/// the last draw). Same row -> column slice; multi-row -> whole lines joined
/// with `\n`, trailing spaces trimmed.
fn extract_selection(row_text: &[String], start: (u16, u16), end: (u16, u16)) -> String {
    let (mut c0, mut r0) = start;
    let (mut c1, mut r1) = end;
    if r0 > r1 || (r0 == r1 && c0 > c1) {
        std::mem::swap(&mut c0, &mut c1);
        std::mem::swap(&mut r0, &mut r1);
    }
    if r0 == r1 {
        let row = row_text.get(r0 as usize).map(String::as_str).unwrap_or("");
        let chars: Vec<char> = row.chars().collect();
        let lo = (c0 as usize).min(chars.len());
        let hi = ((c1 as usize) + 1).min(chars.len()).max(lo);
        chars[lo..hi].iter().collect::<String>().trim_end().to_string()
    } else {
        (r0..=r1)
            .map(|r| row_text.get(r as usize).map(|s| s.trim_end().to_string()).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Wraps an OSC52 clipboard-set sequence for tmux passthrough when running
/// inside a tmux session (`$TMUX` set), doubling internal ESCs per the tmux
/// passthrough protocol.
fn osc52_sequence(text: &str) -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    let encoded = STANDARD.encode(text.as_bytes());
    let seq = format!("\x1b]52;c;{encoded}\x07");
    if std::env::var_os("TMUX").is_some() {
        let escaped = seq.replace('\x1b', "\x1b\x1b");
        format!("\x1bPtmux;\x1b{escaped}\x1b\\")
    } else {
        seq
    }
}

/// Writes the OSC52 clipboard-set sequence directly to stdout, bypassing
/// ratatui's buffer (raw mode is on, so this reaches the terminal untouched).
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    let seq = osc52_sequence(text);
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(seq.as_bytes());
    let _ = stdout.flush();
}

/// Coarse relative-time label ("agora", "2h", "3d") for a past `ts` (unix
/// seconds) compared to `now`.
fn rel_time(ts: u64, now: u64) -> String {
    let secs = now.saturating_sub(ts);
    if secs < 60 {
        "agora".to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// Seed for hatching the buddy: `$USER`, else `$HOSTNAME`/`hostname(1)`, else "regente".
fn buddy_seed() -> String {
    if let Ok(user) = std::env::var("USER") {
        if !user.is_empty() {
            return user;
        }
    }
    if let Ok(host) = std::env::var("HOSTNAME") {
        if !host.is_empty() {
            return host;
        }
    }
    if let Ok(out) = Command::new("hostname").output() {
        if out.status.success() {
            let host = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !host.is_empty() {
                return host;
            }
        }
    }
    "regente".to_string()
}

fn list_agents(logdir: &PathBuf, master_session: &str) -> Vec<AgentRow> {
    let sessions = tmux_sessions();
    sessions
        .into_iter()
        .filter(|s| s.starts_with("regente-") && s != master_session)
        .map(|s| agent_row(&s, logdir))
        .collect()
}

fn tmux_sessions() -> Vec<String> {
    let out = Command::new("tmux").args(["list-sessions", "-F", "#S"]).output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn agent_row(session: &str, logdir: &PathBuf) -> AgentRow {
    let name = session.strip_prefix("regente-").unwrap_or(session).to_string();
    let log = std::fs::read_to_string(logdir.join(format!("{session}.log"))).unwrap_or_default();
    let state = if let Some(code) = find_exit_code(&log) {
        if code == 0 {
            AgentState::Done
        } else {
            AgentState::Failed
        }
    } else if tmux_alive(session) {
        AgentState::Running
    } else {
        AgentState::Unknown
    };
    let last = strip_sentinel(&log).lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_string();
    AgentRow { name, state, last }
}

fn tmux_alive(session: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", session])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn find_exit_code(log: &str) -> Option<i32> {
    let prefix = "__RG_EXIT_";
    let suffix = "__";
    let start = log.find(prefix)? + prefix.len();
    let rest = &log[start..];
    let end = rest.find(suffix)?;
    rest[..end].parse().ok()
}

fn strip_sentinel(log: &str) -> String {
    let prefix = "__RG_EXIT_";
    match log.find(prefix) {
        Some(i) => log[..i].to_string(),
        None => log.to_string(),
    }
}

fn config_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Persist `ui.theme` into the global config, deep-merging so unrelated keys
/// (and other `ui.*` entries) survive.
fn save_theme_to_config(theme: &str) -> Result<()> {
    let path = Config::global_path(&config_home());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut value: serde_yaml::Value = if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        if text.trim().is_empty() {
            serde_yaml::Value::Mapping(Default::default())
        } else {
            serde_yaml::from_str(&text)?
        }
    } else {
        serde_yaml::Value::Mapping(Default::default())
    };
    if !value.is_mapping() {
        value = serde_yaml::Value::Mapping(Default::default());
    }
    let map = value.as_mapping_mut().expect("just ensured mapping");
    let ui_key = serde_yaml::Value::String("ui".into());
    let mut ui_map = match map.get(&ui_key).and_then(|v| v.as_mapping()) {
        Some(m) => m.clone(),
        None => serde_yaml::Mapping::new(),
    };
    ui_map.insert(serde_yaml::Value::String("theme".into()), serde_yaml::Value::String(theme.into()));
    map.insert(ui_key, serde_yaml::Value::Mapping(ui_map));
    std::fs::write(&path, serde_yaml::to_string(&value)?)?;
    Ok(())
}

/// Headless render: draw one frame to an off-screen buffer and return it as
/// plain text (one line per row). No tty needed — used by `regente render` and
/// tests so the TUI can be inspected without a real terminal. `demo` seeds a
/// bit of chat/agent state so the layout is exercised.
pub fn render_frame(config: &Config, repo: &str, cols: u16, rows: u16, demo: bool) -> String {
    use ratatui::backend::TestBackend;
    let mut app = App::new(config, repo);
    if demo {
        app.push(ChatRole::User, "refatora o modulo de auth e adiciona testes");
        app.push(ChatRole::Assistant, "Tarefa difícil. Rodando 3 workers na mesma tarefa.");
        app.push(ChatRole::Info, "⚙ spawn_agent · claude/sonnet · refatora auth");
        app.agents = vec![
            AgentRow { name: "a1".into(), state: AgentState::Running, last: "editando session.rs…".into() },
            AgentRow { name: "a2".into(), state: AgentState::Done, last: "patch pronto".into() },
        ];
    }
    let backend = TestBackend::new(cols, rows);
    let mut terminal = Terminal::new(backend).expect("test backend");
    let mut out = Vec::new();
    terminal
        .draw(|f| {
            draw(f.area(), f.buffer_mut(), &app);
            out = capture_row_text(f.buffer_mut());
        })
        .expect("draw");
    out.join("\n")
}

/// Entry point: run the fullscreen TUI until the user quits.
pub fn run(config: &Config, repo: &str) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config, repo);
    app.refresh_agents();
    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    let mut last_poll = Instant::now();
    loop {
        app.drain_stream();
        if let Some((_, ts)) = &app.flash {
            if ts.elapsed() >= Duration::from_secs(2) {
                app.flash = None;
            }
        }
        let mut rows = Vec::new();
        terminal.draw(|f| {
            draw(f.area(), f.buffer_mut(), app);
            rows = capture_row_text(f.buffer_mut());
        })?;
        app.row_text = rows;

        if last_poll.elapsed() >= Duration::from_secs(1) {
            app.refresh_agents();
            app.buddy_tick = app.buddy_tick.wrapping_add(1);
            last_poll = Instant::now();
        }

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                        break;
                    }
                    match app.mode {
                        Mode::ThemePicker { .. } => match key.code {
                            KeyCode::Up => app.picker_move(-1),
                            KeyCode::Down => app.picker_move(1),
                            KeyCode::Enter => app.picker_confirm(),
                            KeyCode::Esc => app.picker_cancel(),
                            _ => {}
                        },
                        Mode::ResumePicker { .. } => match key.code {
                            KeyCode::Up => app.resume_picker_move(-1),
                            KeyCode::Down => app.resume_picker_move(1),
                            KeyCode::Enter => app.resume_picker_confirm(),
                            KeyCode::Esc => app.resume_picker_cancel(),
                            _ => {}
                        },
                        Mode::Normal => match key.code {
                            KeyCode::Char(c) => app.input.push(c),
                            KeyCode::Backspace => {
                                app.input.pop();
                            }
                            KeyCode::Enter => {
                                let line = app.input.trim().to_string();
                                if matches!(line.as_str(), "/quit" | "/q" | "quit" | "exit" | ":q") {
                                    break;
                                }
                                app.submit();
                            }
                            KeyCode::Esc => break,
                            _ => {}
                        },
                    }
                }
                Event::Mouse(m) => match m.kind {
                    MouseEventKind::Down(MouseButton::Left) => app.mouse_down(m.column, m.row),
                    MouseEventKind::Drag(MouseButton::Left) => app.mouse_drag(m.column, m.row),
                    MouseEventKind::Up(MouseButton::Left) => app.mouse_up(m.column, m.row),
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Ok(())
}

/// Snapshots the last-rendered frame into per-row plain text (no styling),
/// used to resolve a mouse selection's column/row range into real text.
fn capture_row_text(buf: &ratatui::buffer::Buffer) -> Vec<String> {
    let area = buf.area;
    (0..area.height)
        .map(|y| {
            (0..area.width).map(|x| buf.cell((area.x + x, area.y + y)).map(|c| c.symbol()).unwrap_or(" ")).collect::<String>()
        })
        .collect()
}

fn rgb(c: (u8, u8, u8)) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

fn styled(theme: &str, role: Role, text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(rgb(theme::color(theme, role))))
}

fn repo_name(app: &App) -> String {
    std::path::Path::new(&app.repo)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| app.repo.clone())
}

/// Rounded-border panel with a corner title, colored by `role` (dim for secondary panels).
fn rounded_block(theme: &str, title: &str, role: Role) -> Block<'static> {
    let color = rgb(theme::color(theme, role));
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Span::styled(title.to_string(), Style::default().fg(color)))
}

fn draw(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App) {
    let theme = app.active_theme();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(6),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(area);

    draw_header(chunks[0], buf, app, theme);
    draw_chat(chunks[1], buf, app, theme);
    draw_agents(chunks[2], buf, app, theme);
    draw_input(chunks[3], buf, app, theme);
    draw_statusbar(chunks[4], buf, app, theme);

    if let Mode::ThemePicker { cursor } = app.mode {
        draw_theme_picker(area, buf, app, cursor);
    }
    if let Mode::ResumePicker { cursor } = app.mode {
        draw_resume_picker(area, buf, app, cursor);
    }

    // Live highlight while dragging a selection, so the user gets feedback
    // even in terminals/tmux where the OSC52 copy is silently dropped.
    if matches!(app.mode, Mode::Normal) {
        if let (Some(start), Some(end)) = (app.selection_start, app.selection_end) {
            highlight_selection(area, buf, start, end);
        }
    }
}

/// Reverse-videos every cell inside the selection range on the current frame.
fn highlight_selection(area: Rect, buf: &mut ratatui::buffer::Buffer, start: (u16, u16), end: (u16, u16)) {
    let (mut c0, mut r0) = start;
    let (mut c1, mut r1) = end;
    if r0 > r1 || (r0 == r1 && c0 > c1) {
        std::mem::swap(&mut c0, &mut c1);
        std::mem::swap(&mut r0, &mut r1);
    }
    for row in r0..=r1 {
        if row < area.y || row >= area.y + area.height {
            continue;
        }
        let (lo, hi) = if r0 == r1 {
            (c0, c1)
        } else if row == r0 {
            (c0, area.x + area.width - 1)
        } else if row == r1 {
            (area.x, c1)
        } else {
            (area.x, area.x + area.width - 1)
        };
        for col in lo..=hi {
            if col < area.x || col >= area.x + area.width {
                continue;
            }
            if let Some(cell) = buf.cell_mut((col, row)) {
                cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

fn draw_header(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, theme: &str) {
    let suffix = format!(" · {} · ~/{} · {}", app.master, repo_name(app), theme);
    let line = Line::from(vec![styled(theme, Role::Accent, "regente"), styled(theme, Role::Dim, suffix)]);
    Paragraph::new(line).render(area, buf);
}

/// REGENTE wordmark, block letters, shown atop the chat only before the
/// first user message — sober, no glow, just the desaturated dim tone.
const BANNER: [&str; 5] = [
    "████  █████ ████  █████ ██  ██ █████ █████",
    "██ ██ ██    ██    ██    ███ ██   █   ██   ",
    "████  ████  ██ ██ ████  ██████   █   ████ ",
    "██ ██ ██    ██ ██ ██    ██ ███   █   ██   ",
    "██ ██ █████ ████  █████ ██  ██   █   █████",
];

fn draw_chat(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, theme: &str) {
    let inner = Rect {
        x: area.x + 2,
        y: area.y,
        width: area.width.saturating_sub(4),
        height: area.height,
    };

    if app.conversation_started() {
        render_messages(inner, buf, app, theme);
    } else {
        let banner_height = (BANNER.len() as u16 + 1).min(inner.height);
        let banner_rect = Rect { x: inner.x, y: inner.y, width: inner.width, height: banner_height };
        draw_banner(banner_rect, buf, theme);

        let rest = Rect {
            x: inner.x,
            y: inner.y + banner_height,
            width: inner.width,
            height: inner.height.saturating_sub(banner_height),
        };
        render_messages(rest, buf, app, theme);
    }

    if let Some(buddy) = &app.buddy {
        draw_buddy_widget(area, buf, buddy, app.buddy_tick, theme);
    }
}

/// Floating widget pinned to the bottom-right corner of the chat area,
/// persistent across turns once a buddy has been hatched via `/buddy`.
/// Advances one animation frame per tick (~1s, driven by the event loop).
fn draw_buddy_widget(
    area: Rect,
    buf: &mut ratatui::buffer::Buffer,
    buddy: &crate::buddy::Buddy,
    tick: usize,
    theme: &str,
) {
    let lines = buddy.render_lines(tick);
    let width: u16 = 24;
    let height = (lines.len() as u16 + 3).min(area.height);
    if area.width < width + 2 || area.height < height {
        return;
    }
    let rect = Rect {
        x: area.right().saturating_sub(width + 1),
        y: area.bottom().saturating_sub(height + 1),
        width,
        height,
    };

    Paragraph::new("").render(rect, buf);

    let block = rounded_block(theme, "", Role::Accent);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let text: Vec<Line> = lines
        .into_iter()
        .map(|l| Line::from(styled(theme, Role::Accent, l)))
        .collect();
    Paragraph::new(text).render(inner, buf);
}

fn draw_banner(area: Rect, buf: &mut ratatui::buffer::Buffer, theme: &str) {
    let width = BANNER[0].chars().count() as u16;
    let pad = area.width.saturating_sub(width) / 2;
    let lines: Vec<Line> = BANNER
        .iter()
        .map(|row| Line::from(styled(theme, Role::Dim, format!("{}{}", " ".repeat(pad as usize), row))))
        .collect();
    Paragraph::new(lines).render(area, buf);
}

fn render_messages(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, theme: &str) {
    let rows = area.height as usize;
    let msgs: Vec<Line> = app
        .chat
        .iter()
        .rev()
        .take(rows)
        .rev()
        .map(|m| chat_line(theme, m))
        .collect();
    Paragraph::new(msgs).render(area, buf);
}

fn chat_line(theme: &str, m: &ChatMsg) -> Line<'static> {
    match m.role {
        ChatRole::User => Line::from(vec![
            styled(theme, Role::Accent, "❯ "),
            styled(theme, Role::Text, m.text.clone()),
        ]),
        ChatRole::Assistant => Line::from(vec![
            styled(theme, Role::Accent, "● "),
            styled(theme, Role::Text, m.text.clone()),
        ]),
        ChatRole::Tool => Line::from(styled(theme, Role::Dim, format!("  ⚙ {}", m.text))),
        ChatRole::Info => Line::from(styled(theme, Role::Dim, m.text.clone())),
        ChatRole::Error => Line::from(styled(theme, Role::Fail, m.text.clone())),
    }
}

fn draw_agents(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, theme: &str) {
    let block = rounded_block(theme, " agentes ", Role::Dim);
    let inner = block.inner(area);
    block.render(area, buf);

    let lines: Vec<Line> = if app.agents.is_empty() {
        vec![Line::from(styled(theme, Role::Dim, "nenhum agente ativo"))]
    } else {
        app.agents
            .iter()
            .map(|a| {
                let text = format!("{} {:<8} {:<8} ", a.state.icon(), a.name, a.state.label());
                Line::from(vec![
                    styled(theme, a.state.role(), text),
                    styled(theme, Role::Dim, a.last.clone()),
                ])
            })
            .collect()
    };
    Paragraph::new(lines).render(inner, buf);
}

fn draw_input(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, theme: &str) {
    let block = rounded_block(theme, "", Role::Dim);
    let inner = block.inner(area);
    block.render(area, buf);
    let line = Line::from(vec![
        styled(theme, Role::Accent, "❯ "),
        styled(theme, Role::Text, app.input.clone()),
        styled(theme, Role::Neon, "█"),
    ]);
    Paragraph::new(line).render(inner, buf);
}

fn draw_statusbar(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, theme: &str) {
    if let Some((msg, ts)) = &app.flash {
        if ts.elapsed() < Duration::from_secs(2) {
            let line = Line::from(styled(theme, Role::Accent, msg.clone()));
            Paragraph::new(line).render(area, buf);
            return;
        }
    }
    let running = app.agents.iter().filter(|a| a.state == AgentState::Running).count();
    let ready = app.agents.iter().filter(|a| a.state == AgentState::Done).count();
    let line = Line::from(vec![
        styled(theme, Role::Accent, running.to_string()),
        styled(theme, Role::Dim, " rodando · "),
        styled(theme, Role::Accent, ready.to_string()),
        styled(theme, Role::Dim, " pronto · /help · /theme · /model · /resume · /quit"),
    ]);
    Paragraph::new(line).render(area, buf);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn draw_theme_picker(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, cursor: usize) {
    let names = theme::names();
    let theme = app.active_theme();
    let height = names.len() as u16 + 5;
    let rect = centered_rect(46, height, area);

    // Clear the popup area so the background chat text doesn't bleed through.
    Paragraph::new("").render(rect, buf);

    let block = rounded_block(theme, " tema ", Role::Dim);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(styled(theme, Role::Text, "Escolha o tema").add_modifier(Modifier::BOLD)));
    lines.push(Line::from(styled(theme, Role::Dim, "↑↓ navega · Enter seleciona · Esc cancela")));

    for (i, name) in names.iter().enumerate() {
        let chevron = if i == cursor { styled(theme, Role::Accent, "❯ ") } else { styled(theme, Role::Dim, "  ") };
        let index = styled(theme, Role::Dim, format!("{:>2} ", i + 1));
        let block_char = "▀";
        let sw1 = Span::styled(block_char.to_string(), Style::default().fg(rgb(theme::color(name, Role::Accent))));
        let sw2 = Span::styled(block_char.to_string(), Style::default().fg(rgb(theme::color(name, Role::Accent2))));
        let sw3 = Span::styled(block_char.to_string(), Style::default().fg(rgb(theme::color(name, Role::Text))));
        let name_span = if i == cursor {
            styled(theme, Role::Accent, format!(" {name}")).add_modifier(Modifier::BOLD)
        } else {
            styled(theme, Role::Text, format!(" {name}"))
        };
        let mut spans = vec![chevron, index, sw1, sw2, sw3, name_span];
        if *name == app.theme {
            spans.push(styled(theme, Role::Dim, "  "));
            spans.push(styled(theme, Role::Accent, "✓"));
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(styled(theme, Role::Dim, "Enter seleciona · Esc cancela")));
    Paragraph::new(lines).render(inner, buf);
}

fn draw_resume_picker(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, cursor: usize) {
    let theme = app.active_theme();
    let list_height = app.resume_list.len().max(1) as u16;
    let height = list_height + 6;
    let rect = centered_rect(60, height, area);

    // Clear the popup area so the background chat text doesn't bleed through.
    Paragraph::new("").render(rect, buf);

    let block = rounded_block(theme, " sessoes ", Role::Dim);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(styled(theme, Role::Text, "Retomar sessão").add_modifier(Modifier::BOLD)));
    lines.push(Line::from(styled(theme, Role::Dim, "↑↓ navega · Enter retoma · Esc cancela")));

    if app.resume_list.is_empty() {
        lines.push(Line::from(styled(theme, Role::Dim, "nenhuma sessão anterior")));
    } else {
        let now = sessions::now_ts();
        for (i, rec) in app.resume_list.iter().enumerate() {
            let chevron = if i == cursor { styled(theme, Role::Accent, "❯ ") } else { styled(theme, Role::Dim, "  ") };
            let short_id: String = rec.id.chars().take(8).collect();
            let text = format!("{}  ·  {}  ·  há {}", rec.title, short_id, rel_time(rec.ts, now));
            let span = if i == cursor {
                styled(theme, Role::Accent, text).add_modifier(Modifier::BOLD)
            } else {
                styled(theme, Role::Text, text)
            };
            lines.push(Line::from(vec![chevron, span]));
        }
    }

    lines.push(Line::from(styled(theme, Role::Dim, "Enter retoma · Esc cancela")));
    Paragraph::new(lines).render(inner, buf);
}

use ratatui::widgets::Widget;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_exit_code_parses_sentinel() {
        assert_eq!(find_exit_code("hello\n__RG_EXIT_0__\n"), Some(0));
        assert_eq!(find_exit_code("hello\n__RG_EXIT_7__\n"), Some(7));
        assert_eq!(find_exit_code("no sentinel here"), None);
    }

    #[test]
    fn strip_sentinel_removes_marker() {
        assert_eq!(strip_sentinel("out\n__RG_EXIT_0__\n"), "out\n");
        assert_eq!(strip_sentinel("plain"), "plain");
    }

    #[test]
    fn app_dispatch_theme_switches_live() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        assert_eq!(app.theme, "hacker");
        app.input = "/theme dracula".into();
        app.dispatch("/theme dracula");
        assert_eq!(app.theme, "dracula");
    }

    #[test]
    fn app_dispatch_unknown_theme_keeps_current() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.dispatch("/theme nope");
        assert_eq!(app.theme, "hacker");
        assert!(app.chat.iter().any(|m| m.text.contains("tema inexistente")));
    }

    #[test]
    fn app_dispatch_help_lists_commands() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.dispatch("/help");
        let joined: String = app.chat.iter().map(|m| m.text.clone()).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("/model"));
        assert!(joined.contains("/config"));
        assert!(joined.contains("/resume"));
    }

    #[test]
    fn app_dispatch_model_sets_master_model() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.dispatch("/model opus");
        assert_eq!(app.master_model.as_deref(), Some("opus"));
        assert_eq!(app.master, "claude/opus");
    }

    #[test]
    fn app_dispatch_model_no_arg_shows_current() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        let before = app.master_model.clone();
        app.dispatch("/model");
        assert!(app.chat.iter().any(|m| m.text.contains("mestre:")));
        assert_eq!(app.master_model, before); // no-arg não muda nada
    }

    #[test]
    fn app_dispatch_config_shows_effective() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.dispatch("/config");
        let joined: String = app.chat.iter().map(|m| m.text.clone()).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("auto_copy"));
        assert!(joined.contains("tema"));
    }

    #[test]
    fn app_dispatch_unknown_still_errors() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.dispatch("/naoexiste");
        assert!(app.chat.iter().any(|m| m.text.contains("comando desconhecido")));
    }

    #[test]
    fn render_frame_headless_shows_wordmark_and_header() {
        let config = Config::default();
        let out = render_frame(&config, "/tmp/portfolio", 100, 32, false);
        assert!(out.contains("regente")); // header
        assert!(out.contains("/help")); // welcome/status hint
    }

    #[test]
    fn render_frame_demo_shows_agents_and_chat() {
        let config = Config::default();
        let out = render_frame(&config, "/tmp/portfolio", 100, 32, true);
        assert!(out.contains("refatora o modulo de auth"));
        assert!(out.contains("a1"));
        assert!(out.contains("running") || out.contains("done"));
    }

    #[test]
    fn highlight_selection_reverses_cells_in_range() {
        let area = Rect { x: 0, y: 0, width: 10, height: 3 };
        let mut buf = ratatui::buffer::Buffer::empty(area);
        highlight_selection(area, &mut buf, (2, 1), (5, 1));
        assert!(buf.cell((2, 1)).unwrap().modifier.contains(Modifier::REVERSED));
        assert!(buf.cell((5, 1)).unwrap().modifier.contains(Modifier::REVERSED));
        assert!(!buf.cell((0, 0)).unwrap().modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn app_submit_echoes_user_and_spawns_turn() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.input = "faz algo".into();
        app.submit();
        assert!(app.chat.iter().any(|m| m.text.contains("faz algo")));
        assert!(app.rx.is_some());
    }

    #[test]
    fn handle_stream_event_ready_sets_session_id() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.handle_stream_event(stream::Event::Ready { session_id: "sess-1".into() });
        assert_eq!(app.session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn handle_stream_event_text_pushes_assistant_line() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.handle_stream_event(stream::Event::Text("oi".into()));
        assert!(app.chat.iter().any(|m| matches!(m.role, ChatRole::Assistant) && m.text == "oi"));
    }

    #[test]
    fn handle_stream_event_done_closes_rx_and_shows_cost() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        let (_tx, rx) = mpsc::channel();
        app.rx = Some(rx);
        app.handle_stream_event(stream::Event::Done { cost: Some(0.01) });
        assert!(app.rx.is_none());
        assert!(app.chat.iter().any(|m| m.text.contains("0.0100")));
    }

    #[test]
    fn theme_picker_opens_navigates_and_confirms() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.dispatch("/theme");
        assert!(matches!(app.mode, Mode::ThemePicker { .. }));
        app.picker_move(1);
        let names = theme::names();
        assert_eq!(app.active_theme(), names[1]);
        // theme not committed until confirm
        assert_eq!(app.theme, "hacker");
        app.picker_cancel();
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.theme, "hacker");
    }

    #[test]
    fn extract_selection_same_row_slices_columns() {
        let rows = vec!["hello world".to_string()];
        assert_eq!(extract_selection(&rows, (0, 0), (4, 0)), "hello");
        assert_eq!(extract_selection(&rows, (6, 0), (10, 0)), "world");
    }

    #[test]
    fn extract_selection_normalizes_reversed_range() {
        let rows = vec!["hello world".to_string()];
        assert_eq!(extract_selection(&rows, (4, 0), (0, 0)), "hello");
    }

    #[test]
    fn extract_selection_multi_row_joins_whole_lines_trimmed() {
        let rows = vec!["first   ".to_string(), "second".to_string(), "third  ".to_string()];
        assert_eq!(extract_selection(&rows, (2, 0), (3, 2)), "first\nsecond\nthird");
    }

    #[test]
    fn extract_selection_empty_when_row_missing() {
        let rows = vec!["only".to_string()];
        assert_eq!(extract_selection(&rows, (0, 5), (2, 5)), "");
    }

    #[test]
    fn osc52_sequence_wraps_plain_when_no_tmux() {
        std::env::remove_var("TMUX");
        let seq = osc52_sequence("hi");
        assert!(seq.starts_with("\x1b]52;c;"));
        assert!(seq.ends_with('\x07'));
        assert!(!seq.contains("Ptmux"));
    }

    #[test]
    fn osc52_sequence_wraps_tmux_passthrough_when_in_tmux() {
        std::env::set_var("TMUX", "/tmp/tmux-1000/default,1234,0");
        let seq = osc52_sequence("hi");
        std::env::remove_var("TMUX");
        assert!(seq.starts_with("\x1bPtmux;\x1b"));
        assert!(seq.ends_with("\x1b\\"));
        // inner ESCs from the wrapped OSC52 must be doubled
        assert!(seq.contains("\x1b\x1b]52;c;"));
    }

    #[test]
    fn mouse_selection_flow_copies_text_and_sets_flash() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.row_text = vec!["select me please".to_string()];
        app.mouse_down(0, 0);
        app.mouse_drag(9, 0);
        app.mouse_up(9, 0);
        assert!(app.flash.is_some());
        assert!(app.selection_start.is_none());
        assert!(app.selection_end.is_none());
    }

    #[test]
    fn mouse_selection_skips_copy_when_auto_copy_disabled() {
        let mut config = Config::default();
        config.ui.insert("auto_copy".into(), "false".into());
        let mut app = App::new(&config, "/tmp/repo");
        app.row_text = vec!["select me please".to_string()];
        app.mouse_down(0, 0);
        app.mouse_up(9, 0);
        assert!(app.flash.is_none());
    }
}
