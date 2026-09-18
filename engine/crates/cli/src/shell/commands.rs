//! Dot commands: `.cluster`, `.settings`, `.format`, `.history`, `.queries`,
//! `.kill`, `.timing`, `.help`, `.clear`, `.quit`, and the session settings
//! `.settings` edits, validated against the ranges the coordinator enforces.
use crate::theme::Theme;
use ratatui::text::{Line, Span};
use serde_json::{Map, Value};

/// The dot commands this module parses. Metadata commands (`.catalogs`,
/// `.schemas`, `.tables`, `.describe`, `.use`) and `.limit` keep their
/// existing paths; `parse` leaves them alone.
pub const DOT_COMMANDS: &[&str] = &[
    ".cluster",
    ".settings",
    ".format",
    ".history",
    ".queries",
    ".kill",
    ".timing",
    ".help",
    ".clear",
    ".quit",
];

/// Dot commands that other modules own; `parse` returns `None` for them so
/// the shell routes them as before, and they count for suggestions.
const ELSEWHERE: &[&str] = &[
    ".catalogs",
    ".schemas",
    ".tables",
    ".describe",
    ".desc",
    ".use",
    ".limit",
];

const HISTORY_DEFAULT: usize = 20;
const HISTORY_MAX: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Cluster,
    /// `None` shows the session settings; `Some((key, value))` sets one.
    Settings(Option<(String, String)>),
    SettingsReset,
    Format(String),
    History(usize),
    Queries,
    Kill(String),
    Timing,
    Help,
    Clear,
    Quit,
}

/// `None` when `text` is not a dot command this module owns; `Err` when it
/// is one with bad arguments, or an unknown dot command.
pub fn parse(text: &str) -> Option<Result<Command, String>> {
    let text = text.trim().trim_end_matches(';').trim();
    if !text.starts_with('.') {
        return None;
    }
    let mut words = text.split_whitespace();
    let word = words.next()?;
    let command = word.to_ascii_lowercase();
    let arguments: Vec<&str> = words.collect();
    if ELSEWHERE.contains(&command.as_str()) {
        return None;
    }
    let none_expected = |command: Command| -> Result<Command, String> {
        if arguments.is_empty() {
            Ok(command)
        } else {
            Err(format!("{word} takes no arguments"))
        }
    };
    Some(match command.as_str() {
        ".cluster" => none_expected(Command::Cluster),
        ".queries" => none_expected(Command::Queries),
        ".timing" => none_expected(Command::Timing),
        ".help" | ".h" | ".?" => none_expected(Command::Help),
        ".clear" => none_expected(Command::Clear),
        ".quit" | ".exit" | ".q" => none_expected(Command::Quit),
        ".settings" => parse_settings(&arguments),
        ".format" => match arguments.as_slice() {
            [name] => crate::output::OutputFormat::parse(name)
                .map(|_| Command::Format((*name).to_owned()))
                .map_err(|error| {
                    format!(
                        "{error}; formats: ALIGNED, VERTICAL, AUTO, MARKDOWN, CSV, TSV, JSON, NULL"
                    )
                }),
            _ => Err("usage: .format <name>".to_owned()),
        },
        ".history" => match arguments.as_slice() {
            [] => Ok(Command::History(HISTORY_DEFAULT)),
            [count] => match count.replace(['_', ','], "").parse::<usize>() {
                Ok(count) if (1..=HISTORY_MAX).contains(&count) => Ok(Command::History(count)),
                _ => Err(format!(
                    "invalid history count '{count}'; use a number from 1 to {HISTORY_MAX}"
                )),
            },
            _ => Err("usage: .history [n]".to_owned()),
        },
        ".kill" => match arguments.as_slice() {
            [id] => Ok(Command::Kill((*id).to_owned())),
            _ => Err("usage: .kill <query id>".to_owned()),
        },
        unknown => {
            let known: Vec<&str> = DOT_COMMANDS.iter().chain(ELSEWHERE).copied().collect();
            Err(match closest(unknown, &known) {
                Some(suggestion) => {
                    format!("unknown command '{word}'; did you mean {suggestion}?")
                }
                None => format!("unknown command '{word}'; type .help for commands"),
            })
        }
    })
}

