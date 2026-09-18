//! The pinned SQL editor: a `tui-textarea` with the submit rule, history
//! navigation and the box the shell draws around it.
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Block;
use sqlparser::dialect::GenericDialect;
use sqlparser::tokenizer::{Token, Tokenizer};
use tui_textarea::{CursorMove, Input, Key, TextArea};

pub enum EditorAction {
    None,
    Submit(String),
    Quit,
    Interrupt,
    Clear,
}

pub struct Editor {
    area: TextArea<'static>,
    history: Vec<String>,
    /// Index into `history` while browsing; `None` when editing a new statement.
    browsing: Option<usize>,
    draft: String,
}

fn blank_area() -> TextArea<'static> {
    let mut area = TextArea::default();
    area.set_cursor_line_style(Style::default());
    area.set_tab_length(4);
    area
}

impl Editor {
    pub fn new() -> Editor {
        Editor {
            area: blank_area(),
            history: Vec::new(),
            browsing: None,
            draft: String::new(),
        }
    }

    pub fn lines(&self) -> String {
        self.area.lines().join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.area.lines().iter().all(|line| line.trim().is_empty())
    }

    pub fn clear(&mut self) {
        self.area = blank_area();
        self.browsing = None;
    }

    pub fn set_text(&mut self, text: &str) {
        self.clear();
        self.area.insert_str(text);
        self.area.move_cursor(CursorMove::End);
    }

    pub fn set_history(&mut self, history: Vec<String>) {
        self.history = history;
    }

    pub fn push_history(&mut self, statement: String) {
        if self.history.last() != Some(&statement) {
            self.history.push(statement);
        }
        self.browsing = None;
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// A statement is complete when, ignoring trailing whitespace and
    /// comments, it ends with `;` — or it is a one-line dot command or
    /// bare alias (`exit`, `help`, ...).
    pub fn is_complete(&self) -> bool {
        let text = self.lines();
        let trimmed = text.trim();
        if trimmed.starts_with('.') || is_bare_alias(trimmed) {
            return !trimmed.contains('\n');
        }
        let tokens = Tokenizer::new(&GenericDialect {}, trimmed)
            .tokenize()
            .unwrap_or_default();
        tokens
            .iter()
            .rev()
            .find(|token| !matches!(token, Token::Whitespace(_)))
            .is_some_and(|token| matches!(token, Token::SemiColon))
    }

    pub fn handle(&mut self, key: &KeyEvent) -> EditorAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match (key.code, ctrl) {
            (KeyCode::Char('c'), true) => return EditorAction::Interrupt,
            (KeyCode::Char('d'), true) if self.is_empty() => return EditorAction::Quit,
            (KeyCode::Char('d'), true) => return EditorAction::None,
            (KeyCode::Char('l'), true) => return EditorAction::Clear,
            (KeyCode::Enter, true) => return self.submit(),
            (KeyCode::Enter, false) if alt => return self.submit(),
            (KeyCode::Enter, false) => {
                if self.is_complete() {
                    return self.submit();
                }
                self.area.insert_newline();
                return EditorAction::None;
            }
            (KeyCode::Up, false) if self.area.cursor().0 == 0 => return self.history_back(),
            (KeyCode::Down, false) if self.area.cursor().0 + 1 == self.area.lines().len() => {
                return self.history_forward();
            }
            _ => {}
        }
        let input: Input = (*key).into();
        if matches!(input.key, Key::Null) {
            return EditorAction::None;
        }
        self.area.input(input);
        self.browsing = None;
        EditorAction::None
    }

    fn submit(&mut self) -> EditorAction {
        let text = self.lines().trim().to_owned();
        if text.is_empty() {
            return EditorAction::None;
        }
        self.clear();
        EditorAction::Submit(text)
    }

    fn history_back(&mut self) -> EditorAction {
        if self.history.is_empty() {
            return EditorAction::None;
        }
        let next = match self.browsing {
            None => {
                self.draft = self.lines();
                self.history.len() - 1
            }
            Some(0) => return EditorAction::None,
            Some(index) => index - 1,
        };
        let text = self.history[next].clone();
        self.set_text(&text);
        self.browsing = Some(next);
        EditorAction::None
    }

