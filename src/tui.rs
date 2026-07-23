//! Full-screen ratatui app: fixed layout (top bar, scrolling chat, pinned
//! AGENTES dashboard, input line). Porta `legacy/lib/regente/tui.rb` +
//! `screen.rb` + `dashboard.rb`. No streaming yet — Enter just echoes the
//! turn into chat as a placeholder until the master driver lands.

use crate::config::Config;
use crate::theme::{self, Role};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Terminal;
use std::io::Stdout;
use std::path::PathBuf;
use std::process::Command;
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
            AgentState::Running => "◍",
            AgentState::Done => "●",
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
            AgentState::Done => Role::Ok,
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

struct App {
    theme: String,
    master: String,
    repo: String,
    chat: Vec<ChatMsg>,
    input: String,
    agents: Vec<AgentRow>,
    logdir: PathBuf,
    master_session: String,
}

impl App {
    fn new(config: &Config, repo: &str) -> App {
        let theme = config.ui.get("theme").cloned().unwrap_or_else(|| theme::DEFAULT.to_string());
        let master = match &config.master.model {
            Some(m) => format!("{} · {}", config.master.cli, m),
            None => config.master.cli.clone(),
        };
        App {
            theme,
            master,
            repo: repo.to_string(),
            chat: vec![ChatMsg {
                role: ChatRole::Info,
                text: "Regente pronto. Digite uma tarefa. /theme <t> troca tema, /quit sai.".into(),
            }],
            input: String::new(),
            agents: Vec::new(),
            logdir: std::env::temp_dir().join("regente-logs"),
            master_session: "regente-master".into(),
        }
    }

    fn push(&mut self, role: ChatRole, text: impl Into<String>) {
        self.chat.push(ChatMsg { role, text: text.into() });
    }

    fn refresh_agents(&mut self) {
        self.agents = list_agents(&self.logdir, &self.master_session);
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
        self.push(ChatRole::User, format!("{}{}", theme::prompt(&self.theme), line));
        self.push(ChatRole::Assistant, "(streaming em breve)");
    }

    fn dispatch(&mut self, line: &str) {
        let mut parts = line.split_whitespace();
        match parts.next().unwrap_or("") {
            "/quit" | "/q" => {}
            "/theme" => match parts.next() {
                None => {
                    let msg = format!("temas: {}  (atual: {})", theme::names().join(", "), self.theme);
                    self.push(ChatRole::Info, msg);
                }
                Some(name) if theme::exists(name) => {
                    self.theme = name.to_string();
                }
                Some(name) => {
                    self.push(ChatRole::Error, format!("tema inexistente: {name}"));
                }
            },
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
            other => {
                self.push(ChatRole::Error, format!("comando desconhecido: {other}"));
            }
        }
    }
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

/// Entry point: run the fullscreen TUI until the user quits.
pub fn run(config: &Config, repo: &str) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config, repo);
    app.refresh_agents();
    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    let mut last_poll = Instant::now();
    loop {
        terminal.draw(|f| draw(f.area(), f.buffer_mut(), app))?;

        if last_poll.elapsed() >= Duration::from_secs(1) {
            app.refresh_agents();
            last_poll = Instant::now();
        }

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Char(c) => app.input.push(c),
                    KeyCode::Backspace => {
                        app.input.pop();
                    }
                    KeyCode::Enter => {
                        let line = app.input.trim().to_string();
                        if line == "/quit" || line == "/q" {
                            break;
                        }
                        app.submit();
                    }
                    KeyCode::Esc => break,
                    _ => {}
                }
            }
        }
    }
    Ok(())
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
        .title(Span::styled(title.to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD)))
}

