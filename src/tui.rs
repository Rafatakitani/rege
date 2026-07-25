//! Full-screen ratatui app: fixed layout (header line, scrolling chat, pinned
//! AGENTES dashboard, input line, status line). Porta `legacy/lib/rege/tui.rb` +
//! `screen.rb` + `dashboard.rb`. No streaming yet — Enter just echoes the
//! turn into chat as a placeholder until the master driver lands.

use crate::command;
use crate::config::{Config, RosterEntry};
use crate::driver;
use crate::playbook;
use crate::scan;
use crate::sessions::{self, SessionRec};
use crate::stream;
use crate::theme::{self, Role};
use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
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
    /// Roster overlay: agents grouped by provider, each with its role. Also
    /// offers connecting installed-but-unlisted CLIs and a manual-add row.
    AgentsPicker { cursor: usize },
    /// Text prompt for adding a roster entry by hand: `cli [role] [model]`.
    AgentsAdd { input: String },
    /// First run in this directory: offer to scan it into an `AGENTS.md`.
    /// Answered with s/n; either answer is remembered so it's asked once.
    ScanOffer,
}

/// A rendered line in the agents overlay. `Header` rows are labels only; every
/// other variant is selectable and the picker cursor walks them in this order.
enum AgentsRow {
    Header(String),
    /// Index into `App::roster`.
    Entry(usize),
    /// Installed CLI absent from the roster — selecting it adds a worker.
    Connect(String),
    /// The "add by hand" action row.
    AddManual,
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
    input_cursor: usize,
    input_rect: Rect,
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
    roster: Vec<RosterEntry>,
    /// Known CLIs found on PATH, cached when the picker opens so rendering
    /// stays free of filesystem probes.
    installed_clis: Vec<String>,
    /// Highlighted row in the slash-command autocomplete popup. Only meaningful
    /// while the menu is open (input is a bare `/prefix`).
    menu_cursor: usize,
    /// Lines already sent, oldest first — what ↑/↓ walk through.
    history: Vec<String>,
    /// Position in `history` while browsing; `None` means the input is fresh.
    history_idx: Option<usize>,
    /// Whatever was typed before browsing started, restored on the way back.
    history_draft: String,
    /// Result of a running context scan, drained each tick like the chat stream.
    scan_rx: Option<Receiver<Result<String, String>>>,
    /// Where the "already asked here" list lives.
    scanned_path: PathBuf,
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
                text: "Rege pronto. Digite uma tarefa. /help lista comandos, /quit sai.".into(),
            }],
            input: String::new(),
            input_cursor: 0,
            input_rect: Rect::default(),
            agents: Vec::new(),
            logdir: std::env::temp_dir().join("rege-logs"),
            master_session: "rege-master".into(),
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
            roster: config.roster.clone(),
            installed_clis: Vec::new(),
            menu_cursor: 0,
            history: Vec::new(),
            history_idx: None,
            history_draft: String::new(),
            scan_rx: None,
            scanned_path: scan::state_path(&crate::dirs_home()),
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
            Mode::Normal
            | Mode::ResumePicker { .. }
            | Mode::AgentsPicker { .. }
            | Mode::AgentsAdd { .. }
            | Mode::ScanOffer => &self.theme,
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

    fn open_agents_picker(&mut self) {
        self.installed_clis =
            command::KNOWN_CLIS.iter().filter(|c| cli_installed(c)).map(|c| c.to_string()).collect();
        self.mode = Mode::AgentsPicker { cursor: 0 };
    }

    /// The overlay's rows, grouped by provider then by the connect/add actions.
    /// Both navigation and rendering consume this so the cursor never drifts
    /// from what's on screen.
    fn agents_rows(&self) -> Vec<AgentsRow> {
        let mut rows = Vec::new();
        // Providers in a stable order: known CLIs first, then any others the
        // roster references, so grouping is deterministic across frames.
        let mut providers: Vec<String> = Vec::new();
        for c in command::KNOWN_CLIS {
            if self.roster.iter().any(|r| r.cli == *c) {
                providers.push(c.to_string());
            }
        }
        for r in &self.roster {
            if !providers.contains(&r.cli) {
                providers.push(r.cli.clone());
            }
        }
        for cli in &providers {
            rows.push(AgentsRow::Header(cli.clone()));
            for (i, entry) in self.roster.iter().enumerate() {
                if &entry.cli == cli {
                    rows.push(AgentsRow::Entry(i));
                }
            }
        }
        let available: Vec<&String> =
            self.installed_clis.iter().filter(|c| !self.roster.iter().any(|r| &r.cli == *c)).collect();
        if !available.is_empty() {
            rows.push(AgentsRow::Header("disponíveis".into()));
            for cli in available {
                rows.push(AgentsRow::Connect(cli.clone()));
            }
        }
        rows.push(AgentsRow::AddManual);
        rows
    }

    /// Positions in `agents_rows()` that the cursor can land on (everything
    /// except headers).
    fn agents_selectable(&self) -> Vec<usize> {
        self.agents_rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| !matches!(r, AgentsRow::Header(_)))
            .map(|(i, _)| i)
            .collect()
    }

    fn agents_picker_move(&mut self, delta: isize) {
        if let Mode::AgentsPicker { cursor } = self.mode {
            let n = self.agents_selectable().len();
            if n == 0 {
                return;
            }
            let next = (cursor as isize + delta).clamp(0, n as isize - 1);
            self.mode = Mode::AgentsPicker { cursor: next as usize };
        }
    }

    /// Enter on the selected row: connect an available CLI, open the manual-add
    /// prompt, or do nothing on a roster entry (Enter is a no-op there; removal
    /// is `x`/Delete).
    fn agents_picker_confirm(&mut self) {
        let Mode::AgentsPicker { cursor } = self.mode else { return };
        let rows = self.agents_rows();
        let Some(&row_idx) = self.agents_selectable().get(cursor) else { return };
        match &rows[row_idx] {
            AgentsRow::Connect(cli) => {
                let cli = cli.clone();
                self.roster.push(RosterEntry { role: "worker".into(), cli: cli.clone(), model: None });
                self.persist_roster();
                self.push(ChatRole::Info, format!("agente conectado: {cli} (worker)"));
            }
            AgentsRow::AddManual => {
                self.mode = Mode::AgentsAdd { input: String::new() };
            }
            AgentsRow::Entry(_) | AgentsRow::Header(_) => {}
        }
    }

    /// `x`/Delete on a roster entry removes it from the roster and persists.
    fn agents_picker_remove(&mut self) {
        let Mode::AgentsPicker { cursor } = self.mode else { return };
        let rows = self.agents_rows();
        let Some(&row_idx) = self.agents_selectable().get(cursor) else { return };
        if let AgentsRow::Entry(i) = rows[row_idx] {
            if i < self.roster.len() {
                let removed = self.roster.remove(i);
                self.persist_roster();
                self.push(ChatRole::Info, format!("agente removido: {} ({})", removed.cli, removed.role));
                // Clamp the cursor: the row count just shrank.
                let n = self.agents_selectable().len();
                let next = cursor.min(n.saturating_sub(1));
                self.mode = Mode::AgentsPicker { cursor: next };
            }
        }
    }

    fn agents_picker_cancel(&mut self) {
        self.mode = Mode::Normal;
    }

    fn agents_add_confirm(&mut self) {
        let Mode::AgentsAdd { input } = &self.mode else { return };
        let mut parts = input.split_whitespace();
        let cli = parts.next().map(str::to_string);
        let role = parts.next().unwrap_or("worker").to_string();
        let model = parts.next().map(str::to_string);
        match cli {
            None => {
                self.push(ChatRole::Error, "uso: cli [role] [model] — ex: codex worker o3");
            }
            Some(cli) if !command::KNOWN_CLIS.contains(&cli.as_str()) => {
                self.push(
                    ChatRole::Error,
                    format!("cli desconhecido: {cli} (conhecidos: {})", command::KNOWN_CLIS.join(", ")),
                );
            }
            Some(cli) => {
                self.roster.push(RosterEntry { role: role.clone(), cli: cli.clone(), model: model.clone() });
                self.persist_roster();
                let m = model.as_deref().unwrap_or("(default)");
                self.push(ChatRole::Info, format!("agente adicionado: {cli} · {role} · {m}"));
            }
        }
        self.open_agents_picker();
    }

    fn persist_roster(&mut self) {
        if let Err(e) = save_roster_to_config(&self.roster) {
            self.push(ChatRole::Error, format!("erro ao salvar roster: {e}"));
        }
    }

    fn input_insert(&mut self, c: char) {
        let byte_idx = self.input.char_indices().nth(self.input_cursor).map(|(i, _)| i).unwrap_or(self.input.len());
        self.input.insert(byte_idx, c);
        self.input_cursor += 1;
        self.on_input_edited();
    }

    /// Any hand edit drops out of history browsing (the line is now the user's,
    /// not a recalled one) and re-arms the slash popup from the top.
    fn on_input_edited(&mut self) {
        self.menu_cursor = 0;
        self.history_idx = None;
    }

    /// Inserts pasted text as a single logical line: `input` has no real
    /// multi-line support (only visual word-wrap), so embedded newlines are
    /// flattened to spaces instead of becoming raw control glyphs.
    fn input_insert_str(&mut self, text: &str) {
        let text = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
        let byte_idx = self.input.char_indices().nth(self.input_cursor).map(|(i, _)| i).unwrap_or(self.input.len());
        self.input.insert_str(byte_idx, &text);
        self.input_cursor += text.chars().count();
        self.on_input_edited();
    }

    fn input_backspace(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let byte_idx = self.input.char_indices().nth(self.input_cursor - 1).map(|(i, _)| i).unwrap();
        self.input.remove(byte_idx);
        self.input_cursor -= 1;
        self.on_input_edited();
    }

    fn input_delete(&mut self) {
        if let Some((byte_idx, _)) = self.input.char_indices().nth(self.input_cursor) {
            self.input.remove(byte_idx);
        }
        self.on_input_edited();
    }

    fn input_left(&mut self) {
        self.input_cursor = self.input_cursor.saturating_sub(1);
    }

    fn input_right(&mut self) {
        let len = self.input.chars().count();
        self.input_cursor = (self.input_cursor + 1).min(len);
    }

    fn input_home(&mut self) {
        self.input_cursor = 0;
    }

    fn input_end(&mut self) {
        self.input_cursor = self.input.chars().count();
    }

    fn mouse_down(&mut self, col: u16, row: u16) {
        if in_rect(self.input_rect, col, row) {
            let inner_x = self.input_rect.x + 1; // left border
            let inner_y = self.input_rect.y + 1; // top border
            let text: Vec<char> = format!("❯ {}", self.input).chars().collect();
            let width = self.input_rect.width.saturating_sub(2).max(1) as usize;
            let ranges = wrap_char_ranges(&text, width);
            let line_idx = (row.saturating_sub(inner_y) as usize).min(ranges.len() - 1);
            let (start, end) = ranges[line_idx];
            let col_offset = (col.saturating_sub(inner_x) as usize).min(end - start);
            let clicked = (start + col_offset).saturating_sub(2); // drop "❯ " prefix
            self.input_cursor = clicked.min(self.input.chars().count());
            return;
        }
        self.selection_start = Some((col, row));
        self.selection_end = Some((col, row));
    }

    fn mouse_drag(&mut self, col: u16, row: u16) {
        if self.selection_start.is_some() {
            self.selection_end = Some((col, row));
        }
    }

    fn mouse_up(&mut self, col: u16, row: u16) {
        if self.selection_start.is_none() {
            return;
        }
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

    /// Slash-command matches for the autocomplete popup: non-empty only while
    /// the input is a bare `/prefix` (leading `/`, no space yet). Empty means
    /// the popup is closed.
    fn command_menu(&self) -> Vec<(&'static str, &'static str)> {
        // Recalling `/help` from history shouldn't reopen the popup — ↑↓ have
        // to keep meaning "walk the history" until the user types again.
        if self.history_idx.is_some() {
            return Vec::new();
        }
        let inp = self.input.trim_start();
        if !inp.starts_with('/') || inp.chars().any(char::is_whitespace) {
            return Vec::new();
        }
        COMMAND_CATALOG.iter().filter(|(cmd, _)| cmd.starts_with(inp)).copied().collect()
    }

    fn menu_open(&self) -> bool {
        !self.command_menu().is_empty()
    }

    fn menu_move(&mut self, delta: isize) {
        let n = self.command_menu().len();
        if n == 0 {
            return;
        }
        let next = (self.menu_cursor as isize + delta).rem_euclid(n as isize);
        self.menu_cursor = next as usize;
    }

    /// Index of the highlighted row, clamped the same way the popup draws it —
    /// what you see selected is what Tab/Enter take.
    fn menu_selected(&self, len: usize) -> usize {
        self.menu_cursor.min(len.saturating_sub(1))
    }

    /// Tab: replace the input with the highlighted command. No-op when the menu
    /// is closed, so Tab stays inert during normal typing.
    fn menu_complete(&mut self) {
        let menu = self.command_menu();
        if let Some((cmd, _)) = menu.get(self.menu_selected(menu.len())) {
            self.input = cmd.to_string();
            self.input_cursor = self.input.chars().count();
            self.menu_cursor = 0;
        }
    }

    /// Enter with the popup open runs the highlighted command, not the
    /// half-typed prefix under the caret. Returns false when it's closed, so
    /// the caller falls through to submitting whatever was typed.
    fn menu_accept(&mut self) -> bool {
        if self.command_menu().is_empty() {
            return false;
        }
        self.menu_complete();
        true
    }

    /// Records a sent line. Consecutive duplicates collapse — resending the
    /// same thing twice shouldn't cost two ↑ presses to get past.
    fn history_push(&mut self, line: &str) {
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_string());
        }
        self.history_idx = None;
        self.history_draft.clear();
    }

    /// ↑: walk back through sent lines. The half-typed input is stashed on the
    /// first step and comes back when you walk past the newest entry.
    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_idx {
            None => {
                self.history_draft = self.input.clone();
                self.history.len() - 1
            }
            Some(0) => 0, // already at the oldest — stay put
            Some(i) => i - 1,
        };
        self.history_idx = Some(idx);
        self.set_input(self.history[idx].clone());
    }

    /// ↓: walk forward; past the newest entry restores the stashed draft.
    fn history_next(&mut self) {
        let Some(i) = self.history_idx else {
            return;
        };
        if i + 1 < self.history.len() {
            self.history_idx = Some(i + 1);
            self.set_input(self.history[i + 1].clone());
        } else {
            self.history_idx = None;
            let draft = std::mem::take(&mut self.history_draft);
            self.set_input(draft);
        }
    }

    fn set_input(&mut self, text: String) {
        self.input = text;
        self.input_cursor = self.input.chars().count();
        self.menu_cursor = 0;
    }

    fn submit(&mut self) {
        let line = self.input.trim().to_string();
        self.input.clear();
        self.input_cursor = 0;
        if line.is_empty() {
            return;
        }
        self.history_push(&line);
        if line.starts_with('/') && is_known_command(&line) {
            self.dispatch(&line);
            return;
        }
        if line.starts_with('/') {
            let cmd = line.split_whitespace().next().unwrap_or(&line).to_string();
            self.push(ChatRole::Error, format!("comando desconhecido: {cmd}"));
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

    /// Asks about scanning this directory, but only where it's free of
    /// consequence: no context file yet and no answer on record for this path.
    fn offer_scan_if_first_run(&mut self) {
        let dir = PathBuf::from(&self.repo);
        if !scan::should_offer(&dir, &scan::load_state(&self.scanned_path)) {
            return;
        }
        self.mode = Mode::ScanOffer;
        self.push(
            ChatRole::Info,
            format!("primeira vez aqui ({}).", self.repo),
        );
        self.push(
            ChatRole::Info,
            format!(
                "escaneio o diretório e escrevo um {} descrevendo ele? [s/n]",
                scan::CONTEXT_FILE
            ),
        );
    }

    /// Records the answer so this is asked once per directory, whichever way
    /// it went, and kicks off the scan on yes.
    fn answer_scan_offer(&mut self, yes: bool) {
        self.mode = Mode::Normal;
        let dir = PathBuf::from(&self.repo);
        if let Err(e) = scan::record(&self.scanned_path, &dir, if yes { "yes" } else { "no" }) {
            self.push(ChatRole::Error, format!("não consegui gravar a resposta: {e}"));
        }
        if yes {
            self.start_scan(false);
        } else {
            self.push(ChatRole::Info, "ok, não pergunto de novo aqui. `/scan` roda quando quiser.");
        }
    }

    /// Runs the scan off-thread — it's a model call, and the UI shouldn't
    /// freeze for it. The result lands via `scan_rx` on a later tick.
    fn start_scan(&mut self, force: bool) {
        if self.scan_rx.is_some() {
            self.push(ChatRole::Info, "já tem um scan rodando.");
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        self.push(ChatRole::Info, "escaneando o diretório… (uma chamada ao mestre)");
        let dir = PathBuf::from(&self.repo);
        let cfg = self.scan_config();
        let home = crate::dirs_home();
        std::thread::spawn(move || {
            let msg = scan::run(&dir, &cfg, &home, force)
                .map(|p| p.display().to_string())
                .map_err(|e| e.to_string());
            let _ = tx.send(msg);
        });
    }

    /// The master as currently set in the TUI — `/model` changes apply to the
    /// scan too, instead of it silently using the on-disk config.
    fn scan_config(&self) -> Config {
        let mut cfg = Config::default();
        cfg.master.cli = self.master_cli.clone();
        cfg.master.model = self.master_model.clone();
        cfg
    }

    fn drain_scan(&mut self) {
        let Some(rx) = &self.scan_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(path)) => {
                self.push(ChatRole::Info, format!("✓ escrito: {path}"));
                self.scan_rx = None;
            }
            Ok(Err(e)) => {
                self.push(ChatRole::Error, format!("scan falhou: {e}"));
                self.scan_rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.scan_rx = None,
        }
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
            stream::Event::Tool { name, input } => self.push(ChatRole::Tool, tool_running_label(&name, &input)),
            // Result bodies (JSON, file lists, diffs) flood the master's log and
            // carry no signal here — the worker already acted on them. Collapse
            // to a marker; the running-line above shows what ran.
            stream::Event::ToolResult(_) => self.push(ChatRole::Info, "ran command".to_string()),
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
                self.push(ChatRole::Info, "  /agents          roster de agentes (ativos: /agents ativos)");
                self.push(ChatRole::Info, "  /scan [--force]  escaneia o diretório e escreve o AGENTS.md");
                self.push(ChatRole::Info, "  /buddy [pet]     bicho de estimação");
                self.push(ChatRole::Info, "  /quit            sai (ou exit/quit/:q)");
            }
            "/scan" => {
                let force = parts.next() == Some("--force");
                self.start_scan(force);
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
                self.push(ChatRole::Info, "edite ~/.config/rege/config.yml ou .rege.yml no projeto");
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
            "/agents" => match parts.next() {
                // `/agents ativos` keeps the old inline list of running workers;
                // bare `/agents` opens the roster overlay.
                Some("ativos") | Some("running") => {
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
                _ => self.open_agents_picker(),
            },
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

const KNOWN_COMMANDS: &[&str] =
    &["/quit", "/q", "/help", "/?", "/model", "/config", "/theme", "/resume", "/agents", "/buddy", "/scan"];

/// Commands surfaced in the autocomplete popup, with a one-line hint each.
/// Aliases (`/q`, `/?`) stay out of the menu but remain valid to type.
const COMMAND_CATALOG: &[(&str, &str)] = &[
    ("/help", "lista os comandos"),
    ("/theme", "seletor de tema (preview ao vivo)"),
    ("/model", "troca o modelo do mestre"),
    ("/config", "mostra a config efetiva"),
    ("/resume", "retoma sessão anterior"),
    ("/agents", "roster de agentes (conecta/remove)"),
    ("/scan", "escaneia o diretório e escreve o AGENTS.md"),
    ("/buddy", "bicho de estimação animado"),
    ("/quit", "sai do rege"),
];

fn is_known_command(line: &str) -> bool {
    let cmd = line.split_whitespace().next().unwrap_or(line);
    KNOWN_COMMANDS.contains(&cmd)
}

fn in_rect(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
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

/// Seed for hatching the buddy: `$USER`, else `$HOSTNAME`/`hostname(1)`, else "rege".
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
    "rege".to_string()
}

fn list_agents(logdir: &PathBuf, master_session: &str) -> Vec<AgentRow> {
    let sessions = tmux_sessions();
    sessions
        .into_iter()
        .filter(|s| s.starts_with("rege-") && s != master_session)
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
    let name = session.strip_prefix("rege-").unwrap_or(session).to_string();
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
/// plain text (one line per row). No tty needed — used by `rege render` and
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
            draw(f.area(), f.buffer_mut(), &mut app);
            out = capture_row_text(f.buffer_mut());
        })
        .expect("draw");
    out.join("\n")
}

/// Entry point: run the fullscreen TUI until the user quits.
pub fn run(config: &Config, repo: &str) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config, repo);
    app.refresh_agents();
    app.offer_scan_if_first_run();
    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture, DisableBracketedPaste)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    let mut last_poll = Instant::now();
    loop {
        app.drain_stream();
        app.drain_scan();
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
                        Mode::AgentsPicker { .. } => match key.code {
                            KeyCode::Up => app.agents_picker_move(-1),
                            KeyCode::Down => app.agents_picker_move(1),
                            KeyCode::Enter => app.agents_picker_confirm(),
                            KeyCode::Char('a') => app.mode = Mode::AgentsAdd { input: String::new() },
                            KeyCode::Char('x') | KeyCode::Delete => app.agents_picker_remove(),
                            KeyCode::Esc => app.agents_picker_cancel(),
                            _ => {}
                        },
                        Mode::ScanOffer => match key.code {
                            KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Char('y') => {
                                app.answer_scan_offer(true)
                            }
                            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                app.answer_scan_offer(false)
                            }
                            _ => {}
                        },
                        Mode::AgentsAdd { .. } => match key.code {
                            KeyCode::Char(c) => {
                                if let Mode::AgentsAdd { input } = &mut app.mode {
                                    input.push(c);
                                }
                            }
                            KeyCode::Backspace => {
                                if let Mode::AgentsAdd { input } = &mut app.mode {
                                    input.pop();
                                }
                            }
                            KeyCode::Enter => app.agents_add_confirm(),
                            KeyCode::Esc => app.open_agents_picker(),
                            _ => {}
                        },
                        Mode::Normal => match key.code {
                            // Tab / ↑↓ drive the slash-command popup while it's
                            // open; with it closed, ↑↓ walk the sent-line history.
                            KeyCode::Tab => app.menu_complete(),
                            KeyCode::Up => {
                                if app.menu_open() {
                                    app.menu_move(-1)
                                } else {
                                    app.history_prev()
                                }
                            }
                            KeyCode::Down => {
                                if app.menu_open() {
                                    app.menu_move(1)
                                } else {
                                    app.history_next()
                                }
                            }
                            KeyCode::Char(c) => app.input_insert(c),
                            KeyCode::Backspace => app.input_backspace(),
                            KeyCode::Delete => app.input_delete(),
                            KeyCode::Left => app.input_left(),
                            KeyCode::Right => app.input_right(),
                            KeyCode::Home => app.input_home(),
                            KeyCode::End => app.input_end(),
                            KeyCode::Enter => {
                                app.menu_accept();
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
                Event::Paste(text) => app.input_insert_str(&text),
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

fn draw(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &mut App) {
    let theme = app.active_theme().to_string();
    let theme = theme.as_str();
    let input_height = input_area_height(&app.input, area.width);
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(6),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .split(area);
    app.input_rect = chunks[3];

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
    if let Mode::AgentsPicker { cursor } = app.mode {
        draw_agents_picker(area, buf, app, cursor);
    }
    if let Mode::AgentsAdd { .. } = &app.mode {
        draw_agents_add(area, buf, app);
    }
    if matches!(app.mode, Mode::Normal) {
        draw_command_menu(buf, app, app.input_rect);
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
    let line = Line::from(vec![styled(theme, Role::Accent, "rege"), styled(theme, Role::Dim, suffix)]);
    Paragraph::new(line).render(area, buf);
}

/// REGE wordmark, block letters, shown atop the chat only before the
/// first user message — sober, no glow, just the desaturated dim tone.
const BANNER: [&str; 5] = [
    "████  █████ ████  █████",
    "██ ██ ██    ██    ██   ",
    "████  ████  ██ ██ ████ ",
    "██ ██ ██    ██ ██ ██   ",
    "██ ██ █████ ████  █████",
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
    // Expand every message into its wrapped visual rows, then keep only the
    // last `rows` — height is measured in display rows, not messages, so
    // multi-line/long-line output neither collides nor clips.
    let mut lines: Vec<Line> = app
        .chat
        .iter()
        .flat_map(|m| chat_lines(theme, m, area.width))
        .collect();
    let start = lines.len().saturating_sub(rows);
    let visible = lines.split_off(start);
    Paragraph::new(visible).render(area, buf);
}

/// Compact one-line label for a running tool call. For a shell command it
/// surfaces just the opening of the command ("running bash gh pr view 268…");
/// for anything else, the tool name alone. Never dumps the full input.
fn tool_running_label(name: &str, input: &str) -> String {
    let low = name.to_ascii_lowercase();
    let detail = serde_json::from_str::<serde_json::Value>(input)
        .ok()
        .and_then(|v| {
            v.get("command")
                .or_else(|| v.get("description"))
                .and_then(|c| c.as_str())
                .map(str::to_string)
        });
    match detail {
        Some(cmd) => format!("running {low} {}", first_bit(&cmd, 50)),
        None => format!("running {low}"),
    }
}

/// First line of `s`, trimmed to `max` chars, with an ellipsis if anything was
/// dropped (either a newline or the length cap).
fn first_bit(s: &str, max: usize) -> String {
    let line = s.trim_start();
    let first = line.lines().next().unwrap_or("");
    let truncated: String = first.chars().take(max).collect();
    let clipped = truncated.chars().count() < first.chars().count() || first.len() < line.len();
    if clipped {
        format!("{}…", truncated.trim_end())
    } else {
        truncated
    }
}

/// One chat message → its display rows: text is split on embedded `\n` and each
/// segment hard-wrapped to `width`, with continuation rows indented under the
/// role glyph so the left gutter stays aligned.
fn chat_lines(theme: &str, m: &ChatMsg, width: u16) -> Vec<Line<'static>> {
    let (prefix, prefix_role, body_role) = match m.role {
        ChatRole::User => ("❯ ", Role::Accent, Role::Text),
        ChatRole::Assistant => ("● ", Role::Accent, Role::Text),
        ChatRole::Tool => ("  ⚙ ", Role::Dim, Role::Dim),
        ChatRole::Info => ("", Role::Dim, Role::Dim),
        ChatRole::Error => ("", Role::Fail, Role::Fail),
    };
    let indent_cols = prefix.chars().count();
    let budget = (width as usize).saturating_sub(indent_cols).max(1);
    let indent = " ".repeat(indent_cols);
    wrap_text(&m.text, budget)
        .into_iter()
        .enumerate()
        .map(|(i, seg)| {
            let gutter = if i == 0 { prefix.to_string() } else { indent.clone() };
            if gutter.is_empty() {
                Line::from(styled(theme, body_role, seg))
            } else {
                Line::from(vec![
                    styled(theme, prefix_role, gutter),
                    styled(theme, body_role, seg),
                ])
            }
        })
        .collect()
}

/// Splits on `\n`, then hard-wraps each line to `width` chars. Blank lines are
/// preserved. Always returns at least one (possibly empty) segment.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let chars: Vec<char> = raw.chars().collect();
        if chars.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut i = 0;
        while i < chars.len() {
            let end = (i + width).min(chars.len());
            out.push(chars[i..end].iter().collect());
            i = end;
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
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

/// Lines the input text wraps to at `width`, plus 2 border rows, clamped to a
/// sane range so the box grows without swallowing the whole screen. Uses
/// ratatui's own word-wrap line count (same algorithm `draw_input` renders
/// with) so this matches the real wrapped height, not a char-count estimate.
fn input_area_height(input: &str, width: u16) -> u16 {
    let inner_width = width.saturating_sub(2).max(1);
    let text = format!("❯ {input}");
    let lines = Paragraph::new(text).wrap(ratatui::widgets::Wrap { trim: false }).line_count(inner_width) as u16;
    (lines.max(1) + 2).clamp(3, 8)
}

/// Word-wraps `text` at `width` columns, returning `(start, end)` char-index
/// ranges per visual line. Approximates ratatui's `Wrap { trim: false }`
/// composer (which is private) closely enough to map a click's row back to
/// the right line — exact mid-word placement isn't required.
fn wrap_char_ranges(text: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![(0, 0)];
    }
    let mut words: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < text.len() && !text[i].is_whitespace() {
            i += 1;
        }
        words.push((start, i));
    }
    if words.is_empty() {
        return vec![(0, text.len())];
    }
    let mut lines = Vec::new();
    let mut line_start = 0usize;
    let mut line_len = 0usize;
    let mut prev_word_end = 0usize;
    for (idx, &(wstart, wend)) in words.iter().enumerate() {
        let word_len = wend - wstart;
        let sep = if idx == 0 { wstart - line_start } else { wstart - prev_word_end };
        let projected = line_len + sep + word_len;
        if line_len > 0 && projected > width {
            lines.push((line_start, prev_word_end));
            line_start = wstart;
            line_len = word_len;
        } else {
            line_len = projected;
        }
        prev_word_end = wend;
    }
    lines.push((line_start, text.len()));
    lines
}

fn draw_input(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, theme: &str) {
    let block = rounded_block(theme, "", Role::Dim);
    let inner = block.inner(area);
    block.render(area, buf);

    let chars: Vec<char> = app.input.chars().collect();
    let cursor = app.input_cursor.min(chars.len());
    let before: String = chars[..cursor].iter().collect();
    let (cursor_span, after) = if cursor < chars.len() {
        let under = chars[cursor].to_string();
        let after: String = chars[cursor + 1..].iter().collect();
        let span = Span::styled(
            under,
            Style::default().fg(rgb(theme::color(theme, Role::Text))).add_modifier(Modifier::REVERSED),
        );
        (span, after)
    } else {
        (styled(theme, Role::Neon, "█"), String::new())
    };

    let line = Line::from(vec![
        styled(theme, Role::Accent, "❯ "),
        styled(theme, Role::Text, before),
        cursor_span,
        styled(theme, Role::Text, after),
    ]);
    Paragraph::new(line).wrap(ratatui::widgets::Wrap { trim: false }).render(inner, buf);
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

fn draw_agents_picker(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, cursor: usize) {
    let theme = app.active_theme();
    let rows = app.agents_rows();
    let height = rows.len() as u16 + 6;
    let rect = centered_rect(60, height, area);

    // Clear the popup area so the background chat text doesn't bleed through.
    Paragraph::new("").render(rect, buf);

    let block = rounded_block(theme, " agentes ", Role::Dim);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(styled(theme, Role::Text, "Roster de agentes").add_modifier(Modifier::BOLD)));
    lines.push(Line::from(styled(theme, Role::Dim, "↑↓ navega · Enter conecta · a adiciona · x remove · Esc fecha")));

    // Walk the same rows the cursor walks, tracking the selectable index so the
    // highlighted line matches the cursor exactly.
    let mut sel = 0usize;
    for row in &rows {
        match row {
            AgentsRow::Header(name) => {
                lines.push(Line::from(styled(theme, Role::Accent2, name.clone()).add_modifier(Modifier::BOLD)));
            }
            AgentsRow::Entry(i) => {
                let entry = &app.roster[*i];
                let model = entry.model.as_deref().unwrap_or("(default)");
                let text = format!("{:<10} {}", entry.role, model);
                lines.push(agents_line(theme, sel == cursor, &text));
                sel += 1;
            }
            AgentsRow::Connect(cli) => {
                let text = format!("conectar {cli} (instalado)");
                lines.push(agents_line(theme, sel == cursor, &text));
                sel += 1;
            }
            AgentsRow::AddManual => {
                lines.push(agents_line(theme, sel == cursor, "+ adicionar manual"));
                sel += 1;
            }
        }
    }

    lines.push(Line::from(styled(theme, Role::Dim, "grava em ~/.config/rege/config.yml")));
    Paragraph::new(lines).render(inner, buf);
}

/// One selectable line in the agents overlay, with the shared chevron/highlight
/// styling the other pickers use.
fn agents_line(theme: &str, selected: bool, text: &str) -> Line<'static> {
    let chevron =
        if selected { styled(theme, Role::Accent, "❯ ") } else { styled(theme, Role::Dim, "  ") };
    let span = if selected {
        styled(theme, Role::Accent, format!("  {text}")).add_modifier(Modifier::BOLD)
    } else {
        styled(theme, Role::Text, format!("  {text}"))
    };
    Line::from(vec![chevron, span])
}

