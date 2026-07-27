//! Full-screen ratatui app: fixed layout (header line, scrolling chat, pinned
//! AGENTES dashboard, input line, status line). Porta `legacy/lib/rege/tui.rb` +
//! `screen.rb` + `dashboard.rb`. No streaming yet — Enter just echoes the
//! turn into chat as a placeholder until the master driver lands.

use crate::command;
use crate::config::{Config, RosterEntry};
use crate::driver;
use crate::grill;
use crate::playbook;
use crate::scan;
use crate::sessions::{self, SessionRec};
use crate::stream;
use crate::theme::{self, Role};
use crate::transcript;
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
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Terminal;
use std::io::Stdout;
use std::path::PathBuf;
use std::process::{Command, Stdio};
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
    ToolResult,
    Info,
    Error,
}

impl ChatRole {
    fn role(self) -> Role {
        match self {
            ChatRole::User => Role::Accent,
            ChatRole::Assistant => Role::Text,
            ChatRole::Tool => Role::Dim,
            ChatRole::ToolResult => Role::Dim,
            ChatRole::Info => Role::Dim,
            ChatRole::Error => Role::Fail,
        }
    }
}

struct ChatMsg {
    role: ChatRole,
    text: String,
}

/// What the first-run overlay offers. Three routes to the same file, and the
/// answer is recorded either way so the question is asked once per directory.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Offer {
    /// The master interviews the user, then writes the docs.
    Grill,
    /// One-shot read of the tree into an AGENTS.md.
    Scan,
    No,
}

impl Offer {
    fn from_cursor(cursor: usize) -> Self {
        match cursor {
            0 => Offer::Grill,
            1 => Offer::Scan,
            _ => Offer::No,
        }
    }

    /// What lands in `scanned.yml`. Kept distinct so a later version can tell
    /// "was interviewed" from "was scanned" without guessing.
    fn recorded(self) -> &'static str {
        match self {
            Offer::Grill => "grill",
            Offer::Scan => "yes",
            Offer::No => "no",
        }
    }
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
    /// Command list as a picker: Enter runs the highlighted command, so `/help`
    /// is a way *into* the commands instead of a wall of text to read and retype.
    HelpPicker { cursor: usize },
    /// Master model, typed in. A fixed list would be a guess that rots — model
    /// names change under us — so this shows the current one and takes a new one.
    ModelInput { input: String },
    /// Read-only panel for command output that used to scroll past in the log
    /// (`/config`, `/agents ativos`). Any key closes it.
    InfoPanel { title: String, lines: Vec<String> },
    /// First run in this directory: offer to scan it into an `AGENTS.md`.
    /// Answered with s/n or the cursor; either answer is remembered so it's
    /// asked once. `cursor` walks [sim, não] like the other pickers.
    ScanOffer { cursor: usize },
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
    /// Home dir, kept as a field so tests can point the transcript lookup at a
    /// fixture instead of the real `~/.claude`.
    home: PathBuf,
    /// Visual rows scrolled back from the bottom. 0 means pinned to the newest
    /// message, which is where a fresh session sits.
    scroll: usize,
    /// Rows the chat pane had on the last frame — the page size for PageUp/Down
    /// and the clamp for how far back scrolling can go.
    chat_rows: usize,
    /// Total wrapped rows the conversation currently occupies.
    chat_total: usize,
    /// Screen row where the chat pane starts, so a drag knows when it has hit
    /// the top edge and should scroll instead of stopping.
    chat_top: u16,
    /// When the running turn started, for the activity line. `None` = idle.
    turn_started: Option<Instant>,
    /// Characters streamed back this turn, the basis for the token estimate.
    turn_chars: usize,
    /// Frame counter, advanced every event-loop pass (~200ms) so the spinner
    /// animates independently of how often the model sends something.
    spinner: usize,
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
                text: "rege ready. Type a task. /help lists the commands, /quit leaves.".into(),
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
            home: crate::dirs_home(),
            scroll: 0,
            chat_rows: 0,
            chat_total: 0,
            chat_top: 0,
            turn_started: None,
            turn_chars: 0,
            spinner: 0,
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
            | Mode::ScanOffer { .. }
            | Mode::HelpPicker { .. }
            | Mode::ModelInput { .. }
            | Mode::InfoPanel { .. } => &self.theme,
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
                self.push(ChatRole::Error, format!("could not save the theme: {e}"));
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
                let (id, title) = (rec.id.clone(), rec.title.clone());
                self.session_id = Some(id.clone());
                self.push(ChatRole::Info, format!("resuming session: {title}"));
                self.replay_transcript(&id);
            }
            self.mode = Mode::Normal;
        }
    }

    /// Repaints the past conversation into the chat pane. Resuming used to land
    /// on an empty screen with only "retomando sessão: <título>" — the context
    /// was there for the model but invisible to the user.
    ///
    /// The transcript belongs to the driver CLI, so this is best-effort: no file
    /// (other CLI, cleaned-up history) just means no replay, same as before.
    fn replay_transcript(&mut self, session_id: &str) {
        let path = transcript::transcript_path(&self.home, &self.repo, session_id);
        let turns = transcript::read(&path);
        if turns.is_empty() {
            return;
        }
        for turn in &turns {
            let role = if turn.from_user { ChatRole::User } else { ChatRole::Assistant };
            self.push(role, turn.text.clone());
        }
        self.push(ChatRole::Info, format!("— end of history ({} messages) · carry on from here —", turns.len()));
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
                self.push(ChatRole::Info, format!("agent connected: {cli} (worker)"));
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
                self.push(ChatRole::Info, format!("agent removed: {} ({})", removed.cli, removed.role));
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
                self.push(ChatRole::Error, "usage: cli [role] [model] — e.g. codex worker o3");
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
                self.push(ChatRole::Info, format!("agent added: {cli} · {role} · {m}"));
            }
        }
        self.open_agents_picker();
    }

    fn persist_roster(&mut self) {
        if let Err(e) = save_roster_to_config(&self.roster) {
            self.push(ChatRole::Error, format!("could not save the roster: {e}"));
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

    /// Scrolls the conversation back, like a terminal's own scrollback: the
    /// wheel moves it, and it clamps at the first line instead of scrolling
    /// into empty space.
    fn scroll_by(&mut self, delta: isize) {
        let max = self.chat_total.saturating_sub(self.chat_rows);
        let next = self.scroll as isize + delta;
        self.scroll = next.clamp(0, max as isize) as usize;
    }

    fn scroll_page(&mut self, pages: isize) {
        // One row of overlap, so a line isn't skipped between pages.
        let step = self.chat_rows.saturating_sub(1).max(1) as isize;
        self.scroll_by(pages * step);
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
            // Clicking into the input drops the previous highlight too.
            self.selection_start = None;
            self.selection_end = None;
            return;
        }
        self.selection_start = Some((col, row));
        self.selection_end = Some((col, row));
    }

    fn mouse_drag(&mut self, col: u16, row: u16) {
        if self.selection_start.is_none() {
            return;
        }
        self.selection_end = Some((col, row));
        // Dragging against the top edge scrolls, so a selection can reach text
        // that's above the viewport instead of stopping at the first row shown.
        if row <= self.chat_top {
            self.scroll_by(1);
        } else if row >= self.chat_top + self.chat_rows as u16 {
            self.scroll_by(-1);
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
    /// copies it and shows a transient status-bar message.
    ///
    /// The range is deliberately *kept* after copying: a selection that
    /// vanished the instant the button came up gave no confirmation of what had
    /// been grabbed. The next click clears it, like a terminal's own selection.
    fn finalize_selection(&mut self) {
        let (start, end) = match (self.selection_start, self.selection_end) {
            (Some(s), Some(e)) => (s, e),
            _ => return,
        };
        // A bare click selects nothing — don't leave a one-cell smudge behind.
        if start == end {
            self.selection_start = None;
            self.selection_end = None;
            return;
        }
        let text = extract_selection(&self.row_text, start, end);
        if text.is_empty() || !self.auto_copy {
            return;
        }
        let via = copy_to_clipboard(&text).unwrap_or_else(|| "osc52".to_string());
        self.flash =
            Some((format!("copiado {} chars via {via} · /config desativa", text.chars().count()), Instant::now()));
    }

    /// Slash-command matches for the autocomplete popup: non-empty only while
    /// the input is a bare `/prefix` (leading `/`, no space yet). Empty means
    /// the popup is closed.
    fn command_menu(&self) -> Vec<&'static CommandDoc> {
        // Recalling `/help` from history shouldn't reopen the popup — ↑↓ have
        // to keep meaning "walk the history" until the user types again.
        if self.history_idx.is_some() {
            return Vec::new();
        }
        let inp = self.input.trim_start();
        if !inp.starts_with('/') || inp.chars().any(char::is_whitespace) {
            return Vec::new();
        }
        COMMAND_CATALOG.iter().filter(|d| d.cmd.starts_with(inp)).collect()
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
        if let Some(doc) = menu.get(self.menu_selected(menu.len())) {
            self.input = doc.cmd.to_string();
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
        // Sending snaps back to the newest message, the way typing into a
        // terminal jumps out of its scrollback.
        self.scroll = 0;
        self.history_push(&line);
        if line.starts_with('/') && is_known_command(&line) {
            self.dispatch(&line);
            return;
        }
        if line.starts_with('/') {
            let cmd = line.split_whitespace().next().unwrap_or(&line).to_string();
            self.push(ChatRole::Error, format!("unknown command: {cmd}"));
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
        self.turn_started = Some(Instant::now());
        self.turn_chars = 0;
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
        // The question lives in the overlay, not in the log: as a chat line it
        // read as scrollback and got missed. Only the outcome gets logged.
        self.mode = Mode::ScanOffer { cursor: 0 };
    }

    fn help_picker_move(&mut self, delta: isize) {
        let last = COMMAND_CATALOG.len().saturating_sub(1);
        if let Mode::HelpPicker { cursor } = &mut self.mode {
            *cursor = if delta < 0 { cursor.saturating_sub(1) } else { (*cursor + 1).min(last) };
        }
    }

    /// Closes the overlay, then runs the highlighted command — in that order,
    /// so a command that opens its own overlay isn't immediately overwritten.
    fn help_picker_confirm(&mut self) {
        let cmd = match self.mode {
            Mode::HelpPicker { cursor } => COMMAND_CATALOG.get(cursor).map(|d| d.cmd),
            _ => None,
        };
        self.mode = Mode::Normal;
        if let Some(cmd) = cmd {
            self.dispatch(cmd);
        }
    }

    fn model_input_confirm(&mut self) {
        let name = match &self.mode {
            Mode::ModelInput { input } => input.trim().to_string(),
            _ => return,
        };
        self.mode = Mode::Normal;
        if name.is_empty() {
            return;
        }
        self.master = format!("{}/{}", self.master_cli, name);
        self.master_model = Some(name.clone());
        self.push(ChatRole::Info, format!("master model: {name}"));
    }

    fn scan_offer_move(&mut self, delta: isize) {
        if let Mode::ScanOffer { cursor } = &mut self.mode {
            *cursor = if delta < 0 { cursor.saturating_sub(1) } else { (*cursor + 1).min(2) };
        }
    }

    /// Records the answer so this is asked once per directory, whichever way it
    /// went, and starts whichever route was picked.
    fn answer_scan_offer(&mut self, choice: Offer) {
        self.mode = Mode::Normal;
        let dir = PathBuf::from(&self.repo);
        self.push(ChatRole::Info, format!("first time here ({}).", self.repo));
        if let Err(e) = scan::record(&self.scanned_path, &dir, choice.recorded()) {
            self.push(ChatRole::Error, format!("could not record the answer: {e}"));
        }
        match choice {
            Offer::Grill => self.start_grill(),
            Offer::Scan => self.start_scan(false),
            Offer::No => self.push(
                ChatRole::Info,
                "ok, I won't ask here again. `/scan` and `/grill` run whenever you want.",
            ),
        }
    }

    /// Hands the interview to the master as a normal turn. The script goes in
    /// as the request but never onto the screen: what the user should see is
    /// the master's first question, not the briefing behind it.
    fn start_grill(&mut self) {
        if self.rx.is_some() {
            self.push(ChatRole::Info, "the master is busy — wait for this turn to finish.");
            return;
        }
        let facts = scan::collect(&PathBuf::from(&self.repo), &self.home);
        self.push(ChatRole::Info, "interview: the master asks, you answer. Ctrl-C drops out.");
        self.spawn_turn(grill::prompt(&facts));
    }

    /// Runs the scan off-thread — it's a model call, and the UI shouldn't
    /// freeze for it. The result lands via `scan_rx` on a later tick.
    fn start_scan(&mut self, force: bool) {
        if self.scan_rx.is_some() {
            self.push(ChatRole::Info, "a scan is already running.");
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        self.push(ChatRole::Info, "scanning the directory… (one call to the master)");
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
                self.push(ChatRole::Error, format!("scan failed: {e}"));
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
            self.turn_started = None;
        }
    }

    fn handle_stream_event(&mut self, event: stream::Event) {
        match event {
            stream::Event::Ready { session_id } => {
                if !self.session_recorded {
                    let title = self.pending_title.clone().unwrap_or_else(|| "(untitled)".to_string());
                    sessions::add(
                        &self.sessions_path,
                        SessionRec { id: session_id.clone(), title, repo: self.repo.clone(), ts: sessions::now_ts() },
                    );
                    self.session_recorded = true;
                }
                self.session_id = Some(session_id);
            }
            stream::Event::Text(text) => {
                self.turn_chars += text.chars().count();
                self.push(ChatRole::Assistant, text)
            }
            stream::Event::Tool { name, input } => self.push(ChatRole::Tool, tool_running_label(&name, &input)),
            stream::Event::ToolResult(body) => self.push(ChatRole::ToolResult, tool_result_label(&body)),
            stream::Event::Done => {
                self.push(ChatRole::Info, "— concluído".to_string());
                self.rx = None;
                self.turn_started = None;
            }
        }
    }

    fn dispatch(&mut self, line: &str) {
        let mut parts = line.split_whitespace();
        match parts.next().unwrap_or("") {
            "/quit" | "/q" => {}
            "/help" | "/?" => self.mode = Mode::HelpPicker { cursor: 0 },
            "/scan" => {
                let force = parts.next() == Some("--force");
                self.start_scan(force);
            }
            "/grill" => self.start_grill(),
            "/model" => match parts.next() {
                None => self.mode = Mode::ModelInput { input: String::new() },
                Some(name) => {
                    self.master_model = Some(name.to_string());
                    self.master = format!("{}/{}", self.master_cli, name);
                    self.push(ChatRole::Info, format!("master model: {name}"));
                }
            },
            "/config" => {
                let cur = self.master_model.clone().unwrap_or_else(|| "(default)".into());
                let lines = vec![
                    format!("master       {} / {cur}", self.master_cli),
                    format!("theme        {}", self.theme),
                    format!("auto_copy    {}", self.auto_copy),
                    format!("repo         {}", self.repo),
                    format!("sessions     {}", self.sessions_path.display()),
                    String::new(),
                    "edit ~/.config/rege/config.yml or the project's .rege.yml".to_string(),
                ];
                self.mode = Mode::InfoPanel { title: " effective config ".to_string(), lines };
            }
            "/theme" => match parts.next() {
                None => self.open_theme_picker(),
                Some(name) if theme::exists(name) => {
                    self.theme = name.to_string();
                }
                Some(name) => {
                    self.push(ChatRole::Error, format!("no such theme: {name}"));
                }
            },
            "/resume" => self.open_resume_picker(),
            "/agents" => match parts.next() {
                // `/agents ativos` keeps the old inline list of running workers;
                // bare `/agents` opens the roster overlay.
                Some("active") | Some("ativos") | Some("running") => {
                    let lines = if self.agents.is_empty() {
                        vec!["no active agents".to_string()]
                    } else {
                        self.agents.iter().map(|a| format!("{:<14} {}", a.name, a.state.label())).collect()
                    };
                    self.mode = Mode::InfoPanel { title: " active agents ".to_string(), lines };
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
                        self.push(ChatRole::Info, "/buddy first");
                    }
                }
                Some(other) => {
                    self.push(ChatRole::Error, format!("unknown subcommand: {other}"));
                }
            },
            other => {
                self.push(ChatRole::Error, format!("unknown command: {other}"));
            }
        }
    }
}

const KNOWN_COMMANDS: &[&str] =
    &["/quit", "/q", "/help", "/?", "/model", "/config", "/theme", "/resume", "/agents", "/buddy", "/scan"];

/// A command as documentation, not just a label. `/help` teaches from `body`:
/// what the command does, and where it sits in how rege works — a one-line hint
/// can name a command but can't explain the tool.
struct CommandDoc {
    cmd: &'static str,
    /// One line, for the autocomplete popup where there's no room for more.
    hint: &'static str,
    body: &'static [&'static str],
    examples: &'static [&'static str],
}

