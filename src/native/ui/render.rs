//! Presentation for the built-in agent: palette, styled lines, word wrap,
//! markdown-ish blocks and the CarryCode mark.
//!
//! The layout follows the agent console design: a green mark in the header,
//! a boxed prompt, a status line and shaded code blocks. Everything here is
//! pure so it can be tested without a terminal.

use crossterm::style::Color;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const BRAND: Color = Color::Rgb {
    r: 181,
    g: 214,
    b: 178,
};
pub const FG: Color = Color::Rgb {
    r: 236,
    g: 240,
    b: 234,
};
pub const DIM: Color = Color::Rgb {
    r: 124,
    g: 138,
    b: 130,
};
pub const LINE: Color = Color::Rgb {
    r: 74,
    g: 94,
    b: 80,
};
pub const CODE: Color = Color::Rgb {
    r: 30,
    g: 37,
    b: 36,
};
pub const CODEFG: Color = Color::Rgb {
    r: 200,
    g: 214,
    b: 204,
};
/// Muted amber for warnings and declined actions; stays inside the sage palette.
pub const WARN: Color = Color::Rgb {
    r: 214,
    g: 168,
    b: 120,
};
pub const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const MARK: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
}

impl Style {
    pub const fn plain() -> Self {
        Self {
            fg: None,
            bg: None,
            bold: false,
        }
    }

    pub const fn fg(color: Color) -> Self {
        Self {
            fg: Some(color),
            bg: None,
            bold: false,
        }
    }

    pub const fn dim() -> Self {
        Self::fg(DIM)
    }

    pub const fn brand() -> Self {
        Self::fg(BRAND)
    }

    pub fn with_bg(self, color: Color) -> Self {
        Self {
            bg: Some(color),
            ..self
        }
    }

    pub fn with_fg(self, color: Color) -> Self {
        Self {
            fg: Some(color),
            ..self
        }
    }