fn parse_settings(arguments: &[&str]) -> Result<Command, String> {
    match arguments {
        [] => Ok(Command::Settings(None)),
        [word] if word.eq_ignore_ascii_case("reset") => Ok(Command::SettingsReset),
        [assignment] if assignment.contains('=') => {
            let (key, value) = assignment
                .split_once('=')
                .expect("the assignment contains '='");
            validate(key, value)
                .map(|(key, _)| Command::Settings(Some((key, value.trim().to_owned()))))
        }
        [key, value] => {
            validate(key, value).map(|(key, _)| Command::Settings(Some((key, (*value).to_owned()))))
        }
        [key] => Err(format!(
            "usage: .settings {key} <value>; keys: {}",
            KEYS.join(", ")
        )),
        _ => Err("usage: .settings [<key> <value> | reset]".to_owned()),
    }
}

/// The `.settings` keys and the server names they map to.
const KEYS: [&str; 4] = ["memory", "parallelism", "cache", "admission_wait"];
const SERVER_NAMES: [&str; 4] = [
    "query_memory_limit_bytes",
    "local_parallelism",
    "result_cache",
    "admission_wait_seconds",
];
const PARALLELISM_MAX: u64 = 1024;
const ADMISSION_WAIT_MAX: u64 = 86_400;

/// The `.settings` key, normalised, and the value the server takes.
fn validate(key: &str, value: &str) -> Result<(String, Value), String> {
    let key = key.trim().to_ascii_lowercase();
    let value = value.trim();
    match key.as_str() {
        "memory" => parse_bytes(value)
            .filter(|bytes| *bytes >= 1)
            .map(|bytes| (key, Value::from(bytes)))
            .ok_or_else(|| {
                format!(
                    "invalid memory limit '{value}'; use bytes or a size such as 256MiB or 1GiB"
                )
            }),
        "parallelism" => match value.parse::<u64>() {
            Ok(threads) if (1..=PARALLELISM_MAX).contains(&threads) => {
                Ok((key, Value::from(threads)))
            }
            _ => Err(format!(
                "invalid parallelism '{value}'; use a number from 1 to {PARALLELISM_MAX}"
            )),
        },
        "cache" => match value.to_ascii_lowercase().as_str() {
            "on" | "true" => Ok((key, Value::Bool(true))),
            "off" | "false" => Ok((key, Value::Bool(false))),
            _ => Err(format!("invalid cache setting '{value}'; use on or off")),
        },
        "admission_wait" => match value.parse::<u64>() {
            Ok(seconds) if seconds <= ADMISSION_WAIT_MAX => Ok((key, Value::from(seconds))),
            _ => Err(format!(
                "invalid admission wait '{value}'; use seconds from 0 to {ADMISSION_WAIT_MAX}"
            )),
        },
        _ => Err(match closest(&key, &KEYS) {
            Some(suggestion) => format!("unknown setting '{key}'; did you mean {suggestion}?"),
            None => format!("unknown setting '{key}'; keys: {}", KEYS.join(", ")),
        }),
    }
}

/// `1073741824`, `1GiB`, `256 MiB`, `1.5g`: binary multiples, whole bytes.
fn parse_bytes(text: &str) -> Option<u64> {
    let text = text.trim().replace(['_', ','], "");
    let split = text
        .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number = number.parse::<f64>().ok()?;
    let scale = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1u64,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return None,
    };
    let bytes = number * scale as f64;
    if !bytes.is_finite() || bytes < 0.0 || bytes > u64::MAX as f64 {
        return None;
    }
    Some(bytes as u64)
}

fn server_name(key: &str) -> &'static str {
    KEYS.iter()
        .position(|candidate| *candidate == key)
        .map(|index| SERVER_NAMES[index])
        .unwrap_or_default()
}

/// The session's `settings` object, keyed by the server's names, sent with
/// every statement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSettings {
    values: Map<String, Value>,
}

