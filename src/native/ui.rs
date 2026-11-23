//! Full-screen console for the built-in agent.
//!
//! Layout, top to bottom: the mark and session header, the conversation,
//! the boxed prompt, then the slash menu, the live spinner or the status line.
//! The screen follows the agent console design and keeps every existing
//! interaction: history, `/` menu, F2 tool details, scrollback and the
//! suspend/resume handshake used while a reviewed edit or a task runs.

mod render;
mod selection;

use super::Message;
use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    },
    execute, queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
    terminal::{
        self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate,
        EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use render::{Line, Style};
use selection::{Selection, Viewport};
use std::{
    io::{self, Write},
    path::Path,
    time::{Duration, Instant},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MAX_CONTENT: usize = 120;
const MENU_LIMIT: usize = 6;
const CTX_TOKENS: u32 = 128_000;
const INPUT_LIMIT: usize = 64 * 1024;
const PASTE_LIMIT: usize = 192 * 1024;

/// Commands offered by the slash menu. Descriptions are localized.
fn commands() -> [(&'static str, &'static str); 22] {
    [
        ("/project", tr!("Создать проект", "Create project")),
        ("/browser", tr!("Скрытый браузер", "Hidden browser")),
        ("/computer", tr!("Окно Windows", "Windows window")),
        ("/tools", tr!("Доступные инструменты", "Available tools")),
        ("/search", tr!("Поиск в интернете", "Search the web")),
        ("/permissions", tr!("Режим разрешений", "Permission mode")),
        (
            "/model",
            tr!(
                "Модели: добавить, изменить, подключить",
                "Models: add, edit, connect"
            ),
        ),
        (
            "/skills",
            tr!(
                "Скиллы: список и инструкции",
                "Skills: list and instructions"
            ),
        ),
        (
            "/mcp",
            tr!(
                "MCP-серверы: статус, инструменты, импорт",
                "MCP servers: status, tools, import"
            ),
        ),
        (
            "/diagnose",
            tr!(
                "Чтение проекта и поиск причины",
                "Read the project and investigate"
            ),
        ),
        (
            "/code",
            tr!(
                "Правки и команды после подтверждения",
                "Reviewed edits and approved commands"
            ),
        ),
        (
            "/sessions",
            tr!(
                "Сохранённые диалоги этого проекта",
                "Saved conversations for this project"
            ),
        ),
        (
            "/new",
            tr!(
                "Начать новый сохраняемый диалог",
                "Start a new saved conversation"
            ),
        ),
        (
            "/temporary",
            tr!(
                "Диалог без сохранения истории",
                "Chat without saving history"
            ),
        ),
        (
            "/clear",
            tr!("Удалить текущую историю", "Delete the current history"),
        ),
        (
            "/undo",
            tr!("Отменить последнюю правку", "Revert the last applied edit"),
        ),
        (
            "/status",
            tr!(
                "Модель, токены, скорость, время сеанса",
                "Model, tokens, speed, session time"
            ),
        ),
        (
            "/reasoning",
            tr!("Уровень рассуждений модели", "Model reasoning level"),
        ),
        (
            "/init",
            tr!(
                "Создать AGENTS.md по структуре проекта",
                "Create AGENTS.md from the project layout"
            ),
        ),
        ("/help", tr!("Команды и клавиши", "Commands and keys")),
        ("/quit", tr!("Вернуться в меню", "Return to the launcher")),
        ("/exit", tr!("Выйти в главное меню", "Exit to the launcher")),
    ]
}

/// Slash menu contents for the current buffer.
fn menu_items(text: &str, closed: bool, navigating: bool) -> Vec<(&'static str, &'static str)> {
    let typed = text.split_whitespace().last().unwrap_or("").to_lowercase();
    if closed || navigating || !typed.starts_with('/') || text.ends_with(char::is_whitespace) {
        return Vec::new();
    }
    commands()
        .into_iter()
        .filter(|(name, _)| name.starts_with(&typed))
        .collect()
}

fn text_key(key: KeyEvent) -> bool {
    // AltGr arrives as Right Alt + Ctrl on Windows and still types text.
    let mask = KeyModifiers::CONTROL | KeyModifiers::ALT;
    !key.modifiers.intersects(mask) || key.modifiers.contains(mask)
}

/// Windows reports Ctrl+A..Ctrl+Z as control characters on a Latin layout and
/// as layout letters on a Cyrillic one (`Ctrl+C` arrives as `с`). Normalize
/// both to the Latin letter with the CONTROL modifier.
fn normalize_key(key: KeyEvent) -> KeyEvent {
    if !key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) {
        return key;
    }
    let KeyCode::Char(character) = key.code else {
        return key;
    };
    let value = character as u32;
    let letter = if (1..=26).contains(&value) {
        char::from(b'a' + (value as u8 - 1))
    } else if character.is_ascii() {
        character.to_ascii_lowercase()
    } else {
        match character.to_lowercase().next() {
            // The ЙЦУКЕН layout puts Latin control letters on these keys.
            Some('ф') => 'a',
            Some('и') => 'b',
            Some('с') => 'c',
            Some('в') => 'd',
            Some('у') => 'e',
            Some('а') => 'f',
            Some('п') => 'g',
            Some('р') => 'h',
            Some('ш') => 'i',
            Some('о') => 'j',
            Some('л') => 'k',
            Some('д') => 'l',
            Some('ь') => 'm',
            Some('т') => 'n',
            Some('щ') => 'o',
            Some('з') => 'p',
            Some('й') => 'q',
            Some('к') => 'r',
            Some('ы') => 's',
            Some('е') => 't',
            Some('г') => 'u',
            Some('м') => 'v',
            Some('ц') => 'w',
            Some('ч') => 'x',
            Some('н') => 'y',
            Some('я') => 'z',
            _ => return key,
        }
    };
    KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL)
}

fn paste_text(text: &str) -> String {
    let mut out = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.push(' '),
            other if !other.is_control() => out.push(other),
            _ => {}
        }
    }
    out
}

struct Editor {
    buffer: Vec<char>,
    caret: usize,
}

impl Editor {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            caret: 0,
        }
    }

    fn from(text: &str) -> Self {
        let buffer: Vec<char> = text
            .chars()
            .filter(|character| !character.is_control() || *character == '\n')
            .collect();
        let caret = buffer.len();
        Self { buffer, caret }
    }

    fn prefix(&self) -> String {
        self.buffer[..self.caret].iter().collect()
    }

    fn replace_tail(&mut self, chars: usize, replacement: &str) {
        let start = self.caret.saturating_sub(chars);
        self.buffer.drain(start..self.caret);
        self.caret = start;
        self.insert(replacement.chars());
    }

    fn text(&self) -> String {
        self.buffer.iter().collect()
    }

    fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    fn clear(&mut self) {
        self.buffer.clear();
        self.caret = 0;
    }

    fn paste(&mut self, text: &str) {
        self.insert(paste_text(text).chars());
    }

    fn insert(&mut self, text: impl IntoIterator<Item = char>) {
        if self.buffer.len() >= INPUT_LIMIT {
            return;
        }
        let text: Vec<char> = text
            .into_iter()
            .filter(|c| !c.is_control() || *c == '\n')
            .take(INPUT_LIMIT - self.buffer.len())
            .collect();
        let count = text.len();
        self.buffer.splice(self.caret..self.caret, text);
        self.caret += count;
    }

    fn backspace(&mut self) {
        if self.caret > 0 {
            self.caret -= 1;
            self.buffer.remove(self.caret);
        }
    }

    fn delete(&mut self) {
        if self.caret < self.buffer.len() {
            self.buffer.remove(self.caret);
        }
    }

    fn left(&mut self) {
        self.caret = self.caret.saturating_sub(1);
    }

    fn right(&mut self) {
        self.caret = (self.caret + 1).min(self.buffer.len());
    }

    fn home(&mut self) {
        self.caret = 0;
    }

    fn end(&mut self) {
        self.caret = self.buffer.len();
    }

    fn kill_to_start(&mut self) {
        self.buffer.drain(..self.caret);
        self.caret = 0;
    }

    fn kill_to_end(&mut self) {
        self.buffer.truncate(self.caret);
    }

    fn kill_word(&mut self) {
        let mut start = self.caret;
        while start > 0 && self.buffer[start - 1] == ' ' {
            start -= 1;
        }
        while start > 0 && self.buffer[start - 1] != ' ' {
            start -= 1;
        }
        self.buffer.drain(start..self.caret);
        self.caret = start;
    }

    /// Visible slice and caret column (in cells) for a boxed single-line editor.
    fn view(&self, width: usize) -> (String, usize) {
        if width == 0 {
            return (String::new(), 0);
        }
        let mut start = self.caret;
        let mut caret = 0;
        while let Some(&character) = start.checked_sub(1).and_then(|i| self.buffer.get(i)) {
            let cell = editor_width(character);
            if caret + cell >= width {
                break;
            }
            caret += cell;
            start -= 1;
        }
        let mut shown = String::new();
        let mut used = 0;
        for character in &self.buffer[start..] {
            let cell = editor_width(*character);
            if used + cell > width {
                break;
            }
            shown.push(if *character == '\n' {
                '↵'
            } else {
                *character
            });
            used += cell;
        }
        (shown, caret)
    }

    fn completion_prefix(&self) -> String {
        self.buffer[self.caret.saturating_sub(512)..self.caret]
            .iter()
            .collect()
    }
}

