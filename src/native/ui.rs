use super::Message;
use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute, queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::{
    io::{self, Write},
    path::Path,
    time::Duration,
};
use unicode_width::UnicodeWidthChar;
pub(super) struct Screen {
    notice: String,
    scroll: usize,
    details: bool,
    queued: String,
    submit_queued: bool,
}
impl Screen {
    pub fn open() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let screen = Self {
            notice: String::new(),
            scroll: 0,
            details: false,
            queued: String::new(),
            submit_queued: false,
        };
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            Hide,
            EnableBracketedPaste
        )?;
        Ok(screen)
    }
    pub fn notice(&mut self, text: &str) {
        self.notice = text.into();
    }
    pub fn help(&mut self) -> Result<()> {
        let text = tr!(
            "Подключение и работа\n\n/provider   URL, модель и ключ провайдера\n/diagnose   Чтение проекта и поиск причины\n/code       Правки и команды после подтверждения\n/sessions   Сохранённые диалоги этого проекта\n/new        Начать новый сохраняемый диалог\n/temporary  Начать диалог без сохранения\n/clear      Удалить текущую историю\n/undo       Отменить последнюю правку\n/quit       Вернуться в меню\n\n@путь       Приложить текстовый файл\nF2          Показать или скрыть результаты инструментов\nPgUp/PgDn   Прокрутить историю\nEsc         Остановить запрос или вернуться\n\nEnter или Esc — закрыть справку",
            "Connection and workflow\n\n/provider   Provider URL, model and API key\n/diagnose   Read project files and investigate\n/code       Review edits and approve commands\n/sessions   Saved conversations for this project\n/new        Start a new saved conversation\n/temporary  Start without saving history\n/clear      Delete current conversation history\n/undo       Revert the last applied edit\n/quit       Return to the launcher\n\n@path       Attach a text file\nF2          Show or hide tool results\nPgUp/PgDn   Scroll through history\nEsc         Stop a request or go back\n\nEnter or Esc closes help"
        );
        loop {
            let (width, height) = terminal::size()?;
            let lines = wrap(text, usize::from(width.saturating_sub(2)).max(1));
            let mut out = io::stdout();
            queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
            for (index, line) in lines.iter().take(height as usize).enumerate() {
                queue!(out, MoveTo(0, index as u16), Print(line))?;
            }
            out.flush()?;
            if let Event::Key(key) = event::read()?
                && matches!(key.code, KeyCode::Enter | KeyCode::Esc)
            {
                break;
            }
        }
        Ok(())
    }
    fn draw(
        &self,
        history: &[Message],
        pending: &str,
        model: &str,
        directory: &Path,
        input: &str,
        diagnose: bool,
    ) -> Result<()> {
        let (width, height) = terminal::size()?;
        let width = usize::from(width.saturating_sub(2)).max(1);
        let mut out = io::stdout();
        queue!(
            out,
            MoveTo(0, 0),
            Clear(ClearType::All),
            SetForegroundColor(Color::Rgb {
                r: 75,
                g: 125,
                b: 80
            })
        )?;
        queue!(
            out,
            Print(clip(&format!(" ▪─▟ CarryCode Agent   {model}"), width)),
            ResetColor
        )?;
        queue!(
            out,
            MoveTo(0, 1),
            Print(clip(
                &format!(
                    " {}  {}",
                    crate::folders::display(directory),
                    if diagnose {
                        tr!("[Диагностика]", "[Diagnose]")
                    } else {
                        tr!("[Кодинг]", "[Code]")
                    }
                ),
                width
            ))
        )?;
        let full_width = width;
        let panel = if self.details {
            Some(panel_lines(history))
        } else {
            None
        };
        let width = if self.details && full_width >= 110 {
            full_width - 36
        } else {
            full_width
        };
        let mut lines = Vec::new();
        for message in history {
            if message.content.starts_with("Tool result")
                || super::parse_action(&message.content).is_some()
            {
                let summary = if let Some(action) = super::parse_action(&message.content) {
                    format!(
                        "  {} {}",
                        action["action"].as_str().unwrap_or(""),
                        action["path"].as_str().unwrap_or("")
                    )
                } else {
                    tr!(
                        "  Результат инструмента · F2 подробнее",
                        "  Tool result · F2 details"
                    )
                    .to_owned()
                };
                lines.extend(wrap(&summary, width));
                continue;
            }
            lines.push(format!(
                " {}",
                if message.role == "user" {
                    tr!("Ты", "You")
                } else {
                    "CarryCode"
                }
            ));
            lines.extend(wrap(&message.content, width));
            lines.push(String::new());
        }
        lines.extend(wrap(pending, width));
        let rows = usize::from(height.saturating_sub(7));
        if let Some(panel) = &panel {
            if full_width < 110 {
                lines = wrap(panel, full_width);
            } else {
                for (row, text) in wrap(panel, 32).iter().take(rows).enumerate() {
                    queue!(out, MoveTo((width + 3) as u16, row as u16 + 3), Print(text))?;
                }
            }
        }
        let end = lines.len().saturating_sub(self.scroll.min(lines.len()));
        for (row, line) in lines[..end]
            .iter()
            .skip(end.saturating_sub(rows))
            .enumerate()
        {
            queue!(out, MoveTo(0, row as u16 + 3), Print(line))?;
        }
        queue!(
            out,
            MoveTo(0, height.saturating_sub(3)),
            SetForegroundColor(Color::DarkGrey),
            Print(clip(&self.notice, width)),
            ResetColor,
            MoveTo(0, height.saturating_sub(2)),
            Print(clip(
                &format!(
                    "> {}",
                    input
                        .chars()
                        .rev()
                        .take(width.saturating_sub(2))
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>()
                ),
                width
            )),
            MoveTo(0, height.saturating_sub(1)),
            Print(clip(
                tr!(
                    "Enter отправить   Esc стоп/назад   F2 детали · PgUp/PgDn история   /help",
                    "Enter send   Esc stop/back   F2 details · PgUp/PgDn history   /help"
                ),
                width
            ))
        )?;
        out.flush()?;
        Ok(())
    }
    pub fn input(
        &mut self,
        history: &[Message],
        model: &str,
        directory: &Path,
        diagnose: bool,
    ) -> Result<Option<String>> {
        let mut input = std::mem::take(&mut self.queued);
        if std::mem::take(&mut self.submit_queued) && !input.trim().is_empty() {
            return Ok(Some(input));
        }
        loop {
            self.draw(history, "", model, directory, &input, diagnose)?;
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Enter if !input.trim().is_empty() => {
                        self.scroll = 0;
                        return Ok(Some(input));
                    }
                    KeyCode::Backspace => {
                        input.pop();
                    }
                    KeyCode::F(2) => self.details = !self.details,
                    KeyCode::PageUp => self.scroll += 10,
                    KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(None);
                    }
                    KeyCode::Char(c) if !c.is_control() && input.len() < 64 * 1024 => input.push(c),
                    _ => {}
                },
                Event::Paste(text) => {
                    for c in text.chars().filter(|c| !c.is_control()) {
                        if input.len() < 64 * 1024 {
                            input.push(c);
                        }
                    }
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
        self.draw(
            history,
            output,
            model,
            directory,
            tr!("Ожидаю модель…", "Waiting for model…"),
            false,
        )?;
        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if key.code == KeyCode::Esc
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL))
                    {
                        return Ok(true);
                    }
                    match key.code {
                        KeyCode::Char(c) if !c.is_control() && self.queued.len() < 64 * 1024 => {
                            self.queued.push(c)
                        }
                        KeyCode::Backspace => {
                            self.queued.pop();
                        }
                        KeyCode::Enter => self.submit_queued = true,
                        _ => {}
                    }
                }
                Event::Paste(text) => {
                    for c in text.chars().filter(|c| !c.is_control()) {
                        if self.queued.len() < 64 * 1024 {
                            self.queued.push(c);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            ResetColor,
            LeaveAlternateScreen,
            Show
        );
        let _ = terminal::disable_raw_mode();
    }
}
fn panel_lines(history: &[Message]) -> String {
    let mut plan = String::new();
    let mut files = Vec::new();
    for message in history {
        if let Some(action) = super::parse_action(&message.content) {
            if action["action"] == "plan" {
                plan = action["text"].as_str().unwrap_or("").to_owned();
            }
            if action["action"] == "edit"
                && let Some(path) = action["edit"]["path"].as_str()
                && !files.iter().any(|file| file == path)
            {
                files.push(path.to_owned());
            }
        }
    }
    let result = history
        .iter()
        .rev()
        .find(|message| message.content.starts_with("Tool result"))
        .map(|message| message.content.as_str())
        .unwrap_or("");
    format!(
        "{}\n{}\n\n{}\n{}\n\n{}\n{}",
        tr!("План", "Plan"),
        plan,
        tr!("Предложенные файлы", "Proposed files"),
        files.join("\n"),
        tr!("Последний результат", "Last result"),
        result
    )
}
fn clip(text: &str, width: usize) -> String {
    wrap(text, width).into_iter().next().unwrap_or_default()
}
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![];
    let mut line = String::new();
    let mut used = 0;
    for c in text.chars() {
        if c == '\n' {
            lines.push(std::mem::take(&mut line));
            used = 0;
            continue;
        }
        if c.is_control() {
            continue;
        }
        let cells = c.width().unwrap_or(0);
        if used + cells > width {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        if cells <= width {
            line.push(c);
            used += cells;
        }
    }
    lines.push(line);
    lines
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_output_strips_control_sequences_and_wraps_unicode() {
        assert!(!wrap("hello\x1b[2J", 80).join("").contains('\x1b'));
        assert_eq!(wrap("你好", 2), vec!["你", "好"]);
    }
}
