use rustyline::completion::{Completer, Pair};
use rustyline::config::{CompletionType, Config, EditMode};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

const COMPLETIONS: &[&str] = &[
    "SELECT",
    "SHOW",
    "CATALOGS",
    "SCHEMAS",
    "TABLES",
    "FROM",
    "IN",
    "USE",
    "WHERE",
    "LIMIT",
    "HELP",
    "CLEAR",
    "EXIT",
    "QUIT",
    ".catalogs",
    ".schemas",
    ".tables",
    ".use",
    ".help",
    ".clear",
    ".quit",
];

pub enum ReadLine {
    Line(String),
    Interrupted,
    Eof,
}

pub struct TerminalInput {
    editor: Editor<SqlHelper, DefaultHistory>,
    history_file: Option<PathBuf>,
}

impl TerminalInput {
    pub fn new(
        history_file: Option<PathBuf>,
        editing_mode: &str,
        history_enabled: bool,
        auto_suggestion: bool,
    ) -> Result<Self, String> {
        let history_file = history_enabled
            .then(|| history_file.or_else(default_history_file))
            .flatten();
        let edit_mode = if editing_mode.eq_ignore_ascii_case("vi") {
            EditMode::Vi
        } else {
            EditMode::Emacs
        };
        let config = Config::builder()
            .completion_type(CompletionType::List)
            .edit_mode(edit_mode)
            .history_ignore_space(true)
            .build();
        let mut editor = Editor::with_config(config).map_err(|error| error.to_string())?;
        editor.set_helper(Some(SqlHelper {
            auto_suggestion,
            ..SqlHelper::default()
        }));
        if let Some(path) = history_file.as_deref() {
            let _ = editor.load_history(path);
        }
        Ok(Self {
            editor,
            history_file,
        })
    }

    pub fn readline(&mut self, prompt: &str) -> Result<ReadLine, String> {
        match self.editor.readline(prompt) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    self.editor
                        .add_history_entry(line.as_str())
                        .map_err(|error| error.to_string())?;
                }
                Ok(ReadLine::Line(line))
            }
            Err(ReadlineError::Interrupted) => Ok(ReadLine::Interrupted),
            Err(ReadlineError::Eof) => Ok(ReadLine::Eof),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn set_colored_prompt(&mut self, prompt: &str, enabled: bool) {
        let helper = self
            .editor
            .helper_mut()
            .expect("terminal helper is configured");
        helper.colored_prompt = if enabled {
            if let (Some(colon), Some(end)) = (prompt.find(':'), prompt.rfind('>')) {
                format!(
                    "\x1b[90m{}\x1b[97m{}\x1b[90m{}\x1b[0m",
                    &prompt[..=colon],
                    &prompt[colon + 1..end],
                    &prompt[end..]
                )
            } else {
                format!("\x1b[90m{prompt}\x1b[0m")
            }
        } else {
            prompt.to_owned()
        };
    }

    pub fn save_history(&mut self) -> Result<(), String> {
        if let Some(path) = self.history_file.as_deref() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            self.editor
                .save_history(path)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

fn default_history_file() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("kaveon").join("history"))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|path| path.join(".kaveon_history"))
        })
}

struct SqlHelper {
    colored_prompt: String,
    hinter: HistoryHinter,
    auto_suggestion: bool,
}

impl Default for SqlHelper {
    fn default() -> Self {
        Self {
            colored_prompt: String::new(),
            hinter: HistoryHinter::new(),
            auto_suggestion: true,
        }
    }
}

impl Helper for SqlHelper {}
impl Hinter for SqlHelper {
    type Hint = String;

    fn hint(&self, line: &str, position: usize, context: &Context<'_>) -> Option<Self::Hint> {
        self.auto_suggestion
            .then(|| self.hinter.hint(line, position, context))
            .flatten()
    }
}
impl Highlighter for SqlHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        default: bool,
    ) -> Cow<'b, str> {
        if default && !self.colored_prompt.is_empty() {
            Cow::Borrowed(&self.colored_prompt)
        } else {
            Cow::Borrowed(prompt)
        }
    }
}
impl Validator for SqlHelper {}
impl Completer for SqlHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        position: usize,
        _: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let prefix_start = line[..position]
            .rfind(|character: char| character.is_whitespace())
            .map_or(0, |index| index + 1);
        let prefix = &line[prefix_start..position];
        let candidates = COMPLETIONS
            .iter()
            .filter(|candidate| {
                candidate
                    .to_ascii_uppercase()
                    .starts_with(&prefix.to_ascii_uppercase())
            })
            .map(|candidate| Pair {
                display: (*candidate).to_owned(),
                replacement: (*candidate).to_owned(),
            })
            .collect();
        Ok((prefix_start, candidates))
    }
}