/// Commands surfaced in the autocomplete popup and in `/help`.
/// Aliases (`/q`, `/?`) stay out of the menu but remain valid to type.
const COMMAND_CATALOG: &[CommandDoc] = &[
    CommandDoc {
        cmd: "/help",
        hint: "list the commands",
        body: &[
            "This screen. ↑↓ walks the commands and explains each one down here; \
             Enter runs whatever is highlighted.",
            "New to rege? You talk to the MASTER. It doesn't write code — it sizes \
             the task up and delegates to workers, each in its own isolated git \
             worktree. Then it reviews the work and opens a PR.",
            "The master never merges. The final output is always a PR for you to approve.",
        ],
        examples: &["/help", "/? (same thing)"],
    },
    CommandDoc {
        cmd: "/theme",
        hint: "theme picker (live preview)",
        body: &[
            "Changes the interface palette. With no argument it opens the picker, \
             where the cursor previews live: the whole screen changes as you move, \
             so you can choose by looking instead of guessing from the name.",
            "Given a name directly, it applies without opening anything.",
        ],
        examples: &["/theme", "/theme luxury"],
    },
    CommandDoc {
        cmd: "/model",
        hint: "switch the master's model",
        body: &[
            "Switches the model the MASTER runs on — not the workers', which come \
             from the roster in /agents.",
            "Worth scaling up here when the task is a hard call (architecture, \
             triaging something ambiguous) and down when it's routine. An expensive \
             master with cheap workers is the usual pairing.",
        ],
        examples: &["/model", "/model opus", "/model sonnet"],
    },
    CommandDoc {
        cmd: "/config",
        hint: "show the effective config",
        body: &[
            "Shows what is in force RIGHT NOW, already resolved: master, theme, \
             auto_copy, repo, and where sessions are recorded.",
            "This is the effective config, after the layers are merged — \
             ~/.config/rege/config.yml and the project's .rege.yml, which has the \
             last word. If something doesn't seem to be taking effect, look here \
             before editing a file.",
        ],
        examples: &["/config"],
    },
    CommandDoc {
        cmd: "/resume",
        hint: "resume an earlier session",
        body: &[
            "Lists this repo's earlier conversations and resumes the one you pick, \
             with the context of where you left off.",
            "Useful when work spans days: instead of re-explaining the task, you \
             carry on from where you were.",
        ],
        examples: &["/resume"],
    },
    CommandDoc {
        cmd: "/agents",
        hint: "agent roster (connect/remove)",
        body: &[
            "Your roster: which AI CLIs the master may use as workers, and in what \
             role. Enter connects a CLI that is installed but outside the roster; \
             x removes one; a adds one by hand.",
            "This is where workers come from. When the master delegates, it picks \
             among these — each runs isolated in a git worktree, in its own tmux \
             session, without touching your branch.",
            "Saved to ~/.config/rege/config.yml. `/agents active` shows who is \
             running right now.",
        ],
        examples: &["/agents", "/agents active"],
    },
    CommandDoc {
        cmd: "/scan",
        hint: "scan the directory and write AGENTS.md",
        body: &[
            "Looks at the directory and writes an AGENTS.md describing it: what it \
             is, how to run and test it, structure, conventions.",
            "AGENTS.md is the convention claude, codex and friends read on their \
             own — so the file helps any agent working here, not just rege.",
            "Never overwrites an existing AGENTS.md without --force. It is offered \
             once per directory, the first time you open rege in it.",
        ],
        examples: &["/scan", "/scan --force"],
    },
    CommandDoc {
        cmd: "/grill",
        hint: "the master interviews you, then writes the docs",
        body: &[
            "The master asks about the project one question at a time — what you \
             are building, what is already decided and why, what agents must not \
             touch — and pushes back when an answer is vague.",
            "It is the counterpart to /scan, not a variation: a scan reads what \
             the code already says, an interview reaches what the code cannot \
             say. On a fresh `rails new` there is nothing to scan and everything \
             to ask.",
            "At the end it writes the AGENTS.md, one docs/adr/NNN-*.md per \
             decision that came up, and docs/glossary.md when the vocabulary is \
             worth pinning down.",
        ],
        examples: &["/grill"],
    },
    CommandDoc {
        cmd: "/buddy",
        hint: "animated pet",
        body: &[
            "Hatches a critter in the corner of the screen, its looks derived from \
             your username — the same user always hatches the same creature.",
            "It does nothing useful. `/buddy pet` gives it a scratch.",
        ],
        examples: &["/buddy", "/buddy pet"],
    },
    CommandDoc {
        cmd: "/quit",
        hint: "leave rege",
        body: &[
            "Closes the TUI. The session is recorded, so /resume brings it back.",
            "Workers already dispatched carry on in their own tmux sessions — \
             leaving the TUI doesn't kill work in flight.",
        ],
        examples: &["/quit", "/q · exit · :q"],
    },
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
        // Mirror what `highlight_selection` paints: the first row starts at the
        // click column, the last one ends at the release column. Taking whole
        // rows here made the copy disagree with the highlight — starting a
        // selection mid-line and dragging up grabbed the line from its start.
        (r0..=r1)
            .map(|r| {
                let row = row_text.get(r as usize).map(String::as_str).unwrap_or("");
                let chars: Vec<char> = row.chars().collect();
                let (lo, hi) = if r == r0 {
                    ((c0 as usize).min(chars.len()), chars.len())
                } else if r == r1 {
                    (0, ((c1 as usize) + 1).min(chars.len()))
                } else {
                    (0, chars.len())
                };
                chars[lo..hi.max(lo)].iter().collect::<String>().trim_end().to_string()
            })
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

/// Copies by both routes available, because each fails in cases the other
/// covers: OSC52 is the only one that works over ssh/tmux, but terminals are
/// free to ignore it (and many drop it silently, which reads as "copy is
/// broken"); a local helper actually owns the selection but can't reach a
/// remote clipboard.
/// Returns the helper it also used, if any — the status line names it, so a
/// failed paste points at the right suspect instead of "copy is broken".
fn copy_to_clipboard(text: &str) -> Option<String> {
    use std::io::Write;
    let seq = osc52_sequence(text);
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(seq.as_bytes());
    let _ = stdout.flush();

    if let Some(argv) = clipboard_helper() {
        let name = argv[0].clone();
        let text = text.to_string();
        // Off-thread: `xclip` stays alive owning the selection, so waiting on
        // it here would freeze the UI until someone else copies something.
        std::thread::spawn(move || {
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
            if let Ok(mut child) = cmd.spawn() {
                if let Some(stdin) = child.stdin.as_mut() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                drop(child.stdin.take());
                let _ = child.wait();
            }
        });
        return Some(name);
    }
    None
}

/// The system clipboard command for this session, if one is installed. Wayland
/// first — `xclip`/`xsel` reach XWayland's clipboard, which is bridged but a
/// step removed.
fn clipboard_helper() -> Option<Vec<String>> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    pick_clipboard_helper(wayland, x11, cli_installed)
}

