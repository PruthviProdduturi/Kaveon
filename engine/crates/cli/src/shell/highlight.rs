//! SQL highlighting for the editor: keywords in the accent colour and bold,
//! strings green, comments dim, everything else as typed. The text comes
//! back exactly, one `Line` per input line.
use crate::theme::Theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use sqlparser::dialect::GenericDialect;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer, Whitespace};

pub fn highlight(text: &str, theme: &Theme) -> Vec<Line<'static>> {
    let mut tokens = Vec::new();
    let mut tokenizer = Tokenizer::new(&GenericDialect {}, text);
    // On an error the buffer holds the tokens before it; the rest of the
    // text (an unfinished string, say) is drawn as typed.
    let _ = tokenizer.tokenize_with_location_into_buf(&mut tokens);
    let offsets = Offsets::of(text);
    let mut builder = Builder::new(text);
    for TokenWithSpan { token, span } in &tokens {
        let style = style_of(token, theme);
        let (start, end) = (offsets.byte(span.start), offsets.byte(span.end));
        builder.push(text[start.min(end)..end].to_owned(), style);
    }
    let rest = builder.rest();
    if !rest.is_empty() {
        builder.push(rest, Style::default());
    }
    builder.finish()
}

fn style_of(token: &Token, theme: &Theme) -> Style {
    match token {
        Token::Word(word) if word.quote_style.is_none() && word.keyword != Keyword::NoKeyword => {
            theme.title
        }
        Token::SingleQuotedString(_)
        | Token::DollarQuotedString(_)
        | Token::EscapedStringLiteral(_)
        | Token::UnicodeStringLiteral(_)
        | Token::NationalStringLiteral(_)
        | Token::TripleSingleQuotedString(_)
        | Token::SingleQuotedByteStringLiteral(_)
        | Token::TripleSingleQuotedByteStringLiteral(_)
        | Token::SingleQuotedRawStringLiteral(_)
        | Token::TripleSingleQuotedRawStringLiteral(_) => theme.ok,
        Token::Whitespace(Whitespace::SingleLineComment { .. })
        | Token::Whitespace(Whitespace::MultiLineComment(_)) => theme.dim,
        _ => Style::default(),
    }
}

/// Tokenizer locations (1-based line, 1-based character column, the end
/// exclusive) to byte offsets in the text.
struct Offsets {
    /// Byte offset of each character, then the text's length.
    char_bytes: Vec<usize>,
    /// Character index where each line starts.
    line_starts: Vec<usize>,
}

impl Offsets {
    fn of(text: &str) -> Offsets {
        let mut char_bytes = Vec::with_capacity(text.len() + 1);
        let mut line_starts = vec![0];
        for (index, (byte, ch)) in text.char_indices().enumerate() {
            char_bytes.push(byte);
            if ch == '\n' {
                line_starts.push(index + 1);
            }
        }
        char_bytes.push(text.len());
        Offsets {
            char_bytes,
            line_starts,
        }
    }

    fn byte(&self, location: Location) -> usize {
        let line = (location.line.max(1) as usize - 1).min(self.line_starts.len() - 1);
        let column = location.column.max(1) as usize - 1;
        let index = (self.line_starts[line] + column).min(self.char_bytes.len() - 1);
        self.char_bytes[index]
    }
}

struct Builder<'a> {
    text: &'a str,
    consumed: usize,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
}

impl<'a> Builder<'a> {
    fn new(text: &'a str) -> Self {
        Builder {
            text,
            consumed: 0,
            lines: Vec::new(),
            current: Vec::new(),
        }
    }

    fn push(&mut self, piece: String, style: Style) {
        self.consumed += piece.len();
        let mut parts = piece.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                self.current.push(Span::styled(part.to_owned(), style));
            }
            if parts.peek().is_some() {
                self.lines
                    .push(Line::from(std::mem::take(&mut self.current)));
            }
        }
    }

    fn rest(&self) -> String {
        self.text[self.consumed.min(self.text.len())..].to_owned()
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.lines.push(Line::from(self.current));
        self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::to_plain;
    use ratatui::style::{Color, Modifier};

    fn theme() -> Theme {
        Theme {
            accent: Style::default().fg(Color::Cyan),
            dim: Style::default().fg(Color::DarkGray),
            ok: Style::default().fg(Color::Green),
            title: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            ..Theme::mono()
        }
    }

    fn round_trips(text: &str) {
        let lines = highlight(text, &theme());
        assert_eq!(to_plain(&lines), format!("{text}\n"), "{text:?}");
        assert_eq!(lines.len(), text.split('\n').count(), "{text:?}");
    }

    #[test]
    fn content_and_line_count_are_preserved() {
        round_trips("");
        round_trips("SELECT 1");
        round_trips("SELECT region, COUNT(*) FROM kaveon_events\nGROUP BY region;");
        round_trips("SELECT 'a\nb' AS s, 1.5e3, \"Quoted Name\"\n\n-- trailing\n");
        round_trips("/* multi\n   line */ SELECT $$dollar\nquoted$$;");
        round_trips("SELECT 'unterminated");
        round_trips("SELECT ¿ñ, 'ünïcödé' FROM t\twhere x = 1");
    }

    #[test]
    fn keywords_strings_and_comments_are_styled() {
        let theme = theme();
        let lines = highlight("SELECT 'x', 42 -- note\nFROM t", &theme);
        let styles: Vec<(String, Style)> = lines
            .iter()
            .flat_map(|line| {
                line.spans
                    .iter()
                    .map(|span| (span.content.to_string(), span.style))
            })
            .collect();
        assert_eq!(styles[0], ("SELECT".to_owned(), theme.title));
        assert_eq!(styles[1], (" ".to_owned(), Style::default()));
        assert_eq!(styles[2], ("'x'".to_owned(), theme.ok));
        assert!(styles.contains(&("42".to_owned(), Style::default())));
        assert!(styles.contains(&("-- note".to_owned(), theme.dim)));
        assert_eq!(lines[1].spans[0].content, "FROM");
        assert_eq!(lines[1].spans[0].style, theme.title);
        assert_eq!(lines[1].spans[2].content, "t");
        assert_eq!(lines[1].spans[2].style, Style::default());
    }

    #[test]
    fn quoted_identifiers_and_names_are_not_keywords() {
        let theme = theme();
        let lines = highlight("SELECT \"select\", region FROM t", &theme);
        let select_ident = lines[0]
            .spans
            .iter()
            .find(|span| span.content == "\"select\"")
            .unwrap();
        assert_eq!(select_ident.style, Style::default());
        let region = lines[0]
            .spans
            .iter()
            .find(|span| span.content == "region")
            .unwrap();
        assert_eq!(region.style, Style::default());
    }

    #[test]
    fn a_tokenizer_error_keeps_the_prefix_styled_and_the_rest_plain() {
        let theme = theme();
        let lines = highlight("SELECT 'open", &theme);
        assert_eq!(lines[0].spans[0].content, "SELECT");
        assert_eq!(lines[0].spans[0].style, theme.title);
        assert_eq!(lines[0].spans.last().unwrap().content, "'open");
        assert_eq!(lines[0].spans.last().unwrap().style, Style::default());
    }

    #[test]
    fn mono_theme_produces_no_styles() {
        let lines = highlight("SELECT 'x' -- c", &Theme::mono());
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .all(|span| span.style == Style::default())
        );
    }
}