impl SessionSettings {
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let (key, value) = validate(key, value)?;
        self.values.insert(server_name(&key).to_owned(), value);
        Ok(())
    }

    pub fn reset(&mut self) {
        self.values.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// `None` when nothing is set, so the request omits `settings`.
    pub fn as_map(&self) -> Option<Map<String, Value>> {
        (!self.values.is_empty()).then(|| self.values.clone())
    }

    /// One line per setting: the `.settings` key, the value as people read
    /// it, and the server name it is sent under.
    pub fn lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        if self.values.is_empty() {
            return vec![
                Line::from(Span::styled(
                    "  no session settings; the coordinator's defaults apply",
                    theme.dim,
                )),
                Line::from(Span::styled(
                    "  .settings <key> <value> with memory, parallelism, cache or admission_wait",
                    theme.dim,
                )),
            ];
        }
        KEYS.iter()
            .zip(SERVER_NAMES)
            .filter_map(|(key, name)| {
                let value = self.values.get(name)?;
                let shown = match (*key, value) {
                    ("memory", Value::Number(number)) => number
                        .as_u64()
                        .map(crate::render::human_bytes)
                        .unwrap_or_else(|| value.to_string()),
                    ("admission_wait", Value::Number(number)) => format!("{number} s"),
                    ("cache", Value::Bool(flag)) => if *flag { "on" } else { "off" }.to_owned(),
                    _ => value.to_string(),
                };
                Some(Line::from(vec![
                    Span::styled(format!("  {key:<15}"), theme.accent),
                    Span::raw(format!("{shown:<12}")),
                    Span::styled(format!("{name} = {value}"), theme.dim),
                ]))
            })
            .collect()
    }
}

