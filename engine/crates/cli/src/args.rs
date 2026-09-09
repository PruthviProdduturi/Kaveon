use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_SERVER: &str = "http://localhost:8080";
const DEFAULT_CATALOG: &str = "kaveon";
const DEFAULT_SCHEMA: &str = "default";
const DEFAULT_SOURCE: &str = "kaveon-cli";
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

pub use crate::output::OutputFormat;

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    Run(Box<Options>),
    Help,
    Version,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Options {
    pub auth: String,
    pub access_token: Option<String>,
    pub disable_auto_suggestion: bool,
    pub ca_cert: Option<PathBuf>,
    pub local: bool,
    pub server: String,
    pub catalog: String,
    pub schema: String,
    pub user: String,
    pub source: String,
    pub client_tags: Vec<String>,
    pub execute: Option<String>,
    pub file: Option<PathBuf>,
    pub ignore_errors: bool,
    pub no_history: bool,
    pub history_file: Option<PathBuf>,
    pub editing_mode: String,
    pub pager: Option<String>,
    pub output_format_interactive: Option<OutputFormat>,
    pub output_format: OutputFormat,
    pub timeout: Duration,
    pub data_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
}

fn normalize_args(args: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut position = 0;
    while position < args.len() {
        let arg = &args[position];
        if position > 0
            && arg.starts_with("--")
            && let Some((key, value)) = arg.split_once('=')
        {
            normalized.push(key.to_owned());
            normalized.push(value.to_owned());
        } else {
            normalized.push(arg.clone());
            let is_flag = matches!(
                arg.as_str(),
                "--local"
                    | "--ignore-errors"
                    | "--no-history"
                    | "--disable-auto-suggestion"
                    | "--help"
                    | "-h"
                    | "--version"
                    | "-V"
            );
            if position > 0
                && arg.starts_with('-')
                && !is_flag
                && let Some(value) = args.get(position + 1)
            {
                normalized.push(value.clone());
                position += 1;
            }
        }
        position += 1;
    }
    normalized
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let normalized = normalize_args(args);
    let args = normalized.as_slice();
    let mut options = Options {
        auth: "auto".to_owned(),
        access_token: None,
        disable_auto_suggestion: false,
        ca_cert: std::env::var_os("KAVEON_CA_CERT").map(PathBuf::from),
        local: false,
        server: DEFAULT_SERVER.to_owned(),
        catalog: DEFAULT_CATALOG.to_owned(),
        schema: DEFAULT_SCHEMA.to_owned(),
        user: default_user(),
        source: DEFAULT_SOURCE.to_owned(),
        client_tags: Vec::new(),
        execute: None,
        file: None,
        ignore_errors: false,
        no_history: false,
        history_file: std::env::var_os("KAVEON_HISTORY_FILE").map(PathBuf::from),
        editing_mode: "EMACS".to_owned(),
        pager: std::env::var("KAVEON_PAGER").ok(),
        output_format_interactive: None,
        output_format: OutputFormat::Table,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
        data_dir: None,
        config_path: None,
    };
    let mut positional_server = false;
    let mut explicit_server = false;
    let mut explicit_catalog = false;
    let mut explicit_schema = false;
    let mut index = 1;
    while index < args.len() {
        let option = args[index].as_str();
        match option {
            "--help" | "-h" => return Ok(Command::Help),
            "--version" | "-V" => return Ok(Command::Version),
            "--local" => {
                options.local = true;
                index += 1;
            }
            "--auth" => {
                options.auth = take_value(args, &mut index, option)?;
                if !matches!(
                    options.auth.as_str(),
                    "auto" | "azure-cli" | "microsoft" | "none"
                ) {
                    return Err("--auth expects auto, azure-cli, microsoft, or none".to_owned());
                }
            }
            "--access-token" => options.access_token = Some(take_value(args, &mut index, option)?),
            "--disable-auto-suggestion" => {
                options.disable_auto_suggestion = true;
                index += 1;
            }
            "--ca-cert" => {
                options.ca_cert = Some(PathBuf::from(take_value(args, &mut index, option)?))
            }
            "--server" => {
                explicit_server = true;
                options.server = take_value(args, &mut index, option)?;
            }
            "--catalog" => {
                explicit_catalog = true;
                options.catalog = take_value(args, &mut index, option)?;
            }
            "--schema" => {
                explicit_schema = true;
                options.schema = take_value(args, &mut index, option)?;
            }
            "--user" => options.user = take_value(args, &mut index, option)?,
            "--source" => options.source = take_value(args, &mut index, option)?,
            "--client-tags" => {
                options.client_tags = take_value(args, &mut index, option)?
                    .split(',')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            "--execute" | "-e" => options.execute = Some(take_value(args, &mut index, option)?),
            "--output-format" => {
                let value = take_value(args, &mut index, option)?;
                options.output_format = OutputFormat::parse(&value)?;
            }
            "--output-format-interactive" => {
                options.output_format_interactive =
                    Some(OutputFormat::parse(&take_value(args, &mut index, option)?)?)
            }
            "--file" | "-f" => {
                options.file = Some(PathBuf::from(take_value(args, &mut index, option)?))
            }
            "--history-file" => {
                options.history_file = Some(PathBuf::from(take_value(args, &mut index, option)?))
            }
            "--pager" => options.pager = Some(take_value(args, &mut index, option)?),
            "--editing-mode" => {
                options.editing_mode = take_value(args, &mut index, option)?.to_ascii_uppercase();
                if !matches!(options.editing_mode.as_str(), "VI" | "EMACS") {
                    return Err("--editing-mode expects VI or EMACS".to_owned());
                }
            }
            "--ignore-errors" => {
                options.ignore_errors = true;
                index += 1;
            }
            "--no-history" => {
                options.no_history = true;
                index += 1;
            }
            "--timeout" | "--client-request-timeout" => {
                let value = take_value(args, &mut index, option)?;
                let (number, multiplier) = if let Some(number) = value.strip_suffix('m') {
                    (number, 60)
                } else {
                    (value.strip_suffix('s').unwrap_or(&value), 1)
                };
                let seconds = number.parse::<u64>().ok().and_then(|n| n.checked_mul(multiplier))
                    .ok_or_else(|| format!("invalid timeout '{value}': expected seconds or a duration such as 30s or 2m"))?;
                if seconds == 0 {
                    return Err("--timeout must be greater than zero".to_owned());
                }
                options.timeout = Duration::from_secs(seconds);
            }
            "--data-dir" | "-d" => {
                options.data_dir = Some(PathBuf::from(take_value(args, &mut index, option)?));
            }
            "--config" | "-c" => {
                options.config_path = Some(PathBuf::from(take_value(args, &mut index, option)?));
            }
            url if url.starts_with("http://") || url.starts_with("https://") => {
                if positional_server {
                    return Err("only one server URL is allowed".to_owned());
                }
                positional_server = true;
                options.server = url.to_owned();
                index += 1;
            }
            unknown => return Err(format!("unknown argument '{unknown}'")),
        }
    }
    if positional_server && explicit_server {
        return Err("use either a positional URL or --server".to_owned());
    }
    let mut url =
        reqwest::Url::parse(&options.server).map_err(|_| "invalid server URL".to_owned())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "server URL must not contain credentials, query parameters, or a fragment".to_owned(),
        );
    }
    let segments: Vec<_> = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() > 2 {
        return Err("server URL path expects /catalog/schema".to_owned());
    }
    if let Some(catalog) = segments.first() {
        if explicit_catalog {
            return Err("catalog specified both in URL and --catalog".to_owned());
        }
        options.catalog = decode_segment(catalog)?;
    }
    if let Some(schema) = segments.get(1) {
        if explicit_schema {
            return Err("schema specified both in URL and --schema".to_owned());
        }
        options.schema = decode_segment(schema)?;
    }
    url.set_path("");
    options.server = url.to_string().trim_end_matches('/').to_owned();
    if options.file.is_some() && options.execute.is_some() {
        return Err("--file and --execute cannot be combined".to_owned());
    }
    if options.local && (options.file.is_some() || options.ignore_errors) {
        return Err("--file and --ignore-errors currently require remote mode".to_owned());
    }
    if !options.local && (options.data_dir.is_some() || options.config_path.is_some()) {
        return Err("--data-dir and --config require --local".to_owned());
    }
    Ok(Command::Run(Box::new(options)))
}

