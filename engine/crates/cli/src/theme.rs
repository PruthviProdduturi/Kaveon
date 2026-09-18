//! One accent colour, a dim, a warning and an error: everything the shell
//! draws uses these five styles. Plain when the output is not a terminal.
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub accent: Style,
    pub dim: Style,
    pub warning: Style,
    pub error: Style,
    pub ok: Style,
    pub title: Style,
    /// No colour: `--theme mono`, `NO_COLOR`, `TERM=dumb`, or not a TTY.
    pub plain: bool,
}

impl Theme {
    pub fn detect(flag: &str, stdout_is_tty: bool) -> Theme {
        let plain = !stdout_is_tty
            || flag == "mono"
            || std::env::var_os("NO_COLOR").is_some()
            || std::env::var("TERM").is_ok_and(|term| term == "dumb");
        if plain {
            return Theme::mono();
        }
        let accent = if flag == "light" {
            Color::Blue
        } else {
            Color::Cyan
        };
        Theme {
            accent: Style::default().fg(accent),
            dim: Style::default().fg(Color::DarkGray),
            warning: Style::default().fg(Color::Yellow),
            error: Style::default().fg(Color::Red),
            ok: Style::default().fg(Color::Green),
            title: Style::default().fg(accent).add_modifier(Modifier::BOLD),
            plain: false,
        }
    }

    pub fn mono() -> Theme {
        let none = Style::default();
        Theme {
            accent: none,
            dim: none,
            warning: none,
            error: none,
            ok: none,
            title: none,
            plain: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_tty_and_mono_are_plain() {
        assert!(Theme::detect("dark", false).plain);
        assert!(Theme::detect("mono", true).plain);
        assert!(Theme::mono().accent.fg.is_none());
    }
}