fn draw_agents_add(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App) {
    let theme = app.active_theme();
    let input = match &app.mode {
        Mode::AgentsAdd { input } => input.as_str(),
        _ => "",
    };
    let rect = centered_rect(60, 7, area);
    Paragraph::new("").render(rect, buf);
    let block = rounded_block(theme, " novo agente ", Role::Dim);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let lines = vec![
        Line::from(styled(theme, Role::Text, "Adicionar agente").add_modifier(Modifier::BOLD)),
        Line::from(styled(theme, Role::Dim, "formato: cli [role] [model] · Enter grava · Esc volta")),
        Line::from(vec![
            styled(theme, Role::Accent, "❯ "),
            styled(theme, Role::Text, input.to_string()),
            styled(theme, Role::Accent, "▏"),
        ]),
        Line::from(styled(theme, Role::Dim, format!("clis: {}", command::KNOWN_CLIS.join(", ")))),
    ];
    Paragraph::new(lines).render(inner, buf);
}

/// Slash-command autocomplete popup, anchored just above the input box. Draws
/// nothing when the menu is closed (input isn't a bare `/prefix`).
fn draw_command_menu(buf: &mut ratatui::buffer::Buffer, app: &App, input_rect: Rect) {
    let menu = app.command_menu();
    if menu.is_empty() || input_rect.width == 0 {
        return;
    }
    let theme = app.active_theme();
    let height = menu.len() as u16 + 2; // borders
    // Sit directly above the input; clamp so it never overflows the top edge.
    let y = input_rect.y.saturating_sub(height);
    let width = input_rect.width.min(60).max(24);
    let rect = Rect { x: input_rect.x, y, width, height };

    Paragraph::new("").render(rect, buf);
    let block = rounded_block(theme, " comandos ", Role::Dim);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let cursor = app.menu_selected(menu.len());
    let lines: Vec<Line> = menu
        .iter()
        .enumerate()
        .map(|(i, (cmd, desc))| {
            let selected = i == cursor;
            let chevron =
                if selected { styled(theme, Role::Accent, "❯ ") } else { styled(theme, Role::Dim, "  ") };
            let name = if selected {
                styled(theme, Role::Accent, format!("{cmd:<10}")).add_modifier(Modifier::BOLD)
            } else {
                styled(theme, Role::Text, format!("{cmd:<10}"))
            };
            Line::from(vec![chevron, name, styled(theme, Role::Dim, format!(" {desc}"))])
        })
        .collect();
    Paragraph::new(lines).render(inner, buf);
}