/// Split from the env lookup so the precedence is testable without a display.
fn pick_clipboard_helper(wayland: bool, x11: bool, installed: impl Fn(&str) -> bool) -> Option<Vec<String>> {
    let argv = |parts: &[&str]| Some(parts.iter().map(|s| s.to_string()).collect());
    if wayland && installed("wl-copy") {
        return argv(&["wl-copy"]);
    }
    if x11 && installed("xclip") {
        return argv(&["xclip", "-selection", "clipboard"]);
    }
    if x11 && installed("xsel") {
        return argv(&["xsel", "--clipboard", "--input"]);
    }
    None
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
        app.push(ChatRole::User, "refactor the auth module and add tests");
        app.push(ChatRole::Assistant, "Hard task. Running 3 workers on the same task.");
        app.push(ChatRole::Info, "⚙ spawn_agent · claude/sonnet · refactor auth");
        app.agents = vec![
            AgentRow { name: "a1".into(), state: AgentState::Running, last: "editing session.rs…".into() },
            AgentRow { name: "a2".into(), state: AgentState::Done, last: "patch ready".into() },
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
        app.spinner = app.spinner.wrapping_add(1);
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
                        Mode::HelpPicker { .. } => match key.code {
                            KeyCode::Up => app.help_picker_move(-1),
                            KeyCode::Down => app.help_picker_move(1),
                            KeyCode::Enter => app.help_picker_confirm(),
                            KeyCode::Esc => app.mode = Mode::Normal,
                            _ => {}
                        },
                        Mode::ModelInput { .. } => match key.code {
                            KeyCode::Char(c) => {
                                if let Mode::ModelInput { input } = &mut app.mode {
                                    input.push(c);
                                }
                            }
                            KeyCode::Backspace => {
                                if let Mode::ModelInput { input } = &mut app.mode {
                                    input.pop();
                                }
                            }
                            KeyCode::Enter => app.model_input_confirm(),
                            KeyCode::Esc => app.mode = Mode::Normal,
                            _ => {}
                        },
                        // Read-only: any key dismisses, like a "press any key" panel.
                        Mode::InfoPanel { .. } => app.mode = Mode::Normal,
                        Mode::ScanOffer { cursor } => match key.code {
                            KeyCode::Char('g') | KeyCode::Char('G') => {
                                app.answer_scan_offer(Offer::Grill)
                            }
                            KeyCode::Char('s') | KeyCode::Char('S') => {
                                app.answer_scan_offer(Offer::Scan)
                            }
                            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                app.answer_scan_offer(Offer::No)
                            }
                            KeyCode::Up => app.scan_offer_move(-1),
                            KeyCode::Down => app.scan_offer_move(1),
                            KeyCode::Enter => app.answer_scan_offer(Offer::from_cursor(cursor)),
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
                            KeyCode::PageUp => app.scroll_page(1),
                            KeyCode::PageDown => app.scroll_page(-1),
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
                    // Wheel scrolls the conversation, like the terminal's own
                    // scrollback — 3 rows per notch is the usual step.
                    MouseEventKind::ScrollUp => app.scroll_by(3),
                    MouseEventKind::ScrollDown => app.scroll_by(-3),
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
    // Measure the chat before painting it: the wheel and PageUp/Down need the
    // page size and the total, and both depend on this frame's width.
    let (rows, total) = chat_metrics(chunks[1], app, theme);
    app.chat_rows = rows;
    app.chat_total = total;
    app.chat_top = chunks[1].y;
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
    if let Mode::ScanOffer { cursor } = app.mode {
        draw_scan_offer(area, buf, app, cursor);
    }
    if let Mode::HelpPicker { cursor } = app.mode {
        draw_help_picker(area, buf, app, cursor);
    }
    if let Mode::ModelInput { .. } = &app.mode {
        draw_model_input(area, buf, app);
    }
    if let Mode::InfoPanel { title, lines } = &app.mode {
        draw_info_panel(area, buf, app, title, lines);
    }
    if matches!(app.mode, Mode::Normal) {
        draw_command_menu(buf, app, app.input_rect);
    }

    // Live highlight while dragging a selection, so the user gets feedback
    // even in terminals/tmux where the OSC52 copy is silently dropped.
    if matches!(app.mode, Mode::Normal) {
        if let (Some(start), Some(end)) = (app.selection_start, app.selection_end) {
            highlight_selection(area, buf, start, end, theme);
        }
    }
}

/// Selection colors: a solid light background with dark text on top, the way a
/// terminal's own selection looks. `REVERSED` was the obvious choice and the
/// wrong one — against these palettes it rendered almost invisible, so users
/// couldn't see what they were selecting.
fn selection_colors(theme: &str) -> ((u8, u8, u8), (u8, u8, u8)) {
    let bg = theme::color(theme, Role::Text);
    let luma = (bg.0 as u32 * 299 + bg.1 as u32 * 587 + bg.2 as u32 * 114) / 1000;
    let fg = if luma > 128 { (18, 20, 24) } else { (245, 245, 245) };
    (bg, fg)
}

/// Paints the selection range on the current frame.
fn highlight_selection(
    area: Rect,
    buf: &mut ratatui::buffer::Buffer,
    start: (u16, u16),
    end: (u16, u16),
    theme: &str,
) {
    let (bg, fg) = selection_colors(theme);
    let style = Style::default().bg(rgb(bg)).fg(rgb(fg));
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
                cell.set_style(style);
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
/// Buddy panel width. Named because the chat has to reserve exactly this much
/// to avoid being painted over.
const BUDDY_WIDTH: u16 = 24;

const BANNER: [&str; 5] = [
    "████  █████ ████  █████",
    "██ ██ ██    ██    ██   ",
    "████  ████  ██ ██ ████ ",
    "██ ██ ██    ██ ██ ██   ",
    "██ ██ █████ ████  █████",
];

/// Width the conversation text may use. The buddy sits in the bottom-right
/// corner and is painted *after* the messages, so without reserving its column
/// the text ran under it and came out clipped mid-word.
fn chat_text_width(area: Rect, app: &App) -> u16 {
    let margins = 4;
    let reserved = if app.buddy.is_some() { BUDDY_WIDTH + 2 } else { 0 };
    area.width.saturating_sub(margins + reserved)
}

/// Visible rows and total wrapped rows of the chat pane, mirroring how
/// `draw_chat`/`render_messages` lay it out. Kept next to them so the scroll
/// clamp can't drift from what's actually on screen.
fn chat_metrics(area: Rect, app: &App, theme: &str) -> (usize, usize) {
    let width = chat_text_width(area, app);
    let mut rows = area.height as usize;
    if !app.conversation_started() {
        // The wordmark eats the top of the pane before any message shows.
        rows = rows.saturating_sub((BANNER.len() + 1).min(rows));
    }
    let total = app.chat.iter().flat_map(|m| chat_lines(theme, m, width)).count();
    (rows, total)
}

fn draw_chat(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, theme: &str) {
    let inner = Rect {
        x: area.x + 2,
        y: area.y,
        width: chat_text_width(area, app),
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
    let width: u16 = BUDDY_WIDTH;
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

    Clear.render(rect, buf); // real clear: an empty Paragraph leaves cells intact

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
    // Expand every message into its wrapped visual rows, then window into them
    // — height is measured in display rows, not messages, so multi-line/long
    // output neither collides nor clips.
    let lines: Vec<Line> = app
        .chat
        .iter()
        .flat_map(|m| chat_lines(theme, m, area.width))
        .collect();
    // `scroll` counts rows back from the bottom; clamped here too because the
    // pane can shrink (terminal resize) between the scroll and the frame.
    let max_back = lines.len().saturating_sub(rows);
    let back = app.scroll.min(max_back);
    let end = lines.len() - back;
    let start = end.saturating_sub(rows);
    let mut visible: Vec<Line> = lines[start..end].to_vec();

    // Scrolled up: say so, and how much is below. Without this the pane just
    // looks stuck — nothing on screen says the newest message is off-view.
    if back > 0 {
        if let Some(last) = visible.last_mut() {
            *last = Line::from(styled(theme, Role::Accent, format!("↓ {back} more lines · end when you scroll")));
        }
    }
    Paragraph::new(visible).render(area, buf);
}

/// Compact one-line label for a running tool call, in the shape the Claude
/// Code CLI itself uses: `Bash(gh pr view 268)`, `Read(src/tui.rs)`. The tool
/// name stays as the model sent it and the argument is clipped — never the
/// full input. A tool with nothing worth showing is just its name.
fn tool_running_label(name: &str, input: &str) -> String {
    const KEYS: [&str; 7] = ["command", "file_path", "path", "pattern", "url", "query", "description"];
    let detail = serde_json::from_str::<serde_json::Value>(input).ok().and_then(|v| {
        KEYS.iter()
            .find_map(|k| v.get(k).and_then(serde_json::Value::as_str))
            .map(str::to_string)
    });
    match detail {
        Some(arg) => format!("{name}({})", first_bit(&arg, 50)),
        None => name.to_string(),
    }
}

/// The one line a tool result gets: its opening, clipped. The full body (JSON,
/// file lists, diffs) would flood the master's log and carries no signal here
/// — the worker already acted on it.
fn tool_result_label(body: &str) -> String {
    let bit = first_bit(body, 60);
    if bit.is_empty() {
        "ok".to_string()
    } else {
        bit
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
/// The user's own lines get a filled band across the pane, the way a shell
/// echoes what you typed: `❯` alone put user and assistant one glyph apart, and
/// scrolling back through a long conversation they blurred together.
fn user_row_colors(theme: &str) -> ((u8, u8, u8), (u8, u8, u8)) {
    let d = theme::color(theme, Role::Dim);
    // Dim itself is too loud as a fill — darken it so the band reads as a
    // surface behind the text, not as a highlight competing with it.
    let bg = ((d.0 as f32 * 0.45) as u8, (d.1 as f32 * 0.45) as u8, (d.2 as f32 * 0.45) as u8);
    (bg, theme::color(theme, Role::Text))
}

fn chat_lines(theme: &str, m: &ChatMsg, width: u16) -> Vec<Line<'static>> {
    if matches!(m.role, ChatRole::User) {
        return user_lines(theme, &m.text, width);
    }
    let (prefix, prefix_role, body_role) = match m.role {
        ChatRole::User => unreachable!("tratado acima"),
        ChatRole::Assistant => ("● ", Role::Accent, Role::Text),
        ChatRole::Tool => ("● ", Role::Accent2, Role::Text),
        ChatRole::ToolResult => ("  ⎿  ", Role::Dim, Role::Dim),
        ChatRole::Info => ("", Role::Dim, Role::Dim),
        ChatRole::Error => ("", Role::Fail, Role::Fail),
    };
    let indent_cols = prefix.chars().count();
    let budget = (width as usize).saturating_sub(indent_cols).max(1);
    let indent = " ".repeat(indent_cols);
    fenced_segments(&m.text, budget)
        .into_iter()
        .enumerate()
        .map(|(i, (seg, code))| {
            let gutter = if i == 0 { prefix.to_string() } else { indent.clone() };
            // Inside a fence the text is code, not prose: no `**`/backtick
            // parsing, or a shell one-liner loses characters to the renderer.
            let body = if code {
                vec![styled(theme, Role::Accent2, seg)]
            } else {
                markdown_spans(theme, &seg, body_role)
            };
            if gutter.is_empty() {
                Line::from(body)
            } else {
                let mut spans = vec![styled(theme, prefix_role, gutter)];
                spans.extend(body);
                Line::from(spans)
            }
        })
        .collect()
}

/// A user message as a filled band: every row padded to the full width so the
/// background is a solid block, not a ragged one that ends with the text.
fn user_lines(theme: &str, text: &str, width: u16) -> Vec<Line<'static>> {
    let (bg, fg) = user_row_colors(theme);
    let style = Style::default().bg(rgb(bg)).fg(rgb(fg));
    let cols = width as usize;
    let budget = cols.saturating_sub(2).max(1); // "❯ "
    wrap_text(text, budget)
        .into_iter()
        .enumerate()
        .map(|(i, seg)| {
            let gutter = if i == 0 { "❯ " } else { "  " };
            let used = gutter.chars().count() + seg.chars().count();
            let pad = " ".repeat(cols.saturating_sub(used));
            Line::from(Span::styled(format!("{gutter}{seg}{pad}"), style))
        })
        .collect()
}

/// Turns the markdown the master actually emits into styled spans. Not a
/// markdown engine: only the marks that showed up as literal noise on screen —
/// `**bold**`, `` `code` `` and `###` headings. Anything else stays as typed,
/// which is the honest outcome for a renderer this small.
fn markdown_spans(theme: &str, line: &str, body_role: Role) -> Vec<Span<'static>> {
    // A heading colors the whole line and drops the hashes.
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        let text = trimmed.trim_start_matches('#').trim_start();
        if !text.is_empty() {
            return vec![styled(theme, Role::Strong, text.to_string()).add_modifier(Modifier::BOLD)];
        }
    }

    let mut spans = Vec::new();
    let mut rest = line;
    let mut plain = String::new();
    while !rest.is_empty() {
        let bold = rest.find("**").filter(|i| rest[i + 2..].contains("**"));
        let code = rest.find('`').filter(|i| rest[i + 1..].contains('`'));
        // Whichever mark opens first wins; ties can't happen (different chars).
        let (at, close, len, role, bolded) = match (bold, code) {
            (Some(b), Some(c)) if b < c => (b, "**", 2, Role::Strong, true),
            (Some(_), Some(c)) => (c, "`", 1, Role::Accent2, false),
            (Some(b), None) => (b, "**", 2, Role::Strong, true),
            (None, Some(c)) => (c, "`", 1, Role::Accent2, false),
            (None, None) => break,
        };
        plain.push_str(&rest[..at]);
        let after = &rest[at + len..];
        let Some(end) = after.find(close) else { break };
        if !plain.is_empty() {
            spans.push(styled(theme, body_role, std::mem::take(&mut plain)));
        }
        let inner = after[..end].to_string();
        let span = styled(theme, role, inner);
        spans.push(if bolded { span.add_modifier(Modifier::BOLD) } else { span });
        rest = &after[end + len..];
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        spans.push(styled(theme, body_role, plain));
    }
    if spans.is_empty() {
        spans.push(styled(theme, body_role, String::new()));
    }
    spans
}

/// Wraps like [`wrap_text`], but first pulls ``` fences out of the text: the
/// delimiter rows disappear and every row between them is flagged as code.
/// Left as prose they showed up on screen as a stray `` `bash `` line and a
/// lone backtick, which is the one markdown mark the master emits most.
fn fenced_segments(text: &str, width: usize) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for raw in text.split('\n') {
        if raw.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        out.extend(wrap_text(raw, width).into_iter().map(|seg| (seg, in_fence)));
    }
    // An empty message still owes the caller one row to hang the gutter on.
    if out.is_empty() {
        out.push((String::new(), false));
    }
    out
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
            let hard = (i + width).min(chars.len());
            // Break on the last space that fits, so words stay whole ("nesse
            // mo|mento" was the tell). A word longer than the line still gets
            // cut — there's nowhere else to break it.
            let end = if hard == chars.len() {
                hard
            } else {
                match chars[i..hard].iter().rposition(|c| *c == ' ') {
                    Some(rel) if rel > 0 => i + rel,
                    _ => hard,
                }
            };
            out.push(chars[i..end].iter().collect::<String>().trim_end().to_string());
            // Swallow the space we broke on; leading spaces would misalign the
            // continuation against the gutter.
            i = if end < chars.len() && chars[end] == ' ' { end + 1 } else { end };
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn draw_agents(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, theme: &str) {
    let block = rounded_block(theme, " agents ", Role::Dim);
    let inner = block.inner(area);
    block.render(area, buf);

    let lines: Vec<Line> = if app.agents.is_empty() {
        vec![Line::from(styled(theme, Role::Dim, "no active agents"))]
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

/// Words for the activity line. English on purpose: it's the common language of
/// terminal tooling, and these read as status even to someone using the rest of
/// the UI in Portuguese. Orchestration verbs, because that's what's happening.
const ACTIVITY_WORDS: &[&str] = &[
    "Thinking",
    "Delegating",
    "Orchestrating",
    "Dispatching",
    "Marshalling",
    "Pondering",
    "Wrangling",
    "Brewing",
    "Herding",
    "Conjuring",
    "Noodling",
    "Percolating",
];

/// Rotates every 4s so a long turn doesn't read as a hung one, and is derived
/// from elapsed time — same second, same word, no randomness to jitter it.
fn activity_word(elapsed_secs: u64) -> &'static str {
    ACTIVITY_WORDS[(elapsed_secs as usize / 4) % ACTIVITY_WORDS.len()]
}

fn spinner_frame(tick: usize) -> char {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[tick % FRAMES.len()]
}

fn fmt_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

/// Rough token count from characters — the stream doesn't carry usage per
/// chunk, and ~4 chars/token is the usual approximation. It's a progress
/// indicator, not accounting.
fn fmt_tokens(chars: usize) -> String {
    let tokens = chars / 4;
    if tokens >= 1000 {
        format!("{:.1}k tokens", tokens as f64 / 1000.0)
    } else {
        format!("{tokens} tokens")
    }
}

fn draw_statusbar(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, theme: &str) {
    if let Some((msg, ts)) = &app.flash {
        if ts.elapsed() < Duration::from_secs(2) {
            let line = Line::from(styled(theme, Role::Accent, msg.clone()));
            Paragraph::new(line).render(area, buf);
            return;
        }
    }
    // A turn in flight takes over the bar: without it the UI looks frozen
    // between the send and the first streamed token, which can be many seconds.
    if let Some(started) = app.turn_started {
        let elapsed = started.elapsed();
        let line = Line::from(vec![
            styled(theme, Role::Accent, format!("{} ", spinner_frame(app.spinner))),
            styled(theme, Role::Text, activity_word(elapsed.as_secs())),
            styled(theme, Role::Dim, format!("… ({} · ↓ {})", fmt_elapsed(elapsed), fmt_tokens(app.turn_chars))),
        ]);
        Paragraph::new(line).render(area, buf);
        return;
    }
    let running = app.agents.iter().filter(|a| a.state == AgentState::Running).count();
    let ready = app.agents.iter().filter(|a| a.state == AgentState::Done).count();
    let line = Line::from(vec![
        styled(theme, Role::Accent, running.to_string()),
        styled(theme, Role::Dim, " running · "),
        styled(theme, Role::Accent, ready.to_string()),
        styled(theme, Role::Dim, " ready · /help · /theme · /model · /resume · /quit"),
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
    // `Paragraph::new("")` does NOT do this — it draws nothing and leaves the
    // cells underneath intact, which let the banner and panels show through.
    Clear.render(rect, buf);

    let block = rounded_block(theme, " theme ", Role::Dim);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(styled(theme, Role::Text, "Pick a theme").add_modifier(Modifier::BOLD)));
    lines.push(Line::from(styled(theme, Role::Dim, "↑↓ move · Enter selects · Esc cancels")));

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
    // `Paragraph::new("")` does NOT do this — it draws nothing and leaves the
    // cells underneath intact, which let the banner and panels show through.
    Clear.render(rect, buf);

    let block = rounded_block(theme, " sessions ", Role::Dim);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(styled(theme, Role::Text, "Resume a session").add_modifier(Modifier::BOLD)));
    lines.push(Line::from(styled(theme, Role::Dim, "↑↓ move · Enter resumes · Esc cancels")));

    if app.resume_list.is_empty() {
        lines.push(Line::from(styled(theme, Role::Dim, "no earlier sessions")));
    } else {
        let now = sessions::now_ts();
        for (i, rec) in app.resume_list.iter().enumerate() {
            let chevron = if i == cursor { styled(theme, Role::Accent, "❯ ") } else { styled(theme, Role::Dim, "  ") };
            let short_id: String = rec.id.chars().take(8).collect();
            let text = format!("{}  ·  {}  ·  {} ago", rec.title, short_id, rel_time(rec.ts, now));
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
    // `Paragraph::new("")` does NOT do this — it draws nothing and leaves the
    // cells underneath intact, which let the banner and panels show through.
    Clear.render(rect, buf);

    let block = rounded_block(theme, " agents ", Role::Dim);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(styled(theme, Role::Text, "Agent roster").add_modifier(Modifier::BOLD)));
    lines.push(Line::from(styled(theme, Role::Dim, "↑↓ move · Enter connects · a adds · x removes · Esc closes")));

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
                let text = format!("connect {cli} (installed)");
                lines.push(agents_line(theme, sel == cursor, &text));
                sel += 1;
            }
            AgentsRow::AddManual => {
                lines.push(agents_line(theme, sel == cursor, "+ add by hand"));
                sel += 1;
            }
        }
    }

    lines.push(Line::from(styled(theme, Role::Dim, "grava em ~/.config/rege/config.yml")));
    Paragraph::new(lines).render(inner, buf);
}

