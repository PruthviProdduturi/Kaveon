use crate::args::{Options, OutputFormat};
use crate::auth::Session;
use crate::input::{self, ReadLine, TerminalInput};
use reqwest::blocking::Response;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlparser::dialect::GenericDialect;
use sqlparser::tokenizer::{Token, Tokenizer};
use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Clone, Debug, Deserialize)]
struct Column {
    name: String,
    #[serde(rename = "type")]
    _data_type: String,
}

#[derive(Debug, Deserialize)]
struct StatementResponse {
    id: String,
    state: String,
    #[serde(default)]
    columns: Vec<Column>,
    #[serde(default)]
    data: Vec<Vec<Value>>,
    error: Option<String>,
    elapsed_ms: u64,
}

#[derive(Serialize)]
struct StatementRequest<'a> {
    query: &'a str,
    catalog: &'a str,
    schema: &'a str,
    user: &'a str,
    source: &'a str,
    client: &'static str,
    client_tags: &'a [String],
    result_delivery: &'static str,
}

#[derive(Deserialize)]
struct CatalogList {
    catalogs: Vec<String>,
}

#[derive(Deserialize)]
struct SchemaList {
    schemas: Vec<String>,
}

#[derive(Deserialize)]
struct TableList {
    tables: Vec<String>,
}

