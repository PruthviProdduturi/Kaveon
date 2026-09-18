//! The pinned SQL editor: a `tui-textarea` with the submit rule, history
//! navigation, an optional vi editing mode and the box the shell draws
//! around it.
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders};
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

/// The vi state machine: `None` is Emacs editing; otherwise the mode the
/// editor is in. `Operator` is Normal mode with `d`, `y` or `c` waiting for
/// its motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViMode {
    Normal,
    Insert,
    Visual,
    Operator(char),
}

pub struct Editor {
    area: TextArea<'static>,
    history: Vec<String>,
    /// Index into `history` while browsing; `None` when editing a new statement.
    browsing: Option<usize>,
    draft: String,
    vi: Option<ViMode>,
    /// The first key of a two-key vi sequence (`gg`).
    pending: Option<char>,
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
            vi: None,
            pending: None,
        }
    }

    /// Switch between Emacs (the default) and vi editing. Vi starts in
    /// Insert mode so a statement can be typed straight away.
    pub fn set_vi(&mut self, enabled: bool) {
        self.area.cancel_selection();
        self.pending = None;
        self.vi = enabled.then_some(ViMode::Insert);
    }

    /// The vi mode for the status line; `None` in Emacs editing.
    pub fn mode_label(&self) -> Option<&'static str> {
        self.vi.map(|mode| match mode {
            ViMode::Normal | ViMode::Operator(_) => "NORMAL",
            ViMode::Insert => "INSERT",
            ViMode::Visual => "VISUAL",
        })
    }

    pub fn lines(&self) -> String {
        self.area.lines().join("\n")
    }

    pub fn line_count(&self) -> usize {
        self.area.lines().len()
    }

    /// (row, column) of the cursor, in characters.
    pub fn cursor(&self) -> (usize, usize) {
        self.area.cursor()
    }

    /// The line the cursor is on.
    pub fn current_line(&self) -> String {
        self.area
            .lines()
            .get(self.area.cursor().0)
            .cloned()
            .unwrap_or_default()
    }

    /// Replace the `chars` characters before the cursor with `text`.
    pub fn replace_before_cursor(&mut self, chars: usize, text: &str) {
        if chars > 0 {
            self.area.delete_str(chars);
        }
        self.area.insert_str(text);
    }

    pub fn is_empty(&self) -> bool {
        self.area.lines().iter().all(|line| line.trim().is_empty())
    }

    pub fn clear(&mut self) {
        self.area = blank_area();
        self.browsing = None;
        self.pending = None;
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
            _ => {}
        }
        match self.vi {
            Some(ViMode::Insert) if key.code == KeyCode::Esc => {
                self.vi = Some(ViMode::Normal);
                EditorAction::None
            }
            None | Some(ViMode::Insert) => self.handle_emacs(key),
            Some(mode) => self.handle_vi(mode, key),
        }
    }

    /// Emacs editing, and vi Insert mode: the textarea's default key map
    /// with the submit rule on Enter and history on Up/Down.
    fn handle_emacs(&mut self, key: &KeyEvent) -> EditorAction {
        match key.code {
            KeyCode::Enter => {
                if self.is_complete() {
                    return self.submit();
                }
                self.area.insert_newline();
                return EditorAction::None;
            }
            KeyCode::Up if self.on_first_line() => return self.history_back(),
            KeyCode::Down if self.on_last_line() => return self.history_forward(),
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

    /// Vi Normal, Visual and Operator modes.
    fn handle_vi(&mut self, mode: ViMode, key: &KeyEvent) -> EditorAction {
        let input: Input = (*key).into();
        if matches!(input.key, Key::Null) {
            return EditorAction::None;
        }
        let pending = self.pending.take();
        let normal = mode == ViMode::Normal;
        let visual = mode == ViMode::Visual;
        if normal && !input.ctrl {
            match input.key {
                Key::Char('k') | Key::Up if self.on_first_line() => return self.history_back(),
                Key::Char('j') | Key::Down if self.on_last_line() => {
                    return self.history_forward();
                }
                _ => {}
            }
        }
        self.browsing = None;
        match (input.key, input.ctrl) {
            // Motions — in Operator mode they extend the pending selection.
            (Key::Char('h'), false) | (Key::Left, _) | (Key::Backspace, _) => {
                self.area.move_cursor(CursorMove::Back);
            }
            (Key::Char('j'), false) | (Key::Down, _) => self.area.move_cursor(CursorMove::Down),
            (Key::Char('k'), false) | (Key::Up, _) => self.area.move_cursor(CursorMove::Up),
            (Key::Char('l'), false) | (Key::Right, _) | (Key::Char(' '), false) => {
                self.area.move_cursor(CursorMove::Forward);
            }
            (Key::Char('w'), false) => self.area.move_cursor(CursorMove::WordForward),
            (Key::Char('b'), false) => self.area.move_cursor(CursorMove::WordBack),
            (Key::Char('e'), false) => {
                self.area.move_cursor(CursorMove::WordEnd);
                if matches!(mode, ViMode::Operator(_)) {
                    self.area.move_cursor(CursorMove::Forward);
                }
            }
            (Key::Char('0'), false) | (Key::Home, _) => self.area.move_cursor(CursorMove::Head),
            (Key::Char('^'), false) => self.move_to_first_non_blank(),
            (Key::Char('$'), false) | (Key::End, _) => self.area.move_cursor(CursorMove::End),
            (Key::Char('G'), false) => self.area.move_cursor(CursorMove::Bottom),
            (Key::Char('g'), false) if pending == Some('g') => {
                self.area.move_cursor(CursorMove::Top);
            }
            (Key::Char('g'), false) if normal => {
                self.pending = Some('g');
                return EditorAction::None;
            }
            // `dd`, `yy`, `cc`: the whole line.
            (Key::Char(op), false) if mode == ViMode::Operator(op) => {
                self.area.move_cursor(CursorMove::Head);
                self.area.start_selection();
                let cursor = self.area.cursor();
                self.area.move_cursor(CursorMove::Down);
                if cursor == self.area.cursor() {
                    self.area.move_cursor(CursorMove::End);
                }
            }
            // Operators.
            (Key::Char(op @ ('d' | 'y' | 'c')), false) if normal => {
                self.area.start_selection();
                return self.enter(ViMode::Operator(op));
            }
            (Key::Char('d' | 'x'), false) if visual => {
                self.area.move_cursor(CursorMove::Forward);
                self.area.cut();
                return self.enter(ViMode::Normal);
            }
            (Key::Char('y'), false) if visual => {
                self.area.move_cursor(CursorMove::Forward);
                self.area.copy();
                return self.enter(ViMode::Normal);
            }
            (Key::Char('c'), false) if visual => {
                self.area.move_cursor(CursorMove::Forward);
                self.area.cut();
                return self.enter(ViMode::Insert);
            }
            (Key::Char('x'), false) | (Key::Delete, _) => {
                self.area.delete_next_char();
                return self.enter(ViMode::Normal);
            }
            (Key::Char('D'), false) => {
                self.area.delete_line_by_end();
                return self.enter(ViMode::Normal);
            }
            (Key::Char('C'), false) => {
                self.area.delete_line_by_end();
                return self.enter(ViMode::Insert);
            }
            (Key::Char('p'), false) => {
                self.area.paste();
                return self.enter(ViMode::Normal);
            }
            (Key::Char('u'), false) => {
                self.area.undo();
                return self.enter(ViMode::Normal);
            }
            (Key::Char('r'), true) => {
                self.area.redo();
                return self.enter(ViMode::Normal);
            }
            // Into Insert mode.
            (Key::Char('i'), false) => return self.enter(ViMode::Insert),
            (Key::Char('a'), false) => {
                self.area.cancel_selection();
                self.area.move_cursor(CursorMove::Forward);
                return self.enter(ViMode::Insert);
            }
            (Key::Char('I'), false) => {
                self.area.cancel_selection();
                self.move_to_first_non_blank();
                return self.enter(ViMode::Insert);
            }
            (Key::Char('A'), false) => {
                self.area.cancel_selection();
                self.area.move_cursor(CursorMove::End);
                return self.enter(ViMode::Insert);
            }
            (Key::Char('o'), false) => {
                self.area.cancel_selection();
                self.area.move_cursor(CursorMove::End);
                self.area.insert_newline();
                return self.enter(ViMode::Insert);
            }
            (Key::Char('O'), false) => {
                self.area.cancel_selection();
                self.area.move_cursor(CursorMove::Head);
                self.area.insert_newline();
                self.area.move_cursor(CursorMove::Up);
                return self.enter(ViMode::Insert);
            }
            // Visual mode.
            (Key::Char('v'), false) if normal => {
                self.area.start_selection();
                return self.enter(ViMode::Visual);
            }
            (Key::Char('V'), false) if normal => {
                self.area.move_cursor(CursorMove::Head);
                self.area.start_selection();
                self.area.move_cursor(CursorMove::End);
                return self.enter(ViMode::Visual);
            }
            (Key::Char('v'), false) | (Key::Esc, _) => return self.enter(ViMode::Normal),
            // Enter submits a complete statement from Normal mode; otherwise
            // it is the motion to the next line, as in vi.
            (Key::Enter, _) if normal => {
                if self.is_complete() {
                    return self.submit();
                }
                self.area.move_cursor(CursorMove::Down);
            }
            (Key::Enter, _) => return self.enter(ViMode::Normal),
            _ => return EditorAction::None,
        }
        // A motion completes the pending operator.
        match mode {
            ViMode::Operator('y') => {
                self.area.copy();
                self.enter(ViMode::Normal)
            }
            ViMode::Operator('d') => {
                self.area.cut();
                self.enter(ViMode::Normal)
            }
            ViMode::Operator('c') => {
                self.area.cut();
                self.enter(ViMode::Insert)
            }
            _ => EditorAction::None,
        }
    }

    fn enter(&mut self, mode: ViMode) -> EditorAction {
        if !matches!(mode, ViMode::Visual | ViMode::Operator(_)) {
            self.area.cancel_selection();
        }
        self.vi = Some(mode);
        EditorAction::None
    }

    fn on_first_line(&self) -> bool {
        self.area.cursor().0 == 0
    }

    fn on_last_line(&self) -> bool {
        self.area.cursor().0 + 1 == self.area.lines().len()
    }

    fn move_to_first_non_blank(&mut self) {
        let row = self.area.cursor().0;
        let column = self
            .current_line()
            .chars()
            .position(|ch| !ch.is_whitespace())
            .unwrap_or(0);
        self.area.move_cursor(CursorMove::Jump(
            u16::try_from(row).unwrap_or(u16::MAX),
            u16::try_from(column).unwrap_or(u16::MAX),
        ));
    }

    fn submit(&mut self) -> EditorAction {
        let text = self.lines().trim().to_owned();
        if text.is_empty() {
            return EditorAction::None;
        }
        self.clear();
        if self.vi.is_some() {
            self.vi = Some(ViMode::Insert);
        }
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

    /// The widget between two thin rules; dimmed while a statement runs.
    pub fn widget(&mut self, theme: &Theme, running: bool) -> &TextArea<'static> {
        self.area.set_block(
            Block::new()
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(theme.dim),
        );
        self.area
            .set_style(if running { theme.dim } else { Style::default() });
        &self.area
    }

    /// Rows the editor needs: the two rules plus the lines, within `max`.
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

    fn vi_editor() -> Editor {
        let mut editor = Editor::new();
        editor.set_vi(true);
        editor
    }

    fn press(editor: &mut Editor, keys: &str) {
        for ch in keys.chars() {
            editor.handle(&key(KeyCode::Char(ch)));
        }
    }

    #[test]
    fn vi_starts_in_insert_and_moves_between_modes() {
        let mut editor = Editor::new();
        assert_eq!(editor.mode_label(), None);
        editor.set_vi(true);
        assert_eq!(editor.mode_label(), Some("INSERT"));
        type_text(&mut editor, "SELECT 1;");
        assert_eq!(editor.lines(), "SELECT 1;");
        editor.handle(&key(KeyCode::Esc));
        assert_eq!(editor.mode_label(), Some("NORMAL"));
        press(&mut editor, "v");
        assert_eq!(editor.mode_label(), Some("VISUAL"));
        editor.handle(&key(KeyCode::Esc));
        assert_eq!(editor.mode_label(), Some("NORMAL"));
        press(&mut editor, "d");
        assert_eq!(editor.mode_label(), Some("NORMAL"));
        editor.handle(&key(KeyCode::Esc));
        press(&mut editor, "i");
        assert_eq!(editor.mode_label(), Some("INSERT"));
        editor.handle(&key(KeyCode::Esc));
        editor.set_vi(false);
        assert_eq!(editor.mode_label(), None);
    }

    #[test]
    fn vi_dd_cuts_the_line_and_p_pastes_it_back() {
        let mut editor = vi_editor();
        type_text(&mut editor, "SELECT 1");
        editor.handle(&key(KeyCode::Enter));
        type_text(&mut editor, "FROM t;");
        editor.handle(&key(KeyCode::Esc));
        press(&mut editor, "kdd");
        assert_eq!(editor.lines(), "FROM t;");
        assert_eq!(editor.mode_label(), Some("NORMAL"));
        press(&mut editor, "p");
        assert_eq!(editor.lines(), "SELECT 1\nFROM t;");
        press(&mut editor, "GD");
        assert_eq!(editor.lines(), "SELECT 1\n");
    }

    #[test]
    fn vi_x_deletes_under_cursor_and_u_undoes() {
        let mut editor = vi_editor();
        type_text(&mut editor, "SELECT 1;");
        editor.handle(&key(KeyCode::Esc));
        press(&mut editor, "0x");
        assert_eq!(editor.lines(), "ELECT 1;");
        press(&mut editor, "u");
        assert_eq!(editor.lines(), "SELECT 1;");
        editor.handle(&KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(editor.lines(), "ELECT 1;");
    }

    #[test]
    fn vi_word_motions() {
        let mut editor = vi_editor();
        type_text(&mut editor, "SELECT count FROM t;");
        editor.handle(&key(KeyCode::Esc));
        press(&mut editor, "0");
        assert_eq!(editor.cursor(), (0, 0));
        press(&mut editor, "w");
        assert_eq!(editor.cursor(), (0, 7));
        press(&mut editor, "w");
        assert_eq!(editor.cursor(), (0, 13));
        press(&mut editor, "b");
        assert_eq!(editor.cursor(), (0, 7));
        press(&mut editor, "e");
        assert_eq!(editor.cursor(), (0, 11));
        press(&mut editor, "$");
        assert_eq!(editor.cursor(), (0, 20));
        press(&mut editor, "^");
        assert_eq!(editor.cursor(), (0, 0));
    }

    #[test]
    fn vi_visual_delete_and_dw() {
        let mut editor = vi_editor();
        type_text(&mut editor, "SELECT 1;");
        editor.handle(&key(KeyCode::Esc));
        press(&mut editor, "0ved");
        assert_eq!(editor.lines(), " 1;");
        assert_eq!(editor.mode_label(), Some("NORMAL"));
        press(&mut editor, "u0dw");
        assert_eq!(editor.lines(), "1;");
    }

    #[test]
    fn vi_append_then_type_and_enter_submits_from_normal() {
        let mut editor = vi_editor();
        type_text(&mut editor, "SELECT 1");
        editor.handle(&key(KeyCode::Esc));
        assert!(matches!(
            editor.handle(&key(KeyCode::Enter)),
            EditorAction::None
        ));
        assert_eq!(editor.lines(), "SELECT 1");
        press(&mut editor, "0A");
        assert_eq!(editor.mode_label(), Some("INSERT"));
        type_text(&mut editor, ";");
        assert_eq!(editor.lines(), "SELECT 1;");
        editor.handle(&key(KeyCode::Esc));
        assert!(matches!(
            editor.handle(&key(KeyCode::Enter)),
            EditorAction::Submit(sql) if sql == "SELECT 1;"
        ));
        assert_eq!(editor.lines(), "");
        assert_eq!(editor.mode_label(), Some("INSERT"));
    }

    #[test]
    fn vi_normal_k_and_j_browse_history() {
        let mut editor = vi_editor();
        editor.set_history(vec!["SELECT 1;".into(), "SELECT 2;".into()]);
        editor.handle(&key(KeyCode::Esc));
        press(&mut editor, "k");
        assert_eq!(editor.lines(), "SELECT 2;");
        press(&mut editor, "k");
        assert_eq!(editor.lines(), "SELECT 1;");
        press(&mut editor, "j");
        assert_eq!(editor.lines(), "SELECT 2;");
        press(&mut editor, "j");
        assert_eq!(editor.lines(), "");
    }

    #[test]
    fn set_vi_false_restores_emacs_editing() {
        let mut editor = vi_editor();
        editor.handle(&key(KeyCode::Esc));
        press(&mut editor, "x");
        assert_eq!(editor.lines(), "");
        editor.set_vi(false);
        assert_eq!(editor.mode_label(), None);
        press(&mut editor, "x");
        assert_eq!(editor.lines(), "x");
        editor.handle(&key(KeyCode::Esc));
        press(&mut editor, "dd");
        assert_eq!(editor.lines(), "xdd");
    }
}