/// `/help` teaches instead of listing: the highlighted command gets explained
/// right below the list, so the tool is learned by walking it. Enter still runs
/// what's highlighted, so reading and doing are the same screen.
fn draw_help_picker(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, cursor: usize) {
    let theme = app.active_theme();
    let list_height = COMMAND_CATALOG.len() as u16 + 2; // + title and hint rows
    const WIDTH: u16 = 72;
    // Sized by the *longest* explanation, not the current one, so the panel
    // doesn't grow and shrink under the cursor while you read.
    let doc_height = COMMAND_CATALOG.iter().map(|d| doc_height(d, WIDTH - 2)).max().unwrap_or(6);
    // +2 for the block's own borders, which don't belong to the inner layout.
    let rect = centered_rect(WIDTH, (list_height + doc_height + 2).min(area.height), area);
    Clear.render(rect, buf);

    let block = rounded_block(theme, " ajuda ", Role::Accent);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let [list_area, doc_area] =
        Layout::vertical([Constraint::Length(list_height), Constraint::Min(1)]).areas(inner);

    let mut lines = vec![
        Line::from(styled(theme, Role::Text, "Commands").add_modifier(Modifier::BOLD)),
        Line::from(styled(theme, Role::Dim, "↑↓ explains · Enter runs · Esc closes")),
    ];
    for (i, doc) in COMMAND_CATALOG.iter().enumerate() {
        lines.push(agents_line(theme, i == cursor, &format!("{:<10} {}", doc.cmd, doc.hint)));
    }
    Paragraph::new(lines).render(list_area, buf);

    if let Some(doc) = COMMAND_CATALOG.get(cursor) {
        draw_command_doc(doc_area, buf, theme, doc);
    }
}