#[derive(Deserialize)]
struct NamedDefinition {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct TableDefinition {
    name: String,
    columns: Vec<DefinitionColumn>,
}

#[derive(Deserialize)]
struct DefinitionColumn {
    name: String,
    #[serde(rename = "data_type")]
    data_type: Value,
    nullable: bool,
}

#[derive(Debug, Deserialize)]
struct QueryTelemetry {
    #[serde(default)]
    scan_metrics_complete: Option<bool>,
    #[serde(default)]
    stages: Vec<StageTelemetry>,
    #[serde(default)]
    scans: Vec<ScanTelemetry>,
}

#[derive(Debug, Deserialize)]
struct StageTelemetry {
    #[serde(default)]
    task_count: usize,
    #[serde(default)]
    completed_tasks: usize,
    #[serde(default)]
    tasks: Vec<TaskTelemetry>,
}

#[derive(Debug, Deserialize)]
struct TaskTelemetry {
    node_id: String,
}

#[derive(Debug, Deserialize)]
struct ScanTelemetry {
    #[serde(default)]
    rows_emitted: Option<u64>,
    rows_selected: u64,
    compressed_bytes_selected: u64,
}

#[derive(Debug, PartialEq)]
enum MetaCommand {
    Catalogs {
        like: Option<String>,
    },
    Schemas {
        catalog: String,
        like: Option<String>,
    },
    Tables {
        catalog: String,
        schema: String,
        like: Option<String>,
    },
    Describe {
        catalog: String,
        schema: String,
        table: String,
    },
    Use {
        catalog: String,
        schema: String,
    },
}

pub fn run(options: &mut Options) -> Result<(), String> {
    let client = crate::auth::Session::connect(options)?;

    if let Some(sql) = options.execute.clone() {
        execute_script(&client, options, &sql, options.ignore_errors)?;
        return Ok(());
    }
    if let Some(path) = options.file.clone() {
        let script = input::read_file(&path)?;
        execute_script(&client, options, &script, options.ignore_errors)?;
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        let mut script = String::new();
        io::stdin()
            .read_to_string(&mut script)
            .map_err(|error| format!("cannot read standard input: {error}"))?;
        execute_script(&client, options, &script, options.ignore_errors)?;
        return Ok(());
    }

    if let Some(format) = options.output_format_interactive {
        options.output_format = format;
    }

    println!(
        "Kaveon CLI v{} — {}.{}",
        env!("CARGO_PKG_VERSION"),
        options.catalog,
        options.schema
    );
    println!(
        "Connected to {}. Type .help for commands; terminate SQL with ;",
        options.server
    );
    println!();
    repl(&client, options)
}

fn repl(client: &Session, options: &mut Options) -> Result<(), String> {
    let mut input = TerminalInput::new(
        options.history_file.clone(),
        &options.editing_mode,
        !options.no_history,
        !options.disable_auto_suggestion,
    )?;
    let color_prompt = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let mut sql = String::new();
    loop {
        let prompt = if sql.is_empty() {
            format!("kaveon:{}> ", options.schema)
        } else {
            "     -> ".to_owned()
        };
        input.set_colored_prompt(&prompt, color_prompt);
        let line = match input.readline(&prompt)? {
            ReadLine::Line(line) => line,
            ReadLine::Interrupted => {
                sql.clear();
                eprintln!("^C\n");
                continue;
            }
            ReadLine::Eof => {
                input.save_history()?;
                return Ok(());
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if sql.is_empty() && is_repl_alias(trimmed, "exit", "quit") {
            input.save_history()?;
            return Ok(());
        }
        if sql.is_empty() && is_repl_alias(trimmed, "help", "help") {
            print_remote_help();
            continue;
        }
        if sql.is_empty() && is_repl_alias(trimmed, "clear", "clear") {
            if let Err(error) = clear_terminal() {
                eprintln!("error: {error}");
                eprintln!();
            }
            continue;
        }
        if sql.is_empty() && trimmed.starts_with('.') {
            match handle_meta_command(client, options, trimmed) {
                Ok(true) => {
                    input.save_history()?;
                    return Ok(());
                }
                Ok(false) => {}
                Err(error) => {
                    eprintln!("error: {error}");
                    eprintln!();
                }
            }
            continue;
        }
        sql.push_str(&line);
        sql.push('\n');
        match input::split_completed_statements(&sql) {
            Ok((statements, remainder)) if !statements.is_empty() => {
                sql = remainder;
                for statement in statements {
                    if let Err(error) = execute(client, options, &statement) {
                        eprintln!("error: {error}");
                        eprintln!();
                    }
                }
            }
            Ok((_, remainder)) => sql = remainder,
            Err(_) => {}
        }
    }
}

fn execute_script(
    client: &Session,
    options: &mut Options,
    script: &str,
    ignore_errors: bool,
) -> Result<(), String> {
    let mut first_error = None;
    for statement in input::split_statements(script)? {
        if let Err(error) = execute(client, options, &statement) {
            if !ignore_errors {
                return Err(error);
            }
            eprintln!("error: {error}");
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn handle_meta_command(
    client: &Session,
    options: &mut Options,
    command: &str,
) -> Result<bool, String> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    let meta = match parts.as_slice() {
        [".quit" | ".exit" | ".q"] => return Ok(true),
        [".help" | ".h"] => {
            print_remote_help();
            return Ok(false);
        }
        [".clear"] => {
            clear_terminal()?;
            return Ok(false);
        }
        [".catalogs"] => MetaCommand::Catalogs { like: None },
        [".schemas"] => MetaCommand::Schemas {
            catalog: options.catalog.clone(),
            like: None,
        },
        [".schemas", catalog] => MetaCommand::Schemas {
            catalog: (*catalog).to_owned(),
            like: None,
        },
        [".tables"] => MetaCommand::Tables {
            catalog: options.catalog.clone(),
            schema: options.schema.clone(),
            like: None,
        },
        [".tables", target] => metadata_for_target(target, options, false)?,
        [".use", target] => metadata_for_target(target, options, true)?,
        [".describe" | ".desc", target] => {
            parse_sql_metadata(&format!("DESCRIBE {target}"), options)?
                .ok_or_else(|| "invalid table reference".to_owned())?
        }
        _ => {
            return Err(format!(
                "unknown command '{command}'; type .help for commands"
            ));
        }
    };
    run_meta_command(client, options, meta)?;
    Ok(false)
}

fn is_repl_alias(input: &str, first: &str, second: &str) -> bool {
    let input = input.trim_end_matches(';').trim();
    input.eq_ignore_ascii_case(first) || input.eq_ignore_ascii_case(second)
}

fn clear_terminal() -> Result<(), String> {
    if !io::stdout().is_terminal() {
        return Err("CLEAR is available only in an interactive terminal".to_owned());
    }
    print!("\x1b[2J\x1b[H");
    io::stdout().flush().map_err(|error| error.to_string())
}

fn execute(client: &Session, options: &mut Options, sql: &str) -> Result<(), String> {
    if let Some(meta) = parse_sql_metadata(sql, options)? {
        run_meta_command(client, options, meta)?;
        return Ok(());
    }
    let url = endpoint(options, "/v1/statement");
    let request = StatementRequest {
        query: sql,
        catalog: &options.catalog,
        schema: &options.schema,
        user: &options.user,
        source: &options.source,
        client: "kaveon-cli",
        client_tags: &options.client_tags,
        result_delivery: "inline",
    };
    let response = client
        .request(reqwest::Method::POST, &url)?
        .json(&request)
        .send()
        .map_err(connection_error)?;
    let response: StatementResponse = decode_response(response)?;
    if let Some(error) = response.error {
        return Err(format!("query {} failed: {error}", response.id));
    }
    let mut output = format_result(&response, options.output_format)?;
    if is_human_format(options.output_format) {
        output.push_str(&format!(
            "Query {} {} in {} ms ({} {} returned)",
            response.id,
            response.state,
            response.elapsed_ms,
            response.data.len(),
            if response.data.len() == 1 {
                "row"
            } else {
                "rows"
            },
        ));
        output.push('\n');
        if let Ok(telemetry) = get_query_telemetry(client, options, &response.id) {
            output.push_str(&format_query_telemetry(&telemetry, &response));
        }
        output.push('\n');
    }
    write_output(options, &output)?;
    Ok(())
}

fn get_json<T: for<'de> Deserialize<'de>>(
    client: &Session,
    options: &Options,
    path: &str,
) -> Result<T, String> {
    get_json_url(client, &endpoint(options, path))
}

fn get_json_url<T: for<'de> Deserialize<'de>>(client: &Session, url: &str) -> Result<T, String> {
    let response = client
        .request(reqwest::Method::GET, url)?
        .send()
        .map_err(connection_error)?;
    decode_response(response)
}

fn get_json_url_with_timeout<T: for<'de> Deserialize<'de>>(
    client: &Session,
    url: &str,
    timeout: Duration,
) -> Result<T, String> {
    let response = client
        .request(reqwest::Method::GET, url)?
        .timeout(timeout)
        .send()
        .map_err(connection_error)?;
    decode_response(response)
}

fn metadata_url(options: &Options, segments: &[&str]) -> Result<String, String> {
    let mut url = reqwest::Url::parse(&endpoint(options, "/v1/catalog"))
        .map_err(|error| format!("invalid coordinator URL: {error}"))?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| "coordinator URL cannot accept catalog paths".to_owned())?;
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url.into())
}

fn get_query_telemetry(
    client: &Session,
    options: &Options,
    query_id: &str,
) -> Result<QueryTelemetry, String> {
    let mut url = reqwest::Url::parse(&endpoint(options, "/v1/query"))
        .map_err(|error| format!("invalid coordinator URL: {error}"))?;
    url.path_segments_mut()
        .map_err(|_| "coordinator URL cannot accept query paths".to_owned())?
        .push(query_id);
    get_json_url_with_timeout(client, url.as_str(), Duration::from_secs(3))
}

fn format_query_telemetry(telemetry: &QueryTelemetry, response: &StatementResponse) -> String {
    let (nodes, tasks) = if telemetry.stages.is_empty() {
        ("N/A".to_owned(), "N/A".to_owned())
    } else {
        let nodes = telemetry
            .stages
            .iter()
            .flat_map(|stage| stage.tasks.iter().map(|task| task.node_id.as_str()))
            .collect::<BTreeSet<_>>()
            .len();
        let tasks: usize = telemetry.stages.iter().map(|stage| stage.task_count).sum();
        let completed: usize = telemetry
            .stages
            .iter()
            .map(|stage| stage.completed_tasks)
            .sum();
        let progress = if tasks > 0 {
            format!("{:.2}%", completed as f64 * 100.0 / tasks as f64)
        } else {
            "N/A".to_owned()
        };
        (
            nodes.to_string(),
            format!("{tasks} total, {completed} done ({progress})"),
        )
    };
    let result_bytes = serde_json::to_vec(&response.data).map_or(0, |value| value.len());
    let elapsed_seconds = response.elapsed_ms as f64 / 1_000.0;
    let rates = if elapsed_seconds > 0.0 {
        format!(
            "{:.1} rows/s, {:.1} JSON result bytes/s",
            response.data.len() as f64 / elapsed_seconds,
            result_bytes as f64 / elapsed_seconds
        )
    } else {
        "N/A (elapsed time is 0 ms)".to_owned()
    };
    let scan = if telemetry.scans.is_empty() {
        "Scan metrics: not reported".to_owned()
    } else {
        let rows: u64 = telemetry.scans.iter().map(|scan| scan.rows_selected).sum();
        let bytes: u64 = telemetry
            .scans
            .iter()
            .map(|scan| scan.compressed_bytes_selected)
            .sum();
        let scanned = telemetry
            .scans
            .iter()
            .map(|scan| scan.rows_emitted)
            .collect::<Option<Vec<_>>>();
        let coverage = if telemetry.scan_metrics_complete == Some(false) {
            " (partial worker metrics)"
        } else {
            ""
        };
        let scanned_line = match scanned {
            Some(counts) => {
                let count: u64 = counts.iter().sum();
                let rate = if elapsed_seconds > 0.0 {
                    format!("{:.1} rows/s", count as f64 / elapsed_seconds)
                } else {
                    "N/A rows/s".to_owned()
                };
                format!("Scanned: {count} rows from storage readers, {rate}{coverage}")
            }
            None => "Scanned: not reported".to_owned(),
        };
        format!("{scanned_line}\nScan selected: {rows} rows, {bytes} compressed bytes{coverage}")
    };
    format!(
        "Nodes: {nodes}  Tasks: {tasks}\nRows: {} returned  JSON result bytes: {result_bytes}  Rates: {rates}\n{scan}\n",
        response.data.len()
    )
}

fn run_meta_command(
    client: &Session,
    options: &mut Options,
    command: MetaCommand,
) -> Result<(), String> {
    match command {
        MetaCommand::Catalogs { like } => {
            let response: CatalogList = get_json(client, options, "/v1/catalog")?;
            print_metadata(
                "Catalog",
                filter_like(response.catalogs, like.as_deref()),
                options,
            )
        }
        MetaCommand::Schemas { catalog, like } => {
            let url = metadata_url(options, &[&catalog, "schema"])?;
            let response: SchemaList = get_json_url(client, &url)?;
            print_metadata(
                "Schema",
                filter_like(response.schemas, like.as_deref()),
                options,
            )
        }
        MetaCommand::Tables {
            catalog,
            schema,
            like,
        } => {
            let url = metadata_url(options, &[&catalog, "schema", &schema, "table"])?;
            let response: TableList = get_json_url(client, &url)?;
            print_metadata(
                "Table",
                filter_like(response.tables, like.as_deref()),
                options,
            )
        }
        MetaCommand::Describe {
            catalog,
            schema,
            table,
        } => {
            let definitions: Vec<NamedDefinition> =
                get_json(client, options, "/v1/catalog/definitions")?;
            let catalog = definitions
                .into_iter()
                .find(|definition| definition.name == catalog)
                .ok_or_else(|| "catalog definition is not available for DESCRIBE".to_owned())?;
            let schema_url = definition_url(options, &[&catalog.id, "schemas"])?;
            let schemas: Vec<NamedDefinition> = get_json_url(client, &schema_url)?;
            let schema = schemas
                .into_iter()
                .find(|definition| definition.name == schema)
                .ok_or_else(|| "schema definition is not available for DESCRIBE".to_owned())?;
            let table_url = schema_table_definitions_url(options, &schema.id)?;
            let tables: Vec<TableDefinition> = get_json_url(client, &table_url)?;
            let table = tables
                .into_iter()
                .find(|definition| definition.name == table)
                .ok_or_else(|| "table definition is not available for DESCRIBE".to_owned())?;
            let response = StatementResponse {
                id: String::new(),
                state: String::new(),
                error: None,
                elapsed_ms: 0,
                columns: vec![
                    Column {
                        name: "Column".to_owned(),
                        _data_type: "varchar".to_owned(),
                    },
                    Column {
                        name: "Type".to_owned(),
                        _data_type: "varchar".to_owned(),
                    },
                    Column {
                        name: "Nullable".to_owned(),
                        _data_type: "varchar".to_owned(),
                    },
                ],
                data: table
                    .columns
                    .into_iter()
                    .map(|column| {
                        vec![
                            Value::String(column.name),
                            Value::String(
                                column
                                    .data_type
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| column.data_type.to_string()),
                            ),
                            Value::String(if column.nullable { "YES" } else { "NO" }.to_owned()),
                        ]
                    })
                    .collect(),
            };
            let mut output = format_result(&response, options.output_format)?;
            if is_human_format(options.output_format) {
                output.push('\n');
            }
            write_output(options, &output)?;
        }
        MetaCommand::Use { catalog, schema } => {
            let url = metadata_url(options, &[&catalog, "schema"])?;
            let response: SchemaList = get_json_url(client, &url)?;
            if !response.schemas.iter().any(|name| name == &schema) {
                return Err(format!(
                    "schema '{schema}' not found in catalog '{catalog}'"
                ));
            }
            options.catalog = catalog;
            options.schema = schema;
            if is_human_format(options.output_format) {
                println!("Using {}.{}", options.catalog, options.schema);
                println!();
            }
        }
    }
    Ok(())
}

fn decode_response<T: for<'de> Deserialize<'de>>(response: Response) -> Result<T, String> {
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("cannot read coordinator response: {error}"))?;
    if !status.is_success() {
        let detail = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or(body);
        return Err(format!("coordinator returned HTTP {status}: {detail}"));
    }
    serde_json::from_str(&body).map_err(|error| format!("invalid coordinator response: {error}"))
}

fn connection_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "coordinator request timed out".to_owned()
    } else if error.is_connect() {
        format!("cannot connect to coordinator: {error}")
    } else {
        format!("coordinator request failed: {error}")
    }
}