    pub fn bold(self) -> Self {
        Self { bold: true, ..self }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Line {
    spans: Vec<Span>,
}

impl Line {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn of(text: impl Into<String>, style: Style) -> Self {
        let mut line = Self::new();
        line.push(text, style);
        line
    }

    pub fn push(&mut self, text: impl Into<String>, style: Style) {
        let text: String = text
            .into()
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.spans.last_mut()
            && last.style == style
        {
            last.text.push_str(&text);
            return;
        }
        self.spans.push(Span { text, style });
    }

    pub fn extend(&mut self, other: Self) {
        for span in other.spans {
            self.spans.push(span);
        }
        self.compact();
    }

    pub fn width(&self) -> usize {
        self.spans
            .iter()
            .map(|span| span.text.as_str().width())
            .sum()
    }

    pub fn is_blank(&self) -> bool {
        self.width() == 0
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    #[cfg(test)]
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    pub fn map_style(mut self, map: impl Fn(Style) -> Style) -> Self {
        for span in &mut self.spans {
            span.style = map(span.style);
        }
        self
    }

    pub fn truncate(&self, width: usize) -> Self {
        let mut line = Self::new();
        let mut used = 0;
        for span in &self.spans {
            if used >= width {
                break;
            }
            let mut text = String::new();
            for character in span.text.chars() {
                let cell = character.width().unwrap_or(0);
                if used + cell > width {
                    break;
                }
                text.push(character);
                used += cell;
            }
            if !text.is_empty() {
                line.spans.push(Span {
                    text,
                    style: span.style,
                });
            }
        }
        line.compact();
        line
    }

    pub fn pad_to(&mut self, width: usize, style: Style) {
        let used = self.width();
        if used < width {
            self.push(" ".repeat(width - used), style);
        }
    }

    fn compact(&mut self) {
        let mut spans: Vec<Span> = Vec::with_capacity(self.spans.len());
        for span in self.spans.drain(..) {
            if span.text.is_empty() {
                continue;
            }
            if let Some(last) = spans.last_mut()
                && last.style == span.style
            {
                last.text.push_str(&span.text);
                continue;
            }
            spans.push(span);
        }
        self.spans = spans;
    }
}

/// Greedy word wrap that keeps span styles and never exceeds `width` cells.
pub fn wrap(line: &Line, width: usize) -> Vec<Line> {
    if width == 0 {
        return Vec::new();
    }
    let mut flow = Flow {
        width,
        rows: Vec::new(),
        current: Line::new(),
        used: 0,
        space: false,
    };
    let mut word: Vec<(char, Style)> = Vec::new();
    for span in &line.spans {
        for character in span.text.chars() {
            if character.is_control() {
                continue;
            }
            if character == ' ' || character == '\t' {
                if !word.is_empty() {
                    flow.word(&word);
                    word.clear();
                }
                flow.space = true;
            } else {
                word.push((character, span.style));
            }
        }
    }
    if !word.is_empty() {
        flow.word(&word);
    }
    flow.finish()
}

struct Flow {
    width: usize,
    rows: Vec<Line>,
    current: Line,
    used: usize,
    space: bool,
}

impl Flow {
    fn break_row(&mut self) {
        if !self.current.is_blank() {
            let line = std::mem::take(&mut self.current);
            self.rows.push(line);
        }
        self.used = 0;
        self.space = false;
    }

    fn word(&mut self, word: &[(char, Style)]) {
        let cells: usize = word.iter().map(|(c, _)| c.width().unwrap_or(0)).sum();
        let gap = usize::from(self.space && !self.current.is_blank());
        if self.used + gap + cells > self.width && !self.current.is_blank() {
            self.break_row();
        } else if gap == 1 {
            self.current.push(" ", Style::plain());
            self.used += 1;
        }
        self.space = false;
        if cells <= self.width {
            for (character, style) in word {
                self.current.push(character.to_string(), *style);
            }
            self.used += cells;
            return;
        }
        for (character, style) in word {
            let cell = character.width().unwrap_or(0);
            if self.used + cell > self.width && !self.current.is_blank() {
                self.break_row();
            }
            if cell <= self.width {
                self.current.push(character.to_string(), *style);
                self.used += cell;
            }
        }
    }

    fn finish(mut self) -> Vec<Line> {
        if !self.current.is_blank() {
            self.rows.push(self.current);
        }
        self.rows
    }
}

/// Echo of a submitted prompt: a green glyph and the text in the foreground.
pub fn user(text: &str, width: usize) -> Vec<Line> {
    let mut rows = Vec::new();
    let body = Line::of(text, Style::fg(FG));
    block(&mut rows, Line::of("› ", Style::brand()), body, width);
    rows
}

/// Assistant reply: a green bullet for the first paragraph, shaded code blocks.
pub fn assistant(text: &str, width: usize) -> Vec<Line> {
    let mut rows = Vec::new();
    let mut code = false;
    let mut bullet = true;
    for raw in text.split('\n') {
        let raw = raw.replace('\t', "    ");
        let trimmed = raw.trim_end().trim_start();
        if trimmed.starts_with("```") {
            if code {
                rows.push(code_row("", width, Style::dim()));
                code = false;
            } else {
                let language: String = trimmed
                    .trim_start_matches('`')
                    .trim()
                    .chars()
                    .take(32)
                    .collect();
                rows.push(code_row(&language, width, Style::dim()));
                code = true;
            }
            continue;
        }
        if code {
            rows.push(code_row(&raw, width, Style::fg(CODEFG)));
            continue;
        }
        if trimmed.is_empty() {
            rows.push(Line::new());
            continue;
        }
        let first = bullet;
        bullet = false;
        if let Some(item) = list_item(trimmed) {
            block(
                &mut rows,
                Line::of("  • ", Style::dim()),
                inline(item),
                width,
            );
        } else if let Some(heading) = heading(trimmed) {
            block(
                &mut rows,
                Line::of("  ", Style::plain()),
                inline(heading).map_style(|style| style.bold().with_fg(FG)),
                width,
            );
        } else {
            let prefix = if first {
                Line::of("● ", Style::brand())
            } else {
                Line::of("  ", Style::plain())
            };
            block(&mut rows, prefix, inline(trimmed), width);
        }
    }
    if code {
        rows.push(code_row("", width, Style::dim()));
    }
    while rows.last().is_some_and(Line::is_blank) {
        rows.pop();
    }
    rows
}

fn code_row(text: &str, width: usize, text_style: Style) -> Line {
    let base = Style::plain().with_bg(CODE);
    let mut line = Line::new();
    line.push(" ", base);
    line.push(text, text_style.with_bg(CODE));
    line.pad_to(width, base);
    line.truncate(width)
}

fn block(rows: &mut Vec<Line>, prefix: Line, body: Line, width: usize) {
    let indent = prefix.width();
    if body.is_blank() || indent >= width {
        rows.push(prefix);
        return;
    }
    let mut wrapped = wrap(&body, width - indent);
    if wrapped.is_empty() {
        wrapped.push(Line::new());
    }
    for (index, row) in wrapped.into_iter().enumerate() {
        let mut line = if index == 0 {
            prefix.clone()
        } else {
            Line::of(" ".repeat(indent), Style::plain())
        };
        line.extend(row);
        rows.push(line);
    }
}

fn heading(text: &str) -> Option<&str> {
    let hashes = text
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &text[hashes..];
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix(' ').map(str::trim)
    }
}

fn list_item(text: &str) -> Option<&str> {
    ["- ", "* ", "+ "]
        .into_iter()
        .find_map(|marker| text.strip_prefix(marker))
        .map(str::trim_start)
}

/// Inline markdown: `code`, **bold**, [links](url) and plain text.
pub fn inline(text: &str) -> Line {
    let characters: Vec<char> = text
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let mut line = Line::new();
    let mut buffer = String::new();
    let mut index = 0;
    while index < characters.len() {
        match characters[index] {
            '`' => {
                if let Some(end) = find(&characters, index + 1, '`') {
                    flush(&mut line, &mut buffer);
                    line.push(
                        characters[index + 1..end].iter().collect::<String>(),
                        Style::fg(CODEFG).with_bg(CODE),
                    );
                    index = end + 1;
                } else {
                    buffer.push('`');
                    index += 1;
                }
            }
            '*' if characters.get(index + 1) == Some(&'*') => {
                if let Some(end) = find_pair(&characters, index + 2) {
                    flush(&mut line, &mut buffer);
                    line.push(
                        characters[index + 2..end].iter().collect::<String>(),
                        Style::fg(FG).bold(),
                    );
                    index = end + 2;
                } else {
                    buffer.push('*');
                    index += 1;
                }
            }
            '[' => {
                if let Some((label, url, end)) = link(&characters, index) {
                    flush(&mut line, &mut buffer);
                    line.push(label, Style::fg(FG).bold());
                    if !url.is_empty() {
                        line.push(format!(" ({url})"), Style::dim());
                    }
                    index = end;
                } else {
                    buffer.push('[');
                    index += 1;
                }
            }
            character => {
                buffer.push(character);
                index += 1;
            }
        }
    }
    flush(&mut line, &mut buffer);
    line
}

fn flush(line: &mut Line, buffer: &mut String) {
    if !buffer.is_empty() {
        line.push(std::mem::take(buffer), Style::fg(FG));
    }
}

fn find(characters: &[char], from: usize, target: char) -> Option<usize> {
    characters
        .iter()
        .enumerate()
        .skip(from)
        .find(|(_, character)| **character == target)
        .map(|(index, _)| index)
}

fn find_pair(characters: &[char], from: usize) -> Option<usize> {
    let mut index = from;
    while index + 1 < characters.len() {
        if characters[index] == '*' && characters[index + 1] == '*' {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn link(characters: &[char], start: usize) -> Option<(String, String, usize)> {
    let close = find(characters, start + 1, ']')?;
    if characters.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = find(characters, close + 2, ')')?;
    let label: String = characters[start + 1..close].iter().collect();
    let url: String = characters[close + 2..end].iter().collect();
    if label.is_empty() {
        return None;
    }
    Some((label, url, end + 1))
}

/// The green mark from the launcher: three nodes joined by lines, drawn with
/// half blocks from an anti-aliased 16x16 pixel field.
pub fn logo() -> Vec<Line> {
    let mut rows = Vec::with_capacity(MARK / 2);
    for row in 0..MARK / 2 {
        let mut line = Line::new();
        let mut run = String::new();
        let mut current: Option<(Option<Color>, Option<Color>)> = None;
        for column in 0..MARK {
            let (foreground, background, glyph) = mark_cell(column, row);
            if current != Some((foreground, background)) {
                if let Some((fg, bg)) = current {
                    line.push(run.clone(), palette(fg, bg));
                    run.clear();
                }
                current = Some((foreground, background));
            }
            run.push(glyph);
        }
        if let Some((fg, bg)) = current {
            line.push(run, palette(fg, bg));
        }
        rows.push(line);
    }
    rows
}

fn palette(foreground: Option<Color>, background: Option<Color>) -> Style {
    Style {
        fg: foreground,
        bg: background,
        bold: false,
    }
}

fn mark_cell(column: usize, row: usize) -> (Option<Color>, Option<Color>, char) {
    match (mark_pixel(column, 2 * row), mark_pixel(column, 2 * row + 1)) {
        (None, None) => (None, None, ' '),
        (Some(top), None) => (Some(top), None, '▀'),
        (None, Some(bottom)) => (Some(bottom), None, '▄'),
        (Some(top), Some(bottom)) if top == bottom => (Some(top), None, '█'),
        (Some(top), Some(bottom)) => (Some(top), Some(bottom), '▀'),
    }
}

fn mark_pixel(column: usize, row: usize) -> Option<Color> {
    const SCALE: f32 = 1.2;
    let x = (column as f32 + 0.5 - MARK as f32 / 2.) / SCALE - 1.85;
    let y = (row as f32 + 0.5 - MARK as f32 / 2.) / SCALE + 1.5;
    let (a, b, d) = ((1.3, -2.1), (-5.9, 1.3), (1.9, 6.0));
    let node = |(nx, ny): (f32, f32), radius: f32| (x - nx).hypot(y - ny) - radius;
    let distance = segment(x, y, a, b)
        .min(segment(x, y, a, d))
        .min(node(a, 2.7))
        .min(node(b, 1.8))
        .min(node(d, 1.8))
        - 0.85;
    let coverage = (0.5 - distance * SCALE).clamp(0., 1.);
    (coverage >= 0.3).then(|| mix([30., 40., 34.], [181., 214., 178.], coverage))
}

fn segment(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py, dx, dy) = (x - a.0, y - a.1, b.0 - a.0, b.1 - a.1);
    let h = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0., 1.);
    (px - dx * h).hypot(py - dy * h)
}

fn mix(from: [f32; 3], to: [f32; 3], t: f32) -> Color {
    let channel = |index: usize| (from[index] + (to[index] - from[index]) * t) as u8;
    Color::Rgb {
        r: channel(0),
        g: channel(1),
        b: channel(2),
    }
}

pub fn human(tokens: u32) -> String {
    if tokens >= 1000 {
        format!("{:.1}k", tokens as f32 / 1000.)
    } else {
        tokens.to_string()
    }
}

pub fn dur(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

pub fn tail(text: &str, width: usize) -> String {
    let characters: Vec<char> = text.chars().collect();
    if characters.len() <= width {
        text.to_owned()
    } else if width <= 1 {
        "…".to_owned()
    } else {
        format!(
            "…{}",
            characters[characters.len() - width + 1..]
                .iter()
                .collect::<String>()
        )
    }
}

/// Drops terminal control characters from text that came from a model or a file.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_sequences_never_reach_the_screen() {
        let line = inline("hello\x1b[2Jworld");
        assert!(!line.plain_text().contains('\x1b'));
        assert!(
            assistant("a\x1b]0;title\x07b", 40)
                .iter()
                .all(|row| !row.plain_text().contains('\x1b'))
        );
    }

    #[test]
    fn wide_characters_wrap_within_the_width() {
        let rows = wrap(&Line::of("你好世界", Style::fg(FG)), 4);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.width() <= 4));
        assert_eq!(rows[0].plain_text(), "你好");
    }

