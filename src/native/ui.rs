//! Full-screen console for the built-in agent.
//!
//! Layout, top to bottom: the mark and session header, the conversation,
//! the boxed prompt, then the slash menu, the live spinner or the status line.
//! The screen follows the agent console design and keeps every existing
//! interaction: history, `/` menu, F2 tool details, scrollback and the
//! suspend/resume handshake used while a reviewed edit or a task runs.

mod render;

use super::Message;
use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute, queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use render::{Line, Style};
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

/// Commands offered by the slash menu. Descriptions are localized.
fn commands() -> [(&'static str, &'static str); 15] {
    [
        (
            "/provider",
            tr!(
                "Сохранённые подключения и новый провайдер",
                "Saved connections and a new provider"
            ),
        ),
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
            "/init",
            tr!(
                "Создать AGENTS.md по структуре проекта",
                "Create AGENTS.md from the project layout"
            ),
        ),
        ("/help", tr!("Команды и клавиши", "Commands and keys")),
        ("/quit", tr!("Вернуться в меню", "Return to the launcher")),
    ]
}

/// Slash menu contents for the current buffer.
fn menu_items(text: &str, closed: bool, navigating: bool) -> Vec<(&'static str, &'static str)> {
    let typed = text.to_lowercase();
    if closed || navigating || !typed.starts_with('/') || typed.contains(' ') {
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
                out.push(' ');
            }
            '\n' | '\t' => out.push(' '),
            other if !other.is_control() => out.push(other),
            _ => {}
        }
    }
    out
}

fn estimate(chars: usize) -> u32 {
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX)
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
            .filter(|character| !character.is_control())
            .collect();
        let caret = buffer.len();
        Self { buffer, caret }
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

    fn insert(&mut self, text: impl IntoIterator<Item = char>) {
        if self.buffer.len() >= INPUT_LIMIT {
            return;
        }
        let text: Vec<char> = text.into_iter().filter(|c| !c.is_control()).collect();
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
        let mut start = 0;
        while start < self.caret
            && self.buffer[start..self.caret]
                .iter()
                .map(|character| character.width().unwrap_or(0))
                .sum::<usize>()
                > width.saturating_sub(1)
        {
            start += 1;
        }
        let mut shown = String::new();
        let mut used = 0;
        for character in &self.buffer[start..] {
            let cell = character.width().unwrap_or(0);
            if used + cell > width {
                break;
            }
            shown.push(*character);
            used += cell;
        }
        let caret = self.buffer[start..self.caret]
            .iter()
            .map(|character| character.width().unwrap_or(0))
            .sum::<usize>()
            .min(width.saturating_sub(1));
        (shown, caret)
    }
}

#[derive(Clone, Copy)]
struct Receipt {
    input: u32,
    output: u32,
    duration: f32,
    speed: f32,
    ttft: f32,
}

#[derive(Clone, Copy, Default)]
struct Stats {
    tin: u32,
    tout: u32,
    turns: u32,
    speed: f32,
    lat: f32,
    ttft: f32,
    last: Option<Receipt>,
}

#[derive(Clone, Copy)]
struct Turn {
    started: Instant,
    input: u32,
    output: u32,
    reply_started: Option<Instant>,
    first: Option<Instant>,
    chars: usize,
    counted: usize,
    seen: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CacheKey {
    messages: usize,
    bytes: usize,
    details: bool,
    width: usize,
}

struct Cache {
    key: CacheKey,
    lines: Vec<Line>,
    chars: usize,
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
    pub anthropic: bool,
    pub chat_only: bool,
    pub context: u32,
}

pub enum ModelAction {
    Activate(usize),
    Save {
        index: Option<usize>,
        entry: ModelEntry,
    },
    Delete(usize),
    Close,
}

pub struct Screen {
    editor: Editor,
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
    started: Instant,
    cache: Option<Cache>,
    context_limit: u32,
    suspended: bool,
}

impl Screen {
    pub fn open() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let screen = Self {
            editor: Editor::new(),
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
            started: Instant::now(),
            cache: None,
            context_limit: CTX_TOKENS,
            suspended: false,
        };
        if let Err(error) = enter() {
            let _ = terminal::disable_raw_mode();
            return Err(error.into());
        }
        Ok(screen)
    }

    /// Sets the context window of the active model, used by every ctx readout.
    pub fn set_context(&mut self, tokens: u32) {
        self.context_limit = tokens.max(1_000);
    }