fn endpoint(options: &Options, path: &str) -> String {
    format!("{}{}", options.server.trim_end_matches('/'), path)
}

fn parse_sql_metadata(sql: &str, options: &Options) -> Result<Option<MetaCommand>, String> {
    let mut tokenizer = Tokenizer::new(&GenericDialect {}, sql);
    let mut tokens = tokenizer
        .tokenize()
        .map_err(|error| format!("invalid metadata statement: {error}"))?
        .into_iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    if matches!(tokens.last(), Some(Token::SemiColon)) {
        tokens.pop();
    }
    if tokens.iter().any(|token| matches!(token, Token::SemiColon)) {
        return Err("metadata commands accept one statement at a time".to_owned());
    }
    if tokens.is_empty() {
        return Ok(None);
    }
    let Some(first) = word(&tokens[0]) else {
        return Ok(None);
    };
    if first.1.is_some() {
        return Ok(None);
    }
    if first.0.eq_ignore_ascii_case("SHOW") {
        return parse_show(&tokens[1..], options).map(Some);
    }
    if first.0.eq_ignore_ascii_case("USE") {
        let names = parse_names(&tokens[1..])?;
        return match names.as_slice() {
            [schema] => Ok(Some(MetaCommand::Use {
                catalog: options.catalog.clone(),
                schema: schema.clone(),
            })),
            [catalog, schema] => Ok(Some(MetaCommand::Use {
                catalog: catalog.clone(),
                schema: schema.clone(),
            })),
            _ => Err("usage: USE [catalog.]schema".to_owned()),
        };
    }
    if first.0.eq_ignore_ascii_case("DESCRIBE") || first.0.eq_ignore_ascii_case("DESC") {
        let tokens = if matches!(tokens.get(1), Some(Token::Word(word)) if word.quote_style.is_none() && word.value.eq_ignore_ascii_case("TABLE"))
        {
            &tokens[2..]
        } else {
            &tokens[1..]
        };
        return parse_table_reference(tokens, options).map(|(catalog, schema, table)| {
            Some(MetaCommand::Describe {
                catalog,
                schema,
                table,
            })
        });
    }
    Ok(None)
}