/// Load connection defaults without mixing them with the embedded catalog config.
pub fn parse_with_config(args: &[String]) -> Result<Command, String> {
    let normalized = normalize_args(args);
    let mut keys = std::collections::BTreeSet::new();
    let mut positional = false;
    let mut index = 1;
    while let Some(arg) = normalized.get(index) {
        if matches!(arg.as_str(), "--help" | "-h" | "--version" | "-V") {
            return parse(args);
        }
        if arg.starts_with('-') {
            keys.insert(arg.as_str());
            index += if matches!(
                arg.as_str(),
                "--local" | "--ignore-errors" | "--no-history" | "--disable-auto-suggestion"
            ) {
                1
            } else {
                2
            };
        } else {
            positional = true;
            index += 1;
        }
    }
    let explicit = std::env::var_os("KAVEON_CONFIG").map(PathBuf::from);
    let path = explicit.clone().or_else(|| {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|home| PathBuf::from(home).join(".kaveon_config"))
            .filter(|path| path.exists())
    });
    let Some(path) = path else {
        return parse(args);
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read CLI configuration {}: {error}", path.display()))?;
    let defaults = config_arguments(&text)?;
    let mut merged = vec![args.first().cloned().unwrap_or_else(|| "kaveon".into())];
    for pair in defaults.as_chunks::<2>().0 {
        // Explicit command-line options replace defaults, including URL context.
        let key = pair[0].as_str();
        if keys.contains(key)
            || (positional && matches!(key, "--server" | "--catalog" | "--schema"))
        {
            continue;
        }
        merged.extend_from_slice(pair);
    }
    merged.extend_from_slice(&args[1..]);
    parse(&merged)
}