    /// Leaves the alternate screen for a plain prompt (provider setup, task
    /// confirmation, review) and keeps the session state for `resume`.
    pub fn suspend(&mut self) -> Result<()> {
        if self.suspended {
            return Ok(());
        }
        execute!(
            io::stdout(),
            DisableBracketedPaste,
            SetAttribute(Attribute::Reset),
            ResetColor,
            Show,
            LeaveAlternateScreen
        )?;
        terminal::disable_raw_mode()?;
        self.suspended = true;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        if !self.suspended {
            return Ok(());
        }
        terminal::enable_raw_mode()?;
        if let Err(error) = enter() {
            let _ = terminal::disable_raw_mode();
            return Err(error.into());
        }
        self.suspended = false;
        self.cache = None;
        Ok(())
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
        self.finish_turn();
        if !self.queued.is_empty() {
            let queued = std::mem::take(&mut self.queued);
            self.editor = Editor::from(&queued);
        }
        if std::mem::take(&mut self.submit_queued) && !self.editor.text().trim().is_empty() {
            let line = self.editor.text().trim().to_owned();
            return Ok(Some(self.submit(&line)));
        }
        loop {
            self.draw(history, None, model, directory)?;
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let key = normalize_key(key);
                    let items = self.menu();
                    let count = items.len();
                    let mut edited = false;
                    match key.code {
                        KeyCode::Enter => {
                            let line = if count > 0 {
                                items[self.selected.min(count - 1)].0.to_owned()
                            } else {
                                self.editor.text().trim().to_owned()
                            };
                            if line.is_empty() {
                                continue;
                            }
                            if line == "/status" {
                                let _ = self.submit(&line);
                                self.status_panel(model, directory)?;
                                continue;
                            }
                            return Ok(Some(self.submit(&line)));
                        }
                        KeyCode::Tab => {
                            if count > 0 {
                                let name = items[self.selected.min(count - 1)].0;
                                self.editor = Editor::from(&format!("{name} "));
                                self.menu_closed = true;
                                edited = true;
                            }
                        }
                        KeyCode::Esc => {
                            if count > 0 {
                                self.menu_closed = true;
                            } else if !self.editor.is_empty() {
                                self.editor.clear();
                                edited = true;
                            } else {
                                return Ok(None);
                            }
                        }
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            if !self.editor.is_empty() {
                                self.editor.clear();
                                edited = true;
                            } else if self.armed {
                                return Ok(None);
                            } else {
                                self.armed = true;
                            }
                        }
                        KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => {
                            if self.editor.is_empty() {
                                return Ok(None);
                            }
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
                            self.editor.backspace();
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
                        KeyCode::F(2) => self.details = !self.details,
                        KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
                        KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
                        KeyCode::Char(character) if text_key(key) && !character.is_control() => {
                            self.editor.insert([character]);
                            edited = true;
                        }
                        _ => {}
                    }
                    if edited {
                        self.selected = 0;
                        self.menu_closed = false;
                        self.armed = false;
                        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
                            self.nav = None;
                        }
                    }
                }
                Event::Paste(text) => {
                    self.editor.insert(paste_text(&text).chars());
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
        model: &str,
        directory: &Path,
    ) -> Result<bool> {
        self.track(output);
        self.tick = self.tick.wrapping_add(1);
        self.draw(history, Some(output), model, directory)?;
        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let key = normalize_key(key);
                    match key.code {
                        KeyCode::Esc => return Ok(true),
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            return Ok(true);
                        }
                        KeyCode::Enter => {
                            if !self.queued.trim().is_empty() {
                                self.submit_queued = true;
                            }
                        }
                        KeyCode::Backspace => {
                            self.queued.pop();
                        }
                        KeyCode::Char(character)
                            if text_key(key)
                                && !character.is_control()
                                && self.queued.len() < INPUT_LIMIT =>
                        {
                            self.queued.push(character);
                        }
                        _ => {}
                    }
                }
                Event::Paste(text) => {
                    for character in paste_text(&text).chars() {
                        if self.queued.len() >= INPUT_LIMIT {
                            break;
                        }
                        self.queued.push(character);
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }

    fn submit(&mut self, line: &str) -> String {
        let line = line.to_owned();
        if self.submitted.last() != Some(&line) {
            self.submitted.push(line.clone());
        }
        self.editor.clear();
        self.nav = None;
        self.menu_closed = false;
        self.selected = 0;
        self.scroll = 0;
        self.armed = false;
        self.begin_turn(&line);
        line
    }

    fn begin_turn(&mut self, line: &str) {
        if line.starts_with('/') {
            return;
        }
        let now = Instant::now();
        self.turn = Some(Turn {
            started: now,
            input: estimate(line.chars().count()),
            output: 0,
            reply_started: Some(now),
            first: None,
            chars: 0,
            counted: 0,
            seen: 0,
        });
    }

    fn track(&mut self, output: &str) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        if output.len() < turn.seen {
            if turn.chars > 0 {
                turn.output = turn.output.saturating_add(estimate(turn.chars));
            }
            turn.reply_started = Some(Instant::now());
            turn.first = None;
            turn.chars = 0;
            turn.counted = 0;
        }
        turn.seen = output.len();
        if output.is_empty() {
            return;
        }
        let now = Instant::now();
        if turn.reply_started.is_none() {
            turn.reply_started = Some(now);
        }
        if turn.first.is_none() {
            turn.first = Some(now);
        }
        if turn.counted != output.len() {
            turn.chars = output.chars().count();
            turn.counted = output.len();
        }
    }

    fn finish_turn(&mut self) {
        let Some(turn) = self.turn.take() else {
            return;
        };
        let mut output = turn.output;
        if turn.chars > 0 {
            output = output.saturating_add(estimate(turn.chars));
        }
        if output == 0 {
            return;
        }
        let duration = turn
            .reply_started
            .map_or(0., |start| start.elapsed().as_secs_f32());
        let ttft = match (turn.reply_started, turn.first) {
            (Some(start), Some(first)) => first.saturating_duration_since(start).as_secs_f32(),
            _ => duration,
        };
        let speed = if duration > ttft {
            output as f32 / (duration - ttft).max(0.05)
        } else {
            0.
        };
        self.stats.tin = self.stats.tin.saturating_add(turn.input);
        self.stats.tout = self.stats.tout.saturating_add(output);
        self.stats.turns = self.stats.turns.saturating_add(1);
        self.stats.speed = speed;
        self.stats.lat = duration;
        self.stats.ttft = ttft;
        self.stats.last = Some(Receipt {
            input: turn.input,
            output,
            duration,
            speed,
            ttft,
        });
    }

    fn menu(&self) -> Vec<(&'static str, &'static str)> {
        menu_items(&self.editor.text(), self.menu_closed, self.nav.is_some())
    }

    fn context(&self) -> u32 {
        self.cache.as_ref().map_or(0, |cache| estimate(cache.chars))
    }

    fn draw(
        &mut self,
        history: &[Message],
        pending: Option<&str>,
        model: &str,
        directory: &Path,
    ) -> Result<()> {
        let (terminal_width, terminal_height) = terminal::size()?;
        let width = usize::from(terminal_width);
        let height = usize::from(terminal_height);
        let mut out = io::stdout();
        queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
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
            out.flush()?;
            return Ok(());
        }
        let content = width.saturating_sub(4).clamp(40, MAX_CONTENT);
        let margin = (width - content) / 2;
        let show_mark = content >= 70 && height >= 26;
        let header_height = if show_mark { 10 } else { 5 };
        self.header(&mut out, margin, content, model, directory, show_mark)?;

        let streaming = pending.is_some();
        let menu = if streaming { Vec::new() } else { self.menu() };
        let mut menu_rows = if streaming {
            1
        } else {
            menu.len().clamp(1, MENU_LIMIT)
        };
        menu_rows = menu_rows.min(height.saturating_sub(header_height + 7).max(1));
        let mut receipt_rows = usize::from(!streaming && self.stats.last.is_some());
        let mut notice_rows = usize::from(!self.notice.is_empty());
        let mut gap_rows = 1usize;
        let (conv_rows, receipt_y, notice_y, box_top) = loop {
            let top = height.saturating_sub(3 + menu_rows);
            let gap = gap_rows.min(top.saturating_sub(header_height + 2));
            let extras_end = top.saturating_sub(gap);
            let notice = if notice_rows > 0 {
                Some(extras_end.saturating_sub(notice_rows))
            } else {
                None
            };
            let receipt = if receipt_rows > 0 {
                Some(notice.unwrap_or(extras_end).saturating_sub(receipt_rows))
            } else {
                None
            };
            let conv_end = receipt.or(notice).unwrap_or(extras_end);
            let rows = conv_end.saturating_sub(header_height);
            if rows >= 2 {
                break (rows, receipt, notice, top);
            }
            if notice_rows > 0 {
                notice_rows = 0;
                continue;
            }
            if receipt_rows > 0 {
                receipt_rows = 0;
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
            break (rows, receipt, notice, top);
        };

        if conv_rows > 0 {
            let key = CacheKey {
                messages: history.len(),
                bytes: history.iter().map(|message| message.content.len()).sum(),
                details: self.details,
                width: content,
            };
            if self.cache.as_ref().map(|cache| cache.key) != Some(key) {
                let lines = history_lines(history, content, self.details);
                let chars = history
                    .iter()
                    .map(|message| message.content.chars().count())
                    .sum();
                self.cache = Some(Cache { key, lines, chars });
            }
            let pending_lines = pending
                .map(|text| render::assistant(text, content))
                .unwrap_or_default();
            let total =
                self.cache.as_ref().map_or(0, |cache| cache.lines.len()) + pending_lines.len();
            let max_scroll = total.saturating_sub(conv_rows);
            let scroll = self.scroll.min(max_scroll);
            self.scroll = scroll;
            let end = total - scroll;
            let start = end.saturating_sub(conv_rows);
            if let Some(cache) = &self.cache {
                for (index, line) in cache
                    .lines
                    .iter()
                    .chain(pending_lines.iter())
                    .enumerate()
                    .skip(start)
                    .take(conv_rows)
                {
                    put_line(
                        &mut out,
                        margin,
                        header_height + index - start,
                        line,
                        content,
                    )?;
                }
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
            let mut line = Line::of(format!("  {}", self.notice), Style::dim());
            if line.width() > content {
                line = line.truncate(content.saturating_sub(1));
                line.push("…", Style::dim());
            }
            put_line(&mut out, margin, y, &line, content)?;
        }
        if let Some(y) = receipt_y
            && let Some(receipt) = self.stats.last
        {
            put_line(
                &mut out,
                margin,
                y,
                &receipt_line(receipt, content),
                content,
            )?;
        }

        let inner = content.saturating_sub(6);
        let (text, caret) = if streaming {
            (
                render::sanitize(&self.queued)
                    .chars()
                    .take(inner)
                    .collect::<String>(),
                0,
            )
        } else {
            self.editor.view(inner)
        };
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

        if streaming {
            put_line(
                &mut out,
                margin,
                box_top + 3,
                &self.spinner_line(content),
                content,
            )?;
        } else if menu.is_empty() {
            put_line(
                &mut out,
                margin,
                box_top + 3,
                &self.status_line(model, content),
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

        if streaming {
            queue!(out, Hide)?;
        } else {
            let column = margin + 4 + caret;
            queue!(out, MoveTo(column as u16, (box_top + 1) as u16), Show)?;
        }
        out.flush()?;
        Ok(())
    }

    fn header(
        &self,
        out: &mut io::Stdout,
        margin: usize,
        content: usize,
        model: &str,
        directory: &Path,
        mark: bool,
    ) -> io::Result<()> {
        let path = render::tail(&crate::folders::display(directory), content);
        let hint = tr!(
            "/ команды · esc прервать · ctrl-c выйти",
            "/ commands · esc to interrupt · ctrl-c to quit"
        );
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
            format!(" · {} context", render::human(self.context_limit)),
            Style::dim(),
        );
        if self.diagnose {
            line.push(
                tr!(" · диагностика", " · diagnose"),
                Style::fg(render::BRAND),
            );
        }
        line
    }

    fn status_line(&self, model: &str, width: usize) -> Line {
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
        let context = self.context();
        let percent = context.saturating_mul(100) / self.context_limit;
        let mut segments: Vec<Line> = Vec::new();
        let mut head = Line::new();
        head.push("  ", Style::plain());
        head.push("● ", Style::brand());
        head.push(model, Style::fg(render::FG));
        segments.push(head);
        segments.push(Line::of(
            format!(
                "ctx {}/{} ({}%)",
                render::human(context),
                render::human(self.context_limit),
                percent
            ),
            Style::dim(),
        ));
        if self.stats.tin > 0 || self.stats.tout > 0 {
            segments.push(Line::of(
                format!(
                    "↑{} ↓{}",
                    render::human(self.stats.tin),
                    render::human(self.stats.tout)
                ),
                Style::dim(),
            ));
        }
        if self.stats.speed > 0. {
            segments.push(Line::of(
                format!("{:.0} tok/s", self.stats.speed),
                Style::dim(),
            ));
        }
        if self.stats.lat > 0. {
            segments.push(Line::of(format!("{:.1}s", self.stats.lat), Style::dim()));
        }
        if self.scroll > 0 {
            segments.push(Line::of(format!("↑{}", self.scroll), Style::dim()));
        }
        segments.push(Line::of(
            format!("session {}", render::dur(self.started.elapsed().as_secs())),
            Style::dim(),
        ));
        join_segments(&segments, width)
    }

    fn spinner_line(&self, width: usize) -> Line {
        let frame = render::SPIN[(self.tick / 2) % render::SPIN.len()];
        let Some(turn) = self.turn else {
            return Line::of("  Thinking…", Style::brand());
        };
        let elapsed = turn
            .reply_started
            .unwrap_or(turn.started)
            .elapsed()
            .as_secs_f32();
        let tokens = estimate(turn.chars);
        let stage = if turn.first.is_some() {
            tr!("Writing", "Writing")
        } else {
            tr!("Thinking", "Thinking")
        };
        if width < 58 {
            let mut line = Line::new();
            line.push("  ", Style::plain());
            line.push(frame, Style::brand());
            line.push(
                format!(" {stage}… {elapsed:.0}s · ↓ {tokens} · esc"),
                Style::dim(),
            );
            return line.truncate(width);
        }
        let rate = match turn.first {
            Some(first) => {
                let since = first.elapsed().as_secs_f32();
                if since > 0.4 && tokens > 0 {
                    format!(" · {:.0} tok/s", tokens as f32 / since)
                } else {
                    String::new()
                }
            }
            None => String::new(),
        };
        let mut line = Line::new();
        line.push("  ", Style::plain());
        line.push(frame, Style::brand());
        line.push(format!(" {stage}…"), Style::brand());
        line.push(
            trf!(
                " ({elapsed:.0}s · ↓ {tokens} tokens{rate} · esc — прервать)",
                " ({elapsed:.0}s · ↓ {tokens} tokens{rate} · esc to interrupt)"
            ),
            Style::dim(),
        );
        line
    }

    fn status_panel(&mut self, model: &str, directory: &Path) -> Result<()> {
        let context = self.context();
        let percent = context.saturating_mul(100) / self.context_limit;
        let mode = if self.diagnose {
            tr!("диагностика", "diagnose")
        } else {
            tr!("кодинг", "code")
        };
        let rows = [
            (tr!("модель", "model"), model.to_owned()),
            (
                tr!("контекст", "context"),
                format!(
                    "{} / {} ({}%)",
                    render::human(context),
                    render::human(self.context_limit),
                    percent
                ),
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
                format!(
                    "{:.0} tok/s ({})",
                    self.stats.speed,
                    tr!("последний ответ", "last reply")
                ),
            ),
            (
                tr!("последний", "last"),
                format!(
                    "{:.1}s · {} {:.1}s",
                    self.stats.lat,
                    tr!("первый токен", "first token"),
                    self.stats.ttft
                ),
            ),
            (
                tr!("сеанс", "session"),
                format!(
                    "{} · {}",
                    render::dur(self.started.elapsed().as_secs()),
                    trf!("{} запросов", "{} turns", self.stats.turns)
                ),
            ),
            (tr!("режим", "mode"), mode.to_owned()),
            (tr!("проект", "project"), crate::folders::display(directory)),
        ];
        let mut body = Vec::new();
        for (key, value) in rows {
            let mut line = Line::new();
            line.push(format!("{key:<10}"), Style::dim());
            line.push(value, Style::fg(render::FG));
            body.push(line);
        }
        self.overlay(tr!("Состояние сеанса", "Session status"), body)
    }

    fn overlay(&mut self, title: &str, body: Vec<Line>) -> Result<()> {
        let mut scroll = 0usize;
        loop {
            let (terminal_width, terminal_height) = terminal::size()?;
            let width = usize::from(terminal_width);
            let height = usize::from(terminal_height);
            let panel_width = width.saturating_sub(8).clamp(36, 96).min(width);
            let inner = panel_width.saturating_sub(4);
            let panel_height = (body.len() + 2).min(height.saturating_sub(2)).max(4);
            let visible = panel_height.saturating_sub(2);
            scroll = scroll.min(body.len().saturating_sub(visible));
            let left = width.saturating_sub(panel_width) / 2;
            let top = height.saturating_sub(panel_height) / 2;
            let mut out = io::stdout();
            queue!(out, MoveTo(0, 0), Clear(ClearType::All), Hide)?;
            let border = Style::fg(render::LINE);
            let title_text = format!(" {title} ");
            let mut first = Line::new();
            first.push("╭─", border);
            first.push(title_text.clone(), Style::brand().bold());
            let used = 3 + title_text.as_str().width();
            first.push("─".repeat(panel_width.saturating_sub(used + 1)), border);
            first.push("╮", border);
            put_line(&mut out, left, top, &first, panel_width)?;
            for row in 0..visible {
                let mut line = Line::new();
                line.push("│ ", border);
                if let Some(source) = body.get(scroll + row) {
                    line.extend(source.truncate(inner));
                }
                line.pad_to(panel_width.saturating_sub(2), Style::plain());
                line.push(" │", border);
                put_line(&mut out, left, top + 1 + row, &line, panel_width)?;
            }
            let hint = tr!(" Esc / Enter — закрыть ", " Esc / Enter — close ");
            let mut last = Line::new();
            last.push("╰─", border);
            last.push(hint, Style::dim());
            let used = 3 + hint.width();
            last.push("─".repeat(panel_width.saturating_sub(used + 1)), border);
            last.push("╯", border);
            put_line(&mut out, left, top + 1 + visible, &last, panel_width)?;
            out.flush()?;
            match event::read()? {
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
                _ => {}
            }
        }
    }
}

/// Border geometry of a centered panel, enough for cursor placement.
struct Panel {
    visible: usize,
}

impl Screen {
    fn draw_panel(
        &self,
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
        let top = height.saturating_sub(panel_height) / 2;
        let mut out = io::stdout();
        queue!(out, MoveTo(0, 0), Clear(ClearType::All), Hide)?;
        let border = Style::fg(render::LINE);
        let title_text = format!(" {title} ");
        let mut first = Line::new();
        first.push("╭─", border);
        first.push(title_text.clone(), Style::brand().bold());
        let used = 3 + title_text.as_str().width();
        first.push("─".repeat(panel_width.saturating_sub(used + 1)), border);
        first.push("╮", border);
        put_line(&mut out, left, top, &first, panel_width)?;
        for row in 0..visible {
            let mut line = Line::new();
            line.push("│ ", border);
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
            put_line(&mut out, left, top + 1 + row, &line, panel_width)?;
        }
        let footer: String = footer.chars().take(panel_width.saturating_sub(7)).collect();
        let hint = format!(" {footer} ");
        let mut last = Line::new();
        last.push("╰─", border);
        last.push(hint.clone(), Style::dim());
        let used = 3 + hint.as_str().width();
        last.push("─".repeat(panel_width.saturating_sub(used + 1)), border);
        last.push("╯", border);
        put_line(&mut out, left, top + 1 + visible, &last, panel_width)?;
        if let Some((row, column)) = cursor
            && row < visible
        {
            queue!(
                out,
                MoveTo(
                    (left + 2 + column.min(inner.saturating_sub(1))) as u16,
                    (top + 1 + row) as u16
                ),
                Show
            )?;
        }
        out.flush()?;
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
            match event::read()? {
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
    ChatOnly,
}

impl Field {
    const ALL: [Self; 7] = [
        Self::Name,
        Self::Protocol,
        Self::Url,
        Self::Model,
        Self::Key,
        Self::Context,
        Self::ChatOnly,
    ];
}

struct ModelForm {
    index: Option<usize>,
    name: Editor,
    url: Editor,
    model: Editor,
    key: Editor,
    context: Editor,
    anthropic: bool,
    chat_only: bool,
    key_set: bool,
    focus: usize,
    error: Option<String>,
}

impl ModelForm {
    fn new(index: Option<usize>, entry: Option<&ModelEntry>) -> Self {
        let field = |value: &str| Editor::from(value);
        Self {
            index,
            name: field(entry.map(|entry| entry.name.as_str()).unwrap_or("")),
            url: field(entry.map(|entry| entry.url.as_str()).unwrap_or("")),
            model: field(entry.map(|entry| entry.model.as_str()).unwrap_or("")),
            key: Editor::new(),
            context: field(
                &entry
                    .map(|entry| entry.context.max(1_000).to_string())
                    .unwrap_or_else(|| CTX_TOKENS.to_string()),
            ),
            anthropic: entry.map(|entry| entry.anthropic).unwrap_or(false),
            chat_only: entry.map(|entry| entry.chat_only).unwrap_or(false),
            key_set: entry.map(|entry| entry.key_set).unwrap_or(false),
            focus: 0,
            error: None,
        }
    }

    fn field(&self) -> Field {
        Field::ALL[self.focus.min(Field::ALL.len() - 1)]
    }

    fn editor_mut(&mut self) -> Option<&mut Editor> {
        match self.field() {
            Field::Name => Some(&mut self.name),
            Field::Url => Some(&mut self.url),
            Field::Model => Some(&mut self.model),
            Field::Key => Some(&mut self.key),
            Field::Context => Some(&mut self.context),
            Field::Protocol | Field::ChatOnly => None,
        }
    }

    fn move_focus(&mut self, delta: isize) {
        let count = Field::ALL.len() as isize;
        self.focus = ((self.focus as isize + delta).rem_euclid(count)) as usize;
    }

    fn entry(&self) -> std::result::Result<ModelEntry, String> {
        let name = self.name.text().trim().to_owned();
        if name.is_empty() {
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
        if model.is_empty() {
            return Err(tr!("Укажи ID модели", "Enter the model ID").to_owned());
        }
        if model.chars().count() > 160 {
            return Err(tr!(
                "ID модели длиннее 160 символов",
                "The model ID is longer than 160 characters"
            )
            .to_owned());
        }
        let context: u32 =
            self.context.text().trim().parse().map_err(|_| {
                tr!("Контекст — целое число", "Context must be an integer").to_owned()
            })?;
        if !(1_000..=10_000_000).contains(&context) {
            return Err(tr!(
                "Контекст: от 1000 до 10 000 000 токенов",
                "Context: 1000 to 10,000,000 tokens"
            )
            .to_owned());
        }
        let key = self.key.text();
        Ok(ModelEntry {
            name,
            url,
            model,
            key: (!key.is_empty()).then_some(key),
            key_set: self.key_set || !self.key.is_empty(),
            anthropic: self.anthropic,
            chat_only: self.chat_only,
            context,
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
                if self.key_set {
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
        } else if self.key_set {
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
                "  Нажми a, чтобы добавить первую.",
                "  Press a to add the first one."
            ),
            Style::dim(),
        ));
        return lines;
    }
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
        detail.push(format!(" · {}", render::human(entry.context)), Style::dim());
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
        let (terminal_width, terminal_height) = terminal::size()?;
        let width = usize::from(terminal_width);
        let height = usize::from(terminal_height);
        let mut out = io::stdout();
        queue!(out, MoveTo(0, 0), Clear(ClearType::All), Hide)?;
        let line = Line::of(text, Style::brand());
        put_line(&mut out, 2, height / 2, &line, width.saturating_sub(4))?;
        out.flush()?;
        Ok(())
    }

    /// The `/model` screen: list, connect, add, edit and delete saved model
    /// connections without leaving the alternate screen.
    pub fn models(&mut self, entries: &[ModelEntry], active: Option<usize>) -> Result<ModelAction> {
        let mut selected = active.unwrap_or(0).min(entries.len().saturating_sub(1));
        let mut form: Option<ModelForm> = None;
        let mut error: Option<String> = None;
        loop {
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
                    tr!(
                        " Tab/↑↓ — поле · Space/←→ — тумблер · Enter — сохранить · Esc — отмена ",
                        " Tab/↑↓ — field · Space/←→ — toggle · Enter — save · Esc — cancel "
                    ),
                    lines,
                    cursor,
                )
            } else {
                (
                    tr!("Модели", "Models"),
                    tr!(
                        " ↑↓ · Enter подключить · a новая · e правка · d удалить · Esc ",
                        " ↑↓ · Enter connect · a new · e edit · d delete · Esc "
                    ),
                    model_lines(entries, selected, active, error.as_deref()),
                    None,
                )
            };
            self.draw_panel(title, footer, &body, 0, cursor)?;
            match event::read()? {
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
                            KeyCode::Enter => match form.entry() {
                                Ok(entry) => {
                                    return Ok(ModelAction::Save {
                                        index: form.index,
                                        entry,
                                    });
                                }
                                Err(message) => form.error = Some(message),
                            },
                            KeyCode::Left => {
                                if form.field() == Field::Protocol {
                                    form.anthropic = !form.anthropic;
                                } else if let Some(editor) = form.editor_mut() {
                                    editor.left();
                                }
                            }
                            KeyCode::Right => {
                                if form.field() == Field::Protocol {
                                    form.anthropic = !form.anthropic;
                                } else if let Some(editor) = form.editor_mut() {
                                    editor.right();
                                }
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
                                let field = form.field();
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
                                if field == Field::Context {
                                    form.context
                                        .buffer
                                        .retain(|character| character.is_ascii_digit());
                                }
                            }
                            KeyCode::Char(character)
                                if key.modifiers == KeyModifiers::NONE
                                    || key.modifiers == KeyModifiers::SHIFT =>
                            {
                                let field = form.field();
                                if field == Field::Context && !character.is_ascii_digit() {
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
                        let filtered: String = if form.field() == Field::Context {
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
    pub fn review(&mut self, title: &str, text: &str) -> Result<bool> {
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
        let mut scroll = 0usize;
        loop {
            let panel = self.draw_panel(
                title,
                tr!(
                    " ↑↓ · PgUp/PgDn — прокрутка · Enter — создать · Esc — отмена ",
                    " ↑↓ · PgUp/PgDn — scroll · Enter — create · Esc — cancel "
                ),
                &lines,
                scroll,
                None,
            )?;
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let key = normalize_key(key);
                    match key.code {
                        KeyCode::Enter => return Ok(true),
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
                match event::read()? {
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
            match event::read()? {
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
            match event::read()? {
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
        if self.suspended {
            return;
        }
        let _ = execute!(
            io::stdout(),
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
        EnableBracketedPaste
    )
}

fn put_line(out: &mut io::Stdout, x: usize, y: usize, line: &Line, width: usize) -> io::Result<()> {
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

fn apply_style(out: &mut io::Stdout, style: Style, current: &mut Style) -> io::Result<()> {
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
    line.push(
        format!(
            "↑ ~{} ↓ ~{} tokens",
            render::human(receipt.input),
            render::human(receipt.output)
        ),
        Style::dim(),
    );
    let extras = [
        format!("{:.1}s", receipt.duration),
        format!("{:.0} tok/s", receipt.speed),
        format!(
            "{} {:.1}s",
            tr!("первый токен", "first token"),
            receipt.ttft
        ),
    ];
    for extra in extras {
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

fn history_lines(history: &[Message], width: usize, details: bool) -> Vec<Line> {
    let mut lines = Vec::new();
    for message in history {
        if message.role == "assistant" {
            if let Some(action) = super::parse_action(&message.content) {
                lines.push(action_line(&action));
            } else {
                lines.extend(render::assistant(&message.content, width));
            }
        } else if message.content.starts_with("Tool result") {
            lines.extend(tool_lines(&message.content, width, details));
        } else {
            lines.extend(render::user(&message.content, width));
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
            "остановить ответ, очистить ввод или выйти",
            "stop the reply, clear the input or leave"
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
        "PgUp/PgDn",
        tr!("прокрутить историю", "scroll history"),
    ));
    lines.push(row(
        "Ctrl-C",
        tr!(
            "очистить ввод; дважды — выйти",
            "clear the input; twice to exit"
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
        assert_eq!(menu_items("/", false, false).len(), 15);
        let filtered = menu_items("/st", false, false);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].0, "/status");
        assert!(menu_items("/", true, false).is_empty());
        assert!(menu_items("/", false, true).is_empty());
        assert!(menu_items("/help me", false, false).is_empty());
        assert!(menu_items("hello", false, false).is_empty());
    }

    #[test]
    fn tool_turns_count_every_streamed_reply() {
        let mut screen = Screen {
            editor: Editor::new(),
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
            started: Instant::now(),
            cache: None,
            context_limit: CTX_TOKENS,
            suspended: true,
        };
        screen.begin_turn("prompt");
        screen.track("первый ответ");
        screen.track("");
        screen.track("второй ответ");
        screen.finish_turn();
        let receipt = screen.stats.last.expect("receipt");
        assert_eq!(
            receipt.output,
            estimate("первый ответ".chars().count()) + estimate("второй ответ".chars().count())
        );
        assert_eq!(screen.stats.turns, 1);
    }

    #[test]
    fn tool_details_follow_the_f2_toggle() {
        let message = Message {
            role: "user".into(),
            content: "Tool result (untrusted file content)\nfn agent() {}".into(),
        };
        let compact = history_lines(std::slice::from_ref(&message), 60, false);
        let full = history_lines(std::slice::from_ref(&message), 60, true);
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
    fn pasted_newlines_become_spaces() {
        assert_eq!(paste_text("a\r\nb\tc\x07"), "a b c");
    }

    #[test]
    fn token_estimates_follow_the_design() {
        assert_eq!(estimate(0), 0);
        assert_eq!(estimate(1), 1);
        assert_eq!(estimate(4), 1);
        assert_eq!(estimate(5), 2);
    }
}