/// The candidate within a small edit distance of `word`, if any.
fn closest<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let word = word.to_ascii_lowercase();
    candidates
        .iter()
        .map(|candidate| {
            (
                edit_distance(&word, &candidate.to_ascii_lowercase()),
                *candidate,
            )
        })
        .filter(|(distance, candidate)| *distance <= (candidate.len() / 3).max(2))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ca != cb);
            current.push(substitute.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::to_plain;

    fn ok(text: &str) -> Command {
        parse(text).expect("a dot command").expect("valid")
    }

    fn err(text: &str) -> String {
        parse(text).expect("a dot command").expect_err("invalid")
    }

    #[test]
    fn plain_sql_and_commands_owned_elsewhere_are_not_parsed() {
        assert!(parse("SELECT 1;").is_none());
        assert!(parse("  ").is_none());
        assert!(parse(".tables").is_none());
        assert!(parse(".use OpenSource.kaveon_product").is_none());
        assert!(parse(".limit 500").is_none());
        assert!(parse(".describe t").is_none());
    }

    #[test]
    fn simple_commands_parse_and_refuse_arguments() {
        assert_eq!(ok(".cluster"), Command::Cluster);
        assert_eq!(ok(" .CLUSTER ; "), Command::Cluster);
        assert_eq!(ok(".queries"), Command::Queries);
        assert_eq!(ok(".timing"), Command::Timing);
        assert_eq!(ok(".help"), Command::Help);
        assert_eq!(ok(".clear"), Command::Clear);
        assert_eq!(ok(".quit"), Command::Quit);
        assert_eq!(ok(".exit"), Command::Quit);
        assert_eq!(ok(".q"), Command::Quit);
        assert_eq!(err(".cluster now"), ".cluster takes no arguments");
    }

    #[test]
    fn format_history_and_kill_take_their_arguments() {
        assert_eq!(ok(".format VERTICAL"), Command::Format("VERTICAL".into()));
        assert!(err(".format bogus").starts_with("unsupported output format 'bogus'"));
        assert_eq!(err(".format"), "usage: .format <name>");
        assert_eq!(ok(".history"), Command::History(20));
        assert_eq!(ok(".history 5"), Command::History(5));
        assert_eq!(ok(".history 1,000"), Command::History(1000));
        assert!(err(".history 0").contains("1 to 1000"));
        assert!(err(".history many").contains("invalid history count 'many'"));
        assert_eq!(ok(".kill 20260917_1"), Command::Kill("20260917_1".into()));
        assert_eq!(err(".kill"), "usage: .kill <query id>");
    }

    #[test]
    fn unknown_commands_suggest_the_closest_one() {
        assert_eq!(
            err(".clustr"),
            "unknown command '.clustr'; did you mean .cluster?"
        );
        assert_eq!(
            err(".tabels"),
            "unknown command '.tabels'; did you mean .tables?"
        );
        assert_eq!(
            err(".zzzzzzzz"),
            "unknown command '.zzzzzzzz'; type .help for commands"
        );
    }

    #[test]
    fn settings_parse_show_set_and_reset() {
        assert_eq!(ok(".settings"), Command::Settings(None));
        assert_eq!(ok(".settings reset"), Command::SettingsReset);
        assert_eq!(
            ok(".settings memory 1GiB"),
            Command::Settings(Some(("memory".into(), "1GiB".into())))
        );
        assert_eq!(
            ok(".settings cache=off"),
            Command::Settings(Some(("cache".into(), "off".into())))
        );
        assert!(err(".settings memory").starts_with("usage: .settings memory <value>"));
        assert_eq!(
            err(".settings paralelism 4"),
            "unknown setting 'paralelism'; did you mean parallelism?"
        );
        assert!(err(".settings memory zero").contains("256MiB or 1GiB"));
        assert!(err(".settings parallelism 0").contains("1 to 1024"));
        assert!(err(".settings parallelism 1025").contains("1 to 1024"));
        assert!(err(".settings cache maybe").contains("on or off"));
        assert!(err(".settings admission_wait 86401").contains("0 to 86400"));
    }

    #[test]
    fn bytes_accept_plain_and_suffixed_sizes() {
        assert_eq!(parse_bytes("1073741824"), Some(1 << 30));
        assert_eq!(parse_bytes("256MiB"), Some(256 << 20));
        assert_eq!(parse_bytes("1GiB"), Some(1 << 30));
        assert_eq!(parse_bytes("1 gb"), Some(1 << 30));
        assert_eq!(parse_bytes("1.5g"), Some(3 << 29));
        assert_eq!(parse_bytes("512k"), Some(512 << 10));
        assert_eq!(parse_bytes("0"), Some(0));
        assert_eq!(parse_bytes("1MB extra"), None);
        assert_eq!(parse_bytes("-1"), None);
        assert_eq!(parse_bytes("lots"), None);
    }

    #[test]
    fn session_settings_store_the_server_names() {
        let mut settings = SessionSettings::default();
        assert!(settings.as_map().is_none());
        settings.set("memory", "256MiB").unwrap();
        settings.set("parallelism", "8").unwrap();
        settings.set("cache", "off").unwrap();
        settings.set("admission_wait", "30").unwrap();
        let map = settings.as_map().unwrap();
        assert_eq!(map["query_memory_limit_bytes"], Value::from(256u64 << 20));
        assert_eq!(map["local_parallelism"], Value::from(8u64));
        assert_eq!(map["result_cache"], Value::Bool(false));
        assert_eq!(map["admission_wait_seconds"], Value::from(30u64));
        assert_eq!(map.len(), 4);
        settings.set("memory", "1GiB").unwrap();
        assert_eq!(
            settings.as_map().unwrap()["query_memory_limit_bytes"],
            Value::from(1u64 << 30)
        );
        assert_eq!(
            settings.set("memory", "0").unwrap_err(),
            "invalid memory limit '0'; use bytes or a size such as 256MiB or 1GiB"
        );
        settings.reset();
        assert!(settings.is_empty());
        assert!(settings.as_map().is_none());
    }

    #[test]
    fn settings_lines_read_naturally() {
        let theme = Theme::mono();
        let mut settings = SessionSettings::default();
        let empty = to_plain(&settings.lines(&theme));
        assert!(empty.contains("no session settings"));
        settings.set("memory", "1GiB").unwrap();
        settings.set("cache", "on").unwrap();
        settings.set("admission_wait", "5").unwrap();
        let text = to_plain(&settings.lines(&theme));
        assert!(
            text.contains("  memory         1.0 GiB     query_memory_limit_bytes = 1073741824"),
            "{text}"
        );
        assert!(
            text.contains("  cache          on          result_cache = true"),
            "{text}"
        );
        assert!(
            text.contains("  admission_wait 5 s         admission_wait_seconds = 5"),
            "{text}"
        );
        assert!(!text.contains("parallelism"));
    }
}