fn definition_url(options: &Options, segments: &[&str]) -> Result<String, String> {
    let mut url = reqwest::Url::parse(&endpoint(options, "/v1/catalog/definitions"))
        .map_err(|error| format!("invalid coordinator URL: {error}"))?;
    let mut path = url
        .path_segments_mut()
        .map_err(|_| "coordinator URL cannot accept definition paths".to_owned())?;
    for segment in segments {
        path.push(segment);
    }
    drop(path);
    Ok(url.into())
}

fn schema_table_definitions_url(options: &Options, schema_id: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse(&endpoint(options, "/v1/catalog/schemas"))
        .map_err(|error| format!("invalid coordinator URL: {error}"))?;
    let mut path = url
        .path_segments_mut()
        .map_err(|_| "coordinator URL cannot accept definition paths".to_owned())?;
    path.push(schema_id);
    path.push("tables");
    drop(path);
    Ok(url.into())
}

fn parse_table_reference(
    tokens: &[Token],
    options: &Options,
) -> Result<(String, String, String), String> {
    match parse_names(tokens)?.as_slice() {
        [table] => Ok((
            options.catalog.clone(),
            options.schema.clone(),
            table.clone(),
        )),
        [schema, table] => Ok((options.catalog.clone(), schema.clone(), table.clone())),
        [catalog, schema, table] => Ok((catalog.clone(), schema.clone(), table.clone())),
        _ => Err("usage: DESCRIBE [catalog.]schema.table".to_owned()),
    }
}