pub fn read_file(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

pub fn split_statements(input: &str) -> Result<Vec<String>, String> {
    let (mut statements, remainder) = split_statements_with_completion(input)?;
    if has_sql_content(&remainder) {
        statements.push(remainder.trim().to_owned());
    }
    Ok(statements)
}

/// Splits only lexical statements that have a terminator. The remainder stays
/// in the interactive editor until it is itself complete.
pub fn split_completed_statements(input: &str) -> Result<(Vec<String>, String), String> {
    let (statements, remainder) = split_statements_with_completion(input)?;
    let remainder = if has_sql_content(&remainder) {
        remainder
    } else {
        String::new()
    };
    Ok((statements, remainder))
}

fn split_statements_with_completion(input: &str) -> Result<(Vec<String>, String), String> {
    let mut statements = Vec::new();
    let mut start = 0;
    let mut index = 0;
    let bytes = input.as_bytes();
    let mut quote = None;
    let mut dollar_quote: Option<Vec<u8>> = None;
    let mut line_comment = false;
    let mut block_comment_depth = 0_usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            }
            index += 1;
            continue;
        }
        if block_comment_depth > 0 {
            if bytes.get(index..index + 2) == Some(b"/*") {
                block_comment_depth += 1;
                index += 2;
            } else if bytes.get(index..index + 2) == Some(b"*/") {
                block_comment_depth -= 1;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(delimiter) = &dollar_quote {
            if bytes[index..].starts_with(delimiter) {
                index += delimiter.len();
                dollar_quote = None;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            if byte == delimiter {
                if bytes.get(index + 1) == Some(&delimiter) {
                    index += 2;
                } else {
                    quote = None;
                    index += 1;
                }
            } else {
                index += 1;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                line_comment = true;
                index += 1;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                block_comment_depth = 1;
                index += 1;
            }
            b'$' => {
                if let Some(end) = dollar_quote_end(bytes, index) {
                    dollar_quote = Some(bytes[index..end].to_vec());
                    index = end - 1;
                }
            }
            b';' => {
                let statement = input[start..index].trim();
                if !statement.is_empty() {
                    statements.push(statement.to_owned());
                }
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if quote.is_some() || dollar_quote.is_some() || block_comment_depth > 0 {
        return Err("unterminated quoted string or block comment".to_owned());
    }
    Ok((statements, input[start..].to_owned()))
}

fn has_sql_content(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
        } else if bytes.get(index..index + 2) == Some(b"--") {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
        } else if bytes.get(index..index + 2) == Some(b"/*") {
            index += 2;
            while index + 1 < bytes.len() && bytes.get(index..index + 2) != Some(b"*/") {
                index += 1;
            }
            if bytes.get(index..index + 2) == Some(b"*/") {
                index += 2;
            }
        } else {
            return true;
        }
    }
    false
}

fn dollar_quote_end(bytes: &[u8], start: usize) -> Option<usize> {
    if start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        return None;
    }
    let end = bytes[start + 1..].iter().position(|byte| *byte == b'$')? + start + 2;
    bytes[start + 1..end - 1]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .then_some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_multiple_statements_without_splitting_literals_or_comments() {
        assert_eq!(
            split_statements("SELECT ';'; /* ; */ SHOW CATALOGS; -- ;\nUSE medallion.test;")
                .unwrap(),
            [
                "SELECT ';'",
                "/* ; */ SHOW CATALOGS",
                "-- ;\nUSE medallion.test"
            ]
        );
    }

    #[test]
    fn rejects_unterminated_input() {
        assert!(split_statements("SELECT 'unfinished").is_err());
        assert!(split_statements("/* unfinished").is_err());
        assert!(split_statements("SELECT $$unfinished").is_err());
    }

    #[test]
    fn handles_dollar_quotes_and_crlf_without_splitting_literals() {
        assert_eq!(
            split_statements("SELECT $tag$semi; -- literal$tag$;\r\nSHOW CATALOGS;").unwrap(),
            ["SELECT $tag$semi; -- literal$tag$", "SHOW CATALOGS"]
        );
    }

    #[test]
    fn completed_statements_preserve_an_unfinished_next_statement() {
        assert_eq!(
            split_completed_statements("SELECT 1; SELECT").unwrap(),
            (vec!["SELECT 1".to_owned()], " SELECT".to_owned())
        );
        assert_eq!(
            split_completed_statements("SELECT 1; -- pending comment\n").unwrap(),
            (vec!["SELECT 1".to_owned()], String::new())
        );
        assert_eq!(
            split_completed_statements("-- only a comment\n").unwrap(),
            (Vec::new(), String::new())
        );
    }

    #[test]
    fn continuation_prompt_is_colored_without_schema_slicing() {
        let mut input = TerminalInput::new(None, "EMACS", false, true).unwrap();
        input.set_colored_prompt("     -> ", true);
    }
}
