//! The styled result table for the terminal: light box-drawing borders, the
//! header in the accent colour, numbers right-aligned with thousands
//! separators, `NULL` dimmed, and string columns narrowed to the terminal.
//! `output.rs` keeps the plain form that scripts and tests depend on.
use crate::render::thousands;
use crate::theme::Theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The width a table is fitted to when the caller does not know the terminal.
const DEFAULT_WIDTH: usize = 120;
/// String columns are never narrowed below this.
const MIN_COLUMN_WIDTH: usize = 8;
const ELLIPSIS: char = '…';

enum Cell {
    Null,
    Number(String),
    Text(String),
}

impl Cell {
    fn from_value(value: Option<&Value>) -> Cell {
        match value.unwrap_or(&Value::Null) {
            Value::Null => Cell::Null,
            Value::Number(number) => Cell::Number(if let Some(int) = number.as_i64() {
                thousands(i128::from(int))
            } else if let Some(int) = number.as_u64() {
                thousands(i128::from(int))
            } else {
                format!("{:.4}", number.as_f64().unwrap_or(0.0))
            }),
            Value::String(text) => Cell::Text(terminal_text(text)),
            other => Cell::Text(terminal_text(&other.to_string())),
        }
    }

    fn text(&self) -> &str {
        match self {
            Cell::Null => "NULL",
            Cell::Number(text) | Cell::Text(text) => text,
        }
    }

    fn width(&self) -> usize {
        self.text().width()
    }
}

/// Renders `rows` under `names`; the flag says whether any column was
/// narrowed so the summary can point at the vertical format.
pub fn styled(
    names: &[String],
    rows: &[Vec<Value>],
    width: Option<usize>,
    theme: &Theme,
) -> (Vec<Line<'static>>, bool) {
    if names.is_empty() {
        return (vec![Line::raw(format!("({} rows)", rows.len()))], false);
    }
    let names: Vec<String> = names.iter().map(|name| terminal_text(name)).collect();
    let cells: Vec<Vec<Cell>> = rows
        .iter()
        .map(|row| {
            (0..names.len())
                .map(|index| Cell::from_value(row.get(index)))
                .collect()
        })
        .collect();
    let mut widths: Vec<usize> = names.iter().map(|name| name.width()).collect();
    let mut shrinkable = vec![false; names.len()];
    let mut numeric = vec![false; names.len()];
    for row in &cells {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.width());
            shrinkable[index] |= matches!(cell, Cell::Text(_));
            numeric[index] |= matches!(cell, Cell::Number(_));
        }
    }
    let truncated = fit(&mut widths, &shrinkable, width.unwrap_or(DEFAULT_WIDTH));

    let mut lines = Vec::with_capacity(rows.len() + 4);
    lines.push(border(&widths, '┌', '┬', '┐'));
    lines.push(Line::from(row_spans(
        names.iter().enumerate().map(|(index, name)| {
            (
                name.as_str(),
                numeric[index] && !shrinkable[index],
                theme.accent,
            )
        }),
        &widths,
    )));
    lines.push(border(&widths, '├', '┼', '┤'));
    for row in &cells {
        lines.push(Line::from(row_spans(
            row.iter().map(|cell| {
                (
                    cell.text(),
                    matches!(cell, Cell::Number(_)),
                    if matches!(cell, Cell::Null) {
                        theme.dim
                    } else {
                        Style::default()
                    },
                )
            }),
            &widths,
        )));
    }
    lines.push(border(&widths, '└', '┴', '┘'));
    (lines, truncated)
}

/// Total width of a table with these column widths: each column is padded
/// by a space on either side, and there are `n + 1` border characters.
fn table_width(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + widths.len() * 3 + 1
}

/// Narrows the widest string columns, one at a time, until the table fits
/// or every string column is at the minimum. Returns whether any changed.
fn fit(widths: &mut [usize], shrinkable: &[bool], limit: usize) -> bool {
    let mut truncated = false;
    loop {
        let total = table_width(widths);
        if total <= limit {
            return truncated;
        }
        let excess = total - limit;
        let Some(widest) = (0..widths.len())
            .filter(|&index| shrinkable[index] && widths[index] > MIN_COLUMN_WIDTH)
            .max_by_key(|&index| widths[index])
        else {
            return truncated;
        };
        let runner_up = (0..widths.len())
            .filter(|&index| index != widest && shrinkable[index])
            .map(|index| widths[index])
            .max()
            .unwrap_or(0);
        let target = widths[widest]
            .saturating_sub(excess)
            .max(runner_up)
            .max(MIN_COLUMN_WIDTH)
            .min(widths[widest] - 1);
        widths[widest] = target;
        truncated = true;
    }
}