    #[test]
    fn long_words_are_split_at_the_edge() {
        let rows = wrap(&Line::of("supercalifragilistic", Style::fg(FG)), 6);
        assert!(rows.iter().all(|row| row.width() <= 6));
        assert_eq!(
            rows.iter().map(|row| row.plain_text()).collect::<String>(),
            "supercalifragilistic"
        );
    }

    #[test]
    fn code_fences_become_shaded_rows() {
        let rows = assistant("answer\n```rust\nfn main() {}\n```\n", 40);
        assert!(
            rows.iter()
                .any(|row| row.spans.iter().any(|span| span.style.bg == Some(CODE)))
        );
        assert!(rows.iter().all(|row| row.width() <= 40));
        let shaded: String = rows
            .iter()
            .filter(|row| row.spans.iter().any(|span| span.style.bg == Some(CODE)))
            .map(Line::plain_text)
            .collect();
        assert!(shaded.contains("fn main"));
    }

    #[test]
    fn unclosed_fence_is_closed_at_the_end() {
        let rows = assistant("```\nlet x = 1;", 24);
        let last = rows.last().expect("rows");
        assert_eq!(last.width(), 24);
        assert!(last.spans.iter().all(|span| span.style.bg == Some(CODE)));
    }

    #[test]
    fn inline_markup_keeps_its_text() {
        let line = inline("use `cargo test` and **read** [docs](https://example.org)");
        let text = line.plain_text();
        assert!(text.contains("cargo test"));
        assert!(text.contains("read"));
        assert!(text.contains("https://example.org"));
        assert!(
            line.spans
                .iter()
                .any(|span| span.style.bg == Some(CODE) && span.text == "cargo test")
        );
    }

    #[test]
    fn bullet_marks_only_the_first_paragraph() {
        let rows = assistant("first\n\nsecond", 30);
        let bullets: Vec<_> = rows
            .iter()
            .filter(|row| row.plain_text().starts_with("● "))
            .collect();
        assert_eq!(bullets.len(), 1);
    }

    #[test]
    fn the_mark_is_sixteen_cells_wide() {
        let rows = logo();
        assert_eq!(rows.len(), 8);
        assert!(rows.iter().all(|row| row.width() == 16));
        assert!(rows.iter().all(|row| {
            row.plain_text()
                .chars()
                .all(|c| matches!(c, ' ' | '▀' | '▄' | '█'))
        }));
    }

    #[test]
    fn status_numbers_stay_short() {
        assert_eq!(human(102), "102");
        assert_eq!(human(128_000), "128.0k");
        assert_eq!(dur(59), "59s");
        assert_eq!(dur(3_725), "1h 02m");
        assert_eq!(tail("C:/very/long/path", 8), "…ng/path");
    }
}