fn parse_show(tokens: &[Token], options: &Options) -> Result<MetaCommand, String> {
    let Some((kind, quote_style)) = tokens.first().and_then(word) else {
        return Err(
            "usage: SHOW CATALOGS | SHOW SCHEMAS [IN catalog] | SHOW TABLES [IN [catalog.]schema]"
                .to_owned(),
        );
    };
    if quote_style.is_some() {
        return Err("unsupported SHOW statement".to_owned());
    }
    let (scope, like) = split_like(&tokens[1..])?;
    if kind.eq_ignore_ascii_case("COLUMNS") {
        let [Token::Word(connector), rest @ ..] = scope else {
            return Err("usage: SHOW COLUMNS FROM [catalog.]schema.table".to_owned());
        };
        if connector.quote_style.is_some()
            || !connector.value.eq_ignore_ascii_case("FROM")
            || like.is_some()
        {
            return Err("usage: SHOW COLUMNS FROM [catalog.]schema.table".to_owned());
        }
        let (catalog, schema, table) = parse_table_reference(rest, options)?;
        return Ok(MetaCommand::Describe {
            catalog,
            schema,
            table,
        });
    }
    if kind.eq_ignore_ascii_case("CATALOGS") {
        if scope.is_empty() {
            return Ok(MetaCommand::Catalogs { like });
        }
        return Err("unsupported SHOW CATALOGS clause".to_owned());
    }
    if !(kind.eq_ignore_ascii_case("SCHEMAS") || kind.eq_ignore_ascii_case("TABLES")) {
        return Err("unsupported SHOW statement".to_owned());
    }
    let names = match scope {
        [] => Vec::new(),
        [Token::Word(connector), rest @ ..]
            if connector.quote_style.is_none()
                && (connector.value.eq_ignore_ascii_case("IN")
                    || connector.value.eq_ignore_ascii_case("FROM")) =>
        {
            parse_names(rest)?
        }
        _ => {
            return Err(format!(
                "unsupported SHOW {} clause",
                kind.to_ascii_uppercase()
            ));
        }
    };
    if kind.eq_ignore_ascii_case("SCHEMAS") {
        return match names.as_slice() {
            [] => Ok(MetaCommand::Schemas {
                catalog: options.catalog.clone(),
                like,
            }),
            [catalog] => Ok(MetaCommand::Schemas {
                catalog: catalog.clone(),
                like,
            }),
            _ => Err("usage: SHOW SCHEMAS [IN catalog]".to_owned()),
        };
    }
    match names.as_slice() {
        [] => Ok(MetaCommand::Tables {
            catalog: options.catalog.clone(),
            schema: options.schema.clone(),
            like,
        }),
        [schema] => Ok(MetaCommand::Tables {
            catalog: options.catalog.clone(),
            schema: schema.clone(),
            like,
        }),
        [catalog, schema] => Ok(MetaCommand::Tables {
            catalog: catalog.clone(),
            schema: schema.clone(),
            like,
        }),
        _ => Err("usage: SHOW TABLES [IN [catalog.]schema]".to_owned()),
    }
}

fn split_like(tokens: &[Token]) -> Result<(&[Token], Option<String>), String> {
    if let [
        prefix @ ..,
        Token::Word(keyword),
        Token::SingleQuotedString(pattern),
    ] = tokens
        && keyword.quote_style.is_none()
        && keyword.value.eq_ignore_ascii_case("LIKE")
    {
        return Ok((prefix, Some(pattern.clone())));
    }
    if tokens.iter().any(|token| matches!(token, Token::Word(word) if word.quote_style.is_none() && word.value.eq_ignore_ascii_case("LIKE"))) {
        return Err("SHOW LIKE requires a single-quoted pattern".to_owned());
    }
    Ok((tokens, None))
}

fn filter_like(names: Vec<String>, pattern: Option<&str>) -> Vec<String> {
    match pattern {
        Some(pattern) => names
            .into_iter()
            .filter(|name| sql_like(name, pattern))
            .collect(),
        None => names,
    }
}

fn sql_like(value: &str, pattern: &str) -> bool {
    let value = value.chars().collect::<Vec<_>>();
    let pattern = pattern.chars().collect::<Vec<_>>();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for character in pattern {
        let mut current = vec![false; value.len() + 1];
        match character {
            '%' => {
                current[0] = previous[0];
                for index in 1..=value.len() {
                    current[index] = previous[index] || current[index - 1];
                }
            }
            '_' => {
                current[1..].copy_from_slice(&previous[..value.len()]);
            }
            character => {
                for index in 1..=value.len() {
                    current[index] = previous[index - 1] && value[index - 1] == character;
                }
            }
        }
        previous = current;
    }
    previous[value.len()]
}