/// True when `cli` is a file on any PATH entry.
fn cli_installed(cli: &str) -> bool {
    cli_on_path(cli, std::env::var_os("PATH"))
}

fn cli_on_path(cli: &str, paths: Option<std::ffi::OsString>) -> bool {
    let Some(paths) = paths else { return false };
    std::env::split_paths(&paths).any(|dir| dir.join(cli).is_file())
}

/// Overwrite the `roster:` key in the global config with the current roster,
/// preserving every other key (same load-merge-write dance as the theme save).
fn save_roster_to_config(roster: &[RosterEntry]) -> Result<()> {
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
    map.insert(serde_yaml::Value::String("roster".into()), serde_yaml::to_value(roster)?);
    std::fs::write(&path, serde_yaml::to_string(&value)?)?;
    Ok(())
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
    fn tool_label_surfaces_command_opening_only() {
        // Short command: shown in full.
        let short = r#"{"command":"gh pr view 268"}"#;
        assert_eq!(tool_running_label("Bash", short), "running bash gh pr view 268");
        // Long command: clipped to the opening with an ellipsis.
        let long = r#"{"command":"gh pr view 268 --json additions,body,files,commits,reviews"}"#;
        assert_eq!(tool_running_label("Bash", long), "running bash gh pr view 268 --json additions,body,files,commits…");
        // Non-bash tool with no command → name only.
        assert_eq!(tool_running_label("mcp__rege__consult", "{}"), "running mcp__rege__consult");
        // Garbage input never panics.
        assert_eq!(tool_running_label("Bash", "not json"), "running bash");
    }

    #[test]
    fn first_bit_clips_multiline_and_long() {
        assert_eq!(first_bit("short", 50), "short");
        assert_eq!(first_bit("line1\nline2", 50), "line1…");
        assert_eq!(first_bit("abcdefghij", 4), "abcd…");
    }

    #[test]
    fn wrap_text_splits_newlines_and_hard_wraps() {
        // Embedded newlines become separate rows — no more jammed output.
        assert_eq!(wrap_text("a\nb", 10), vec!["a", "b"]);
        // Long line wraps at the width budget instead of being clipped.
        assert_eq!(wrap_text("abcdef", 3), vec!["abc", "def"]);
        // Blank input still yields one (empty) row.
        assert_eq!(wrap_text("", 5), vec![String::new()]);
        // Zero width is clamped to 1, never panics or loops forever.
        assert_eq!(wrap_text("ab", 0), vec!["a", "b"]);
    }

    #[test]
    fn chat_lines_expands_multiline_message_to_multiple_rows() {
        let m = ChatMsg { role: ChatRole::Tool, text: "line1\nline2".into() };
        let lines = chat_lines("hacker", &m, 40);
        assert_eq!(lines.len(), 2);
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
        assert!(out.contains("rege")); // header
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

    /// `TMUX` is process-global env; both osc52 tests mutate it, so serialize
    /// them to keep the assertions from racing under parallel test execution.
    static TMUX_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn osc52_sequence_wraps_plain_when_no_tmux() {
        let _guard = TMUX_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("TMUX");
        let seq = osc52_sequence("hi");
        assert!(seq.starts_with("\x1b]52;c;"));
        assert!(seq.ends_with('\x07'));
        assert!(!seq.contains("Ptmux"));
    }

    #[test]
    fn osc52_sequence_wraps_tmux_passthrough_when_in_tmux() {
        let _guard = TMUX_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    #[test]
    fn agents_rows_group_by_provider_and_end_with_add_manual() {
        let config = Config::default();
        let app = App::new(&config, "/tmp/repo");
        let rows = app.agents_rows();
        // Default roster references claude, codex, opencode → a header each.
        let headers: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                AgentsRow::Header(h) => Some(h.as_str()),
                _ => None,
            })
            .collect();
        assert!(headers.contains(&"claude"));
        assert!(headers.contains(&"codex"));
        // Last row is always the manual-add action.
        assert!(matches!(rows.last(), Some(AgentsRow::AddManual)));
        // Every Entry under the claude header actually has cli == claude.
        for r in &rows {
            if let AgentsRow::Entry(i) = r {
                assert!(command::KNOWN_CLIS.contains(&app.roster[*i].cli.as_str()));
            }
        }
    }

    #[test]
    fn agents_selectable_skips_headers() {
        let config = Config::default();
        let app = App::new(&config, "/tmp/repo");
        let rows = app.agents_rows();
        let headers = rows.iter().filter(|r| matches!(r, AgentsRow::Header(_))).count();
        assert_eq!(app.agents_selectable().len(), rows.len() - headers);
        // None of the selectable positions point at a header.
        for &idx in &app.agents_selectable() {
            assert!(!matches!(rows[idx], AgentsRow::Header(_)));
        }
    }

    #[test]
    fn agents_picker_move_clamps_to_bounds() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.mode = Mode::AgentsPicker { cursor: 0 };
        app.agents_picker_move(-5);
        assert!(matches!(app.mode, Mode::AgentsPicker { cursor: 0 }));
        app.agents_picker_move(10_000);
        let last = app.agents_selectable().len() - 1;
        assert!(matches!(app.mode, Mode::AgentsPicker { cursor } if cursor == last));
    }

    #[test]
    fn agents_add_confirm_rejects_unknown_cli() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        let before = app.roster.len();
        app.mode = Mode::AgentsAdd { input: "notacli worker".into() };
        // HOME points at a throwaway dir so a valid add wouldn't touch the real
        // config; here the cli is invalid so nothing is written regardless.
        app.agents_add_confirm();
        assert_eq!(app.roster.len(), before, "roster unchanged on unknown cli");
        assert!(matches!(app.mode, Mode::AgentsPicker { .. }));
        assert!(app.chat.iter().any(|m| matches!(m.role, ChatRole::Error)));
    }

    #[test]
    fn command_menu_filters_by_prefix_and_closes_on_space() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        // Bare slash lists everything.
        app.input = "/".into();
        assert_eq!(app.command_menu().len(), COMMAND_CATALOG.len());
        // Prefix narrows it.
        app.input = "/co".into();
        let m = app.command_menu();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, "/config");
        // A space means the command is chosen — popup closes so args can be typed.
        app.input = "/model ".into();
        assert!(app.command_menu().is_empty());
        // Non-slash input never opens the menu.
        app.input = "faz X".into();
        assert!(app.command_menu().is_empty());
    }

    #[test]
    fn menu_move_wraps_and_complete_fills_input() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.input = "/".into();
        let n = app.command_menu().len();
        app.menu_move(-1); // wrap from 0 to last
        assert_eq!(app.menu_cursor, n - 1);
        app.menu_move(1); // wrap back to 0
        assert_eq!(app.menu_cursor, 0);
        app.menu_complete();
        assert_eq!(app.input, COMMAND_CATALOG[0].0);
        assert_eq!(app.input_cursor, app.input.chars().count());
    }

    #[test]
    fn enter_runs_the_highlighted_command_not_the_typed_prefix() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        // Bare `/` with the popup open and the cursor moved off the first row:
        // Enter must take that row, not the lone slash under the caret.
        app.input = "/".into();
        app.menu_move(2);
        let expected = app.command_menu()[2].0;
        assert!(app.menu_accept());
        assert_eq!(app.input, expected);
    }

    #[test]
    fn menu_accept_leaves_plain_text_alone() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.input = "faz X".into();
        assert!(!app.menu_accept(), "closed menu must fall through to submit");
        assert_eq!(app.input, "faz X");
    }

    fn scan_tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rege-tui-scan-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn scan_offer_asks_once_then_remembers_the_no() {
        let dir = scan_tmp("offer");
        let config = Config::default();
        let mut app = App::new(&config, dir.to_str().unwrap());
        app.scanned_path = dir.join("state/scanned.yml");

        app.offer_scan_if_first_run();
        assert!(matches!(app.mode, Mode::ScanOffer));
        assert!(app.chat.iter().any(|m| m.text.contains(scan::CONTEXT_FILE)));

        app.answer_scan_offer(false);
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.scan_rx.is_none(), "não é pra ter disparado scan nenhum");

        // Segunda abertura no mesmo diretório: silêncio.
        let mut app2 = App::new(&config, dir.to_str().unwrap());
        app2.scanned_path = dir.join("state/scanned.yml");
        app2.offer_scan_if_first_run();
        assert!(matches!(app2.mode, Mode::Normal));
    }

    #[test]
    fn scan_offer_stays_quiet_when_the_file_already_exists() {
        let dir = scan_tmp("existing");
        std::fs::write(dir.join(scan::CONTEXT_FILE), "# escrito à mão\n").unwrap();
        let config = Config::default();
        let mut app = App::new(&config, dir.to_str().unwrap());
        app.scanned_path = dir.join("state/scanned.yml");

        app.offer_scan_if_first_run();
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn scan_uses_the_master_currently_set_in_the_tui() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.master_cli = "codex".into();
        app.master_model = Some("o3".into());
        let cfg = app.scan_config();
        assert_eq!(cfg.master.cli, "codex");
        assert_eq!(cfg.master.model.as_deref(), Some("o3"), "/model vale pro scan também");
    }

    #[test]
    fn history_walks_back_and_forward_through_sent_lines() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.history_push("primeira");
        app.history_push("segunda");

        app.history_prev();
        assert_eq!(app.input, "segunda", "↑ pega a mais recente");
        assert_eq!(app.input_cursor, "segunda".chars().count(), "cursor no fim");
        app.history_prev();
        assert_eq!(app.input, "primeira");
        app.history_prev();
        assert_eq!(app.input, "primeira", "para na mais antiga, não some");

        app.history_next();
        assert_eq!(app.input, "segunda");
        app.history_next();
        assert_eq!(app.input, "", "passou da mais nova: volta pro input vazio");
        assert!(app.history_idx.is_none());
    }

    #[test]
    fn history_restores_the_half_typed_draft() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.history_push("comando antigo");
        app.input = "estava escrevendo isso".into();

        app.history_prev();
        assert_eq!(app.input, "comando antigo");
        app.history_next();
        assert_eq!(app.input, "estava escrevendo isso", "o rascunho volta intacto");
    }

    #[test]
    fn history_collapses_consecutive_duplicates() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.history_push("igual");
        app.history_push("igual");
        assert_eq!(app.history.len(), 1);
        app.history_push("outra");
        app.history_push("igual");
        assert_eq!(app.history.len(), 3, "não-consecutiva entra de novo");
    }

    #[test]
    fn history_prev_is_noop_on_first_run() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.input = "rascunho".into();
        app.history_prev();
        assert_eq!(app.input, "rascunho", "sem histórico, ↑ não mexe no input");
    }

    #[test]
    fn recalled_slash_command_does_not_reopen_the_popup() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.history_push("/help");
        app.history_prev();
        assert_eq!(app.input, "/help");
        assert!(!app.menu_open(), "↑↓ seguem sendo histórico até digitar de novo");
        // Digitar re-arma o popup.
        app.input_insert('x');
        app.input_backspace();
        assert!(app.menu_open());
    }

    #[test]
    fn typing_leaves_history_browsing() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.history_push("antiga");
        app.history_prev();
        assert!(app.history_idx.is_some());
        app.input_insert('!');
        assert!(app.history_idx.is_none(), "a linha agora é do usuário");
        assert_eq!(app.input, "antiga!");
    }

    #[test]
    fn menu_complete_is_noop_when_menu_closed() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.input = "faz X".into();
        app.menu_complete();
        assert_eq!(app.input, "faz X");
    }

    #[test]
    fn typing_resets_menu_cursor() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.input = "/".into();
        app.menu_move(1);
        assert_ne!(app.menu_cursor, 0);
        app.input_insert('h');
        assert_eq!(app.menu_cursor, 0);
    }

    #[test]
    fn cli_on_path_finds_binary_only_when_present() {
        let dir = std::env::temp_dir().join(format!("rege-clipath-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("mycli"), "#!/bin/sh\n").unwrap();
        let paths = std::env::join_paths([dir.as_os_str()]).unwrap();
        assert!(cli_on_path("mycli", Some(paths.clone())));
        assert!(!cli_on_path("ghostcli", Some(paths)));
        assert!(!cli_on_path("mycli", None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
