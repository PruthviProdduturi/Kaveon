//! One accent colour, a dim, a warning and an error: everything the shell
//! draws uses these five styles. Plain when the output is not a terminal.
use ratatui::style::{Color, Modifier, Style};

/// Kaveon blue, the Studio `--accent` (#4A9EE8), and its darker form for
/// light backgrounds (#2D7DD2).
pub const KAVEON_BLUE: Color = Color::Rgb(74, 158, 232);
pub const KAVEON_BLUE_DARK: Color = Color::Rgb(45, 125, 210);
/// Two lighter steps of the brand blue for the session context: the
/// catalog, then the schema, each a shade lighter than the word before it.
pub const KAVEON_BLUE_LIGHT: Color = Color::Rgb(128, 190, 240);
pub const KAVEON_BLUE_LIGHTER: Color = Color::Rgb(184, 218, 247);
/// SQL keywords in the editor: the lightest step, lighter than the schema.
pub const KAVEON_BLUE_LIGHTEST: Color = Color::Rgb(214, 234, 251);
/// On a light background the steps go darker instead.
pub const KAVEON_BLUE_DARKER: Color = Color::Rgb(30, 95, 165);
pub const KAVEON_BLUE_DARKEST: Color = Color::Rgb(20, 70, 125);
pub const KAVEON_BLUE_INK: Color = Color::Rgb(12, 50, 95);

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub accent: Style,
    pub dim: Style,
    pub warning: Style,
    pub error: Style,
    pub ok: Style,
    pub title: Style,
    /// The session catalog: a shade lighter than the accent.
    pub catalog: Style,
    /// The session schema: a shade lighter than the catalog.
    pub schema: Style,
    /// SQL keywords in the editor: lighter than the schema, bold.
    pub keyword: Style,
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
        let (accent, catalog, schema, keyword) = if flag == "light" {
            (
                KAVEON_BLUE_DARK,
                KAVEON_BLUE_DARKER,
                KAVEON_BLUE_DARKEST,
                KAVEON_BLUE_INK,
            )
        } else {
            (
                KAVEON_BLUE,
                KAVEON_BLUE_LIGHT,
                KAVEON_BLUE_LIGHTER,
                KAVEON_BLUE_LIGHTEST,
            )
        };
        Theme {
            accent: Style::default().fg(accent),
            dim: Style::default().fg(Color::DarkGray),
            warning: Style::default().fg(Color::Yellow),
            error: Style::default().fg(Color::Red),
            ok: Style::default().fg(Color::Green),
            title: Style::default().fg(accent).add_modifier(Modifier::BOLD),
            catalog: Style::default().fg(catalog),
            schema: Style::default().fg(schema),
            keyword: Style::default().fg(keyword).add_modifier(Modifier::BOLD),
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
            catalog: none,
            schema: none,
            keyword: none,
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