fn draw(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App) {
    let outer = Layout::default().margin(1).constraints([Constraint::Min(0)]).split(area)[0];
    let chunks = Layout::vertical([
        Constraint::Length(6),
        Constraint::Min(1),
        Constraint::Length(6),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(outer);

    draw_header(chunks[0], buf, app);
    draw_chat(chunks[1], buf, app);
    draw_agents(chunks[2], buf, app);
    draw_input(chunks[3], buf, app);
    draw_statusbar(chunks[4], buf, app);
}

fn draw_header(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let left = rounded_block(&app.theme, " regente ", Role::Accent);
    let left_inner = left.inner(cols[0]);
    left.render(cols[0], buf);
    let lines = vec![
        Line::from(styled(&app.theme, Role::Accent, "REGENTE").add_modifier(Modifier::BOLD)),
        Line::from(styled(&app.theme, Role::Dim, format!("mestre {}", app.master))),
        Line::from(styled(&app.theme, Role::Dim, format!("repo {}", repo_name(app)))),
        Line::from(styled(&app.theme, Role::Dim, format!("tema {}", app.theme))),
    ];
    Paragraph::new(lines).render(left_inner, buf);

    let right = rounded_block(&app.theme, " comandos ", Role::Dim);
    let right_inner = right.inner(cols[1]);
    right.render(cols[1], buf);
    let lines = vec![
        Line::from(styled(&app.theme, Role::Dim, "/theme <t>  troca tema")),
        Line::from(styled(&app.theme, Role::Dim, "/agents  status")),
        Line::from(styled(&app.theme, Role::Dim, "/quit  sair")),
        Line::from(styled(&app.theme, Role::Accent, "digite uma tarefa e o mestre orquestra")),
    ];
    Paragraph::new(lines).render(right_inner, buf);
}

fn draw_chat(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App) {
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let rows = inner.height as usize;
    let msgs: Vec<Line> = app
        .chat
        .iter()
        .rev()
        .take(rows)
        .rev()
        .map(|m| chat_line(app, m))
        .collect();
    Paragraph::new(msgs).render(inner, buf);
}

fn chat_line(app: &App, m: &ChatMsg) -> Line<'static> {
    match m.role {
        ChatRole::User => Line::from(vec![
            styled(&app.theme, Role::Accent, "> "),
            styled(&app.theme, Role::Text, m.text.clone()),
        ]),
        ChatRole::Assistant => Line::from(vec![
            styled(&app.theme, Role::Accent, "● "),
            styled(&app.theme, Role::Text, m.text.clone()),
        ]),
        ChatRole::Tool => Line::from(styled(&app.theme, Role::Dim, format!("  ⚙ {}", m.text))),
        ChatRole::Info => Line::from(styled(&app.theme, Role::Dim, m.text.clone())),
        ChatRole::Error => Line::from(styled(&app.theme, Role::Fail, m.text.clone())),
    }
}

fn draw_agents(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App) {
    let block = rounded_block(&app.theme, " agentes ", Role::Accent);
    let inner = block.inner(area);
    block.render(area, buf);

    let lines: Vec<Line> = if app.agents.is_empty() {
        vec![Line::from(styled(&app.theme, Role::Dim, "nenhum agente ativo"))]
    } else {
        app.agents
            .iter()
            .map(|a| {
                let text = format!("{} {:<8} {:<8} ", a.state.icon(), a.name, a.state.label());
                Line::from(vec![
                    styled(&app.theme, a.state.role(), text),
                    styled(&app.theme, Role::Dim, a.last.clone()),
                ])
            })
            .collect()
    };
    Paragraph::new(lines).render(inner, buf);
}

fn draw_input(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App) {
    let block = rounded_block(&app.theme, "", Role::Accent);
    let inner = block.inner(area);
    block.render(area, buf);
    let line = Line::from(vec![
        styled(&app.theme, Role::Accent, theme::prompt(&app.theme)).add_modifier(Modifier::BOLD),
        Span::raw(app.input.clone()),
        Span::raw("█"),
    ]);
    Paragraph::new(line).render(inner, buf);
}

fn draw_statusbar(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App) {
    let line = Line::from(vec![
        styled(&app.theme, Role::Accent, "mestre "),
        styled(&app.theme, Role::Dim, format!("{}  ·  ", app.master)),
        styled(&app.theme, Role::Dim, format!("{}  ·  ", repo_name(app))),
        styled(&app.theme, Role::Accent, "tema "),
        styled(&app.theme, Role::Dim, format!("{}  ·  ", app.theme)),
        styled(&app.theme, Role::Dim, format!("{} agentes", app.agents.len())),
    ]);
    Paragraph::new(line).render(area, buf);
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
    fn app_submit_echoes_user_and_placeholder() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.input = "faz algo".into();
        app.submit();
        assert!(app.chat.iter().any(|m| m.text.contains("faz algo")));
        assert!(app.chat.iter().any(|m| m.text.contains("streaming em breve")));
    }
}