fn config_arguments(text: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("CLI config line {} expects key=value", line_number + 1))?;
        let key = key.trim();
        if !matches!(
            key,
            "server"
                | "catalog"
                | "schema"
                | "user"
                | "source"
                | "client-tags"
                | "auth"
                | "ca-cert"
                | "timeout"
                | "output-format"
                | "output-format-interactive"
                | "history-file"
                | "editing-mode"
                | "pager"
        ) {
            return Err(format!(
                "unsupported CLI config key '{key}' on line {}",
                line_number + 1
            ));
        }
        args.push(format!("--{key}"));
        args.push(value.trim().to_owned());
    }
    Ok(args)
}

fn decode_segment(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let pair = bytes.get(i + 1..i + 3).ok_or("invalid URL escape")?;
            let hex = std::str::from_utf8(pair).map_err(|_| "invalid URL escape")?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| "invalid URL escape")?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| "URL context must be UTF-8".to_owned())
}

fn take_value(args: &[String], index: &mut usize, option: &str) -> Result<String, String> {
    let value = args
        .get(*index + 1)
        .ok_or_else(|| format!("{option} requires a value"))?
        .clone();
    *index += 2;
    Ok(value)
}

fn default_user() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn remote_mode_is_default() {
        let Command::Run(options) = parse(&strings(&["kaveon"])).unwrap() else {
            panic!("expected run command");
        };
        assert!(!options.local);
        assert_eq!(options.server, DEFAULT_SERVER);
    }

    #[test]
    fn parses_remote_connection_options() {
        let Command::Run(options) = parse(&strings(&[
            "kaveon",
            "--server",
            "https://engine.example",
            "--catalog",
            "lake",
            "--schema",
            "gold",
            "--user",
            "analyst",
            "--source",
            "ci",
            "--client-tags",
            "batch, nightly",
            "--execute",
            "SELECT 1",
            "--output-format",
            "json",
            "--timeout",
            "45",
        ]))
        .unwrap() else {
            panic!("expected run command")
        };
        assert_eq!(options.catalog, "lake");
        assert_eq!(options.schema, "gold");
        assert_eq!(options.client_tags, ["batch", "nightly"]);
        assert_eq!(options.output_format, OutputFormat::Json);
        assert_eq!(options.timeout, Duration::from_secs(45));
    }

    #[test]
    fn local_paths_require_explicit_local_mode() {
        let error = parse(&strings(&["kaveon", "--data-dir", "data"])).unwrap_err();
        assert!(error.contains("require --local"));
    }

    #[test]
    fn rejects_unknown_output_format() {
        let error = parse(&strings(&["kaveon", "--output-format", "xml"])).unwrap_err();
        assert!(error.contains("unsupported output format"));
    }

    #[test]
    fn accepts_azure_cli_auth() {
        let Command::Run(options) = parse(&strings(&["kaveon", "--auth", "azure-cli"])).unwrap()
        else {
            panic!("expected run command")
        };
        assert_eq!(options.auth, "azure-cli");
    }
    #[test]
    fn parses_batch_options_and_equals_syntax() {
        let Command::Run(options) = parse(&strings(&[
            "kaveon",
            "--file=queries.sql",
            "--ignore-errors",
            "--editing-mode=vi",
            "--pager=",
        ]))
        .unwrap() else {
            panic!("run");
        };
        assert_eq!(options.file, Some(PathBuf::from("queries.sql")));
        assert!(options.ignore_errors);
        assert_eq!(options.editing_mode, "VI");
        assert_eq!(options.pager, Some(String::new()));
        assert!(parse(&strings(&["kaveon", "-f", "q.sql", "-e", "SELECT 1"])).is_err());
    }

    #[test]
    fn url_context_and_conflicts_are_explicit() {
        let Command::Run(options) = parse(&strings(&[
            "kaveon",
            "https://localhost:18443/medallion/test%20schema",
        ]))
        .unwrap() else {
            panic!("run");
        };
        assert_eq!(options.server, "https://localhost:18443");
        assert_eq!(options.catalog, "medallion");
        assert_eq!(options.schema, "test schema");
        assert!(
            parse(&strings(&[
                "kaveon",
                "https://localhost/a/b",
                "--catalog",
                "c"
            ]))
            .is_err()
        );
        assert!(
            parse(&strings(&[
                "kaveon",
                "https://localhost",
                "--server",
                "http://localhost"
            ]))
            .is_err()
        );
        assert!(parse(&strings(&["kaveon", "https://user:secret@localhost"])).is_err());
    }

    #[test]
    fn config_defaults_do_not_allow_sql_or_secrets() {
        assert_eq!(
            config_arguments("# connection\nserver=https://localhost:18443\npager=\n").unwrap(),
            strings(&["--server", "https://localhost:18443", "--pager", ""])
        );
        assert!(config_arguments("execute=DROP TABLE x").is_err());
        assert!(config_arguments("access-token=secret").is_err());
        assert!(config_arguments("invalid line").is_err());
    }
    #[test]
    fn option_like_sql_is_never_reparsed_as_a_flag() {
        let Command::Run(options) =
            parse(&strings(&["kaveon", "-e", "-- label=value\nSELECT 1;"])).unwrap()
        else {
            panic!("run");
        };
        assert_eq!(
            options.execute.as_deref(),
            Some("-- label=value\nSELECT 1;")
        );
    }
}
