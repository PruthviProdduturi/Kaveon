//! Everything the client prints. `to_plain` strips styling for tests and
//! for output that is not a terminal.
pub mod cluster;
pub mod error;
pub mod help;
pub mod plan;
pub mod summary;
pub mod table;

use ratatui::text::Line;

/// The lines as text, no styling: what non-TTY output and tests see.
pub fn to_plain(lines: &[Line<'_>]) -> String {
    let mut out = String::new();
    for line in lines {
        for span in &line.spans {
            out.push_str(&span.content);
        }
        out.push('\n');
    }
    out
}

/// The lines with ANSI colour for a terminal outside the viewport (the
/// header before the shell starts, batch-mode panels on a TTY).
pub fn to_ansi(lines: &[Line<'_>]) -> String {
    use ratatui::style::{Color, Modifier};
    let mut out = String::new();
    for line in lines {
        for span in &line.spans {
            let mut codes = Vec::new();
            if span.style.add_modifier.contains(Modifier::BOLD) {
                codes.push("1".to_owned());
            }
            if let Some(Color::Rgb(r, g, b)) = span.style.fg {
                codes.push(format!("38;2;{r};{g};{b}"));
            } else if let Some(color) = span.style.fg {
                let code = match color {
                    Color::Black => 30,
                    Color::Red => 31,
                    Color::Green => 32,
                    Color::Yellow => 33,
                    Color::Blue => 34,
                    Color::Magenta => 35,
                    Color::Cyan => 36,
                    Color::Gray => 37,
                    Color::DarkGray => 90,
                    Color::LightRed => 91,
                    Color::LightGreen => 92,
                    Color::LightYellow => 93,
                    Color::LightBlue => 94,
                    Color::LightMagenta => 95,
                    Color::LightCyan => 96,
                    Color::White => 97,
                    _ => 39,
                };
                codes.push(code.to_string());
            }
            if codes.is_empty() {
                out.push_str(&span.content);
            } else {
                out.push_str(&format!("\x1b[{}m{}\x1b[0m", codes.join(";"), span.content));
            }
        }
        out.push('\n');
    }
    out
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn human_count(count: u64) -> String {
    match count {
        0..=9_999 => count.to_string(),
        10_000..=999_999 => format!("{:.1}K", count as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", count as f64 / 1e6),
        _ => format!("{:.2}B", count as f64 / 1e9),
    }
}

pub fn thousands(value: i128) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if negative { format!("-{out}") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_wraps_styled_spans_only() {
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::Span;
        let line = Line::from(vec![
            Span::raw("a"),
            Span::styled(
                "b",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        assert_eq!(to_ansi(&[line]), "a\x1b[1;36mb\x1b[0m\n");
    }

    #[test]
    fn human_units_read_naturally() {
        assert_eq!(human_bytes(4 * 1024 * 1024 * 1024), "4.0 GiB");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_count(18_000_000), "18.0M");
        assert_eq!(human_count(4_321), "4321");
        assert_eq!(thousands(4_647_390), "4,647,390");
        assert_eq!(thousands(-12), "-12");
    }
}