fn border(widths: &[usize], left: char, middle: char, right: char) -> Line<'static> {
    let mut text = String::with_capacity(table_width(widths));
    text.push(left);
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            text.push(middle);
        }
        text.extend(std::iter::repeat_n('─', width + 2));
    }
    text.push(right);
    Line::raw(text)
}

/// `│ cell │ cell │` from (text, right-aligned, style) triples.
fn row_spans<'a>(
    cells: impl Iterator<Item = (&'a str, bool, Style)>,
    widths: &[usize],
) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(widths.len() * 2 + 1);
    spans.push(Span::raw("│"));
    for (index, (text, right, style)) in cells.enumerate() {
        let width = widths[index];
        let text = clip(text, width);
        let padding = " ".repeat(width.saturating_sub(text.width()));
        let content = if right {
            format!(" {padding}{text} ")
        } else {
            format!(" {text}{padding} ")
        };
        spans.push(Span::styled(content, style));
        spans.push(Span::raw("│"));
    }
    spans
}

/// The text as-is when it fits, else as much as fits before `…`.
fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let budget = width.saturating_sub(ELLIPSIS.width().unwrap_or(1));
    let mut used = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        used += w;
        out.push(ch);
    }
    out.push(ELLIPSIS);
    out
}

/// Line breaks, tabs and other control characters shown as escapes so a
/// value can never move the cursor or clear the screen.
fn terminal_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{{{:04x}}}", character as u32));
            }
            character => output.push(character),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::to_plain;
    use serde_json::json;

    #[test]
    fn styled_table_uses_box_drawing_right_aligns_numbers_and_truncates() {
        let names = vec!["region".into(), "events".into()];
        let rows = vec![
            vec![json!("Europe"), json!(4647390)],
            vec![json!(null), json!(12)],
        ];
        let (lines, truncated) = styled(&names, &rows, Some(40), &Theme::mono());
        let text = to_plain(&lines);
        assert!(!truncated);
        assert_eq!(
            text,
            "┌────────┬───────────┐\n│ region │    events │\n├────────┼───────────┤\n│ Europe │ 4,647,390 │\n│ NULL   │        12 │\n└────────┴───────────┘\n"
        );
        let wide = vec![vec![json!("x".repeat(80)), json!(1)]];
        let (lines, truncated) = styled(&names, &wide, Some(40), &Theme::mono());
        let text = to_plain(&lines);
        assert!(truncated);
        assert!(text.lines().all(|l| l.chars().count() <= 40), "{text}");
        assert!(text.contains('…'));
        assert!(text.contains("│ events │"), "{text}");
    }

    #[test]
    fn null_unicode_floats_and_control_characters_are_shown_safely() {
        let names = vec!["na\x1bme".into(), "ratio".into(), "flag".into()];
        let rows = vec![
            vec![json!("ok\x1b[2J🙂\nnext"), json!(0.5), json!(true)],
            vec![json!(null), json!(-1234567.25), json!(null)],
        ];
        let (lines, truncated) = styled(&names, &rows, None, &Theme::mono());
        let text = to_plain(&lines);
        assert!(!truncated);
        assert!(!text.contains('\x1b'));
        assert_eq!(
            text,
            "┌───────────────────────┬───────────────┬──────┐\n│ na\\u{001b}me          │         ratio │ flag │\n├───────────────────────┼───────────────┼──────┤\n│ ok\\u{001b}[2J🙂\\nnext │        0.5000 │ true │\n│ NULL                  │ -1234567.2500 │ NULL │\n└───────────────────────┴───────────────┴──────┘\n"
        );
        let (lines, _) = styled(&[], &rows, None, &Theme::mono());
        assert_eq!(to_plain(&lines), "(2 rows)\n");
    }

    #[test]
    fn wide_characters_are_clipped_by_display_width() {
        let names = vec!["a".into(), "b".into()];
        let rows = vec![vec![json!("🙂".repeat(20)), json!("y".repeat(30))]];
        let (lines, truncated) = styled(&names, &rows, Some(40), &Theme::mono());
        assert!(truncated);
        for line in &lines {
            assert!(to_plain(std::slice::from_ref(line)).trim_end().width() <= 40);
        }
    }

    #[test]
    fn header_is_accented_and_null_is_dimmed() {
        let theme = Theme::detect("dark", true);
        if theme.plain {
            return;
        }
        let names = vec!["n".into()];
        let rows = vec![vec![json!(null)], vec![json!(1)]];
        let (lines, _) = styled(&names, &rows, None, &theme);
        assert_eq!(lines[1].spans[1].style, theme.accent);
        assert_eq!(lines[3].spans[1].style, theme.dim);
        assert_eq!(lines[4].spans[1].style, Style::default());
    }
}