    fn history_forward(&mut self) -> EditorAction {
        let Some(current) = self.browsing else {
            return EditorAction::None;
        };
        if current + 1 < self.history.len() {
            let text = self.history[current + 1].clone();
            self.set_text(&text);
            self.browsing = Some(current + 1);
        } else {
            let draft = self.draft.clone();
            self.set_text(&draft);
            self.browsing = None;
        }
        EditorAction::None
    }

    /// The widget, boxed and titled; dimmed while a statement runs.
    pub fn widget(&mut self, title: &str, theme: &Theme, running: bool) -> &TextArea<'static> {
        let border = if running { theme.dim } else { theme.accent };
        self.area.set_block(
            Block::bordered()
                .border_style(border)
                .title(Span::styled(format!(" {title} "), theme.title)),
        );
        self.area
            .set_style(if running { theme.dim } else { Style::default() });
        &self.area
    }

    /// Rows the box needs: the lines plus two borders, within `max`.
    pub fn height(&self, max: u16) -> u16 {
        let lines = self.area.lines().len() as u16;
        (lines + 2).clamp(3, max.max(3))
    }
}

fn is_bare_alias(text: &str) -> bool {
    let word = text.trim_end_matches(';').trim();
    ["exit", "quit", "help", "clear"]
        .iter()
        .any(|alias| word.eq_ignore_ascii_case(alias))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(editor: &mut Editor, text: &str) {
        for ch in text.chars() {
            editor.handle(&key(KeyCode::Char(ch)));
        }
    }

    #[test]
    fn enter_submits_only_a_terminated_statement() {
        let mut editor = Editor::new();
        type_text(&mut editor, "SELECT 1");
        assert!(matches!(
            editor.handle(&key(KeyCode::Enter)),
            EditorAction::None
        ));
        assert_eq!(editor.lines(), "SELECT 1\n");
        type_text(&mut editor, "FROM t;");
        assert!(matches!(
            editor.handle(&key(KeyCode::Enter)),
            EditorAction::Submit(sql) if sql == "SELECT 1\nFROM t;"
        ));
        assert_eq!(editor.lines(), "");
    }

    #[test]
    fn trailing_comment_after_semicolon_still_submits() {
        let mut editor = Editor::new();
        type_text(&mut editor, "SELECT 1; -- done");
        assert!(matches!(
            editor.handle(&key(KeyCode::Enter)),
            EditorAction::Submit(_)
        ));
    }

    #[test]
    fn dot_commands_and_aliases_submit_without_semicolon() {
        let mut editor = Editor::new();
        type_text(&mut editor, ".tables");
        assert!(matches!(
            editor.handle(&key(KeyCode::Enter)),
            EditorAction::Submit(sql) if sql == ".tables"
        ));
        type_text(&mut editor, "exit");
        assert!(matches!(
            editor.handle(&key(KeyCode::Enter)),
            EditorAction::Submit(sql) if sql == "exit"
        ));
    }

    #[test]
    fn ctrl_enter_forces_submit_and_history_navigates() {
        let mut editor = Editor::new();
        editor.set_history(vec!["SELECT 1;".into(), "SELECT 2;".into()]);
        editor.handle(&key(KeyCode::Up));
        assert_eq!(editor.lines(), "SELECT 2;");
        editor.handle(&key(KeyCode::Up));
        assert_eq!(editor.lines(), "SELECT 1;");
        editor.handle(&key(KeyCode::Down));
        assert_eq!(editor.lines(), "SELECT 2;");
        editor.handle(&key(KeyCode::Down));
        assert_eq!(editor.lines(), "");
        type_text(&mut editor, "SELECT 3");
        assert!(matches!(
            editor.handle(&KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)),
            EditorAction::Submit(sql) if sql == "SELECT 3"
        ));
    }

    #[test]
    fn ctrl_c_and_ctrl_d_map_to_actions() {
        let mut editor = Editor::new();
        assert!(matches!(
            editor.handle(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            EditorAction::Interrupt
        ));
        assert!(matches!(
            editor.handle(&KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            EditorAction::Quit
        ));
        type_text(&mut editor, "x");
        assert!(matches!(
            editor.handle(&KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            EditorAction::None
        ));
        assert_eq!(editor.height(8), 3);
    }
}