fn editor_width(character: char) -> usize {
    if character == '\n' {
        1
    } else {
        character.width().unwrap_or(0)
    }
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(super) struct Receipt {
    input: u32,
    output: u32,
    estimated: bool,
    duration: f32,
    speed: f32,
    ttft: Option<f32>,
}

#[derive(Clone, Copy, Default)]
struct Stats {
    tin: u32,
    tout: u32,
    turns: u32,
    last: Option<Receipt>,
}

#[derive(Clone, Copy)]
struct Turn {
    started: Instant,
    input: u32,
    output: u32,
    estimated: bool,
    first: Option<Instant>,
    request_seconds: f32,
    writing: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CacheKey {
    messages: usize,
    bytes: usize,
    reasoning_bytes: usize,
    details: bool,
    reasoning_details: bool,
    width: usize,
}

struct Cache {
    key: CacheKey,
    lines: Vec<Line>,
}

/// One saved model connection as the manager needs to see it.
#[derive(Clone, Default)]
pub struct ModelEntry {
    pub name: String,
    pub url: String,
    pub model: String,
    /// `Some` only when the user typed a new key in the form.
    pub key: Option<String>,
    pub key_set: bool,
    pub key_source: Option<usize>,
    pub anthropic: bool,
    pub responses: bool,
    pub vision: bool,
    pub chat_only: bool,
    pub context: u32,
    pub input: u32,
    pub output: u32,
    pub reasoning: super::Reasoning,
    pub reasoning_api: super::ReasoningApi,
}

impl ModelEntry {
    pub(super) fn missing_limits(&self) -> bool {
        [self.context, self.input, self.output].contains(&0)
    }

    pub(super) fn validation_error(&self) -> Option<String> {
        ModelForm::new(None, Some(self)).entry().err()
    }
}

pub enum ModelAction {
    Activate(usize),
    Test(usize),
    Duplicate(usize, bool),
    Catalog {
        index: Option<usize>,
        entry: ModelEntry,
    },
    Auto {
        index: Option<usize>,
        entry: ModelEntry,
    },
    Save {
        index: Option<usize>,
        entry: ModelEntry,
    },
    Delete(usize),
    Close,
}

pub(super) enum StreamControl {
    Continue,
    Stop,
    Command(String),
}

pub struct Screen {
    editor: Editor,
    pastes: Vec<String>,
    submitted_pastes: Vec<String>,
    notice: String,
    scroll: usize,
    details: bool,
    queued: String,
    submit_queued: bool,
    submitted: Vec<String>,
    nav: Option<usize>,
    selected: usize,
    menu_closed: bool,
    armed: bool,
    diagnose: bool,
    tick: usize,
    turn: Option<Turn>,
    stats: Stats,
    cache: Option<Cache>,
    context_limit: u32,
    input_limit: u32,
    output_limit: u32,
    input_estimate: Option<u32>,
    reasoning: super::Reasoning,
    reasoning_details: bool,
    background: Vec<u8>,
    desktop_active: bool,
    panel_frame: Option<PanelFrame>,
    viewport: Option<Viewport>,
    selection: Option<Selection>,
    permission_mode: super::permissions::Mode,
    approval_card: bool,
}

impl Screen {
    pub fn open() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let screen = Self {
            editor: Editor::new(),
            pastes: Vec::new(),
            submitted_pastes: Vec::new(),
            notice: String::new(),
            scroll: 0,
            details: false,
            queued: String::new(),
            submit_queued: false,
            submitted: Vec::new(),
            nav: None,
            selected: 0,
            menu_closed: false,
            armed: false,
            diagnose: false,
            tick: 0,
            turn: None,
            stats: Stats::default(),
            cache: None,
            context_limit: CTX_TOKENS,
            input_limit: CTX_TOKENS.saturating_sub(4096),
            output_limit: 4096,
            input_estimate: None,
            reasoning: crate::native::Reasoning::Off,
            reasoning_details: false,
            background: Vec::new(),
            desktop_active: false,
            panel_frame: None,
            viewport: None,
            selection: None,
            permission_mode: Default::default(),
            approval_card: false,
        };
        if let Err(error) = enter() {
            let _ = terminal::disable_raw_mode();
            return Err(error.into());
        }
        Ok(screen)
    }

    /// Sets the context window of the active model, used by every ctx readout.
    pub fn set_limits(
        &mut self,
        context: u32,
        input: u32,
        output: u32,
        reasoning: super::Reasoning,
    ) {
        self.context_limit = context.max(1_000);
        self.input_limit = input.clamp(1, self.context_limit);
        self.output_limit = output.max(256);
        self.reasoning = reasoning;
    }

    #[cfg(windows)]
    pub fn desktop_active(&mut self, active: bool) {
        self.desktop_active = active;
    }

    pub fn set_input_estimate(&mut self, tokens: Option<u32>) {
        self.input_estimate = tokens;
    }

    pub fn report(&mut self, title: &str, body: &str) -> Result<()> {
        let (terminal_width, _) = terminal::size()?;
        let width = (usize::from(terminal_width).saturating_sub(8).clamp(36, 96)).saturating_sub(4);
        let mut lines = Vec::new();
        for paragraph in body.lines() {
            if paragraph.trim().is_empty() {
                lines.push(Line::new());
                continue;
            }
            let mut line = Line::new();
            line.push(paragraph, Style::fg(render::FG));
            lines.extend(render::wrap(&line, width));
        }
        self.overlay(title, lines)
    }

    pub fn reset_conversation(&mut self) {
        self.permission_mode = Default::default();
        self.stats = Stats::default();
        self.turn = None;
        self.input_estimate = None;
        self.notice.clear();
        self.editor.clear();
        self.pastes.clear();
        self.submitted_pastes.clear();
        self.queued.clear();
        self.submit_queued = false;
        self.submitted.clear();
        self.nav = None;
        self.scroll = 0;
        self.selected = 0;
        self.menu_closed = false;
        self.armed = false;
        self.cache = None;
        self.selection = None;
        self.viewport = None;
        self.panel_frame = None;
        self.background.clear();
    }
    pub fn notice(&mut self, text: &str) {
        self.notice = render::sanitize(text);
    }

    pub fn help(&mut self) -> Result<()> {
        self.overlay(tr!("Команды и клавиши", "Commands and keys"), help_lines())
    }

    pub fn input(
        &mut self,
        history: &[Message],
        model: &str,
        directory: &Path,
        diagnose: bool,
    ) -> Result<Option<String>> {
        self.diagnose = diagnose;
        if std::mem::take(&mut self.submit_queued) {
            let line = std::mem::take(&mut self.queued);
            if line == "/status" {
                self.status_panel(model, directory)?;
            } else {
                return Ok(Some(line));
            }
        }
        self.draw(history, None, model, directory)?;
        let mut redraw = false;
        loop {
            if redraw && !event::poll(Duration::ZERO)? {
                self.draw(history, None, model, directory)?;
                redraw = false;
            }
            let event = event::read()?;
            if ignored_event(&event) {
                continue;
            }
            redraw = true;
            match event {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let key = normalize_key(key);
                    let items = self.menu(directory);
                    let count = items.len();
                    let mut edited = false;
                    match key.code {
                        KeyCode::F(6) => {
                            self.permissions_menu()?;
                        }
                        KeyCode::Enter | KeyCode::Tab if count > 0 => {
                            let name = &items[self.selected.min(count - 1)].0;
                            if key.code == KeyCode::Tab && name.starts_with('/') {
                                let prefix = self.editor.prefix();
                                let token = prefix.split_whitespace().last().unwrap_or("");
                                self.editor
                                    .replace_tail(token.chars().count(), &format!("{name} "));
                                self.menu_closed = true;
                            } else if let Some(command) = self.complete(name) {
                                if command == "/status" {
                                    self.status_panel(model, directory)?;
                                } else {
                                    return Ok(Some(command));
                                }
                            }
                            self.selected = 0;
                            continue;
                        }
                        KeyCode::Enter => {
                            let line = self.editor.text().trim().to_owned();
                            if line.is_empty() && self.pastes.is_empty() {
                                continue;
                            }
                            if line == "/status" {
                                let _ = self.submit(&line);
                                self.status_panel(model, directory)?;
                                continue;
                            }
                            return Ok(Some(self.submit(&line)));
                        }
                        KeyCode::F(8) if self.desktop_active => {
                            return Ok(Some("/computer stop".into()));
                        }
                        #[cfg(windows)]
                        KeyCode::F(7) => self.paste_clipboard(),
                        #[cfg(windows)]
                        KeyCode::Char('v') if key.modifiers == KeyModifiers::CONTROL => {
                            self.paste_clipboard();
                        }
                        KeyCode::Esc => {
                            if self.selection.take().is_some() {
                                continue;
                            }
                            if count > 0 {
                                self.menu_closed = true;
                            } else if !self.editor.is_empty() || !self.pastes.is_empty() {
                                self.editor.clear();
                                self.pastes.clear();
                                edited = true;
                            }
                            self.armed = false;
                        }
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            if self.copy_selection() {
                                continue;
                            }
                            if !self.editor.is_empty() || !self.pastes.is_empty() {
                                self.editor.clear();
                                self.pastes.clear();
                                edited = true;
                            } else if self.armed {
                                return Ok(None);
                            } else {
                                self.armed = true;
                            }
                        }
                        KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => {
                            if self.editor.is_empty() && self.pastes.is_empty() {
                                return Ok(None);
                            }
                        }
                        KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            self.scroll = self.scroll.saturating_add(5);
                        }
                        KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            self.scroll = self.scroll.saturating_sub(5);
                        }
                        KeyCode::Up => {
                            if count > 0 {
                                self.selected = (self.selected.min(count - 1) + count - 1) % count;
                            } else if !self.submitted.is_empty() {
                                let next = self.nav.map_or(self.submitted.len() - 1, |index| {
                                    index.saturating_sub(1)
                                });
                                self.editor = Editor::from(&self.submitted[next]);
                                self.nav = Some(next);
                                edited = true;
                            }
                        }
                        KeyCode::Down => {
                            if count > 0 {
                                self.selected = (self.selected.min(count - 1) + 1) % count;
                            } else if let Some(index) = self.nav {
                                if index + 1 < self.submitted.len() {
                                    self.editor = Editor::from(&self.submitted[index + 1]);
                                    self.nav = Some(index + 1);
                                } else {
                                    self.editor.clear();
                                    self.nav = None;
                                }
                                edited = true;
                            }
                        }
                        KeyCode::Left => self.editor.left(),
                        KeyCode::Right => self.editor.right(),
                        KeyCode::Home => self.editor.home(),
                        KeyCode::End => self.editor.end(),
                        KeyCode::Backspace => {
                            if self.editor.is_empty() {
                                self.pastes.pop();
                            } else {
                                self.editor.backspace();
                            }
                            edited = true;
                        }
                        KeyCode::Delete => {
                            self.editor.delete();
                            edited = true;
                        }
                        KeyCode::Char('a') if key.modifiers == KeyModifiers::CONTROL => {
                            self.editor.home();
                        }
                        KeyCode::Char('e') if key.modifiers == KeyModifiers::CONTROL => {
                            self.editor.end();
                        }
                        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                            self.editor.kill_to_start();
                            edited = true;
                        }
                        KeyCode::Char('k') if key.modifiers == KeyModifiers::CONTROL => {
                            self.editor.kill_to_end();
                            edited = true;
                        }
                        KeyCode::Char('w') if key.modifiers == KeyModifiers::CONTROL => {
                            self.editor.kill_word();
                            edited = true;
                        }
                        KeyCode::F(2) => {
                            self.selection = None;
                            self.details = !self.details;
                        }
                        KeyCode::F(3) => {
                            self.selection = None;
                            self.reasoning_details = !self.reasoning_details;
                        }
                        KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
                        KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
                        KeyCode::Char(character) if text_key(key) && !character.is_control() => {
                            self.editor.insert([character]);
                            edited = true;
                        }
                        _ => {}
                    }
                    if edited {
                        self.selection = None;
                        self.selected = 0;
                        self.menu_closed = false;
                        self.armed = false;
                        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
                            self.nav = None;
                        }
                    }
                }
                Event::Mouse(mouse) => self.chat_mouse(mouse),
                Event::Paste(text) => {
                    self.selection = None;
                    self.paste_prompt(&text);
                    self.selected = 0;
                    self.menu_closed = false;
                    self.nav = None;
                    self.armed = false;
                }
                _ => {}
            }
        }
    }

    pub fn streaming(
        &mut self,
        history: &[Message],
        output: &str,
        reasoning: &str,
        model: &str,
        directory: &Path,
    ) -> Result<StreamControl> {
        self.track(output, reasoning);
        self.tick = self.tick.wrapping_add(1);
        self.draw(history, Some((output, reasoning)), model, directory)?;
        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let key = normalize_key(key);
                    let items = self.menu(directory);
                    match key.code {
                        KeyCode::F(8) if self.desktop_active => {
                            return Ok(StreamControl::Command("/computer stop".into()));
                        }
                        KeyCode::Esc => return Ok(StreamControl::Stop),
                        #[cfg(windows)]
                        KeyCode::F(7) => self.paste_clipboard(),
                        #[cfg(windows)]
                        KeyCode::Char('v') if key.modifiers == KeyModifiers::CONTROL => {
                            self.paste_clipboard();
                        }
                        KeyCode::F(6) => self.permissions_menu()?,
                        KeyCode::F(2) => {
                            self.selection = None;
                            self.details = !self.details;
                        }
                        KeyCode::F(3) => {
                            self.selection = None;
                            self.reasoning_details = !self.reasoning_details;
                        }
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            if !self.copy_selection() {
                                return Ok(StreamControl::Stop);
                            }
                        }
                        KeyCode::Enter | KeyCode::Tab if !items.is_empty() => {
                            if let Some(command) =
                                self.complete(&items[self.selected.min(items.len() - 1)].0)
                            {
                                return Ok(StreamControl::Command(command));
                            }
                            self.selected = 0;
                        }
                        KeyCode::Enter if self.editor.text().trim_start().starts_with('/') => {
                            let command = self.editor.text().trim().to_owned();
                            self.editor.clear();
                            return Ok(StreamControl::Command(command));
                        }
                        KeyCode::Up if !items.is_empty() && key.modifiers == KeyModifiers::NONE => {
                            self.selected = (self.selected + items.len() - 1) % items.len()
                        }
                        KeyCode::Down
                            if !items.is_empty() && key.modifiers == KeyModifiers::NONE =>
                        {
                            self.selected = (self.selected + 1) % items.len()
                        }
                        KeyCode::PageUp | KeyCode::Up => {
                            self.scroll = self.scroll.saturating_add(5)
                        }
                        KeyCode::PageDown | KeyCode::Down => {
                            self.scroll = self.scroll.saturating_sub(5)
                        }
                        KeyCode::Left => self.editor.left(),
                        KeyCode::Right => self.editor.right(),
                        KeyCode::Home => self.editor.home(),
                        KeyCode::End => self.editor.end(),
                        KeyCode::Backspace => {
                            if self.editor.is_empty() {
                                self.pastes.pop();
                            } else {
                                self.editor.backspace();
                            }
                            self.selected = 0;
                            self.menu_closed = false;
                        }
                        KeyCode::Delete => {
                            self.editor.delete();
                            self.selected = 0;
                            self.menu_closed = false;
                        }
                        KeyCode::Char(character) if text_key(key) && !character.is_control() => {
                            self.editor.insert([character]);
                            self.selected = 0;
                            self.menu_closed = false;
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => self.chat_mouse(mouse),
                Event::Paste(text) => {
                    self.paste_prompt(&text);
                    self.selected = 0;
                    self.menu_closed = false;
                }
                _ => {}
            }
        }
        Ok(StreamControl::Continue)
    }

    fn submit(&mut self, line: &str) -> String {
        let line = line.to_owned();
        if !line.starts_with('/') {
            self.submitted_pastes = std::mem::take(&mut self.pastes);
        }
        if self.submitted.last() != Some(&line) {
            self.submitted.push(line.clone());
        }
        self.editor.clear();
        self.nav = None;
        self.menu_closed = false;
        self.selected = 0;
        self.scroll = 0;
        self.armed = false;
        self.notice.clear();
        line
    }

    pub(super) fn take_pastes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.submitted_pastes)
    }

    pub(super) fn restore_prompt(&mut self, input: &str, pastes: Vec<String>) {
        self.editor = Editor::from(input);
        self.pastes = pastes;
    }

    fn paste_prompt(&mut self, text: &str) {
        if text.len() < 2048 && text.lines().take(8).count() < 8 {
            self.editor.paste(text);
            return;
        }
        if self.pastes.iter().map(String::len).sum::<usize>() + text.len() > PASTE_LIMIT {
            self.notice(tr!(
                "Вставка больше 192 КиБ. Раздели текст на части или приложи файл через @.",
                "The paste exceeds 192 KiB. Split the text or attach a file with @."
            ));
            return;
        }
        self.pastes.push(text.to_owned());
        self.notice.clear();
    }

    #[cfg(windows)]
    fn paste_clipboard(&mut self) {
        match selection::paste() {
            Ok(Some(text)) => {
                self.paste_prompt(&text);
                self.selection = None;
                self.selected = 0;
                self.menu_closed = false;
                self.armed = false;
                self.nav = None;
            }
            Ok(None) => self.notice(tr!(
                "В буфере обмена нет текста.",
                "There is no text on the clipboard."
            )),
            Err(error) => self.notice(&error.to_string()),
        }
    }

    pub(super) fn begin_turn(&mut self, started: Instant) {
        self.turn = Some(Turn {
            started,
            input: 0,
            output: 0,
            estimated: false,
            first: None,
            request_seconds: 0.0,
            writing: false,
        });
    }

    pub(super) fn track(&mut self, output: &str, reasoning: &str) {
        if let Some(turn) = &mut self.turn {
            if !output.is_empty() || !reasoning.is_empty() {
                turn.first.get_or_insert_with(Instant::now);
            }
            turn.writing = !output.is_empty();
        }
    }

    pub(super) fn record_reply(&mut self, input: u32, output: u32, estimated: bool, elapsed: f32) {
        if let Some(turn) = &mut self.turn {
            turn.input = turn.input.saturating_add(input);
            turn.output = turn.output.saturating_add(output);
            turn.estimated |= estimated;
            turn.request_seconds += elapsed;
            turn.writing = false;
        }
    }

    pub(super) fn finish_turn(&mut self) -> Option<Receipt> {
        let turn = self.turn.take()?;
        let duration = turn.started.elapsed().as_secs_f32();
        let speed = turn.output as f32 / turn.request_seconds.max(0.05);
        let ttft = turn
            .first
            .map(|first| first.duration_since(turn.started).as_secs_f32());
        self.stats.tin = self.stats.tin.saturating_add(turn.input);
        self.stats.tout = self.stats.tout.saturating_add(turn.output);
        self.stats.turns = self.stats.turns.saturating_add(1);
        let receipt = Receipt {
            input: turn.input,
            output: turn.output,
            estimated: turn.estimated,
            duration,
            speed,
            ttft,
        };
        self.stats.last = Some(receipt);
        Some(receipt)
    }

    fn menu(&self, directory: &Path) -> Vec<(String, &'static str)> {
        if self.menu_closed || self.nav.is_some() {
            return Vec::new();
        }
        let prefix = self.editor.completion_prefix();
        if let Some(query) = super::attachment_prefix(&prefix) {
            return super::attachment_choices(directory, query);
        }
        menu_items(&prefix, false, false)
            .into_iter()
            .map(|(name, description)| (name.to_owned(), description))
            .collect()
    }

    fn complete(&mut self, item: &str) -> Option<String> {
        let prefix = self.editor.completion_prefix();
        if item.starts_with('@') {
            if let Some(query) = super::attachment_prefix(&prefix) {
                self.editor.replace_tail(query.chars().count() + 1, item);
                self.menu_closed = !item.ends_with('/');
            }
            return None;
        }
        let token = prefix.split_whitespace().last().unwrap_or("");
        self.editor.replace_tail(token.chars().count(), "");
        self.menu_closed = true;
        Some(item.to_owned())
    }

    fn chat_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_add(5),
            MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_sub(5),
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection = self.viewport.as_ref().and_then(|view| view.begin(mouse));
            }
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => {
                if let (Some(view), Some(selection)) = (&self.viewport, &mut self.selection) {
                    selection.extend(view, mouse);
                    if matches!(mouse.kind, MouseEventKind::Drag(_)) {
                        if usize::from(mouse.row) <= view.top {
                            self.scroll = self.scroll.saturating_add(1);
                        } else if usize::from(mouse.row)
                            >= view.top + view.rows.len().saturating_sub(1)
                        {
                            self.scroll = self.scroll.saturating_sub(1);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn copy_selection(&mut self) -> bool {
        let (Some(view), Some(selection)) = (&self.viewport, &self.selection) else {
            return false;
        };
        let cached = self
            .cache
            .as_ref()
            .map_or(&[][..], |cache| cache.lines.as_slice());
        let live = view
            .rows
            .iter()
            .enumerate()
            .map(|(i, line)| (view.start + i, line))
            .filter(|(row, _)| *row >= cached.len());
        let Some(text) = selection.text(cached.iter().enumerate().chain(live)) else {
            return false;
        };
        match selection::copy(&text) {
            Ok(()) => self.notice(tr!("Скопировано.", "Copied.")),
            Err(error) => self.notice(&error.to_string()),
        }
        self.armed = false;
        true
    }

    fn draw(
        &mut self,
        history: &[Message],
        pending: Option<(&str, &str)>,
        model: &str,
        directory: &Path,
    ) -> Result<()> {
        let (terminal_width, terminal_height) = terminal::size()?;
        let width = usize::from(terminal_width);
        let height = usize::from(terminal_height);
        self.panel_frame = None;
        self.viewport = None;
        let mut out = Vec::new();
        queue!(
            out,
            BeginSynchronizedUpdate,
            MoveTo(0, 0),
            Clear(ClearType::All)
        )?;
        if width < 40 || height < 14 {
            put_line(
                &mut out,
                0,
                0,
                &Line::of(
                    tr!(
                        "Увеличь окно: нужно минимум 40×14",
                        "Enlarge the window: at least 40×14"
                    ),
                    Style::dim(),
                ),
                width,
            )?;
            queue!(out, EndSynchronizedUpdate)?;
            io::stdout().lock().write_all(&out)?;
            self.background = out;
            return Ok(());
        }
        let content = width.saturating_sub(4).clamp(40, MAX_CONTENT);
        let margin = (width - content) / 2;
        let show_mark = content >= 70 && height >= 26;
        let header_height = if show_mark { 10 } else { 5 };
        self.header(&mut out, margin, content, model, directory, show_mark)?;

        let streaming = pending.is_some();
        let notice = if self.notice.is_empty() && !self.pastes.is_empty() {
            super::paste_label(
                self.pastes
                    .iter()
                    .map(|text| text.lines().count().max(1))
                    .sum(),
            )
        } else {
            self.notice.clone()
        };
        let menu = self.menu(directory);
        let mut menu_rows = menu.len().clamp(1, MENU_LIMIT);
        menu_rows = menu_rows.min(height.saturating_sub(header_height + 7).max(1));
        let mut notice_rows = usize::from(!notice.is_empty());
        let mut gap_rows = 1usize;
        let (conv_rows, notice_y, box_top) = loop {
            let top = height.saturating_sub(3 + menu_rows);
            let gap = gap_rows.min(top.saturating_sub(header_height + 2));
            let extras_end = top.saturating_sub(gap);
            let notice = if notice_rows > 0 {
                Some(extras_end.saturating_sub(notice_rows))
            } else {
                None
            };
            let conv_end = notice.unwrap_or(extras_end);
            let rows = conv_end.saturating_sub(header_height);
            if rows >= 2 {
                break (rows, notice, top);
            }
            if notice_rows > 0 {
                notice_rows = 0;
                continue;
            }
            if gap_rows > 0 {
                gap_rows = 0;
                continue;
            }
            if menu_rows > 1 {
                menu_rows -= 1;
                continue;
            }
            break (rows, notice, top);
        };

        if conv_rows > 0 {
            let key = CacheKey {
                messages: history.len(),
                bytes: history.iter().map(|message| message.content.len()).sum(),
                reasoning_bytes: history
                    .iter()
                    .map(|message| {
                        message
                            .reasoning
                            .as_deref()
                            .map_or(0, |reasoning| reasoning.len())
                    })
                    .sum(),
                details: self.details,
                reasoning_details: self.reasoning_details,
                width: content,
            };
            if self.cache.as_ref().map(|cache| cache.key) != Some(key) {
                let lines = history_lines(history, content, self.details, self.reasoning_details);
                self.cache = Some(Cache { key, lines });
            }
            let mut pending_lines = Vec::new();
            if let Some((text, reasoning)) = pending {
                pending_lines.extend(reasoning_lines(reasoning, content, false, true));
                if !text.is_empty() {
                    pending_lines.extend(render::assistant(text, content));
                }
            }
            let total =
                self.cache.as_ref().map_or(0, |cache| cache.lines.len()) + pending_lines.len();
            let max_scroll = total.saturating_sub(conv_rows);
            let scroll = self.scroll.min(max_scroll);
            self.scroll = scroll;
            let end = total - scroll;
            let start = end.saturating_sub(conv_rows);
            if let Some(cache) = &self.cache {
                let viewport = Viewport {
                    top: header_height,
                    left: margin,
                    width: content,
                    start,
                    rows: cache
                        .lines
                        .iter()
                        .chain(pending_lines.iter())
                        .skip(start)
                        .take(conv_rows)
                        .cloned()
                        .collect(),
                };
                if self
                    .selection
                    .as_ref()
                    .is_some_and(|range| range.width() != content)
                {
                    self.selection = None;
                }
                for (row, line) in viewport.rows.iter().enumerate() {
                    if let Some(highlight) = self
                        .selection
                        .as_ref()
                        .and_then(|range| range.highlight(start + row, line))
                    {
                        put_line(&mut out, margin, header_height + row, &highlight, content)?;
                    } else {
                        put_line(&mut out, margin, header_height + row, line, content)?;
                    }
                }
                self.viewport = Some(viewport);
            }
            if start > 0 {
                let hint = Line::of(format!("↑ {start}"), Style::dim());
                let used = hint.width();
                put_line(
                    &mut out,
                    margin + content.saturating_sub(used),
                    header_height.saturating_sub(1),
                    &hint,
                    used,
                )?;
            }
        }

        if let Some(y) = notice_y {
            let mut line = Line::of(
                format!("  {notice}"),
                if self.notice.is_empty() {
                    Style::fg(render::WARN)
                } else {
                    Style::dim()
                },
            );
            if line.width() > content {
                line = line.truncate(content.saturating_sub(1));
                line.push("…", Style::dim());
            }
            put_line(&mut out, margin, y, &line, content)?;
        }
        let inner = content.saturating_sub(6);
        let (text, caret) = self.editor.view(inner);
        let border = Style::fg(render::LINE);
        let mut top = Line::new();
        top.push("╭", border);
        top.push("─".repeat(content.saturating_sub(2)), border);
        top.push("╮", border);
        put_line(&mut out, margin, box_top, &top, content)?;
        let mut row = Line::new();
        row.push("│ ", border);
        row.push("› ", Style::brand());
        row.push(text.clone(), Style::fg(render::FG));
        let text_width = text.as_str().width();
        if text_width < inner {
            row.push(" ".repeat(inner - text_width), Style::plain());
        }
        row.push(" │", border);
        put_line(&mut out, margin, box_top + 1, &row, content)?;
        let mut foot = Line::new();
        foot.push("╰", border);
        foot.push("─".repeat(content.saturating_sub(2)), border);
        foot.push("╯", border);
        put_line(&mut out, margin, box_top + 2, &foot, content)?;

        if menu.is_empty() {
            put_line(
                &mut out,
                margin,
                box_top + 3,
                &self.footer(model, content, streaming),
                content,
            )?;
        } else {
            let window = self.selected.saturating_sub(menu_rows.saturating_sub(1));
            for (index, (name, description)) in menu.iter().enumerate().skip(window).take(menu_rows)
            {
                let selected = index == self.selected;
                let mut line = Line::new();
                line.push("  ", Style::plain());
                line.push(
                    if selected { "› " } else { "  " },
                    if selected {
                        Style::brand()
                    } else {
                        Style::plain()
                    },
                );
                line.push(
                    format!("{name:<14}"),
                    if selected {
                        Style::brand()
                    } else {
                        Style::fg(render::FG)
                    },
                );
                line.push(
                    *description,
                    if selected {
                        Style::brand()
                    } else {
                        Style::dim()
                    },
                );
                put_line(
                    &mut out,
                    margin,
                    box_top + 3 + index - window,
                    &line,
                    content,
                )?;
            }
        }

        let column = margin + 4 + caret;
        queue!(out, MoveTo(column as u16, (box_top + 1) as u16), Show)?;
        queue!(out, EndSynchronizedUpdate)?;
        io::stdout().lock().write_all(&out)?;
        self.background = out;
        Ok(())
    }

    fn header(
        &self,
        out: &mut impl Write,
        margin: usize,
        content: usize,
        model: &str,
        directory: &Path,
        mark: bool,
    ) -> io::Result<()> {
        let path = render::tail(&crate::folders::display(directory), content);
        let hint = if self.desktop_active {
            tr!(
                "F8 · остановить управление окном",
                "F8 · stop window control"
            )
        } else {
            tr!(
                "/ команды · esc прервать · ctrl-c выйти",
                "/ commands · esc to interrupt · ctrl-c to quit"
            )
        };
        if !mark {
            put_line(out, margin, 0, &self.brand_line(), content)?;
            put_line(out, margin, 1, &self.model_line(model), content)?;
            put_line(out, margin, 2, &Line::of(path, Style::dim()), content)?;
            put_line(out, margin, 3, &Line::of(hint, Style::dim()), content)?;
            return Ok(());
        }
        let brand = self.brand_line();
        let model_line = self.model_line(model);
        let path_line = Line::of(
            render::tail(&path, content.saturating_sub(20)),
            Style::dim(),
        );
        let hint_line = Line::of(hint, Style::dim());
        let info = |row: usize| match row {
            2 => Some(&brand),
            3 => Some(&model_line),
            4 => Some(&path_line),
            6 => Some(&hint_line),
            _ => None,
        };
        for (row, mark_line) in render::logo().iter().enumerate() {
            put_line(out, margin, 1 + row, mark_line, 16)?;
            if let Some(line) = info(row) {
                put_line(out, margin + 18, 1 + row, line, content.saturating_sub(18))?;
            }
        }
        Ok(())
    }

    fn brand_line(&self) -> Line {
        let mut line = Line::new();
        line.push("Carry", Style::fg(render::FG).bold());
        line.push("Code", Style::fg(render::BRAND).bold());
        line.push(format!(" v{}", env!("CARGO_PKG_VERSION")), Style::dim());
        line
    }

    fn model_line(&self, model: &str) -> Line {
        let mut line = Line::new();
        line.push(model, Style::dim());
        line.push(
            format!(
                " · ctx {} · in {} · out {}",
                render::human(self.context_limit),
                render::human(self.input_limit),
                render::human(self.output_limit)
            ),
            Style::dim(),
        );
        if self.reasoning != super::Reasoning::Off {
            line.push(
                format!(" · r {}", self.reasoning.label()),
                Style::fg(render::BRAND),
            );
        }
        if self.diagnose {
            line.push(
                tr!(" · диагностика", " · diagnose"),
                Style::fg(render::BRAND),
            );
        }
        line
    }

    fn input_percentage(&self) -> Option<u64> {
        self.input_estimate
            .map(|used| u64::from(used) * 100 / u64::from(self.input_limit.max(1)))
    }

    fn footer(&self, model: &str, width: usize, streaming: bool) -> Line {
        if self.armed && !streaming {
            return self.status_line(model, width);
        }
        let over_limit = self
            .input_estimate
            .is_some_and(|used| used > self.input_limit);
        let indicator = render::context_indicator(self.input_percentage(), over_limit);
        let indicator_start = width.saturating_sub(indicator.width());
        let left_width = indicator_start.saturating_sub(2);
        let mut line = if streaming {
            self.spinner_line(left_width)
        } else {
            self.status_line(model, left_width)
        };
        line.pad_to(indicator_start, Style::plain());
        line.extend(indicator);
        line.truncate(width)
    }

    fn status_line(&self, model: &str, width: usize) -> Line {
        let model = if width < 60 {
            model.rsplit(" / ").next().unwrap_or(model)
        } else {
            model
        };
        if self.armed {
            let mut line = Line::new();
            line.push("  ", Style::plain());
            line.push(
                tr!(
                    "Нажми Ctrl-C ещё раз, чтобы выйти",
                    "Press Ctrl-C again to exit"
                ),
                Style::dim(),
            );
            return line.truncate(width);
        }
        let mut segments: Vec<Line> = Vec::new();
        let mut head = Line::new();
        head.push("  ", Style::plain());
        head.push("● ", Style::brand());
        head.push(model, Style::fg(render::FG));
        segments.push(head);
        if self.reasoning != super::Reasoning::Off {
            segments.push(Line::of(
                format!("r {}", self.reasoning.label()),
                Style::dim(),
            ));
        }
        if self.scroll > 0 {
            segments.push(Line::of(format!("↑{}", self.scroll), Style::dim()));
        }
        join_segments(&segments, width)
    }

    fn spinner_line(&self, width: usize) -> Line {
        let frame = render::SPIN[(self.tick / 2) % render::SPIN.len()];
        let Some(turn) = self.turn else {
            return Line::of("  Thinking…", Style::brand());
        };
        let elapsed = render::dur(turn.started.elapsed().as_secs());
        let stage = if turn.writing {
            tr!("Пишу", "Writing")
        } else {
            tr!("Думаю", "Thinking")
        };
        if width < 58 {
            let mut line = Line::new();
            line.push("  ", Style::plain());
            line.push(frame, Style::brand());
            line.push(format!(" {stage}… {elapsed} · Esc"), Style::dim());
            return line.truncate(width);
        }
        let mut line = Line::new();
        line.push("  ", Style::plain());
        line.push(frame, Style::brand());
        line.push(format!(" {stage}…"), Style::brand());
        line.push(
            trf!(
                " ({elapsed} · Esc — прервать)",
                " ({elapsed} · Esc to interrupt)"
            ),
            Style::dim(),
        );
        line
    }

    fn status_panel(&mut self, model: &str, directory: &Path) -> Result<()> {
        let used = self
            .input_estimate
            .map_or("—".into(), |used| format!("~{used}"));
        let remaining = self.input_estimate.map_or("—".into(), |used| {
            format!("~{}", self.input_limit.saturating_sub(used))
        });
        let mode = if self.diagnose {
            tr!("диагностика", "diagnose")
        } else {
            tr!("кодинг", "code")
        };
        let rows = [
            (tr!("модель", "model"), model.to_owned()),
            (tr!("окно", "window"), render::human(self.context_limit)),
            (
                tr!("лимит входа", "input limit"),
                self.input_limit.to_string(),
            ),
            (tr!("выход", "output"), render::human(self.output_limit)),
            (tr!("занято (оценка)", "used (estimate)"), used),
            (tr!("осталось", "remaining"), remaining),
            (
                tr!("reasoning", "reasoning"),
                if self.reasoning == super::Reasoning::Off {
                    tr!("выкл", "off").to_owned()
                } else {
                    self.reasoning.label().to_owned()
                },
            ),
            (
                tr!("токены", "tokens"),
                format!(
                    "↑ ~{}  ↓ ~{}",
                    render::human(self.stats.tin),
                    render::human(self.stats.tout)
                ),
            ),
            (
                tr!("скорость", "speed"),
                self.stats
                    .last
                    .map_or("—".into(), |reply| format!("{:.0} tok/s", reply.speed)),
            ),
            (
                tr!("последний", "last"),
                self.stats
                    .last
                    .map_or("—".into(), |reply| format!("{:.1}s", reply.duration)),
            ),
            (
                tr!("сеанс", "session"),
                trf!("{} запросов", "{} turns", self.stats.turns),
            ),
            (tr!("режим", "mode"), mode.to_owned()),
            (tr!("проект", "project"), crate::folders::display(directory)),
        ];
        let mut body = Vec::new();
        for (key, value) in rows {
            let mut line = Line::new();
            line.push(format!("{key:<16} "), Style::dim());
            line.push(value, Style::fg(render::FG));
            body.push(line);
        }
        self.overlay(tr!("Состояние сеанса", "Session status"), body)
    }

    fn overlay(&mut self, title: &str, body: Vec<Line>) -> Result<()> {
        let mut scroll = 0usize;
        loop {
            let panel = self.draw_panel(
                title,
                tr!("Esc / Enter — закрыть", "Esc / Enter — close"),
                &body,
                scroll,
                None,
            )?;
            scroll = scroll.min(body.len().saturating_sub(panel.visible));
            match read_ui_event()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let key = normalize_key(key);
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => return Ok(()),
                        KeyCode::Up => scroll = scroll.saturating_sub(1),
                        KeyCode::Down => scroll = scroll.saturating_add(1),
                        KeyCode::PageUp => scroll = scroll.saturating_sub(10),
                        KeyCode::PageDown => scroll = scroll.saturating_add(10),
                        KeyCode::Home => scroll = 0,
                        KeyCode::End => scroll = body.len(),
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => scroll = scroll.saturating_sub(3),
                    MouseEventKind::ScrollDown => scroll = scroll.saturating_add(3),
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

/// Border geometry of a centered panel, enough for cursor placement.
struct Panel {
    visible: usize,
}

struct PanelFrame {
    size: (u16, u16),
    left: usize,
    top: usize,
    width: usize,
    rows: Vec<Line>,
    cursor: Option<(usize, usize)>,
}

impl Screen {
    fn draw_panel(
        &mut self,
        title: &str,
        footer: &str,
        body: &[Line],
        scroll: usize,
        cursor: Option<(usize, usize)>,
    ) -> Result<Panel> {
        let (terminal_width, terminal_height) = terminal::size()?;
        let width = usize::from(terminal_width);
        let height = usize::from(terminal_height);
        let panel_width = width.saturating_sub(8).clamp(36, 96).min(width);
        let inner = panel_width.saturating_sub(4);
        let panel_height = (body.len() + 2).min(height.saturating_sub(2)).max(4);
        let visible = panel_height.saturating_sub(2);
        let scroll = scroll.min(body.len().saturating_sub(visible));
        let left = width.saturating_sub(panel_width) / 2;
        let top = if self.approval_card {
            height.saturating_sub(panel_height + 4)
        } else {
            height.saturating_sub(panel_height) / 2
        };
        let border = Style::fg(if self.approval_card {
            render::WARN
        } else {
            render::LINE
        });
        let edge = |left: &str, right: &str, text: &str, style: Style| {
            let mut line = Line::of(left, border);
            let limit = panel_width.saturating_sub(3);
            let mut label = Line::of(format!(" {text} "), style);
            if label.width() > limit && limit > 0 {
                label = label.truncate(limit - 1);
                label.push("…", style);
            }
            line.extend(label);
            let tail = panel_width.saturating_sub(line.width() + 1);
            if tail > 0 {
                line.push("─".repeat(tail), border);
            }
            line.push(right, border);
            line
        };
        let mut rows = Vec::with_capacity(visible + 2);
        let title_style = if self.approval_card {
            border.bold()
        } else {
            Style::brand().bold()
        };
        rows.push(edge("╭─", "╮", title, title_style));
        for row in 0..visible {
            let mut line = Line::of("│ ", border);
            if let Some(source) = body.get(scroll + row) {
                if source.width() > inner && inner > 1 {
                    line.extend(source.truncate(inner - 1));
                    line.push("…", Style::dim());
                } else {
                    line.extend(source.clone());
                }
            }
            line.pad_to(panel_width.saturating_sub(2), Style::plain());
            line.push(" │", border);
            rows.push(line);
        }
        rows.push(edge("╰─", "╯", footer.trim(), Style::dim()));
        let cursor = cursor.and_then(|(row, column)| {
            let row = row.checked_sub(scroll)?;
            (row < visible).then_some((
                left + 2 + column.min(inner.saturating_sub(1)),
                top + 1 + row,
            ))
        });
        let size = (terminal_width, terminal_height);
        let previous = self.panel_frame.as_ref().filter(|previous| {
            previous.size == size
                && previous.left == left
                && previous.top == top
                && previous.width == panel_width
                && previous.rows.len() == rows.len()
        });
        let changed = |row: usize| previous.is_none_or(|frame| frame.rows[row] != rows[row]);
        if (0..rows.len()).any(changed) || previous.is_none_or(|frame| frame.cursor != cursor) {
            let mut out = io::BufWriter::new(io::stdout().lock());
            queue!(out, BeginSynchronizedUpdate)?;
            if previous.is_none() {
                if self.background.is_empty()
                    || self
                        .panel_frame
                        .as_ref()
                        .is_some_and(|frame| frame.size != size)
                {
                    queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
                } else {
                    out.write_all(&self.background)?;
                    queue!(out, BeginSynchronizedUpdate)?;
                }
            }
            for (row, line) in rows.iter().enumerate().filter(|(row, _)| changed(*row)) {
                put_line(&mut out, left, top + row, line, panel_width)?;
            }
            if let Some((x, y)) = cursor {
                queue!(out, MoveTo(x as u16, y as u16), Show)?;
            } else {
                queue!(out, Hide)?;
            }
            queue!(out, EndSynchronizedUpdate)?;
            out.flush()?;
            self.panel_frame = Some(PanelFrame {
                size,
                left,
                top,
                width: panel_width,
                rows,
                cursor,
            });
        }
        Ok(Panel { visible })
    }

    /// Yes/no panel for destructive or privileged actions. Enter allows, Esc
    /// declines; the body is wrapped and capped so it can never push the hints
    /// off screen.
    pub fn confirm(&mut self, title: &str, body: &str) -> Result<bool> {
        let (terminal_width, _) = terminal::size()?;
        let width = (usize::from(terminal_width).saturating_sub(8).clamp(36, 96)).saturating_sub(4);
        let mut lines = Vec::new();
        for paragraph in body.lines() {
            if paragraph.trim().is_empty() {
                lines.push(Line::new());
                continue;
            }
            let mut line = Line::new();
            line.push(paragraph, Style::fg(render::FG));
            lines.extend(render::wrap(&line, width));
        }
        if lines.len() > 10 {
            lines.truncate(9);
            let mut line = Line::new();
            line.push("…", Style::dim());
            lines.push(line);
        }
        loop {
            self.draw_panel(
                title,
                tr!(" Enter — да · Esc — нет ", " Enter — yes · Esc — no "),
                &lines,
                0,
                None,
            )?;
            match read_ui_event()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let key = normalize_key(key);
                    match key.code {
                        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('д') => {
                            return Ok(true);
                        }
                        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('н') => {
                            return Ok(false);
                        }
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            return Ok(false);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Protocol,
    Url,
    Model,
    Key,
    Context,
    Input,
    Output,
    Reasoning,
    ReasoningApi,
    ChatOnly,
    Vision,
}

impl Field {
    const ALL: [Self; 12] = [
        Self::Name,
        Self::Protocol,
        Self::Url,
        Self::Model,
        Self::Key,
        Self::Context,
        Self::Input,
        Self::Output,
        Self::Reasoning,
        Self::ReasoningApi,
        Self::ChatOnly,
        Self::Vision,
    ];
}

struct ModelForm {
    index: Option<usize>,
    name: Editor,
    url: Editor,
    model: Editor,
    key: Editor,
    context: Editor,
    input: Editor,
    output: Editor,
    anthropic: bool,
    responses: bool,
    vision: bool,
    chat_only: bool,
    key_url: Option<String>,
    key_source: Option<usize>,
    reasoning: super::Reasoning,
    reasoning_api: super::ReasoningApi,
    focus: usize,
    error: Option<String>,
}

impl ModelForm {
    fn new(index: Option<usize>, entry: Option<&ModelEntry>) -> Self {
        let field = |value: &str| Editor::from(value);
        let limit = |value: Option<u32>| match value.filter(|value| *value != 0) {
            Some(value) => Editor::from(&value.to_string()),
            None => Editor::new(),
        };
        Self {
            index,
            name: field(entry.map(|entry| entry.name.as_str()).unwrap_or("")),
            url: field(entry.map(|entry| entry.url.as_str()).unwrap_or("")),
            model: field(entry.map(|entry| entry.model.as_str()).unwrap_or("")),
            key: field(entry.and_then(|entry| entry.key.as_deref()).unwrap_or("")),
            context: limit(entry.map(|entry| entry.context)),
            input: limit(entry.map(|entry| entry.input)),
            output: limit(entry.map(|entry| entry.output)),
            anthropic: entry.map(|entry| entry.anthropic).unwrap_or(false),
            responses: entry.is_some_and(|entry| entry.responses),
            vision: entry.is_some_and(|entry| entry.vision),
            chat_only: entry.map(|entry| entry.chat_only).unwrap_or(false),
            key_url: entry
                .filter(|entry| entry.key_set)
                .map(|entry| entry.url.clone()),
            key_source: entry.and_then(|entry| entry.key_source),
            reasoning: entry
                .map(|entry| entry.reasoning)
                .unwrap_or(super::Reasoning::Off),
            reasoning_api: entry
                .map(|entry| entry.reasoning_api)
                .unwrap_or(super::ReasoningApi::Auto),
            focus: 0,
            error: None,
        }
    }

    fn saved_key_matches(&self) -> bool {
        self.key_url
            .as_deref()
            .is_some_and(|url| self.url.text().trim().trim_end_matches('/') == url)
    }

    fn cycle_protocol(&mut self) {
        (self.anthropic, self.responses) = match (self.anthropic, self.responses) {
            (false, false) => (true, false),
            (true, _) => (false, true),
            _ => (false, false),
        };
    }

    fn field(&self) -> Field {
        Field::ALL[self.focus.min(Field::ALL.len() - 1)]
    }

    fn focus_row(&self) -> usize {
        match self.field() {
            Field::Name => 1,
            Field::Protocol => 2,
            Field::Url => 3,
            Field::Model => 4,
            Field::Key => 5,
            Field::Context => 6,
            Field::Input => 7,
            Field::Output => 8,
            Field::Reasoning => 9,
            Field::ReasoningApi => 10,
            Field::ChatOnly => 11,
            Field::Vision => 12,
        }
    }

    fn editor_mut(&mut self) -> Option<&mut Editor> {
        match self.field() {
            Field::Name => Some(&mut self.name),
            Field::Url => Some(&mut self.url),
            Field::Model => Some(&mut self.model),
            Field::Key => Some(&mut self.key),
            Field::Context => Some(&mut self.context),
            Field::Input => Some(&mut self.input),
            Field::Output => Some(&mut self.output),
            Field::Protocol
            | Field::Reasoning
            | Field::ReasoningApi
            | Field::ChatOnly
            | Field::Vision => None,
        }
    }

    fn move_focus(&mut self, delta: isize) {
        let count = Field::ALL.len() as isize;
        self.focus = ((self.focus as isize + delta).rem_euclid(count)) as usize;
    }

    fn entry(&self) -> std::result::Result<ModelEntry, String> {
        self.build_entry(false)
    }
    fn build_entry(&self, allow_empty_model: bool) -> std::result::Result<ModelEntry, String> {
        let name = self.name.text().trim().to_owned();
        if name.is_empty() && !allow_empty_model {
            return Err(
                tr!("Название не может быть пустым", "The name cannot be empty").to_owned(),
            );
        }
        if name.chars().count() > 64 {
            return Err(tr!(
                "Название длиннее 64 символов",
                "The name is longer than 64 characters"
            )
            .to_owned());
        }
        let url = super::endpoint(self.url.text().trim()).map_err(|error| error.to_string())?;
        let model = self.model.text().trim().to_owned();
        if model.is_empty() && !allow_empty_model {
            return Err(tr!("Укажи ID модели", "Enter the model ID").to_owned());
        }
        if model.chars().count() > 160 {
            return Err(tr!(
                "ID модели длиннее 160 символов",
                "The model ID is longer than 160 characters"
            )
            .to_owned());
        }
        let limit = |editor: &Editor, name: &str, minimum: u32| {
            if editor.is_empty() {
                return Ok(0);
            }
            editor.text().trim().parse::<u32>()
                .ok()
                .filter(|value| (minimum..=10_000_000).contains(value))
                .ok_or_else(|| trf!(
                    "{}: укажи целое число от {} до 10 000 000 или оставь поле пустым для авто.",
                    "{}: enter an integer from {} to 10,000,000 or leave blank for auto.",
                    name, minimum,
                ))
        };
        let context = limit(&self.context, tr!("Контекст", "Context"), 1256)?;
        let input = limit(&self.input, tr!("Вход", "Input"), 1000)?;
        let output = limit(&self.output, tr!("Выход", "Output"), 256)?;
        if context > 0 && input.saturating_add(output) > context {
            return Err(tr!(
                "Вход + выход больше окна контекста",
                "Input + output exceed the context window"
            )
            .to_owned());
        }
        if self.anthropic
            && self.reasoning != super::Reasoning::Off
            && !matches!(
                self.reasoning_api,
                super::ReasoningApi::None
                    | super::ReasoningApi::Adaptive
                    | super::ReasoningApi::DeepSeek
            )
            && let Some(budget) = self.reasoning.budget()
            && output > 0
            && output <= budget
        {
            return Err(trf!(
                "Выход {} должен быть больше бюджета reasoning {}",
                "Output {} must be greater than the reasoning budget {}",
                output,
                budget
            ));
        }
        let key = self.key.text();
        Ok(ModelEntry {
            name,
            url,
            model,
            key: (!key.is_empty()).then_some(key),
            key_set: self.saved_key_matches() || !self.key.is_empty(),
            key_source: self.key_source,
            anthropic: self.anthropic,
            responses: self.responses,
            vision: self.vision,
            chat_only: self.chat_only,
            context,
            input,
            output,
            reasoning: self.reasoning,
            reasoning_api: self.reasoning_api,
        })
    }

    fn lines(&self, inner: usize) -> (Vec<Line>, Option<(usize, usize)>) {
        let label_width = 12usize;
        let value_width = inner.saturating_sub(label_width + 2).max(8);
        let focused = self.field();
        let mut lines = Vec::new();
        let mut cursor = None;
        let label = |field: Field, text: &str| {
            let style = if field == focused {
                Style::brand().bold()
            } else {
                Style::dim()
            };
            let mut line = Line::new();
            line.push(format!("{text:<label_width$}"), style);
            line
        };
        let push_text = |lines: &mut Vec<Line>,
                         field: Field,
                         text: &str,
                         placeholder: &str,
                         value: &Editor|
         -> Option<(usize, usize)> {
            let mut line = label(field, text);
            let mut position = None;
            if field == focused {
                let (shown, caret) = value.view(value_width);
                if shown.is_empty() && !placeholder.is_empty() {
                    line.push(placeholder.to_owned(), Style::dim());
                } else {
                    line.push(shown, Style::fg(render::FG).bold());
                }
                position = Some((lines.len(), label_width + caret));
            } else if value.is_empty() && !placeholder.is_empty() {
                line.push(placeholder.to_owned(), Style::dim());
            } else {
                line.push(value.text(), Style::fg(render::FG));
            }
            lines.push(line.truncate(inner));
            position
        };
        lines.push(Line::new());
        if let Some(position) = push_text(
            &mut lines,
            Field::Name,
            tr!("Название", "Name"),
            tr!("например, OpenAI", "for example OpenAI"),
            &self.name,
        ) {
            cursor = Some(position);
        }
        let mut protocol = label(Field::Protocol, tr!("Протокол", "Protocol"));
        let name = if self.anthropic {
            "Anthropic Messages"
        } else if self.responses {
            "OpenAI Responses"
        } else {
            tr!("OpenAI-совместимый", "OpenAI-compatible")
        };
        let style = if focused == Field::Protocol {
            Style::brand().bold()
        } else {
            Style::fg(render::FG)
        };
        protocol.push(format!("‹ {name} ›"), style);
        if focused == Field::Protocol {
            protocol.push(tr!("   ←/→ — сменить", "   ←/→ to change"), Style::dim());
        }
        lines.push(protocol.truncate(inner));
        if let Some(position) = push_text(
            &mut lines,
            Field::Url,
            tr!("Адрес", "Base URL"),
            "https://api.example.com/v1",
            &self.url,
        ) {
            cursor = Some(position);
        }
        if let Some(position) = push_text(
            &mut lines,
            Field::Model,
            tr!("Модель", "Model"),
            tr!("ID модели провайдера", "provider model ID"),
            &self.model,
        ) {
            cursor = Some(position);
        }
        let mut key = label(Field::Key, tr!("API-ключ", "API key"));
        if focused == Field::Key {
            let (shown, caret) = self.key.view(value_width);
            if shown.is_empty() {
                if self.saved_key_matches() {
                    key.push(
                        tr!(
                            "(сохранён — введи новый, чтобы заменить)",
                            "(saved — type a new one to replace)"
                        ),
                        Style::dim(),
                    );
                } else {
                    key.push(
                        tr!(
                            "(пусто для локального сервера)",
                            "(empty for a local server)"
                        ),
                        Style::dim(),
                    );
                }
            } else {
                key.push(
                    "•".repeat(shown.chars().count()),
                    Style::fg(render::FG).bold(),
                );
            }
            cursor = Some((lines.len(), label_width + caret));
        } else if self.saved_key_matches() {
            key.push(tr!("сохранён ••••", "saved ••••"), Style::dim());
        } else if self.key.is_empty() {
            key.push(tr!("не задан", "not set"), Style::dim());
        } else {
            key.push("•".repeat(self.key.text().chars().count()), Style::dim());
        }
        lines.push(key.truncate(inner));
        if let Some(position) = push_text(
            &mut lines,
            Field::Context,
            tr!("Контекст", "Context"),
            "128000",
            &self.context,
        ) {
            cursor = Some(position);
        }
        if let Some(position) = push_text(
            &mut lines,
            Field::Input,
            tr!("Вход", "Input"),
            tr!("лимит входа, токены", "input limit, tokens"),
            &self.input,
        ) {
            cursor = Some(position);
        }
        if let Some(position) = push_text(
            &mut lines,
            Field::Output,
            tr!("Выход", "Output"),
            tr!("авто · лимит ответа", "auto · reply limit"),
            &self.output,
        ) {
            cursor = Some(position);
        }
        let mut reasoning = label(Field::Reasoning, tr!("Reasoning", "Reasoning"));
        let style = if focused == Field::Reasoning {
            Style::brand().bold()
        } else {
            Style::fg(render::FG)
        };
        reasoning.push(format!("‹ {} ›", self.reasoning.label()), style);
        if focused == Field::Reasoning {
            reasoning.push(tr!("   ←/→ — сменить", "   ←/→ to change"), Style::dim());
        }
        lines.push(reasoning.truncate(inner));
        let mut api = label(Field::ReasoningApi, tr!("Параметр", "Reasoning API"));
        let style = if focused == Field::ReasoningApi {
            Style::brand().bold()
        } else {
            Style::fg(render::FG)
        };
        api.push(format!("‹ {} ›", self.reasoning_api.label()), style);
        if focused == Field::ReasoningApi {
            api.push(tr!("   ←/→ — сменить", "   ←/→ to change"), Style::dim());
        }
        lines.push(api.truncate(inner));
        let mut chat = label(Field::ChatOnly, tr!("Режим", "Mode"));
        let mark = if self.chat_only { "[ ]" } else { "[x]" };
        chat.push(
            format!(
                "{mark} {}",
                if self.chat_only {
                    tr!("только чат, без инструментов", "chat only, no tools")
                } else {
                    tr!("агент: файлы и команды", "agent: files and commands")
                }
            ),
            if focused == Field::ChatOnly {
                Style::brand().bold()
            } else {
                Style::fg(render::FG)
            },
        );
        if focused == Field::ChatOnly {
            chat.push("   Space", Style::dim());
        }
        lines.push(chat.truncate(inner));
        let mut vision = label(Field::Vision, tr!("Изображения", "Images"));
        vision.push(
            if self.vision {
                tr!("вкл", "on")
            } else {
                tr!("выкл", "off")
            },
            Style::fg(render::FG),
        );
        if focused == Field::Vision {
            vision.push(
                tr!("  · Пробел — сменить", "  · Space to toggle"),
                Style::dim(),
            );
        }
        lines.push(vision.truncate(inner));
        lines.push(Line::new());
        if let Some(error) = &self.error {
            lines.push(Line::of(format!("! {error}"), Style::fg(render::WARN)).truncate(inner));
        } else {
            lines.push(
                Line::of(
                    tr!(
                        "Проверь адрес и модель: агент обратится к ним сразу.",
                        "Check the URL and model: the agent uses them right away."
                    ),
                    Style::dim(),
                )
                .truncate(inner),
            );
        }
        (lines, cursor)
    }
}

fn model_lines(
    entries: &[ModelEntry],
    selected: usize,
    active: Option<usize>,
    error: Option<&str>,
) -> Vec<Line> {
    let mut lines = Vec::new();
    if entries.is_empty() {
        lines.push(Line::of(
            tr!("  Моделей нет.", "  No models yet."),
            Style::fg(render::FG),
        ));
        lines.push(Line::of(
            tr!(
                "  A — добавить модель или открыть каталог F4 в форме.",
                "  A adds a model; open the catalog with F4 in the form."
            ),
            Style::dim(),
        ));
        return lines;
    }
    lines.push(Line::of(
        tr!(
            "  A  Добавить · N  На том же API · F4  Каталог",
            "  A  Add model · N  Same connection · F4  Catalog"
        ),
        Style::brand(),
    ));
    for (index, entry) in entries.iter().enumerate() {
        let is_selected = index == selected;
        let mut line = Line::new();
        line.push("  ", Style::plain());
        line.push(
            if is_selected { "›" } else { " " },
            if is_selected {
                Style::brand().bold()
            } else {
                Style::dim()
            },
        );
        line.push(
            if Some(index) == active { "●" } else { " " },
            if Some(index) == active {
                Style::brand()
            } else {
                Style::dim()
            },
        );
        line.push(" ", Style::plain());
        line.push(
            entry.name.clone(),
            if is_selected {
                Style::brand().bold()
            } else {
                Style::fg(render::FG)
            },
        );
        line.push("  ", Style::plain());
        line.push(entry.model.clone(), Style::dim());
        lines.push(line);
        let mut detail = Line::new();
        detail.push("      ", Style::plain());
        detail.push(entry.url.clone(), Style::dim());
        detail.push(" · ", Style::dim());
        detail.push(
            if entry.anthropic {
                "Anthropic"
            } else {
                "OpenAI"
            },
            Style::dim(),
        );
        detail.push(
            format!(" · ctx {}", render::human(entry.context)),
            Style::dim(),
        );
        detail.push(
            format!(" · in {}", render::human(entry.input)),
            Style::dim(),
        );
        detail.push(
            format!(" · out {}", render::human(entry.output)),
            Style::dim(),
        );
        if entry.reasoning != super::Reasoning::Off {
            detail.push(format!(" · r {}", entry.reasoning.label()), Style::dim());
        }
        if entry.chat_only {
            detail.push(tr!(" · только чат", " · chat only"), Style::dim());
        }
        if !entry.key_set {
            detail.push(tr!(" · без ключа", " · no key"), Style::dim());
        }
        lines.push(detail);
        lines.push(Line::new());
    }
    if let Some(error) = error {
        lines.push(Line::of(format!("! {error}"), Style::fg(render::WARN)));
    }
    lines
}

impl Screen {
    /// Full-screen "please wait" line used before slow MCP startup.
    pub fn busy(&mut self, text: &str) -> Result<()> {
        self.draw_panel(text, "", &[Line::new()], 0, None)?;
        Ok(())
    }

    /// `/reasoning`: pick a level for the active model.
    pub fn reasoning(&mut self, current: super::Reasoning) -> Result<Option<super::Reasoning>> {
        let levels = [
            super::Reasoning::Off,
            super::Reasoning::Low,
            super::Reasoning::Medium,
            super::Reasoning::High,
            super::Reasoning::Xhigh,
            super::Reasoning::Max,
            super::Reasoning::Ultra,
        ];
        let mut selected = levels
            .iter()
            .position(|level| *level == current)
            .unwrap_or(0);
        loop {
            let mut body = vec![Line::new()];
            for (index, level) in levels.iter().enumerate() {
                let is_selected = index == selected;
                let mut row = Line::new();
                row.push("  ", Style::plain());
                row.push(
                    if is_selected { "›" } else { " " },
                    if is_selected {
                        Style::brand().bold()
                    } else {
                        Style::dim()
                    },
                );
                row.push(if *level == current { " ●" } else { "  " }, Style::brand());
                row.push(" ", Style::plain());
                row.push(
                    level.label(),
                    if is_selected {
                        Style::brand().bold()
                    } else {
                        Style::fg(render::FG)
                    },
                );
                if let Some(budget) = level.budget() {
                    row.push(format!("  · budget {budget}"), Style::dim());
                } else {
                    if *level == super::Reasoning::Off {
                        row.push(
                            tr!("  · по умолчанию провайдера", "  · provider default"),
                            Style::dim(),
                        );
                    }
                }
                body.push(row);
            }
            body.push(Line::new());
            body.push(Line::of(
                tr!(
                    "Уровень хранится в профиле для этой модели. F3 — раскрыть reasoning в истории.",
                    "The level is saved in the profile for this model. F3 expands reasoning in history."
                ),
                Style::dim(),
            ));
            self.draw_panel(
                tr!("Reasoning", "Reasoning"),
                tr!(
                    " ↑↓ — выбор · Enter — применить · Esc — отмена ",
                    " ↑↓ — select · Enter — apply · Esc — cancel "
                ),
                &body,
                0,
                None,
            )?;
            match read_ui_event()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let key = normalize_key(key);
                    match key.code {
                        KeyCode::Up => selected = selected.saturating_sub(1),
                        KeyCode::Down => selected = (selected + 1).min(levels.len() - 1),
                        KeyCode::Home => selected = 0,
                        KeyCode::End => selected = levels.len() - 1,
                        KeyCode::Enter => return Ok(Some(levels[selected])),
                        KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            return Ok(None);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// The `/model` screen: list, connect, add, edit and delete saved model
    /// connections without leaving the alternate screen.
    pub fn models(
        &mut self,
        entries: &[ModelEntry],
        active: Option<usize>,
        draft: Option<(Option<usize>, ModelEntry, Option<String>)>,
    ) -> Result<ModelAction> {
        let mut selected = active.unwrap_or(0).min(entries.len().saturating_sub(1));
        let mut form = draft.map(|(index, entry, error)| {
            let mut form = ModelForm::new(index, Some(&entry));
            if error.is_some() {
                form.focus = [entry.context, entry.input, entry.output]
                    .iter()
                    .position(|value| *value == 0)
                    .map_or(5, |index| 5 + index);
                form.error = error;
            }
            form
        });
        let mut error: Option<String> = None;
        let mut form_scroll = 0usize;
        loop {
            let term_width = usize::from(terminal::size()?.0);
            let (title, footer, body, cursor) = if let Some(form) = &form {
                let width = terminal::size()?.0;
                let inner = usize::from(width)
                    .saturating_sub(8)
                    .clamp(36, 96)
                    .min(usize::from(width))
                    .saturating_sub(4);
                let (lines, cursor) = form.lines(inner);
                (
                    if form.index.is_some() {
                        tr!("Изменить модель", "Edit model")
                    } else {
                        tr!("Новая модель", "New model")
                    },
                    if term_width >= 72 {
                        tr!(
                            " Tab/↑↓ — поле · Space/←→ — тумблер · F4 каталог · F5 авто · Enter сохранить · Esc ",
                            " Tab/↑↓ — field · Space/←→ — toggle · F4 catalog · F5 auto · Enter save · Esc "
                        )
                    } else {
                        tr!(
                            " Tab · F4 · F5 · Enter · Esc ",
                            " Tab · F4 · F5 · Enter · Esc "
                        )
                    },
                    lines,
                    cursor,
                )
            } else {
                (
                    tr!("Модели", "Models"),
                    if term_width >= 72 {
                        tr!(
                            " E изменить · D удалить · Enter выбрать · A новая · N на том же API · T тест · F5 авто ",
                            " E edit · D delete · Enter select · A add · N same API · T test · F5 setup "
                        )
                    } else {
                        tr!(
                            " A добавить · N ещё · E правка ",
                            " A add · N another · E edit "
                        )
                    },
                    model_lines(entries, selected, active, error.as_deref()),
                    None,
                )
            };
            let focus_row = form.as_ref().map(ModelForm::focus_row);
            if let Some(row) = focus_row {
                let visible = usize::from(terminal::size()?.1)
                    .saturating_sub(4)
                    .min(body.len());
                if row < form_scroll {
                    form_scroll = row;
                } else if row >= form_scroll.saturating_add(visible) {
                    form_scroll = row.saturating_add(1).saturating_sub(visible);
                }
                form_scroll = form_scroll.min(body.len().saturating_sub(visible));
            } else {
                let visible = usize::from(terminal::size()?.1).saturating_sub(4);
                let list_top = if selected == 0 { 0 } else { 1 + selected * 3 };
                if list_top < form_scroll {
                    form_scroll = list_top;
                } else if list_top + 1 >= form_scroll.saturating_add(visible) {
                    form_scroll = (list_top + 2).saturating_sub(visible);
                }
                form_scroll = form_scroll.min(body.len().saturating_sub(visible));
            }
            self.draw_panel(title, footer, &body, form_scroll, cursor)?;
            match read_ui_event()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let key = normalize_key(key);
                    let mut cancel = false;
                    if let Some(form) = form.as_mut() {
                        match key.code {
                            KeyCode::Esc => {
                                form.error = None;
                                cancel = true;
                            }
                            KeyCode::Tab | KeyCode::Down => form.move_focus(1),
                            KeyCode::BackTab | KeyCode::Up => form.move_focus(-1),
                            KeyCode::F(4) => match form.build_entry(true) {
                                Ok(entry) => {
                                    return Ok(ModelAction::Catalog {
                                        index: form.index,
                                        entry,
                                    });
                                }
                                Err(message) => form.error = Some(message),
                            },
                            KeyCode::F(5) => match form.entry() {
                                Ok(entry) => {
                                    return Ok(ModelAction::Auto {
                                        index: form.index,
                                        entry,
                                    });
                                }
                                Err(message) => form.error = Some(message),
                            },
                            KeyCode::Enter => match form.entry() {
                                Ok(entry) if entry.missing_limits() => {
                                    return Ok(ModelAction::Auto {
                                        index: form.index,
                                        entry,
                                    });
                                }
                                Ok(entry) => {
                                    return Ok(ModelAction::Save {
                                        index: form.index,
                                        entry,
                                    });
                                }
                                Err(message) => form.error = Some(message),
                            },
                            KeyCode::Left => match form.field() {
                                Field::Protocol => form.cycle_protocol(),
                                Field::Reasoning => form.reasoning = form.reasoning.next(),
                                Field::ReasoningApi => {
                                    form.reasoning_api = form.reasoning_api.next()
                                }
                                _ => {
                                    if let Some(editor) = form.editor_mut() {
                                        editor.left();
                                    }
                                }
                            },
                            KeyCode::Right => match form.field() {
                                Field::Protocol => form.cycle_protocol(),
                                Field::Reasoning => form.reasoning = form.reasoning.next(),
                                Field::ReasoningApi => {
                                    form.reasoning_api = form.reasoning_api.next()
                                }
                                _ => {
                                    if let Some(editor) = form.editor_mut() {
                                        editor.right();
                                    }
                                }
                            },
                            KeyCode::Char(' ') if form.field() == Field::Vision => {
                                form.vision = !form.vision
                            }
                            KeyCode::Char(' ') if form.field() == Field::ChatOnly => {
                                form.chat_only = !form.chat_only;
                            }
                            KeyCode::Backspace => {
                                if let Some(editor) = form.editor_mut() {
                                    editor.backspace();
                                }
                            }
                            KeyCode::Delete => {
                                if let Some(editor) = form.editor_mut() {
                                    editor.delete();
                                }
                            }
                            KeyCode::Home => {
                                if let Some(editor) = form.editor_mut() {
                                    editor.home();
                                }
                            }
                            KeyCode::End => {
                                if let Some(editor) = form.editor_mut() {
                                    editor.end();
                                }
                            }
                            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                                cancel = true;
                            }
                            KeyCode::Char(character) if key.modifiers == KeyModifiers::CONTROL => {
                                if let Some(editor) = form.editor_mut() {
                                    match character {
                                        'u' => editor.kill_to_start(),
                                        'k' => editor.kill_to_end(),
                                        'w' => editor.kill_word(),
                                        'a' => editor.home(),
                                        'e' => editor.end(),
                                        _ => {}
                                    }
                                }
                            }
                            KeyCode::Char(character)
                                if key.modifiers == KeyModifiers::NONE
                                    || key.modifiers == KeyModifiers::SHIFT =>
                            {
                                let field = form.field();
                                if matches!(field, Field::Context | Field::Input | Field::Output)
                                    && !character.is_ascii_digit()
                                {
                                    continue;
                                }
                                if let Some(editor) = form.editor_mut() {
                                    editor.insert([character]);
                                }
                            }
                            _ => {}
                        }
                    }
                    if cancel {
                        form = None;
                    } else if form.is_none() {
                        match key.code {
                            KeyCode::Up => selected = selected.saturating_sub(1),
                            KeyCode::Down => {
                                selected = (selected + 1).min(entries.len().saturating_sub(1))
                            }
                            KeyCode::Home => selected = 0,
                            KeyCode::End => {
                                selected = entries.len().saturating_sub(1);
                            }
                            KeyCode::Enter if !entries.is_empty() => {
                                return Ok(ModelAction::Activate(selected));
                            }
                            KeyCode::F(5) if !entries.is_empty() => {
                                return Ok(ModelAction::Auto {
                                    index: Some(selected),
                                    entry: entries[selected].clone(),
                                });
                            }
                            KeyCode::F(4) if !entries.is_empty() => {
                                return Ok(ModelAction::Duplicate(selected, true));
                            }
                            KeyCode::Char('t') if !entries.is_empty() => {
                                return Ok(ModelAction::Test(selected));
                            }
                            KeyCode::Char('n') | KeyCode::Char('т') if !entries.is_empty() => {
                                return Ok(ModelAction::Duplicate(selected, false));
                            }
                            KeyCode::Char('a') | KeyCode::Char('а') | KeyCode::Char('ф') => {
                                error = None;
                                form = Some(ModelForm::new(None, None));
                            }
                            KeyCode::Char('e') | KeyCode::Char('е') | KeyCode::Char('у')
                                if !entries.is_empty() =>
                            {
                                error = None;
                                form = Some(ModelForm::new(Some(selected), entries.get(selected)));
                            }
                            KeyCode::Char('d') | KeyCode::Char('д') | KeyCode::Char('в')
                                if !entries.is_empty() =>
                            {
                                if entries.len() == 1 {
                                    error = Some(
                                        tr!(
                                            "Нельзя удалить единственную модель",
                                            "The only model cannot be deleted"
                                        )
                                        .to_owned(),
                                    );
                                } else {
                                    let entry = &entries[selected];
                                    if self.confirm(
                                        tr!("Удалить модель?", "Delete model?"),
                                        &format!(
                                            "{} · {}\n{}\n\n{}",
                                            entry.name,
                                            entry.model,
                                            entry.url,
                                            tr!(
                                                "Ключ исчезнет из профиля. Отменить нельзя.",
                                                "The key will be removed from the profile. This cannot be undone."
                                            )
                                        ),
                                    )? {
                                        return Ok(ModelAction::Delete(selected));
                                    }
                                }
                            }
                            KeyCode::Esc | KeyCode::Char('q') => return Ok(ModelAction::Close),
                            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                                return Ok(ModelAction::Close);
                            }
                            _ => {}
                        }
                    }
                }
                Event::Paste(text) => {
                    if let Some(form) = form.as_mut() {
                        let filtered: String = text
                            .chars()
                            .filter(|character| !character.is_control())
                            .collect();
                        let filtered: String = if matches!(
                            form.field(),
                            Field::Context | Field::Input | Field::Output
                        ) {
                            filtered
                                .chars()
                                .filter(|character| character.is_ascii_digit())
                                .collect()
                        } else {
                            filtered
                        };
                        if let Some(editor) = form.editor_mut() {
                            editor.insert(filtered.chars());
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

pub enum McpAction {
    Close,
    Restart,
    Import(Vec<usize>),
}

impl Screen {
    /// Scrollable preview used by `/init`: Enter accepts, Esc declines.
    pub fn stage_input(&mut self, input: String) {
        self.queued = input;
        self.submit_queued = true;
    }

    pub fn ask_text(&mut self, title: &str, secret: bool) -> Result<Option<String>> {
        let mut input = String::new();
        loop {
            let mut line = Line::new();
            let shown = if secret {
                "•".repeat(input.chars().count())
            } else {
                input.clone()
            };
            let shown_width = shown.width();
            line.push(shown, Style::fg(render::FG));
            self.draw_panel(
                title,
                tr!(
                    " Enter — принять · Esc — назад ",
                    " Enter accept · Esc back "
                ),
                &[line],
                0,
                Some((0, shown_width)),
            )?;
            match read_ui_event()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Enter if !input.trim().is_empty() => return Ok(Some(input)),
                    KeyCode::Backspace => {
                        input.pop();
                    }
                    KeyCode::Char(c)
                        if !key.modifiers.contains(KeyModifiers::CONTROL)
                            && !c.is_control()
                            && input.len() < 2048 =>
                    {
                        input.push(c)
                    }
                    _ => {}
                },
                Event::Paste(text) => {
                    input.extend(
                        text.chars()
                            .filter(|c| !c.is_control())
                            .take(2048usize.saturating_sub(input.len())),
                    );
                }
                _ => {}
            }
        }
    }

    pub fn choose(&mut self, title: &str, options: &[String]) -> Result<Option<usize>> {
        if options.is_empty() {
            return Ok(None);
        }
        let mut selected = options
            .iter()
            .position(|option| option.starts_with('●'))
            .unwrap_or(0);
        loop {
            let lines: Vec<_> = options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let mut row = Line::new();
                    if index == selected {
                        row.push(" › ", Style::brand().bold());
                    } else {
                        row.push("   ", Style::plain());
                    }
                    row.push(option, Style::fg(render::FG));
                    row
                })
                .collect();
            let visible = usize::from(terminal::size()?.1).saturating_sub(5).max(1);
            let scroll = selected.saturating_add(1).saturating_sub(visible);
            self.draw_panel(
                title,
                tr!(
                    " ↑↓ — выбор · Enter — открыть · Esc — назад ",
                    " ↑↓ select · Enter open · Esc back "
                ),
                &lines,
                scroll,
                None,
            )?;
            if let Event::Key(key) = read_ui_event()?
                && key.kind != KeyEventKind::Release
            {
                match key.code {
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected = (selected + 1).min(options.len() - 1),
                    KeyCode::Enter => return Ok(Some(selected)),
                    KeyCode::Esc => return Ok(None),
                    _ => {}
                }
            }
        }
    }

    pub fn tool_background(
        &mut self,
        history: &[Message],
        model: &str,
        directory: &Path,
    ) -> Result<()> {
        self.draw(history, None, model, directory)
    }

    pub fn wait_for_tool<T>(
        &mut self,
        label: &str,
        cancel: &std::sync::atomic::AtomicBool,
        worker: std::thread::ScopedJoinHandle<'_, T>,
    ) -> Result<T> {
        let message = trf!(
            "{} · F6 — разрешения · Esc — остановить",
            "{} · F6 permissions · Esc to stop",
            label
        );
        self.busy(&message)?;
        while !worker.is_finished() {
            if self.poll_tool_stop(&message)? {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("Tool worker stopped"))
    }

    pub(super) fn wait_for_web<T: Send + 'static>(
        &mut self,
        label: &str,
        task: impl FnOnce(&std::sync::atomic::AtomicBool) -> Result<T> + Send + 'static,
    ) -> Result<Option<Result<T>>> {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let message = trf!(
            "{} · F6 — разрешения · Esc — отменить",
            "{} · F6 permissions · Esc to cancel",
            label
        );
        self.busy(&message)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker = std::thread::spawn(move || task(&worker_cancel));
        let started = Instant::now();
        while !worker.is_finished() {
            let stop = self.poll_tool_stop(&message);
            if !matches!(stop, Ok(false)) {
                cancel.store(true, Ordering::Relaxed);
                return stop.map(|_| None);
            }
            if started.elapsed() >= super::web::TIMEOUT && !worker.is_finished() {
                cancel.store(true, Ordering::Relaxed);
                return Ok(Some(Err(anyhow::anyhow!(tr!(
                    "Веб-запрос занял слишком много времени. Проверь соединение и настройку /search.",
                    "The web request timed out. Check your connection and /search settings."
                )))));
            }
        }
        Ok(Some(worker.join().unwrap_or_else(|_| {
            Err(anyhow::anyhow!("Web worker stopped"))
        })))
    }

    fn poll_tool_stop(&mut self, message: &str) -> Result<bool> {
        if !event::poll(Duration::from_millis(55))? {
            return Ok(false);
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let key = normalize_key(key);
                if key.code == KeyCode::F(6) {
                    self.permissions_menu()?;
                    self.busy(message)?;
                }
                return Ok(key.code == KeyCode::Esc
                    || (key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL));
            }
            Event::Resize(_, _) => self.busy(message)?,
            _ => {}
        }
        Ok(false)
    }

    pub fn permission_mode(&self) -> super::permissions::Mode {
        self.permission_mode
    }

    pub fn permissions_menu(&mut self) -> Result<()> {
        use super::permissions::Mode;
        let modes = Mode::ALL;
        let choices = modes.map(|mode| {
            format!(
                "{} {}",
                if mode == self.permission_mode {
                    "●"
                } else {
                    " "
                },
                mode.label()
            )
        });
        let was_approval = std::mem::replace(&mut self.approval_card, true);
        let selected = self.choose(tr!("Разрешения · F6", "Permissions · F6"), &choices);
        let selection = selected.and_then(|index| {
            if let Some(index) = index {
                let mode = modes[index];
                if mode != Mode::Full
                    || self.review_card(mode.label(), mode.description(), false)?
                {
                    self.permission_mode = mode;
                }
            }
            Ok(())
        });
        self.approval_card = was_approval;
        selection?;
        self.notice(self.permission_mode.label());
        Ok(())
    }

    pub(super) fn request_permission(
        &mut self,
        mode: super::permissions::Mode,
        reason: &str,
    ) -> Result<bool> {
        let detail = format!(
            "{} → {}\n{}\n\n{}",
            self.permission_mode.label(),
            mode.label(),
            mode.description(),
            reason
        );
        if self.review(
            tr!("Агент запрашивает разрешение", "Agent requests permission"),
            &detail,
        )? {
            self.permission_mode = mode;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn review(&mut self, title: &str, text: &str) -> Result<bool> {
        let was_approval = std::mem::replace(&mut self.approval_card, true);
        let result = self.review_card(title, text, true);
        self.approval_card = was_approval;
        result
    }

    fn review_card(&mut self, title: &str, text: &str, permissions_shortcut: bool) -> Result<bool> {
        let (terminal_width, _) = terminal::size()?;
        let width = (usize::from(terminal_width).saturating_sub(8).clamp(36, 96)).saturating_sub(4);
        let mut lines = Vec::new();
        for paragraph in text.lines() {
            if paragraph.trim().is_empty() {
                lines.push(Line::new());
                continue;
            }
            let mut line = Line::new();
            line.push(paragraph, Style::fg(render::FG));
            lines.extend(render::wrap(&line, width));
        }
        while event::poll(Duration::ZERO)? {
            let _ = event::read()?;
        }
        let mut scroll = 0usize;
        loop {
            let panel = self.draw_panel(
                title,
                tr!(
                    " Enter — подтвердить · Esc — отмена · F6 — разрешения · ↑↓ — прокрутка ",
                    " Enter confirm · Esc cancel · F6 permissions · ↑↓ scroll "
                ),
                &lines,
                scroll,
                None,
            )?;
            match read_ui_event()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let key = normalize_key(key);
                    match key.code {
                        KeyCode::Enter => return Ok(true),
                        KeyCode::F(6) if permissions_shortcut => self.permissions_menu()?,
                        KeyCode::Esc | KeyCode::Char('q') => return Ok(false),
                        KeyCode::Up => scroll = scroll.saturating_sub(1),
                        KeyCode::Down => scroll = scroll.saturating_add(1),
                        KeyCode::PageUp => scroll = scroll.saturating_sub(10),
                        KeyCode::PageDown => scroll = scroll.saturating_add(10),
                        KeyCode::Home => scroll = 0,
                        KeyCode::End => scroll = lines.len().saturating_sub(panel.visible),
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            return Ok(false);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// The `/skills` browser: list with sources, Enter opens the instruction
    /// body, Esc goes back to the list.
    pub fn skills(&mut self, catalog: &super::skills::Catalog) -> Result<()> {
        if catalog.is_empty() {
            return self.overlay(
                tr!("Скиллы", "Skills"),
                vec![
                    Line::of(
                        tr!("Скиллов не найдено.", "No skills found."),
                        Style::fg(render::FG),
                    ),
                    Line::new(),
                    Line::of(
                        tr!(
                            "Проект: .carrycode/skills · пользователь: ~/.carrycode/skills",
                            "Project: .carrycode/skills · user: ~/.carrycode/skills"
                        ),
                        Style::dim(),
                    ),
                    Line::of(
                        tr!(
                            "Также читаются ~/.agents/skills, ~/.claude/skills и ~/.codex/skills.",
                            "Also reads ~/.agents/skills, ~/.claude/skills and ~/.codex/skills."
                        ),
                        Style::dim(),
                    ),
                ],
            );
        }
        let mut selected = 0usize;
        let mut scroll = 0usize;
        let mut preview: Option<usize> = None;
        let mut preview_scroll = 0usize;
        loop {
            if let Some(index) = preview {
                let Some(skill) = catalog.get(index) else {
                    preview = None;
                    continue;
                };
                let body = match super::skills::body(skill) {
                    Ok(body) => body,
                    Err(error) => error.to_string(),
                };
                let width = {
                    let (terminal_width, _) = terminal::size()?;
                    (usize::from(terminal_width).saturating_sub(8).clamp(36, 96)).saturating_sub(4)
                };
                let mut lines = Vec::new();
                for paragraph in body.lines() {
                    if paragraph.trim().is_empty() {
                        lines.push(Line::new());
                        continue;
                    }
                    let mut line = Line::new();
                    line.push(paragraph, Style::fg(render::FG));
                    lines.extend(render::wrap(&line, width));
                }
                let panel = self.draw_panel(
                    &trf!("Скилл: {}", "Skill: {}", skill.name),
                    tr!(
                        " ↑↓ · PgUp/PgDn — прокрутка · Esc — назад ",
                        " ↑↓ · PgUp/PgDn — scroll · Esc — back "
                    ),
                    &lines,
                    preview_scroll,
                    None,
                )?;
                match read_ui_event()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        let key = normalize_key(key);
                        match key.code {
                            KeyCode::Esc => preview = None,
                            KeyCode::Up => preview_scroll = preview_scroll.saturating_sub(1),
                            KeyCode::Down => preview_scroll = preview_scroll.saturating_add(1),
                            KeyCode::PageUp => preview_scroll = preview_scroll.saturating_sub(10),
                            KeyCode::PageDown => preview_scroll = preview_scroll.saturating_add(10),
                            KeyCode::Home => preview_scroll = 0,
                            KeyCode::End => {
                                preview_scroll = lines.len().saturating_sub(panel.visible)
                            }
                            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                continue;
            }
            let mut lines = Vec::new();
            for (index, skill) in catalog.skills.iter().enumerate() {
                let is_selected = index == selected;
                let mut row = Line::new();
                row.push("  ", Style::plain());
                row.push(
                    if is_selected { "›" } else { " " },
                    if is_selected {
                        Style::brand().bold()
                    } else {
                        Style::dim()
                    },
                );
                row.push(" ", Style::plain());
                row.push(
                    skill.name.clone(),
                    if is_selected {
                        Style::brand().bold()
                    } else {
                        Style::fg(render::FG)
                    },
                );
                row.push("  ", Style::plain());
                row.push(skill.source.label(), Style::dim());
                lines.push(row);
                let description = if skill.description.is_empty() {
                    tr!("без описания", "no description").to_owned()
                } else {
                    skill.description.clone()
                };
                let mut detail = Line::new();
                detail.push("      ", Style::plain());
                detail.push(description, Style::dim());
                lines.push(detail);
                lines.push(Line::new());
            }
            let (_, terminal_height) = terminal::size()?;
            let visible = (lines.len() + 2)
                .min(usize::from(terminal_height).saturating_sub(2))
                .max(4)
                .saturating_sub(2);
            let row = selected * 3;
            if row < scroll {
                scroll = row;
            }
            if row + 1 >= scroll + visible {
                scroll = (row + 1).saturating_sub(visible.saturating_sub(1));
            }
            let panel = self.draw_panel(
                &trf!("Скиллы · {}", "Skills · {}", catalog.skills.len()),
                tr!(
                    " ↑↓ — выбор · Enter — инструкции · Esc — закрыть ",
                    " ↑↓ — select · Enter — instructions · Esc — close "
                ),
                &lines,
                scroll,
                None,
            )?;
            scroll = scroll.min(lines.len().saturating_sub(panel.visible));
            match read_ui_event()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let key = normalize_key(key);
                    match key.code {
                        KeyCode::Up => selected = selected.saturating_sub(1),
                        KeyCode::Down => selected = (selected + 1).min(catalog.skills.len() - 1),
                        KeyCode::Home => selected = 0,
                        KeyCode::End => selected = catalog.skills.len() - 1,
                        KeyCode::PageUp => selected = selected.saturating_sub(10),
                        KeyCode::PageDown => {
                            selected = (selected + 10).min(catalog.skills.len() - 1)
                        }
                        KeyCode::Enter => {
                            preview = Some(selected);
                            preview_scroll = 0;
                        }
                        KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// `/mcp`: server states, tool lists and import from other CLIs.
    pub fn mcp(
        &mut self,
        servers: &[super::mcp::ServerStatus],
        tools: &[super::mcp::ToolInfo],
        candidates: &[super::mcp::Candidate],
    ) -> Result<McpAction> {
        #[derive(Clone, Copy)]
        enum Mode {
            Servers,
            Tools(usize),
            Import,
        }
        let mut mode = Mode::Servers;
        let mut selected = 0usize;
        let mut scroll = 0usize;
        let mut checked: Vec<bool> = candidates.iter().map(|item| item.enabled).collect();
        let mut error: Option<String> = None;
        loop {
            let (title, footer, body) = match mode {
                Mode::Servers => {
                    let mut lines = Vec::new();
                    if servers.is_empty() {
                        lines.push(Line::of(
                            tr!("  MCP-серверов нет.", "  No MCP servers."),
                            Style::fg(render::FG),
                        ));
                        lines.push(Line::of(
                            tr!(
                                "  i — импортировать из Claude Code, Codex или OpenCode.",
                                "  i — import from Claude Code, Codex or OpenCode."
                            ),
                            Style::dim(),
                        ));
                    }
                    for (index, server) in servers.iter().enumerate() {
                        let is_selected = index == selected;
                        let mut row = Line::new();
                        row.push("  ", Style::plain());
                        row.push(
                            if is_selected { "›" } else { " " },
                            if is_selected {
                                Style::brand().bold()
                            } else {
                                Style::dim()
                            },
                        );
                        row.push(
                            if server.running { "●" } else { "○" },
                            if server.running {
                                Style::brand()
                            } else {
                                Style::dim()
                            },
                        );
                        row.push(" ", Style::plain());
                        row.push(
                            server.name.clone(),
                            if is_selected {
                                Style::brand().bold()
                            } else {
                                Style::fg(render::FG)
                            },
                        );
                        row.push("  ", Style::plain());
                        let detail = if server.running {
                            trf!("{} инстр.", "{} tools", server.tools)
                        } else if let Some(error) = &server.error {
                            trf!("ошибка: {}", "error: {}", error)
                        } else {
                            tr!("выключен", "disabled").to_owned()
                        };
                        row.push(
                            detail,
                            if server.error.is_some() {
                                Style::fg(render::WARN)
                            } else {
                                Style::dim()
                            },
                        );
                        row.push("  ", Style::plain());
                        row.push(server.source.clone(), Style::dim());
                        lines.push(row);
                    }
                    if let Some(error) = &error {
                        lines.push(Line::new());
                        lines.push(Line::of(format!("! {error}"), Style::fg(render::WARN)));
                    }
                    (
                        tr!("MCP-серверы", "MCP servers"),
                        tr!(
                            " ↑↓ — выбор · Enter — инструменты · r — перезапуск · i — импорт · Esc — закрыть ",
                            " ↑↓ — select · Enter — tools · r — restart · i — import · Esc — close "
                        ),
                        lines,
                    )
                }
                Mode::Tools(index) => {
                    let server = servers.get(index);
                    let mut lines = Vec::new();
                    if let Some(server) = server {
                        let matching: Vec<_> = tools
                            .iter()
                            .filter(|tool| tool.server == server.name)
                            .collect();
                        if matching.is_empty() {
                            lines.push(Line::of(
                                tr!(
                                    "  У сервера нет доступных инструментов.",
                                    "  The server exposes no tools."
                                ),
                                Style::dim(),
                            ));
                        }
                        for tool in matching {
                            let mut row = Line::new();
                            row.push("  ", Style::plain());
                            row.push(tool.name.clone(), Style::brand());
                            row.push("  ", Style::plain());
                            row.push(tool.description.clone(), Style::dim());
                            lines.push(row);
                            lines.push(Line::new());
                        }
                    }
                    (
                        tr!("Инструменты MCP", "MCP tools"),
                        tr!(
                            " ↑↓ · PgUp/PgDn — прокрутка · Esc — назад ",
                            " ↑↓ · PgUp/PgDn — scroll · Esc — back "
                        ),
                        lines,
                    )
                }
                Mode::Import => {
                    let mut lines = Vec::new();
                    if candidates.is_empty() {
                        lines.push(Line::of(
                            tr!(
                                "  Новых серверов в конфигах Claude Code, Codex и OpenCode нет.",
                                "  No new servers in the Claude Code, Codex or OpenCode configs."
                            ),
                            Style::dim(),
                        ));
                    }
                    for (index, candidate) in candidates.iter().enumerate() {
                        let is_selected = index == selected;
                        let mut row = Line::new();
                        row.push("  ", Style::plain());
                        row.push(
                            if is_selected { "›" } else { " " },
                            if is_selected {
                                Style::brand().bold()
                            } else {
                                Style::dim()
                            },
                        );
                        row.push(
                            if checked.get(index).copied().unwrap_or(false) {
                                "[x] "
                            } else {
                                "[ ] "
                            },
                            Style::brand(),
                        );
                        row.push(
                            candidate.name.clone(),
                            if is_selected {
                                Style::brand().bold()
                            } else {
                                Style::fg(render::FG)
                            },
                        );
                        row.push("  ", Style::plain());
                        row.push(candidate.source.clone(), Style::dim());
                        lines.push(row);
                        let mut command = Line::new();
                        command.push("        ", Style::plain());
                        command.push(candidate.command.clone(), Style::dim());
                        for argument in &candidate.args {
                            command.push(" ", Style::dim());
                            command.push(argument.clone(), Style::dim());
                        }
                        lines.push(command);
                        lines.push(Line::new());
                    }
                    if let Some(error) = &error {
                        lines.push(Line::of(format!("! {error}"), Style::fg(render::WARN)));
                    }
                    (
                        tr!("Импорт MCP", "MCP import"),
                        tr!(
                            " Space — отметить · a — все · Enter — импортировать · Esc — назад ",
                            " Space — toggle · a — all · Enter — import · Esc — back "
                        ),
                        lines,
                    )
                }
            };
            self.draw_panel(title, footer, &body, scroll, None)?;
            match read_ui_event()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let key = normalize_key(key);
                    match mode {
                        Mode::Servers => match key.code {
                            KeyCode::Up => selected = selected.saturating_sub(1),
                            KeyCode::Down => {
                                selected = (selected + 1).min(servers.len().saturating_sub(1))
                            }
                            KeyCode::Home => selected = 0,
                            KeyCode::End => selected = servers.len().saturating_sub(1),
                            KeyCode::Enter if !servers.is_empty() => {
                                mode = Mode::Tools(selected);
                                scroll = 0;
                            }
                            KeyCode::Char('r') | KeyCode::Char('р') | KeyCode::Char('к') => {
                                return Ok(McpAction::Restart);
                            }
                            KeyCode::Char('i') | KeyCode::Char('и') | KeyCode::Char('ш') => {
                                error = None;
                                checked = candidates.iter().map(|item| item.enabled).collect();
                                selected = 0;
                                scroll = 0;
                                mode = Mode::Import;
                            }
                            KeyCode::Esc | KeyCode::Char('q') => return Ok(McpAction::Close),
                            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                                return Ok(McpAction::Close);
                            }
                            _ => {}
                        },
                        Mode::Tools(_) => match key.code {
                            KeyCode::Esc => mode = Mode::Servers,
                            KeyCode::Up => scroll = scroll.saturating_sub(1),
                            KeyCode::Down => scroll = scroll.saturating_add(1),
                            KeyCode::PageUp => scroll = scroll.saturating_sub(10),
                            KeyCode::PageDown => scroll = scroll.saturating_add(10),
                            KeyCode::Home => scroll = 0,
                            KeyCode::End => scroll = body.len(),
                            KeyCode::Char('q') => return Ok(McpAction::Close),
                            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                                return Ok(McpAction::Close);
                            }
                            _ => {}
                        },
                        Mode::Import => match key.code {
                            KeyCode::Esc => mode = Mode::Servers,
                            KeyCode::Up => selected = selected.saturating_sub(1),
                            KeyCode::Down => {
                                selected = (selected + 1).min(candidates.len().saturating_sub(1))
                            }
                            KeyCode::Char(' ') if !candidates.is_empty() => {
                                if let Some(value) = checked.get_mut(selected) {
                                    *value = !*value;
                                }
                            }
                            KeyCode::Char('a') | KeyCode::Char('а') | KeyCode::Char('ф') => {
                                let all = checked.iter().all(|value| *value);
                                checked.iter_mut().for_each(|value| *value = !all);
                            }
                            KeyCode::Enter => {
                                let chosen: Vec<usize> = checked
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(index, value)| value.then_some(index))
                                    .collect();
                                if chosen.is_empty() {
                                    error = Some(
                                        tr!(
                                            "Отметь хотя бы один сервер",
                                            "Select at least one server"
                                        )
                                        .to_owned(),
                                    );
                                } else {
                                    return Ok(McpAction::Import(chosen));
                                }
                            }
                            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                                return Ok(McpAction::Close);
                            }
                            _ => {}
                        },
                    }
                }
                _ => {}
            }
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            SetAttribute(Attribute::Reset),
            ResetColor,
            LeaveAlternateScreen,
            Show
        );
        let _ = terminal::disable_raw_mode();
    }
}

fn enter() -> io::Result<()> {
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        Hide,
        EnableBracketedPaste,
        EnableMouseCapture
    )
}

fn ignored_event(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            ..
        }) | Event::Key(KeyEvent {
            kind: KeyEventKind::Release,
            ..
        })
    )
}

fn read_ui_event() -> io::Result<Event> {
    loop {
        let event = event::read()?;
        if !ignored_event(&event) {
            return Ok(event);
        }
    }
}

fn put_line(out: &mut impl Write, x: usize, y: usize, line: &Line, width: usize) -> io::Result<()> {
    if width == 0 {
        return Ok(());
    }
    queue!(
        out,
        MoveTo(x as u16, y as u16),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    let mut current = Style::plain();
    let mut remaining = width;
    for span in line.spans() {
        if remaining == 0 {
            break;
        }
        let text = clip(&span.text, remaining);
        if text.is_empty() {
            continue;
        }
        remaining = remaining.saturating_sub(text.as_str().width());
        if span.style != current {
            apply_style(out, span.style, &mut current)?;
        }
        queue!(out, Print(text))?;
    }
    queue!(out, SetAttribute(Attribute::Reset), ResetColor)
}

fn apply_style(out: &mut impl Write, style: Style, current: &mut Style) -> io::Result<()> {
    if style.reversed != current.reversed {
        queue!(
            out,
            SetAttribute(if style.reversed {
                Attribute::Reverse
            } else {
                Attribute::NoReverse
            })
        )?;
    }
    if style.bold != current.bold {
        queue!(
            out,
            SetAttribute(if style.bold {
                Attribute::Bold
            } else {
                Attribute::NormalIntensity
            })
        )?;
    }
    if style.fg != current.fg {
        queue!(out, SetForegroundColor(style.fg.unwrap_or(Color::Reset)))?;
    }
    if style.bg != current.bg {
        queue!(out, SetBackgroundColor(style.bg.unwrap_or(Color::Reset)))?;
    }
    *current = style;
    Ok(())
}

fn clip(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for character in text.chars() {
        let cell = character.width().unwrap_or(0);
        if used + cell > width {
            break;
        }
        out.push(character);
        used += cell;
    }
    out
}

fn receipt_line(receipt: Receipt, width: usize) -> Line {
    let mut line = Line::new();
    line.push("  ", Style::plain());
    line.push("╰─ ", Style::dim());
    line.push(format!("{:.1}s", receipt.duration), Style::dim());
    let approx = if receipt.estimated { "~" } else { "" };
    let extras = [
        Some(trf!(
            "↑{}{} ↓{}{} токенов",
            "↑{}{} ↓{}{} tokens",
            approx,
            render::human(receipt.input),
            approx,
            render::human(receipt.output)
        )),
        (receipt.speed > 0.0).then(|| format!("{approx}{:.0} tok/s", receipt.speed)),
        receipt
            .ttft
            .map(|seconds| trf!("первый токен {:.1}s", "first token {:.1}s", seconds)),
    ];
    for extra in extras.into_iter().flatten() {
        let piece = Line::of(format!(" · {extra}"), Style::dim());
        if line.width().saturating_add(piece.width()) > width {
            break;
        }
        line.extend(piece);
    }
    line.truncate(width)
}

/// Joins status segments with ` · ` and drops the rightmost ones that do not fit.
fn join_segments(segments: &[Line], width: usize) -> Line {
    let mut line = Line::new();
    for (index, segment) in segments.iter().enumerate() {
        if index == 0 {
            line = segment.truncate(width);
            continue;
        }
        let mut piece = Line::of(" · ", Style::dim());
        piece.extend(segment.clone());
        if line.width().saturating_add(piece.width()) > width {
            break;
        }
        line.extend(piece);
    }
    line
}

fn reasoning_lines(reasoning: &str, width: usize, open: bool, live: bool) -> Vec<Line> {
    let text = reasoning.trim_end();
    if text.is_empty() {
        return Vec::new();
    }
    let mut body = Vec::new();
    for raw in text.lines() {
        if raw.trim().is_empty() {
            body.push(Line::of("  │", Style::dim()));
            continue;
        }
        let mut line = Line::new();
        line.push(raw, Style::dim());
        for row in render::wrap(&line, width.saturating_sub(4)) {
            let mut prefixed = Line::of("  │ ", Style::dim());
            prefixed.extend(row);
            body.push(prefixed);
        }
    }
    let limit = if live {
        2
    } else if open {
        80
    } else {
        3
    };
    let start = if live {
        body.len().saturating_sub(limit)
    } else {
        0
    };
    let mut lines = Vec::new();
    let mut head = Line::of("  │ ", Style::dim());
    head.push(tr!("reasoning", "reasoning"), Style::brand());
    lines.push(head);
    for row in body.iter().skip(start).take(limit) {
        lines.push(row.clone());
    }
    if !live && body.len() > limit {
        lines.push(Line::of(
            format!("  │ … +{} · F3", body.len() - limit),
            Style::dim(),
        ));
    }
    lines.push(Line::new());
    lines
}

fn history_lines(
    history: &[Message],
    width: usize,
    details: bool,
    reasoning_details: bool,
) -> Vec<Line> {
    let mut lines = Vec::new();
    for message in history {
        if matches!(
            message.role.as_str(),
            "assistant" | "narration" | "interrupted"
        ) {
            if message.role == "interrupted" {
                lines.push(Line::of(tr!("  Остановлено", "  Stopped"), Style::dim()));
            }
            if let Some(reasoning) = message.reasoning.as_deref() {
                lines.extend(reasoning_lines(reasoning, width, reasoning_details, false));
            }
            if let Some(action) = super::parse_action(&message.content) {
                lines.push(action_line(&action));
            } else if !message.content.is_empty() {
                lines.extend(render::assistant(&message.content, width));
            }
        } else if message.role == "event" {
            for text in message.content.lines() {
                let line = Line::of(format!("  {text}"), Style::dim());
                if line.is_blank() {
                    lines.push(line);
                } else {
                    lines.extend(render::wrap(&line, width));
                }
            }
        } else if message.role == "permission" {
            for line in message.content.lines() {
                lines.extend(render::wrap(
                    &Line::of(line, Style::fg(render::WARN).bold()),
                    width,
                ));
            }
        } else if message.role == "error" {
            for text in message.content.lines() {
                lines.extend(render::wrap(
                    &Line::of(format!("  {text}"), Style::fg(render::WARN)),
                    width,
                ));
            }
        } else if message.content.starts_with("Tool result") {
            lines.extend(tool_lines(&message.content, width, details));
        } else {
            lines.extend(render::user(
                message.display.as_deref().unwrap_or(&message.content),
                width,
            ));
        }
        if let Some(receipt) = message.receipt {
            lines.push(receipt_line(receipt, width));
        }
        lines.push(Line::new());
    }
    while lines.last().is_some_and(Line::is_blank) {
        lines.pop();
    }
    lines
}

fn action_line(action: &serde_json::Value) -> Line {
    let name = action["action"].as_str().unwrap_or("action");
    let detail = ["path", "query", "task", "text"]
        .iter()
        .find_map(|key| action[*key].as_str())
        .unwrap_or("");
    let mut line = Line::new();
    line.push("  ", Style::plain());
    line.push("· ", Style::dim());
    line.push(name, Style::fg(render::BRAND));
    if !detail.is_empty() {
        line.push(" ", Style::plain());
        line.push(
            detail
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(96)
                .collect::<String>(),
            Style::dim(),
        );
    }
    line
}

fn tool_lines(content: &str, width: usize, details: bool) -> Vec<Line> {
    if !details {
        return vec![Line::of("  · tool result · F2", Style::dim())];
    }
    let mut lines = vec![Line::of("  · tool result", Style::dim())];
    let body = content
        .strip_prefix("Tool result")
        .and_then(|rest| rest.split_once('\n').map(|(_, body)| body))
        .unwrap_or(content)
        .trim_start();
    for raw in body.lines().take(60) {
        let wrapped = render::wrap(&Line::of(raw, Style::dim()), width.saturating_sub(4));
        if wrapped.is_empty() {
            lines.push(Line::new());
            continue;
        }
        for row in wrapped {
            let mut prefixed = Line::of("    ", Style::plain());
            prefixed.extend(row);
            lines.push(prefixed);
        }
    }
    if body.lines().nth(60).is_some() {
        lines.push(Line::of("    …", Style::dim()));
    }
    lines
}

fn help_lines() -> Vec<Line> {
    let mut lines = Vec::new();
    let heading = |text: &str| Line::of(text, Style::brand().bold());
    let row = |name: &str, description: &str| {
        let mut line = Line::new();
        line.push(format!("  {name:<12}"), Style::fg(render::FG));
        line.push(description.to_owned(), Style::dim());
        line
    };
    lines.push(heading(tr!(
        "Подключение и работа",
        "Connection and workflow"
    )));
    for (name, description) in commands() {
        lines.push(row(name, description));
    }
    lines.push(Line::new());
    lines.push(heading(tr!("Файлы и клавиши", "Files and keys")));
    lines.push(row(
        "@путь",
        tr!("приложить текстовый файл", "attach a text file"),
    ));
    lines.push(row("Enter", tr!("отправить", "send")));
    lines.push(row("Tab", tr!("дополнить команду", "complete a command")));
    lines.push(row(
        "↑ / ↓",
        tr!(
            "история ввода или выбор в меню",
            "input history or menu selection"
        ),
    ));
    lines.push(row(
        "Esc",
        tr!(
            "остановить ответ, закрыть меню или очистить ввод",
            "stop the reply, close the menu or clear the input"
        ),
    ));
    lines.push(row(
        "F2",
        tr!(
            "показать или скрыть результаты инструментов",
            "show or hide tool results"
        ),
    ));
    lines.push(row(
        "F3",
        tr!(
            "раскрыть или скрыть reasoning",
            "expand or collapse reasoning"
        ),
    ));
    lines.push(row(
        "F6",
        tr!(
            "сменить разрешения во время работы",
            "change permissions during a request"
        ),
    ));
    lines.push(row(
        "PgUp/PgDn",
        tr!("прокрутить историю", "scroll history"),
    ));
    lines.push(row(
        "Shift+↑/↓",
        tr!("прокрутить историю", "scroll history"),
    ));
    lines.push(row(
        tr!("колесо", "wheel"),
        tr!("прокрутить историю", "scroll history"),
    ));
    lines.push(row(
        "Ctrl-C",
        tr!(
            "копировать выделенное мышью; иначе очистить ввод",
            "copy mouse selection; otherwise clear the input"
        ),
    ));
    lines.push(row(
        "Ctrl-D",
        tr!("выйти с пустой строкой", "exit from an empty line"),
    ));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_filters_commands_by_prefix() {
        assert_eq!(menu_items("/", false, false).len(), commands().len());
        let filtered = menu_items("/st", false, false);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].0, "/status");
        assert!(menu_items("/", true, false).is_empty());
        assert!(menu_items("/", false, true).is_empty());
        assert!(menu_items("/help me", false, false).is_empty());
        assert!(menu_items("hello", false, false).is_empty());
        assert_eq!(menu_items("draft /per", false, false)[0].0, "/permissions");
    }

    fn test_screen() -> Screen {
        Screen {
            editor: Editor::new(),
            pastes: Vec::new(),
            submitted_pastes: Vec::new(),
            notice: String::new(),
            scroll: 0,
            details: false,
            queued: String::new(),
            submit_queued: false,
            submitted: Vec::new(),
            nav: None,
            selected: 0,
            menu_closed: false,
            armed: false,
            diagnose: false,
            tick: 0,
            turn: None,
            stats: Stats::default(),
            cache: None,
            context_limit: CTX_TOKENS,
            input_limit: CTX_TOKENS.saturating_sub(4096),
            output_limit: 4096,
            input_estimate: None,
            reasoning: crate::native::Reasoning::Off,
            reasoning_details: false,
            background: Vec::new(),
            desktop_active: false,
            panel_frame: None,
            viewport: None,
            selection: None,
            permission_mode: crate::native::permissions::Mode::Ask,
            approval_card: false,
        }
    }

    #[test]
    fn tool_turns_count_every_streamed_reply() {
        let mut screen = test_screen();
        screen.begin_turn(Instant::now() - Duration::from_secs(2));
        screen.track("first", "");
        screen.record_reply(100, 10, false, 0.1);
        screen.track("second", "");
        screen.record_reply(200, 20, true, 0.1);
        let _ = screen.finish_turn();
        let receipt = screen.stats.last.expect("receipt");
        assert_eq!(receipt.input, 300);
        assert_eq!(receipt.output, 30);
        assert!(receipt.duration >= 2.0 && receipt.estimated);
        assert_eq!(screen.stats.turns, 1);
        screen.stage_input("old draft".to_owned());
        screen.set_input_estimate(Some(500));
        screen.reset_conversation();
        screen.paste_prompt(&"draft line\n".repeat(60));
        assert!(screen.editor.is_empty() && screen.turn.is_none());
        assert_eq!(screen.pastes.len(), 1);
        assert_eq!(screen.submit(""), "");
        assert_eq!(screen.take_pastes()[0].lines().count(), 60);
        assert!(screen.pastes.is_empty() && screen.turn.is_none());
        assert!(screen.permission_mode() == crate::native::permissions::Mode::Auto);
        assert!(screen.stats.last.is_none());
        assert_eq!(screen.stats.turns, 0);
        assert_eq!(screen.input_estimate, None);
        assert!(!screen.submit_queued);
        assert!(screen.queued.is_empty());
    }

    #[test]
    fn tool_details_follow_the_f2_toggle() {
        let message = Message {
            role: "user".into(),
            content: "Tool result (untrusted file content)\nfn agent() {}".into(),
            reasoning: None,
            ..Default::default()
        };
        let compact = history_lines(std::slice::from_ref(&message), 60, false, false);
        let full = history_lines(std::slice::from_ref(&message), 60, true, false);
        assert!(compact.iter().any(|line| line.plain_text().contains("F2")));
        assert!(
            full.iter()
                .any(|line| line.plain_text().contains("fn agent"))
        );
    }

    #[test]
    fn status_segments_drop_from_the_right() {
        let segments = vec![
            Line::of("● model", Style::plain()),
            Line::of("ctx 1/2", Style::plain()),
            Line::of("session 1s", Style::plain()),
        ];
        let line = join_segments(&segments, 14);
        assert!(line.width() <= 14);
        assert!(!line.plain_text().contains("session"));
        let full = join_segments(&segments, 80);
        assert!(full.plain_text().contains("session"));
    }

    #[test]
    fn the_editor_keeps_the_caret_visible_in_a_narrow_box() {
        let mut editor = Editor::from("abcdefghij");
        let (shown, caret) = editor.view(5);
        assert_eq!(shown, "ghij");
        assert_eq!(caret, 4);
        editor.left();
        editor.left();
        let (shown, caret) = editor.view(5);
        assert!(shown.len() <= 5);
        assert!(caret < 5);
        assert_eq!(editor.text(), "abcdefghij");
    }

    #[test]
    fn editing_keys_move_and_delete_around_the_caret() {
        let mut editor = Editor::from("one two");
        editor.kill_word();
        assert_eq!(editor.text(), "one ");
        editor.kill_to_start();
        assert_eq!(editor.text(), "");
        editor.insert("hello world".chars());
        editor.home();
        editor.delete();
        assert_eq!(editor.text(), "ello world");
        for _ in 0..4 {
            editor.right();
        }
        editor.kill_to_end();
        assert_eq!(editor.text(), "ello");
        editor.insert("!".chars());
        assert_eq!(editor.text(), "ello!");
    }

    #[test]
    fn windows_control_characters_are_normalized() {
        let key = normalize_key(KeyEvent::new(
            KeyCode::Char('\u{15}'),
            KeyModifiers::CONTROL,
        ));
        assert_eq!(key.code, KeyCode::Char('u'));
        assert!(key.modifiers.contains(KeyModifiers::CONTROL));
        let plain = normalize_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        assert_eq!(plain.code, KeyCode::Char('u'));
        assert!(plain.modifiers.is_empty());
        let cyrillic = normalize_key(KeyEvent::new(KeyCode::Char('с'), KeyModifiers::CONTROL));
        assert_eq!(cyrillic.code, KeyCode::Char('c'));
        let altgr = normalize_key(KeyEvent::new(
            KeyCode::Char('с'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert_eq!(altgr.code, KeyCode::Char('с'));
    }

    #[test]
    fn pasted_newlines_stay_in_the_draft() {
        assert_eq!(paste_text("a\r\nb\tc\x07"), "a\nb c");
    }

    #[test]
    fn footer_reserves_space_for_context_in_idle_and_streaming_views() {
        let mut screen = test_screen();
        screen.set_limits(128_000, 100_000, 4_000, super::super::Reasoning::Off);
        screen.set_input_estimate(Some(72_000));
        screen.begin_turn(Instant::now());
        let model = "provider / a-model-with-a-very-long-name-that-can-fill-the-whole-footer";
        for streaming in [false, true] {
            for width in [40, 60, 80, 120] {
                let footer = screen.footer(model, width, streaming);
                let text = footer.plain_text();
                assert_eq!(footer.width(), width);
                assert!(text.ends_with("  ctx 72% "));
                assert!(!text.contains(['▰', '▱']));
            }
        }
        screen.stage_input("draft".repeat(500));
        assert_eq!(screen.input_percentage(), Some(72));
        screen.set_limits(128_000, 72_000, 4_000, super::super::Reasoning::Off);
        assert!(
            screen
                .footer(model, 80, false)
                .plain_text()
                .ends_with("100% ")
        );
        screen.set_input_estimate(Some(72_001));
        assert!(
            screen
                .footer(model, 80, false)
                .plain_text()
                .ends_with("100%+ ")
        );
        screen.reset_conversation();
        assert!(screen.footer(model, 80, false).plain_text().ends_with("— "));
    }
    #[test]
    fn reasoning_history_collapses_and_f3_expands() {
        let message = Message {
            role: "assistant".into(),
            content: "answer".into(),
            reasoning: Some("one\ntwo\nthree\nfour".into()),
            ..Default::default()
        };
        let compact = history_lines(std::slice::from_ref(&message), 60, false, false);
        let full = history_lines(std::slice::from_ref(&message), 60, false, true);
        let compact_text: Vec<String> = compact.iter().map(|line| line.plain_text()).collect();
        let full_text: Vec<String> = full.iter().map(|line| line.plain_text()).collect();
        assert!(compact_text.iter().any(|line| line.contains("reasoning")));
        assert!(compact_text.iter().any(|line| line.contains("+1")));
        assert!(compact_text.iter().any(|line| line.contains("F3")));
        assert!(!compact_text.iter().any(|line| line.contains("four")));
        assert!(full_text.iter().any(|line| line.contains("four")));
        assert!(!full_text.iter().any(|line| line.contains("+1")));
    }

    #[test]
    fn model_form_rejects_over_budget_limits() {
        let mut form = ModelForm::new(None, None);
        form.name = Editor::from("Test");
        form.url = Editor::from("https://example.org/v1");
        form.model = Editor::from("model");
        form.context = Editor::from("10000");
        form.input = Editor::from("9000");
        form.output = Editor::from("2000");
        assert!(form.entry().is_err());
        form.output = Editor::from("1000");
        assert!(form.entry().is_ok());
    }

    #[test]
    fn anthropic_reasoning_requires_larger_output() {
        let mut form = ModelForm::new(None, None);
        form.name = Editor::from("Test");
        form.url = Editor::from("https://example.org/v1");
        form.model = Editor::from("model");
        form.anthropic = true;
        form.context = Editor::from("10000");
        form.input = Editor::from("5000");
        form.output = Editor::from("4000");
        form.reasoning = crate::native::Reasoning::High;
        assert!(form.entry().is_err());
        form.output = Editor::from("5000");
        assert!(form.entry().is_ok());
        form.reasoning_api = crate::native::ReasoningApi::None;
        form.output = Editor::from("4000");
        assert!(form.entry().is_ok());
    }
}