/// Rows `doc` needs at `width`, counting the wrap — the panel is sized from
/// this so nothing gets clipped, least of all the examples at the bottom.
fn doc_height(doc: &CommandDoc, width: u16) -> u16 {
    let w = width.max(1) as usize;
    let wrapped: usize = doc.body.iter().map(|p| p.chars().count().div_ceil(w) + 1).sum();
    // rule + body + examples
    1 + wrapped as u16 + if doc.examples.is_empty() { 0 } else { 1 }
}

/// The explanation half of `/help`. Wrapped, because these are paragraphs — the
/// point is to say what the command is *for*, not to fit in a column.
fn draw_command_doc(area: Rect, buf: &mut ratatui::buffer::Buffer, theme: &str, doc: &CommandDoc) {
    let rule = "─".repeat(area.width.saturating_sub(doc.cmd.len() as u16 + 2).max(1) as usize);
    let mut lines = vec![Line::from(vec![
        styled(theme, Role::Accent2, format!("{} ", doc.cmd)).add_modifier(Modifier::BOLD),
        styled(theme, Role::Dim, rule),
    ])];
    for para in doc.body {
        lines.push(Line::from(styled(theme, Role::Text, *para)));
        lines.push(Line::from(""));
    }
    if !doc.examples.is_empty() {
        lines.push(Line::from(styled(theme, Role::Dim, format!("ex: {}", doc.examples.join("  ·  ")))));
    }
    Paragraph::new(lines).wrap(Wrap { trim: true }).render(area, buf);
}

/// Master model as a typed field. A fixed model list would be a guess that goes
/// stale — the names change upstream — so this states the current one and takes
/// whatever the user types.
fn draw_model_input(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App) {
    let theme = app.active_theme();
    let input = match &app.mode {
        Mode::ModelInput { input } => input.as_str(),
        _ => "",
    };
    let rect = centered_rect(60, 9, area);
    Clear.render(rect, buf);

    let block = rounded_block(theme, " master model ", Role::Accent);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let cur = app.master_model.clone().unwrap_or_else(|| "(default do CLI)".into());
    let lines = vec![
        Line::from(styled(theme, Role::Text, format!("CLI: {}", app.master_cli)).add_modifier(Modifier::BOLD)),
        Line::from(styled(theme, Role::Dim, format!("current model: {cur}"))),
        Line::from(""),
        Line::from(vec![styled(theme, Role::Accent, "❯ "), styled(theme, Role::Text, input.to_string())]),
        Line::from(""),
        Line::from(styled(theme, Role::Dim, "ex: opus · sonnet · haiku · Enter aplica · Esc cancela")),
    ];
    Paragraph::new(lines).render(inner, buf);
}

