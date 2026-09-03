use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_SERVER: &str = "http://localhost:8080";
const DEFAULT_CATALOG: &str = "kaveon";
const DEFAULT_SCHEMA: &str = "default";
const DEFAULT_SOURCE: &str = "kaveon-cli";
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    Table,
    Csv,
    Tsv,
    Json,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    Run(Box<Options>),
    Help,
    Version,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Options {
    pub local: bool,
    pub server: String,
    pub catalog: String,
    pub schema: String,
    pub user: String,
    pub source: String,
    pub client_tags: Vec<String>,
    pub execute: Option<String>,
    pub output_format: OutputFormat,
    pub timeout: Duration,
    pub data_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut options = Options {
        local: false,
        server: DEFAULT_SERVER.to_owned(),
        catalog: DEFAULT_CATALOG.to_owned(),
        schema: DEFAULT_SCHEMA.to_owned(),
        user: default_user(),
        source: DEFAULT_SOURCE.to_owned(),
        client_tags: Vec::new(),
        execute: None,
        output_format: OutputFormat::Table,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
        data_dir: None,
        config_path: None,
    };
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
            "--server" => options.server = take_value(args, &mut index, option)?,
            "--catalog" => options.catalog = take_value(args, &mut index, option)?,
            "--schema" => options.schema = take_value(args, &mut index, option)?,
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
                options.output_format = match value.as_str() {
                    "table" => OutputFormat::Table,
                    "csv" => OutputFormat::Csv,
                    "tsv" => OutputFormat::Tsv,
                    "json" => OutputFormat::Json,
                    _ => return Err(format!("unsupported output format '{value}'")),
                };
            }
            "--timeout" => {
                let value = take_value(args, &mut index, option)?;
                let seconds = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid timeout '{value}': expected seconds"))?;
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
            unknown => return Err(format!("unknown argument '{unknown}'")),
        }
    }
    if !options.local && (options.data_dir.is_some() || options.config_path.is_some()) {
        return Err("--data-dir and --config require --local".to_owned());
    }
    Ok(Command::Run(Box::new(options)))
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
}