fn metadata_for_target(
    target: &str,
    options: &Options,
    use_command: bool,
) -> Result<MetaCommand, String> {
    let prefix = if use_command { "USE" } else { "SHOW TABLES IN" };
    let meta = parse_sql_metadata(&format!("{prefix} {target}"), options)?
        .ok_or_else(|| "invalid metadata target".to_owned())?;
    Ok(meta)
}

fn parse_names(tokens: &[Token]) -> Result<Vec<String>, String> {
    if tokens.is_empty() {
        return Err("missing identifier".to_owned());
    }
    let mut names = Vec::new();
    let mut expects_name = true;
    for token in tokens {
        if expects_name {
            let Some((name, _)) = word(token) else {
                return Err("expected an identifier".to_owned());
            };
            names.push(name.to_owned());
        } else if !matches!(token, Token::Period) {
            return Err("expected '.' between identifiers".to_owned());
        }
        expects_name = !expects_name;
    }
    if expects_name {
        return Err("expected an identifier after '.'".to_owned());
    }
    Ok(names)
}

fn word(token: &Token) -> Option<(&str, Option<char>)> {
    match token {
        Token::Word(word) => Some((&word.value, word.quote_style)),
        _ => None,
    }
}

fn format_result(response: &StatementResponse, format: OutputFormat) -> Result<String, String> {
    let names = response
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    Ok(crate::output::format_rows(&names, &response.data, format))
}

fn write_output(options: &Options, output: &str) -> Result<(), String> {
    if io::stdout().is_terminal()
        && is_human_format(options.output_format)
        && let Some(pager) = options
            .pager
            .as_deref()
            .filter(|pager| !pager.trim().is_empty())
    {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", pager]);
            command
        } else {
            let mut parts = pager.split_whitespace();
            let Some(program) = parts.next() else {
                print!("{output}");
                return Ok(());
            };
            let mut command = Command::new(program);
            command.args(parts);
            command
        };
        match command.stdin(Stdio::piped()).spawn() {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut()
                    && let Err(error) = stdin.write_all(output.as_bytes())
                {
                    eprintln!("warning: pager input failed: {error}; printing directly");
                    print!("{output}");
                }
                let _ = child.wait();
                return Ok(());
            }
            Err(error) => {
                eprintln!("warning: cannot start pager '{pager}': {error}; printing directly");
            }
        }
    }
    print!("{output}");
    Ok(())
}

fn is_human_format(format: OutputFormat) -> bool {
    matches!(
        format,
        OutputFormat::Table
            | OutputFormat::Aligned
            | OutputFormat::Vertical
            | OutputFormat::Auto
            | OutputFormat::Markdown
    )
}

fn print_metadata(header: &str, names: Vec<String>, options: &Options) {
    let response = StatementResponse {
        id: String::new(),
        state: String::new(),
        columns: vec![Column {
            name: header.to_owned(),
            _data_type: "varchar".to_owned(),
        }],
        data: names
            .into_iter()
            .map(|name| vec![Value::String(name)])
            .collect(),
        error: None,
        elapsed_ms: 0,
    };
    let mut output =
        format_result(&response, options.output_format).expect("metadata output is serializable");
    if is_human_format(options.output_format) {
        output.push('\n');
    }
    let _ = write_output(options, &output);
}