/// Read-only panel for what used to be printed into the log and scroll away.
fn draw_info_panel(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, title: &str, body: &[String]) {
    let theme = app.active_theme();
    let width = body.iter().map(|l| l.chars().count()).max().unwrap_or(20).clamp(30, 72) as u16 + 4;
    let rect = centered_rect(width, body.len() as u16 + 4, area);
    Clear.render(rect, buf);

    let block = rounded_block(theme, title, Role::Accent);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let mut lines: Vec<Line> = body.iter().map(|l| Line::from(styled(theme, Role::Text, l.clone()))).collect();
    lines.push(Line::from(""));
    lines.push(Line::from(styled(theme, Role::Dim, "any key closes")));
    Paragraph::new(lines).render(inner, buf);
}

/// First-run scan offer. It's a question that blocks the session, so it gets
/// the same centered panel the pickers use — as a chat line it read as
/// scrollback and users typed straight past it.
fn draw_scan_offer(area: Rect, buf: &mut ratatui::buffer::Buffer, app: &App, cursor: usize) {
    let theme = app.active_theme();
    let rect = centered_rect(66, 11, area);

    // Clear the popup area so the background chat text doesn't bleed through.
    // `Paragraph::new("")` does NOT do this — it draws nothing and leaves the
    // cells underneath intact, which let the banner and panels show through.
    Clear.render(rect, buf);

    let block = rounded_block(theme, " first time here ", Role::Accent);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let lines = vec![
        Line::from(
            styled(theme, Role::Text, format!("Get to know {}?", repo_name(app))).add_modifier(Modifier::BOLD),
        ),
        Line::from(styled(theme, Role::Dim, app.repo.clone())),
        Line::from(styled(theme, Role::Dim, format!("Both routes end in an {}.", scan::CONTEXT_FILE))),
        Line::from(""),
        agents_line(theme, cursor == 0, "interview me about it (/grill)"),
        agents_line(theme, cursor == 1, "just scan the files (/scan)"),
        agents_line(theme, cursor == 2, "no, and don't ask here again"),
        Line::from(""),
        Line::from(styled(theme, Role::Dim, "↑↓ move · Enter confirms · g/s/n answers directly")),
    ];
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
    Clear.render(rect, buf); // real clear: an empty Paragraph leaves cells intact
    let block = rounded_block(theme, " new agent ", Role::Dim);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let lines = vec![
        Line::from(styled(theme, Role::Text, "Add an agent").add_modifier(Modifier::BOLD)),
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

    Clear.render(rect, buf); // real clear: an empty Paragraph leaves cells intact
    let block = rounded_block(theme, " commands ", Role::Dim);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let cursor = app.menu_selected(menu.len());
    let lines: Vec<Line> = menu
        .iter()
        .enumerate()
        .map(|(i, doc)| {
            let (cmd, desc) = (doc.cmd, doc.hint);
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
        // Short command: shown in full, in the CLI's own `Tool(arg)` shape.
        let short = r#"{"command":"gh pr view 268"}"#;
        assert_eq!(tool_running_label("Bash", short), "Bash(gh pr view 268)");
        // Long command: clipped to the opening with an ellipsis.
        let long = r#"{"command":"gh pr view 268 --json additions,body,files,commits,reviews"}"#;
        assert_eq!(tool_running_label("Bash", long), "Bash(gh pr view 268 --json additions,body,files,commits…)");
        // A file tool labels itself with the path it touched.
        assert_eq!(tool_running_label("Read", r#"{"file_path":"src/tui.rs"}"#), "Read(src/tui.rs)");
        // Nothing worth showing → name only.
        assert_eq!(tool_running_label("mcp__rege__consult", "{}"), "mcp__rege__consult");
        // Garbage input never panics.
        assert_eq!(tool_running_label("Bash", "not json"), "Bash");
    }

    #[test]
    fn tool_result_collapses_to_one_clipped_line() {
        assert_eq!(tool_result_label("ok\nmais coisa\n"), "ok…");
        assert_eq!(tool_result_label(""), "ok");
        let flood = "x".repeat(200);
        assert!(tool_result_label(&flood).chars().count() <= 61, "a result never floods the log");
    }

    #[test]
    fn fenced_block_loses_the_backticks_and_keeps_the_code() {
        let text = "install it like this:\n```bash\ncargo install --path .\n```\ndone";
        let segs = fenced_segments(text, 80);
        assert!(!segs.iter().any(|(s, _)| s.contains("```")), "the fence must not show: {segs:?}");
        assert!(!segs.iter().any(|(s, _)| s.trim() == "bash"), "the fence language must not become a row");
        assert_eq!(
            segs.iter().find(|(s, _)| s.contains("cargo")).map(|(_, code)| *code),
            Some(true),
            "the fence body comes marked as code"
        );
        assert_eq!(segs.iter().find(|(s, _)| s.contains("done")).map(|(_, code)| *code), Some(false));
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
        assert!(app.chat.iter().any(|m| m.text.contains("no such theme")));
    }

    #[test]
    fn app_dispatch_help_opens_a_picker_that_runs_the_command() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.dispatch("/help");
        assert!(matches!(app.mode, Mode::HelpPicker { .. }), "/help opens an overlay, it does not dump into the log");
        let painted = render_to_lines(&mut app, 90, 30);
        for cmd in ["/model", "/config", "/resume"] {
            assert!(painted.iter().any(|l| l.contains(cmd)), "{cmd} should be in the panel");
        }

        // The highlighted command gets explained, not just labeled — /help has
        // to teach the tool, and a one-line hint can't.
        let agents_idx = COMMAND_CATALOG.iter().position(|d| d.cmd == "/agents").unwrap();
        app.mode = Mode::HelpPicker { cursor: agents_idx };
        let doc = render_to_lines(&mut app, 90, 34).join(" ");
        assert!(doc.contains("worktree"), "the /agents explanation should place the worker: {doc}");
        assert!(doc.contains("ex:"), "usage examples in the panel");

        // Every command carries real documentation, so no entry can rot into a
        // bare label as the catalog grows.
        for d in COMMAND_CATALOG {
            assert!(!d.body.is_empty(), "{} has no explanation", d.cmd);
            assert!(!d.examples.is_empty(), "{} sem exemplo", d.cmd);
            assert!(d.body[0].len() > 40, "{}: explanation too short to teach anything", d.cmd);
        }

        // Enter runs what's highlighted — walk to /theme and confirm it opens.
        app.mode = Mode::HelpPicker { cursor: 0 };
        let theme_idx = COMMAND_CATALOG.iter().position(|d| d.cmd == "/theme").unwrap();
        for _ in 0..theme_idx {
            app.help_picker_move(1);
        }
        app.help_picker_confirm();
        assert!(matches!(app.mode, Mode::ThemePicker { .. }), "Enter should run the highlighted command");
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
    fn app_dispatch_model_no_arg_opens_a_field_and_applies_it() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        let before = app.master_model.clone();
        app.dispatch("/model");
        assert!(matches!(app.mode, Mode::ModelInput { .. }));
        assert_eq!(app.master_model, before, "opening the panel changes nothing");
        let painted = render_to_lines(&mut app, 90, 30);
        assert!(painted.iter().any(|l| l.contains("current model")), "the panel shows the model in use");

        app.mode = Mode::ModelInput { input: " opus ".to_string() };
        app.model_input_confirm();
        assert_eq!(app.master_model.as_deref(), Some("opus"), "espaços são aparados");
        assert_eq!(app.master, "claude/opus");
        assert!(matches!(app.mode, Mode::Normal));

        // Empty input is a cancel, not a model named "".
        app.dispatch("/model");
        app.model_input_confirm();
        assert_eq!(app.master_model.as_deref(), Some("opus"));
    }

    #[test]
    fn app_dispatch_config_shows_effective_in_a_panel() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.dispatch("/config");
        assert!(matches!(app.mode, Mode::InfoPanel { .. }));
        let painted = render_to_lines(&mut app, 100, 30);
        assert!(painted.iter().any(|l| l.contains("auto_copy")));
        assert!(painted.iter().any(|l| l.contains("theme")));
        assert!(painted.iter().any(|l| l.contains("effective config")), "panel title");
    }

    #[test]
    fn agents_ativos_also_gets_a_panel() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.dispatch("/agents active");
        let painted = render_to_lines(&mut app, 100, 30);
        assert!(painted.iter().any(|l| l.contains("no active agents")));
        assert!(painted.iter().any(|l| l.contains("active agents")), "panel title");
    }

    #[test]
    fn app_dispatch_unknown_still_errors() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.dispatch("/naoexiste");
        assert!(app.chat.iter().any(|m| m.text.contains("unknown command")));
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
        assert!(out.contains("refactor the auth module"));
        assert!(out.contains("a1"));
        assert!(out.contains("running") || out.contains("done"));
    }

    #[test]
    fn highlight_selection_paints_solid_bg_in_range() {
        let area = Rect { x: 0, y: 0, width: 10, height: 3 };
        let mut buf = ratatui::buffer::Buffer::empty(area);
        let theme = theme::DEFAULT;
        let (bg, fg) = selection_colors(theme);
        highlight_selection(area, &mut buf, (2, 1), (5, 1), theme);
        // Solid background, not REVERSED: the reversed version rendered nearly
        // invisible against these palettes.
        assert_eq!(buf.cell((2, 1)).unwrap().bg, rgb(bg));
        assert_eq!(buf.cell((2, 1)).unwrap().fg, rgb(fg));
        assert_eq!(buf.cell((5, 1)).unwrap().bg, rgb(bg));
        assert_ne!(buf.cell((0, 0)).unwrap().bg, rgb(bg), "fora do range fica intacto");
    }

    #[test]
    fn selection_colors_keep_text_readable_on_every_theme() {
        for name in theme::names() {
            let (bg, fg) = selection_colors(name);
            let luma = |c: (u8, u8, u8)| (c.0 as i32 * 299 + c.1 as i32 * 587 + c.2 as i32 * 114) / 1000;
            assert!((luma(bg) - luma(fg)).abs() > 90, "{name}: weak contrast between selection and text");
        }
    }

    #[test]
    fn selection_survives_the_mouse_release_and_clears_on_next_click() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.row_text = vec!["select me please".to_string()];
        app.mouse_down(0, 0);
        app.mouse_drag(9, 0);
        app.mouse_up(9, 0);
        // Still highlighted after release — that's the visual confirmation.
        assert!(app.selection_start.is_some() && app.selection_end.is_some());

        app.mouse_down(3, 0);
        app.mouse_up(3, 0);
        assert!(app.selection_start.is_none(), "clique simples limpa em vez de deixar uma célula suja");
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
    fn handle_stream_event_done_closes_rx_and_marks_end() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        let (_tx, rx) = mpsc::channel();
        app.rx = Some(rx);
        app.handle_stream_event(stream::Event::Done);
        assert!(app.rx.is_none());
        assert!(app.chat.iter().any(|m| m.text.contains("concluído")));
    }

    #[test]
    fn grill_hands_the_interview_to_the_master_without_showing_the_script() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.dispatch("/grill");

        // A turn is in flight: the master was asked, not a worker.
        assert!(app.rx.is_some(), "the interview goes to the master as a turn");
        // The briefing is machinery. What belongs on screen is the master's
        // first question, and the script would bury it.
        let shown: String = app.chat.iter().map(|m| m.text.clone()).collect::<Vec<_>>().join("\n");
        assert!(!shown.contains("One question at a time"), "the script leaked onto the screen: {shown}");
        assert!(shown.contains("interview"), "the user is told what is starting: {shown}");
    }

    #[test]
    fn grill_waits_instead_of_talking_over_a_running_turn() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        let (_tx, rx) = mpsc::channel();
        app.rx = Some(rx);
        app.dispatch("/grill");
        assert!(app.chat.iter().any(|m| m.text.contains("busy")), "says why nothing happened");
    }

    #[test]
    fn the_offer_records_which_route_was_taken() {
        // "grill" and "yes" both mean answered, but a later version should be
        // able to tell an interviewed directory from a scanned one.
        assert_eq!(Offer::from_cursor(0).recorded(), "grill");
        assert_eq!(Offer::from_cursor(1).recorded(), "yes");
        assert_eq!(Offer::from_cursor(2).recorded(), "no");
        // Out of range is a no, never an accidental model call.
        assert_eq!(Offer::from_cursor(9), Offer::No);
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
    fn extract_selection_multi_row_respects_the_click_columns() {
        // First row starts where the drag started, last row ends where it
        // ended, middle rows come whole — same shape the highlight paints.
        let rows = vec!["first   ".to_string(), "second".to_string(), "third  ".to_string()];
        assert_eq!(extract_selection(&rows, (2, 0), (3, 2)), "rst\nsecond\nthir");
    }

    #[test]
    fn selection_dragged_upward_starts_where_the_click_was() {
        // The reported bug: click mid-line on "localhost" and drag up, and the
        // copy came back with that line from its very beginning.
        let rows = vec!["acima da selecao".to_string(), "roda localhost pra testar".to_string()];
        let clique = (5, 1); // on "localhost"
        let solta = (6, 0); // dragged up into the row above
        // Normalized: from (6,0) down to (5,1) — row 0 from col 6, row 1 up to col 5.
        assert_eq!(extract_selection(&rows, clique, solta), "da selecao\nroda l");
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
        let (msg, _) = app.flash.clone().expect("flash de cópia");
        assert!(msg.contains("copiado"));
        // Names the route used, so a failed paste points at the right suspect.
        assert!(msg.contains("via"), "flash devia dizer por onde copiou: {msg}");
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
        assert_eq!(m[0].cmd, "/config");
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
        assert_eq!(app.input, COMMAND_CATALOG[0].cmd);
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
        let expected = app.command_menu()[2].cmd;
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

    /// Paints one frame of `app` and returns the rows as text, so a test can
    /// assert on what an overlay actually puts on screen.
    fn render_to_lines(app: &mut App, cols: u16, rows: u16) -> Vec<String> {
        let area = Rect::new(0, 0, cols, rows);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        draw(area, &mut buf, app);
        capture_row_text(&buf)
    }

    /// Regression: every overlay "cleared" its area with `Paragraph::new("")`,
    /// which draws nothing and leaves the cells underneath — so the banner and
    /// the agents panel showed straight through the popup.
    #[test]
    fn overlays_clear_what_is_underneath_them() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.open_agents_picker();
        let painted = render_to_lines(&mut app, 80, 24);
        let overlay: Vec<&String> = painted.iter().filter(|l| l.contains("Agent roster")).collect();
        assert_eq!(overlay.len(), 1, "overlay devia aparecer uma vez: {painted:?}");
        // The wordmark is drawn behind the popup; none of it may survive on the
        // overlay's own rows.
        assert!(!overlay[0].contains('█'), "banner vazando por dentro do overlay: {}", overlay[0]);

        let d = scan_tmp("bleed");
        let mut app2 = App::new(&config, "/tmp/outro");
        app2.scanned_path = d.join("s.yml");
        app2.offer_scan_if_first_run();
        let painted2 = render_to_lines(&mut app2, 80, 24);
        let row = painted2.iter().find(|l| l.contains("interview me")).expect("the overlay row");
        assert!(!row.contains("agents"), "agents panel bleeding through: {row}");
        assert!(!row.contains('─'), "another panel border bleeding through: {row}");
    }

    #[test]
    fn markdown_marks_render_instead_of_showing_as_literal_noise() {
        let t = theme::DEFAULT;
        let junta = |spans: Vec<Span<'static>>| -> String { spans.iter().map(|s| s.content.to_string()).collect() };

        let b = markdown_spans(t, "vem **forte** aqui", Role::Text);
        assert_eq!(junta(b.clone()), "vem forte aqui", "the stars leave the text");
        assert!(b.iter().any(|s| s.content == "forte" && s.style.add_modifier.contains(Modifier::BOLD)));

        let c = markdown_spans(t, "roda `bin/dev` agora", Role::Text);
        assert_eq!(junta(c.clone()), "roda bin/dev agora");
        assert!(c.iter().any(|s| s.content == "bin/dev"), "code becomes its own span");

        let h = markdown_spans(t, "### 4. Modelos", Role::Text);
        assert_eq!(junta(h.clone()), "4. Modelos", "the hashes go away");
        assert!(h[0].style.add_modifier.contains(Modifier::BOLD));

        // Unpaired marks stay literal rather than eating the rest of the line.
        assert_eq!(junta(markdown_spans(t, "2 ** 3 = 8", Role::Text)), "2 ** 3 = 8");
        assert_eq!(junta(markdown_spans(t, "crase ` solta", Role::Text)), "crase ` solta");
    }

    #[test]
    fn wrap_breaks_on_spaces_instead_of_mid_word() {
        let linhas = wrap_text("at this point the conversion improves", 20);
        assert!(linhas.iter().all(|l| l.len() <= 20));
        for l in &linhas {
            assert!(!l.ends_with(' '), "sem espaço sobrando na quebra: {l:?}");
        }
        assert_eq!(linhas.join(" "), "at this point the conversion improves", "nada se perde nem se duplica");

        // A single word longer than the line has nowhere to break — still cut.
        let gigante = wrap_text("supercalifragilistico", 8);
        assert_eq!(gigante, vec!["supercal", "ifragili", "stico"]);
    }

    #[test]
    fn user_messages_get_a_filled_band_and_assistant_ones_do_not() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.push(ChatRole::User, "sobe o servidor");
        app.push(ChatRole::Assistant, "running on localhost:3000");
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        draw(area, &mut buf, &mut app);

        let fundo_da_linha = |texto: &str| -> Option<Vec<ratatui::style::Color>> {
            (0..area.height).find_map(|row| {
                let linha: String = (0..area.width)
                    .map(|c| buf.cell((c, row)).unwrap().symbol().to_string())
                    .collect();
                linha.contains(texto).then(|| {
                    (0..area.width).map(|c| buf.cell((c, row)).unwrap().bg).collect()
                })
            })
        };
        let (bg, _) = user_row_colors(theme::DEFAULT);
        let user = fundo_da_linha("sobe o servidor").expect("the user row");
        let assistant = fundo_da_linha("running on localhost").expect("the master row");

        assert!(user.iter().filter(|c| **c == rgb(bg)).count() > 40, "filled band on the user speech");
        assert!(!assistant.iter().any(|c| *c == rgb(bg)), "the master speech takes no band");
    }

    #[test]
    fn the_user_band_stays_readable_on_every_theme() {
        for name in theme::names() {
            let (bg, fg) = user_row_colors(name);
            let luma = |c: (u8, u8, u8)| (c.0 as i32 * 299 + c.1 as i32 * 587 + c.2 as i32 * 114) / 1000;
            assert!((luma(fg) - luma(bg)).abs() > 70, "{name}: texto do usuário sem contraste na faixa");
        }
    }

    #[test]
    fn running_turn_shows_an_activity_line() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        // The status bar is the last row; the welcome message also mentions
        // /help, so look at the bar itself rather than the whole screen.
        let barra = |app: &mut App| render_to_lines(app, 90, 24).last().cloned().unwrap_or_default();
        let ocioso = barra(&mut app);
        assert!(ocioso.contains("/help"), "parado: barra normal");

        app.turn_started = Some(Instant::now());
        app.turn_chars = 34_000;
        let rodando = barra(&mut app);
        assert!(rodando.contains("8.5k tokens"), "token estimate: {rodando}");
        assert!(rodando.contains("0s"), "tempo decorrido");
        assert!(ACTIVITY_WORDS.iter().any(|w| rodando.contains(w)), "activity word: {rodando}");
        assert!(!rodando.contains("/help"), "a barra normal dá lugar à de atividade");
    }

    #[test]
    fn activity_line_rotates_words_and_animates() {
        // Same second, same word: no jitter between frames.
        assert_eq!(activity_word(7), activity_word(7));
        assert_ne!(activity_word(0), activity_word(4), "troca a cada 4s");
        // Wraps instead of running off the end of the list.
        assert_eq!(activity_word(0), activity_word(4 * ACTIVITY_WORDS.len() as u64));
        assert_ne!(spinner_frame(0), spinner_frame(1));
        assert_eq!(spinner_frame(0), spinner_frame(10));
    }

    #[test]
    fn elapsed_and_tokens_read_like_a_status_line() {
        assert_eq!(fmt_elapsed(Duration::from_secs(9)), "9s");
        assert_eq!(fmt_elapsed(Duration::from_secs(258)), "4m 18s");
        assert_eq!(fmt_tokens(0), "0 tokens");
        assert_eq!(fmt_tokens(400), "100 tokens");
        assert_eq!(fmt_tokens(34_400), "8.6k tokens");
    }

    #[test]
    fn turn_tracking_starts_and_stops_with_the_stream() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        assert!(app.turn_started.is_none(), "idle on open");

        app.turn_started = Some(Instant::now());
        app.handle_stream_event(stream::Event::Text("abcd".into()));
        assert_eq!(app.turn_chars, 4, "counts what arrived to estimate tokens");

        app.handle_stream_event(stream::Event::Done);
        assert!(app.turn_started.is_none(), "finished: the activity line goes away");
    }

    #[test]
    fn buddy_does_not_get_painted_over_the_conversation() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        let long = "palavra ".repeat(12);
        app.push(ChatRole::Assistant, long);
        let sem_buddy = render_to_lines(&mut app, 100, 30);

        app.buddy = Some(crate::buddy::Buddy::hatch("semente"));
        let com_buddy = render_to_lines(&mut app, 100, 30);

        // The buddy paints over the text with `Clear`, so the failure isn't
        // overlap — it's the message silently losing characters under the panel.
        // Wrapping can split a word across rows, so compare the whole sequence
        // with whitespace removed, cut before the buddy's column.
        let borda = 100 - (BUDDY_WIDTH as usize + 2);
        let seq = |ls: &[String], cut: usize| -> String {
            ls.iter()
                .flat_map(|l| l.chars().take(cut))
                .filter(|c| !c.is_whitespace())
                .collect()
        };
        let esperado = "palavra".repeat(12);
        assert!(seq(&sem_buddy, 100).contains(&esperado), "with no buddy the whole message shows");
        assert!(
            seq(&com_buddy, borda).contains(&esperado),
            "with the buddy open no stretch may vanish: the text has to rewrap narrower"
        );
    }

    #[test]
    fn wheel_scrolls_the_conversation_and_stops_at_both_ends() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        for i in 0..40 {
            app.push(ChatRole::Info, format!("linha {i}"));
        }
        // One frame to measure the pane, like the event loop does.
        let painted = render_to_lines(&mut app, 80, 24);
        assert!(painted.iter().any(|l| l.contains("linha 39")), "começa colado no fim");
        assert_eq!(app.scroll, 0);

        app.scroll_by(3); // one wheel notch up
        assert_eq!(app.scroll, 3);
        let painted = render_to_lines(&mut app, 80, 24);
        assert!(painted.iter().any(|l| l.contains("3 more lines")), "avisa que há coisa abaixo: {painted:?}");

        // Can't scroll past the first line...
        app.scroll_by(9999);
        let top = app.scroll;
        assert_eq!(top, app.chat_total.saturating_sub(app.chat_rows), "stops at the first row");
        let painted = render_to_lines(&mut app, 80, 24);
        assert!(painted.iter().any(|l| l.contains("linha 0")), "topo à vista: {painted:?}");

        // ...nor below the newest.
        app.scroll_by(-9999);
        assert_eq!(app.scroll, 0);

        // Sending snaps back to the bottom.
        app.scroll_by(5);
        app.input = "oi".into();
        app.submit();
        assert_eq!(app.scroll, 0, "enviar volta pro fim");
    }

    #[test]
    fn page_keys_move_a_screenful_at_a_time() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        for i in 0..100 {
            app.push(ChatRole::Info, format!("linha {i}"));
        }
        render_to_lines(&mut app, 80, 24);
        let page = app.chat_rows.saturating_sub(1);
        app.scroll_page(1);
        assert_eq!(app.scroll, page, "PageUp sobe uma tela cheia menos a linha de overlap");
        app.scroll_page(-1);
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn resume_replays_the_past_conversation_into_the_chat() {
        let home = scan_tmp("resume-home");
        let repo = "/home/u/proj";
        let dir = home.join(".claude/projects").join(transcript::project_slug(repo));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("sess-9.jsonl"),
            [
                r#"{"type":"user","message":{"role":"user","content":"playbook\n\nTarefa: resuma o projeto"}}"#,
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"x"}]}}"#,
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"É um app Rails."}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let config = Config::default();
        let mut app = App::new(&config, repo);
        app.home = home;
        app.resume_list =
            vec![SessionRec { id: "sess-9".into(), title: "resuma".into(), repo: repo.into(), ts: 1 }];
        app.mode = Mode::ResumePicker { cursor: 0 };
        app.resume_picker_confirm();

        assert_eq!(app.session_id.as_deref(), Some("sess-9"));
        let chat: Vec<(bool, String)> =
            app.chat.iter().map(|m| (matches!(m.role, ChatRole::User), m.text.clone())).collect();
        assert!(chat.contains(&(true, "resuma o projeto".to_string())), "user speech: {chat:?}");
        assert!(chat.contains(&(false, "É um app Rails.".to_string())), "master answer: {chat:?}");
        assert!(!chat.iter().any(|(_, t)| t.contains("playbook")), "the playbook does not reach the screen");
        assert!(app.chat.iter().any(|m| m.text.contains("end of history")), "marks where the past ends");
    }

    #[test]
    fn resume_without_a_transcript_still_resumes() {
        let config = Config::default();
        let mut app = App::new(&config, "/home/u/sem-historico");
        app.home = scan_tmp("resume-vazio");
        app.resume_list =
            vec![SessionRec { id: "nada".into(), title: "t".into(), repo: "/home/u/sem-historico".into(), ts: 1 }];
        app.mode = Mode::ResumePicker { cursor: 0 };
        app.resume_picker_confirm();
        // Best-effort: no transcript (other CLI, cleaned history) is not a failure.
        assert_eq!(app.session_id.as_deref(), Some("nada"));
        assert!(app.chat.iter().any(|m| m.text.contains("resuming session")));
    }

    #[test]
    fn scan_offer_asks_once_then_remembers_the_no() {
        let dir = scan_tmp("offer");
        let config = Config::default();
        let mut app = App::new(&config, dir.to_str().unwrap());
        app.scanned_path = dir.join("state/scanned.yml");

        app.offer_scan_if_first_run();
        assert!(matches!(app.mode, Mode::ScanOffer { .. }));
        // The question lives in the overlay, not in the chat log — as a log
        // line it read as scrollback and got typed straight past.
        assert!(!app.chat.iter().any(|m| m.text.contains(scan::CONTEXT_FILE)), "the question does not reach the log");
        let painted = render_to_lines(&mut app, 100, 40);
        assert!(painted.iter().any(|l| l.contains("Get to know")), "the overlay asks: {painted:?}");
        assert!(painted.iter().any(|l| l.contains(scan::CONTEXT_FILE)));
        assert!(painted.iter().any(|l| l.contains("interview me")), "the interview is offered first");
        assert!(painted.iter().any(|l| l.contains("just scan the files")));

        app.answer_scan_offer(Offer::No);
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.scan_rx.is_none(), "no scan should have been fired");

        // Opening the same directory again: silence.
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
        assert_eq!(app.input, "primeira", "stops at the oldest, does not vanish");

        app.history_next();
        assert_eq!(app.input, "segunda");
        app.history_next();
        assert_eq!(app.input, "", "past the newest: back to an empty input");
        assert!(app.history_idx.is_none());
    }

    #[test]
    fn history_restores_the_half_typed_draft() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.history_push("old command");
        app.input = "was typing this".into();

        app.history_prev();
        assert_eq!(app.input, "old command");
        app.history_next();
        assert_eq!(app.input, "was typing this", "o rascunho volta intacto");
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
        assert_eq!(app.history.len(), 3, "a non-consecutive one goes in again");
    }

    #[test]
    fn history_prev_is_noop_on_first_run() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.input = "rascunho".into();
        app.history_prev();
        assert_eq!(app.input, "rascunho", "with no history, ↑ leaves the input alone");
    }

    #[test]
    fn recalled_slash_command_does_not_reopen_the_popup() {
        let config = Config::default();
        let mut app = App::new(&config, "/tmp/repo");
        app.history_push("/help");
        app.history_prev();
        assert_eq!(app.input, "/help");
        assert!(!app.menu_open(), "↑↓ keep browsing history until you type again");
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
    fn clipboard_helper_prefers_wayland_then_x11() {
        let all = |_: &str| true;
        assert_eq!(pick_clipboard_helper(true, true, all).unwrap(), vec!["wl-copy"]);
        assert_eq!(pick_clipboard_helper(false, true, all).unwrap()[0], "xclip");
        // Wayland session but wl-copy missing: fall through to the X11 bridge.
        assert_eq!(pick_clipboard_helper(true, true, |b| b != "wl-copy").unwrap()[0], "xclip");
        assert_eq!(pick_clipboard_helper(false, true, |b| b == "xsel").unwrap()[0], "xsel");
        // Headless/ssh: no helper, OSC52 carries it alone.
        assert!(pick_clipboard_helper(false, false, all).is_none());
        assert!(pick_clipboard_helper(true, false, |_| false).is_none());
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