fn print_remote_help() {
    println!(
        "SQL metadata: SHOW CATALOGS|SCHEMAS|TABLES [IN scope] [LIKE 'pattern']; USE [catalog.]schema;"
    );
    println!("DESCRIBE [TABLE] [catalog.]schema.table; SHOW COLUMNS FROM [catalog.]schema.table;");
    println!(
        ".catalogs  .schemas [catalog]  .tables [[catalog.]schema]  .describe <table>  .use [catalog.]schema  .clear  .quit"
    );
    println!("Aliases: HELP, CLEAR, EXIT, QUIT (a trailing ; is accepted).");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn response() -> StatementResponse {
        StatementResponse {
            id: "query-1".to_owned(),
            state: "FINISHED".to_owned(),
            columns: vec![
                Column {
                    name: "name".to_owned(),
                    _data_type: "Utf8".to_owned(),
                },
                Column {
                    name: "count".to_owned(),
                    _data_type: "Int64".to_owned(),
                },
            ],
            data: vec![vec![Value::String("a,b".to_owned()), Value::from(2)]],
            error: None,
            elapsed_ms: 4,
        }
    }

    #[test]
    fn csv_output_escapes_delimiters() {
        assert_eq!(
            format_result(&response(), OutputFormat::Csv).unwrap(),
            "name,count\n\"a,b\",2\n"
        );
    }

    #[test]
    fn json_output_is_valid_json() {
        let output = format_result(&response(), OutputFormat::Json).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&output).unwrap()[0][1], 2);
    }

    #[test]
    fn table_output_contains_schema_and_row_count() {
        let output = format_result(&response(), OutputFormat::Table).unwrap();
        assert!(output.contains("name"));
        assert!(output.contains("a,b"));
        assert!(output.contains("(1 row)"));
    }

    fn options() -> Options {
        match crate::args::parse(&["kaveon".to_owned()]).unwrap() {
            crate::args::Command::Run(options) => *options,
            _ => panic!("expected options"),
        }
    }

    #[test]
    fn parses_case_insensitive_show_with_quoted_identifier_and_comment() {
        let mut options = options();
        options.catalog = "default_catalog".to_owned();
        assert_eq!(
            parse_sql_metadata(
                "-- list\nshow tables from \"sales.catalog\".\"gold schema\";",
                &options
            )
            .unwrap(),
            Some(MetaCommand::Tables {
                catalog: "sales.catalog".to_owned(),
                schema: "gold schema".to_owned(),
                like: None,
            })
        );
    }

    #[test]
    fn rejects_unsupported_metadata_clauses_and_multiple_statements() {
        let options = options();
        assert!(
            parse_sql_metadata("SHOW TABLES WHERE name = 'orders'", &options)
                .unwrap_err()
                .contains("unsupported")
        );
        assert!(
            parse_sql_metadata("SHOW CATALOGS; USE other", &options)
                .unwrap_err()
                .contains("one statement")
        );
    }

    #[test]
    fn show_like_filters_names_with_sql_wildcards() {
        let options = options();
        assert_eq!(
            parse_sql_metadata("SHOW TABLES IN bronze LIKE 'order_%'", &options).unwrap(),
            Some(MetaCommand::Tables {
                catalog: options.catalog.clone(),
                schema: "bronze".to_owned(),
                like: Some("order_%".to_owned()),
            })
        );
        assert_eq!(
            filter_like(
                vec![
                    "orders_2026".to_owned(),
                    "order".to_owned(),
                    "users".to_owned()
                ],
                Some("order_%")
            ),
            ["orders_2026"]
        );
        assert!(parse_sql_metadata("SHOW CATALOGS LIKE orders", &options).is_err());
    }

    #[test]
    fn describe_and_show_columns_resolve_qualified_references() {
        let options = options();
        let expected = Some(MetaCommand::Describe {
            catalog: "lake".to_owned(),
            schema: "gold".to_owned(),
            table: "orders".to_owned(),
        });
        assert_eq!(
            parse_sql_metadata("DESCRIBE lake.gold.orders", &options).unwrap(),
            expected
        );
        assert_eq!(
            parse_sql_metadata("SHOW COLUMNS FROM lake.gold.orders", &options).unwrap(),
            Some(MetaCommand::Describe {
                catalog: "lake".to_owned(),
                schema: "gold".to_owned(),
                table: "orders".to_owned(),
            })
        );
    }

    #[test]
    fn describe_uses_catalog_schema_and_table_definition_routes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let responses = [
                (
                    "/v1/catalog/definitions",
                    r#"[{"id":"catalog-id","name":"lake"}]"#,
                ),
                (
                    "/v1/catalog/definitions/catalog-id/schemas",
                    r#"[{"id":"schema-id","name":"gold"}]"#,
                ),
                (
                    "/v1/catalog/schemas/schema-id/tables",
                    r#"[{"name":"orders","columns":[{"name":"id","data_type":"Int64","nullable":false}]}]"#,
                ),
            ];
            for (path, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 2048];
                let bytes = stream.read(&mut request).unwrap();
                let line = std::str::from_utf8(&request[..bytes])
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap();
                assert_eq!(line, format!("GET {path} HTTP/1.1"));
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        let mut options = options();
        options.auth = "none".to_owned();
        options.server = format!("http://{address}");
        let client = Session::connect(&options).unwrap();
        run_meta_command(
            &client,
            &mut options,
            MetaCommand::Describe {
                catalog: "lake".to_owned(),
                schema: "gold".to_owned(),
                table: "orders".to_owned(),
            },
        )
        .unwrap();
        server.join().unwrap();
    }

    #[test]
    fn use_defaults_to_current_catalog_and_does_not_parse_injection() {
        let mut options = options();
        options.catalog = "medallion".to_owned();
        assert_eq!(
            parse_sql_metadata("USE \"test schema\"", &options).unwrap(),
            Some(MetaCommand::Use {
                catalog: "medallion".to_owned(),
                schema: "test schema".to_owned(),
            })
        );
        assert!(parse_sql_metadata("USE test; SELECT 1", &options).is_err());
    }

    #[test]
    fn metadata_paths_percent_encode_identifier_segments() {
        let mut options = options();
        options.server = "https://engine.example/".to_owned();
        assert_eq!(
            metadata_url(&options, &["sales/catalog", "schema", "gold schema"]).unwrap(),
            "https://engine.example/v1/catalog/sales%2Fcatalog/schema/gold%20schema"
        );
    }

    #[test]
    fn table_output_right_aligns_numeric_values() {
        let output = format_result(&response(), OutputFormat::Table).unwrap();
        assert!(output.contains("a,b"));
        assert!(!output.contains("Int64"));
    }

    #[test]
    fn bare_repl_aliases_are_case_insensitive() {
        assert!(is_repl_alias("EXIT;", "exit", "quit"));
        assert!(is_repl_alias(" quit ", "exit", "quit"));
        assert!(!is_repl_alias("exit now", "exit", "quit"));
    }

    #[test]
    fn telemetry_footer_deduplicates_task_nodes_and_reports_scan_metrics() {
        let telemetry = QueryTelemetry {
            scan_metrics_complete: None,
            stages: vec![
                StageTelemetry {
                    task_count: 3,
                    completed_tasks: 3,
                    tasks: vec![
                        TaskTelemetry {
                            node_id: "worker-a".to_owned(),
                        },
                        TaskTelemetry {
                            node_id: "worker-b".to_owned(),
                        },
                    ],
                },
                StageTelemetry {
                    task_count: 1,
                    completed_tasks: 1,
                    tasks: vec![TaskTelemetry {
                        node_id: "worker-a".to_owned(),
                    }],
                },
            ],
            scans: vec![ScanTelemetry {
                rows_emitted: None,
                rows_selected: 12,
                compressed_bytes_selected: 34,
            }],
        };
        let footer = format_query_telemetry(&telemetry, &response());
        assert!(footer.contains("Nodes: 2  Tasks: 4 total, 4 done (100.00%)"));
        assert!(footer.contains("Scan selected: 12 rows, 34 compressed bytes"));
        assert!(footer.ends_with('\n'));
    }

    #[test]
    fn telemetry_footer_handles_missing_metrics_and_zero_elapsed() {
        let mut result = response();
        result.elapsed_ms = 0;
        let footer = format_query_telemetry(
            &QueryTelemetry {
                scan_metrics_complete: None,
                stages: Vec::new(),
                scans: Vec::new(),
            },
            &result,
        );
        assert!(footer.contains("Nodes: N/A  Tasks: N/A"));
        assert!(footer.contains("Scan metrics: not reported"));
        assert!(footer.contains("Rates: N/A"));
    }

    #[test]
    fn table_footer_has_a_trailing_blank_line() {
        let footer = format_query_telemetry(
            &QueryTelemetry {
                scan_metrics_complete: None,
                stages: Vec::new(),
                scans: Vec::new(),
            },
            &response(),
        );
        let rendered = format!(
            "{}Query summary\n{}\n",
            format_result(&response(), OutputFormat::Table).unwrap(),
            footer
        );
        assert!(rendered.ends_with("\n\n"));
    }

    #[test]
    fn metadata_http_paths_work_and_failed_use_preserves_context() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let expected = [
                "/v1/catalog/medallion/schema",
                "/v1/catalog/medallion/schema/test/table",
                "/v1/catalog/medallion/schema",
                "/v1/catalog/medallion/schema",
                "/v1/query/query%2F1",
            ];
            for path in expected {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 2048];
                let bytes = stream.read(&mut request).unwrap();
                let line = std::str::from_utf8(&request[..bytes])
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap();
                assert_eq!(line, format!("GET {path} HTTP/1.1"));
                let body = if path.starts_with("/v1/query/") {
                    r#"{"stages":[{"task_count":1,"completed_tasks":1,"tasks":[{"node_id":"worker"}]}],"scans":[]}"#
                } else if path.ends_with("/table") {
                    r#"{"tables":["orders"]}"#
                } else {
                    r#"{"schemas":["test"]}"#
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body
                )
                .unwrap();
            }
        });
        let mut options = options();
        options.auth = "none".to_owned();
        options.server = format!("http://{address}");
        let client = Session::connect(&options).unwrap();

        run_meta_command(
            &client,
            &mut options,
            MetaCommand::Schemas {
                catalog: "medallion".to_owned(),
                like: None,
            },
        )
        .unwrap();
        run_meta_command(
            &client,
            &mut options,
            MetaCommand::Tables {
                catalog: "medallion".to_owned(),
                schema: "test".to_owned(),
                like: None,
            },
        )
        .unwrap();
        run_meta_command(
            &client,
            &mut options,
            MetaCommand::Use {
                catalog: "medallion".to_owned(),
                schema: "test".to_owned(),
            },
        )
        .unwrap();
        let error = run_meta_command(
            &client,
            &mut options,
            MetaCommand::Use {
                catalog: "medallion".to_owned(),
                schema: "missing".to_owned(),
            },
        )
        .unwrap_err();
        assert!(error.contains("not found"));
        assert_eq!(options.catalog, "medallion");
        assert_eq!(options.schema, "test");
        let telemetry = get_query_telemetry(&client, &options, "query/1").unwrap();
        assert_eq!(telemetry.stages[0].tasks[0].node_id, "worker");
        server.join().unwrap();
    }

    #[test]
    fn endpoint_normalizes_trailing_slash() {
        let mut options = match crate::args::parse(&["kaveon".to_owned()]).unwrap() {
            crate::args::Command::Run(options) => options,
            _ => panic!("expected options"),
        };
        options.server = "http://localhost:8080/".to_owned();
        assert_eq!(
            endpoint(&options, "/v1/statement"),
            "http://localhost:8080/v1/statement"
        );
    }
    #[test]
    fn scan_counts_are_separate_from_returned_rows_and_mark_partial_coverage() {
        let telemetry: QueryTelemetry = serde_json::from_value(serde_json::json!({
            "scan_metrics_complete": false,
            "scans": [{"rows_selected": 10000, "rows_emitted": 8192, "compressed_bytes_selected": 2048}]
        })).unwrap();
        let text = format_query_telemetry(&telemetry, &response());
        assert!(text.contains("Scanned: 8192 rows from storage readers"));
        assert!(text.contains("partial worker metrics"));
        assert!(text.contains("Scan selected: 10000 rows"));
    }
}
