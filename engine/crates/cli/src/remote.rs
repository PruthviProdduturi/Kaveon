use crate::args::{Options, OutputFormat};
use crate::auth::Session;
use crate::input::{self, ReadLine, TerminalInput};
use reqwest::blocking::Response;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlparser::dialect::GenericDialect;
use sqlparser::tokenizer::{Token, Tokenizer};
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
    #[serde(default)]
    columns: Vec<Column>,
    #[serde(default)]
    data: Vec<Vec<Value>>,
    error: Option<String>,
    elapsed_ms: u64,
    /// With `--paged`: where the first page is; the rows are not inline.
    #[serde(default)]
    next_uri: Option<String>,
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

    if io::stdout().is_terminal() {
        return crate::shell::run(client, options);
    }
    print_header(&client, options);
    repl(&client, options)
}

/// What one executed statement produced, for the shell's status line.
/// `output` is what remains to be written; with `--paged` the rows of a
/// machine format have already been streamed to stdout page by page.
pub(crate) struct Executed {
    pub output: String,
    pub elapsed_ms: Option<u64>,
    pub scanned_rows: Option<u64>,
}

/// The session header: what the coordinator says about itself and about us.
fn print_header(client: &Session, options: &Options) {
    if options.no_header {
        return;
    }
    let theme = crate::theme::Theme::detect(&options.theme, io::stdout().is_terminal());
    let cluster = crate::client::session::fetch_cluster(client, &options.server).ok();
    let whoami = crate::client::session::fetch_whoami(client, &options.server)
        .ok()
        .flatten();
    let insecure_development = whoami.as_ref().is_some_and(|who| who.auth == "development")
        || (whoami.is_none() && options.auth == "none");
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let lines = crate::render::cluster::header(
        &crate::render::cluster::HeaderFacts {
            cli_version: env!("CARGO_PKG_VERSION"),
            server: &options.server,
            cluster: cluster.as_ref(),
            whoami: whoami.as_ref(),
            auth_mode: &options.auth,
            insecure_development,
            user: &options.user,
            now_unix,
            embedded: None,
        },
        &theme,
    );
    print!("{}", crate::render::to_ansi(&lines));
}

/// `kaveon catalog|schema|table …`: one catalog statement through
/// `POST /v1/statement`, or the durable definition for `catalog show`.
pub fn run_admin(options: &Options, command: &crate::admin::AdminCommand) -> Result<(), String> {
    let client = Session::connect(options)?;
    let Some(sql) = command.statement()? else {
        let crate::admin::AdminCommand::CatalogShow { name } = command else {
            return Err("internal error: command has neither a statement nor a lookup".into());
        };
        let definitions: Vec<Value> = get_json(&client, options, "/v1/catalog/definitions")?;
        let definition = definitions
            .into_iter()
            .find(|definition| definition.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| format!("catalog '{name}' not found"))?;
        let rendered = serde_json::to_string_pretty(&definition)
            .map_err(|error| format!("cannot render the definition: {error}"))?;
        println!("{rendered}");
        return Ok(());
    };
    let response =
        submit_statement(&client, options, &sql).map_err(|error| humanize(&error, &sql))?;
    if let Some(error) = response.error {
        return Err(humanize(
            &format!("query {} failed: {error}", response.id),
            &sql,
        ));
    }
    let mut output = format_result(&response, options.output_format)?;
    if is_human_format(options.output_format) {
        output.push('\n');
    }
    write_output(options, &output)
}

fn submit_statement(
    client: &Session,
    options: &Options,
    sql: &str,
) -> Result<StatementResponse, String> {
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
    decode_response(response)
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
            let error = humanize(&error, &statement);
            if !ignore_errors {
                return Err(error);
            }
            eprintln!("error: {error}");
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// A statement's failure as one line for scripts: the kind, the message
/// once, the query id when known.
fn humanize(error: &str, sql: &str) -> String {
    let error = crate::client::error::CliError::from_message(error, Some(sql));
    crate::render::error::plain(&error)
        .trim_start_matches("error: ")
        .to_owned()
}

fn handle_meta_command(
    client: &Session,
    options: &mut Options,
    command: &str,
) -> Result<bool, String> {
    if matches!(command.trim(), ".quit" | ".exit" | ".q") {
        return Ok(true);
    }
    if command.trim() == ".clear" {
        clear_terminal()?;
        return Ok(false);
    }
    print!("{}", meta_command_to_string(client, options, command)?);
    Ok(false)
}

/// A dot command's output. `.quit` and `.clear` are the caller's to act on.
pub(crate) fn meta_command_to_string(
    client: &Session,
    options: &mut Options,
    command: &str,
) -> Result<String, String> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    let meta = match parts.as_slice() {
        [".quit" | ".exit" | ".q" | ".clear"] => return Ok(String::new()),
        [".help" | ".h"] => {
            let theme = crate::theme::Theme::detect(&options.theme, io::stdout().is_terminal());
            let mut text = crate::render::to_ansi(&crate::render::help::help(&theme));
            text.push('\n');
            return Ok(text);
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
        [unknown, ..] => {
            const DOT_COMMANDS: [&str; 8] = [
                ".catalogs",
                ".schemas",
                ".tables",
                ".describe",
                ".use",
                ".help",
                ".clear",
                ".quit",
            ];
            return Err(match closest(unknown, &DOT_COMMANDS) {
                Some(suggestion) => {
                    format!("unknown command '{unknown}'; did you mean {suggestion}?")
                }
                None => format!("unknown command '{unknown}'; type .help for commands"),
            });
        }
        [] => return Ok(String::new()),
    };
    run_meta_command(client, options, meta)
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
    if sql.trim_start().starts_with('.') {
        let output = meta_command_to_string(client, options, sql.trim())?;
        return write_output(options, &output);
    }
    let executed = match execute_to_string(client, options, sql) {
        Ok(executed) => executed,
        Err(error) => return Err(explain_missing_table(client, options, &error).unwrap_or(error)),
    };
    write_output(options, &executed.output)
}

pub(crate) fn execute_to_string(
    client: &Session,
    options: &mut Options,
    sql: &str,
) -> Result<Executed, String> {
    execute_with_limit(client, options, sql, None)
}

/// `preview_limit` is the interactive row limit the shell appended, so the
/// summary can say the result is a preview when it fills.
///
/// Three parts, so the shell can run them on different threads: the
/// metadata check and the summary on the UI thread, the POST on a worker.
pub(crate) fn execute_with_limit(
    client: &Session,
    options: &mut Options,
    sql: &str,
    preview_limit: Option<usize>,
) -> Result<Executed, String> {
    if let Some(executed) = run_metadata_statement(client, options, sql)? {
        return Ok(executed);
    }
    let mut response = post_statement(client, options, sql)?;
    let rows = match response.next_uri.take() {
        Some(next_uri) => stream_pages(client, options, &mut response, &next_uri)?,
        None => response.data.len(),
    };
    let mut output = if response.next_uri.is_none() && rows == response.data.len() {
        format_result(&response, options.output_format)?
    } else {
        String::new()
    };
    let mut scanned_rows = None;
    if let Some(summary) = statement_summary(
        client,
        options,
        &response.id,
        rows,
        response.elapsed_ms,
        preview_limit,
    ) {
        scanned_rows = summary.rows_scanned;
        output.push_str(&styled_or_plain(&crate::render::summary::lines(
            &summary,
            &human_theme(options),
        )));
        output.push('\n');
    }
    Ok(Executed {
        output,
        elapsed_ms: Some(response.elapsed_ms),
        scanned_rows,
    })
}

/// The pages of a `--paged` result. A machine format (CSV, TSV, JSON
/// lines, NULL) is written to stdout page by page with its header once,
/// so a result of any size streams; a format that needs every row first
/// (the tables, VERTICAL, MARKDOWN, the JSON array) is collected into
/// `response.data` and rendered by the caller. Returns the row count;
/// `response.next_uri` is left `Some` when the rows were streamed.
fn stream_pages(
    client: &Session,
    options: &Options,
    response: &mut StatementResponse,
    next_uri: &str,
) -> Result<usize, String> {
    use crate::client::pages::PageCursor;
    let mut cursor =
        PageCursor::new(&options.server, next_uri).map_err(|failure| failure.message)?;
    let names: Vec<String> = response
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect();
    let format = options.output_format;
    let streams = !is_human_format(format) && format != OutputFormat::Json;
    if !streams {
        while let Some(page) = cursor
            .fetch_next(client)
            .map_err(|failure| failure.message)?
        {
            response.data.extend(page.rows);
        }
        return Ok(response.data.len());
    }
    // The header alone, to drop from every page after the first.
    let header = crate::output::format_rows(&names, &[], format);
    let mut rows = 0usize;
    let mut first = true;
    let mut stdout = io::stdout().lock();
    let mut write = |page_rows: &[Vec<Value>]| -> Result<(), String> {
        let text = crate::output::format_rows(&names, page_rows, format);
        let text = if first {
            first = false;
            text.as_str()
        } else {
            text.strip_prefix(header.as_str()).unwrap_or(&text)
        };
        stdout
            .write_all(text.as_bytes())
            .and_then(|()| stdout.flush())
            .map_err(|error| format!("cannot write to standard output: {error}"))
    };
    if !response.data.is_empty() {
        rows += response.data.len();
        write(&response.data)?;
        response.data.clear();
    }
    while let Some(page) = cursor
        .fetch_next(client)
        .map_err(|failure| failure.message)?
    {
        rows += page.rows.len();
        write(&page.rows)?;
    }
    response.next_uri = Some(next_uri.to_owned());
    Ok(rows)
}

/// SHOW, USE and DESCRIBE are answered over the catalog API without a
/// statement; `None` when `sql` is a statement for the coordinator.
pub(crate) fn run_metadata_statement(
    client: &Session,
    options: &mut Options,
    sql: &str,
) -> Result<Option<Executed>, String> {
    let Some(meta) = parse_sql_metadata(sql, options)? else {
        return Ok(None);
    };
    Ok(Some(Executed {
        output: run_meta_command(client, options, meta)?,
        elapsed_ms: None,
        scanned_rows: None,
    }))
}

/// The blocking POST: inline delivery, or paged with `--paged`. The shell
/// does the same on a worker thread through `client::statement::submit`.
fn post_statement(
    client: &Session,
    options: &Options,
    sql: &str,
) -> Result<StatementResponse, String> {
    let url = endpoint(options, "/v1/statement");
    let request = StatementRequest {
        query: sql,
        catalog: &options.catalog,
        schema: &options.schema,
        user: &options.user,
        source: &options.source,
        client: "kaveon-cli",
        client_tags: &options.client_tags,
        result_delivery: if options.paged { "paged" } else { "inline" },
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
    Ok(response)
}

/// The summary under a result, with the query record fetched for its
/// provenance; `None` for machine formats, which carry no summary.
pub(crate) fn statement_summary(
    client: &Session,
    options: &Options,
    id: &str,
    rows: usize,
    elapsed_ms: u64,
    preview_limit: Option<usize>,
) -> Option<crate::render::summary::Summary> {
    if !is_human_format(options.output_format) {
        return None;
    }
    let record = crate::client::session::fetch_query(client, &options.server, id).ok();
    let mut summary =
        crate::render::summary::Summary::from_record(elapsed_ms, rows, id, record.as_ref());
    if let Some(limit) = preview_limit
        && rows >= limit
    {
        summary.message = Some(crate::shell::rowlimit::note(limit));
    }
    Some(summary)
}

fn human_theme(options: &Options) -> crate::theme::Theme {
    crate::theme::Theme::detect(&options.theme, io::stdout().is_terminal())
}

/// ANSI on a terminal, plain text otherwise.
fn styled_or_plain(lines: &[ratatui::text::Line<'_>]) -> String {
    if io::stdout().is_terminal() {
        crate::render::to_ansi(lines)
    } else {
        crate::render::to_plain(lines)
    }
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
        .timeout(crate::client::session::METADATA_TIMEOUT)
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

fn run_meta_command(
    client: &Session,
    options: &mut Options,
    command: MetaCommand,
) -> Result<String, String> {
    match command {
        MetaCommand::Catalogs { like } => {
            let started = std::time::Instant::now();
            let response: CatalogList = get_json(client, options, "/v1/catalog")?;
            Ok(metadata_to_string(
                "Catalog",
                "catalogs",
                filter_like(response.catalogs, like.as_deref()),
                started.elapsed(),
                options,
            ))
        }
        MetaCommand::Schemas { catalog, like } => {
            let started = std::time::Instant::now();
            let url = metadata_url(options, &[&catalog, "schema"])?;
            let response: SchemaList = get_json_url(client, &url)?;
            Ok(metadata_to_string(
                "Schema",
                "schemas",
                filter_like(response.schemas, like.as_deref()),
                started.elapsed(),
                options,
            ))
        }
        MetaCommand::Tables {
            catalog,
            schema,
            like,
        } => {
            let started = std::time::Instant::now();
            let url = metadata_url(options, &[&catalog, "schema", &schema, "table"])?;
            let response: TableList = get_json_url(client, &url)?;
            Ok(metadata_to_string(
                "Table",
                "tables",
                filter_like(response.tables, like.as_deref()),
                started.elapsed(),
                options,
            ))
        }
        MetaCommand::Describe {
            catalog,
            schema,
            table,
        } => {
            let started = std::time::Instant::now();
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
                error: None,
                elapsed_ms: 0,
                next_uri: None,
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
                            Value::String(presented_type(&column.data_type)),
                            Value::String(if column.nullable { "YES" } else { "NO" }.to_owned()),
                        ]
                    })
                    .collect(),
            };
            let mut output = format_result(&response, options.output_format)?;
            if is_human_format(options.output_format) {
                let summary = crate::render::summary::Summary::metadata(
                    started.elapsed().as_millis() as u64,
                    response.data.len(),
                    "columns",
                );
                output.push_str(&styled_or_plain(&crate::render::summary::lines(
                    &summary,
                    &human_theme(options),
                )));
                output.push('\n');
            }
            Ok(output)
        }
        MetaCommand::Use { catalog, schema } => {
            let (catalog, schema) = resolve_use(client, options, catalog, schema)?;
            options.catalog = catalog;
            options.schema = schema;
            if is_human_format(options.output_format) {
                let theme = human_theme(options);
                let line = ratatui::text::Line::from(vec![
                    ratatui::text::Span::styled(" ✓ ", theme.ok),
                    ratatui::text::Span::raw(format!(
                        "session is {}.{}",
                        options.catalog, options.schema
                    )),
                ]);
                Ok(format!("{}\n", styled_or_plain(&[line])))
            } else {
                Ok(String::new())
            }
        }
    }
}

/// For a `table '<c>.<s>.<t>' not found` error: where that table (or the
/// closest name) does exist, and how to get there. `None` when the error is
/// something else or nothing similar exists.
pub(crate) fn explain_missing_table(
    client: &Session,
    options: &Options,
    error: &str,
) -> Option<String> {
    let start = error.find("table '")? + "table '".len();
    let end = start + error[start..].find('\'')?;
    let missing = &error[start..end];
    let table = missing.rsplit('.').next()?.to_owned();
    let catalogs: CatalogList = get_json(client, options, "/v1/catalog").ok()?;
    let mut exact = Vec::new();
    let mut names: Vec<(String, String, String)> = Vec::new();
    for catalog in catalogs.catalogs.iter().take(16) {
        let url = metadata_url(options, &[catalog, "schema"]).ok()?;
        let Ok(schemas) = get_json_url::<SchemaList>(client, &url) else {
            continue;
        };
        for schema in schemas.schemas.iter().take(32) {
            let url = metadata_url(options, &[catalog, "schema", schema, "table"]).ok()?;
            let Ok(tables) = get_json_url::<TableList>(client, &url) else {
                continue;
            };
            for name in tables.tables {
                if name.eq_ignore_ascii_case(&table) {
                    exact.push((catalog.clone(), schema.clone(), name.clone()));
                }
                names.push((catalog.clone(), schema.clone(), name));
            }
        }
    }
    let context = if options.context_explicit {
        format!(
            "table '{table}' is not in {}.{}",
            options.catalog, options.schema
        )
    } else {
        format!("no catalog.schema is selected, and '{table}' is not in the default")
    };
    match exact.as_slice() {
        [(catalog, schema, name)] => Some(format!(
            "{context}; it is in {catalog}.{schema} — run USE {catalog}.{schema}; or query {catalog}.{schema}.{name}"
        )),
        [_, _, ..] => Some(format!(
            "{context}; it exists in {} — pick one with USE catalog.schema;",
            exact
                .iter()
                .map(|(c, s, _)| format!("{c}.{s}"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        [] => {
            let candidates: Vec<&str> = names.iter().map(|(_, _, n)| n.as_str()).collect();
            let suggestion = closest(&table, &candidates)?;
            let (catalog, schema, name) = names.iter().find(|(_, _, n)| n == suggestion)?;
            Some(format!(
                "table '{table}' not found; did you mean {catalog}.{schema}.{name}?"
            ))
        }
    }
}

/// `USE x` means the schema `x` in the current catalog when it exists,
/// else the catalog `x` (keeping the current schema when that catalog has
/// it, or its only schema). Anything else is an error that lists what is
/// available.
fn resolve_use(
    client: &Session,
    options: &Options,
    catalog: String,
    schema: String,
) -> Result<(String, String), String> {
    let schemas_of = |catalog: &str| -> Result<Option<Vec<String>>, String> {
        let url = metadata_url(options, &[catalog, "schema"])?;
        match client
            .request(reqwest::Method::GET, &url)?
            .timeout(crate::client::session::METADATA_TIMEOUT)
            .send()
            .map_err(connection_error)
        {
            Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => Ok(None),
            Ok(response) => decode_response::<SchemaList>(response).map(|list| Some(list.schemas)),
            Err(error) => Err(error),
        }
    };
    let current = schemas_of(&catalog)?;
    if let Some(schemas) = &current
        && schemas.iter().any(|name| name == &schema)
    {
        return Ok((catalog, schema));
    }
    let explicit_catalog = catalog != options.catalog;
    if !explicit_catalog && let Some(schemas) = schemas_of(&schema)? {
        // `USE <catalog>`: keep the session schema if the catalog has it.
        if schemas.iter().any(|name| name == &options.schema) {
            return Ok((schema, options.schema.clone()));
        }
        return match schemas.as_slice() {
            [only] => Ok((schema, only.clone())),
            [] => Err(format!("catalog '{schema}' has no schemas")),
            many => Err(format!(
                "catalog '{schema}' has {} schemas; choose one: USE {schema}.{}",
                many.len(),
                many.join(" | ")
            )),
        };
    }
    match current {
        None => {
            let catalogs: CatalogList = get_json(client, options, "/v1/catalog")?;
            Err(
                match closest(
                    &catalog,
                    &catalogs
                        .catalogs
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                ) {
                    Some(suggestion) => {
                        format!("catalog '{catalog}' not found; did you mean {suggestion}?")
                    }
                    None => format!(
                        "catalog '{catalog}' not found; catalogs: {}",
                        catalogs.catalogs.join(", ")
                    ),
                },
            )
        }
        Some(schemas) => Err(
            match closest(
                &schema,
                &schemas.iter().map(String::as_str).collect::<Vec<_>>(),
            ) {
                Some(suggestion) => format!(
                    "schema '{schema}' not found in catalog '{catalog}'; did you mean {suggestion}?"
                ),
                None => format!(
                    "schema '{schema}' not found in catalog '{catalog}'; schemas: {}",
                    schemas.join(", ")
                ),
            },
        ),
    }
}

/// The SQL spelling of a stored column type (`bigint`, `varchar`), as the
/// coordinator's own `DESCRIBE` presents it; an unrecognised encoding is
/// shown as received.
fn presented_type(data_type: &Value) -> String {
    match serde_json::from_value::<arrow::datatypes::DataType>(data_type.clone()) {
        Ok(data_type) => kaveon_sql::ddl::sql_type_name(&data_type),
        Err(_) => data_type
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| data_type.to_string()),
    }
}

fn decode_response<T: for<'de> Deserialize<'de>>(response: Response) -> Result<T, String> {
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("cannot read coordinator response: {error}"))?;
    if !status.is_success() {
        let parsed = serde_json::from_str::<Value>(&body).ok();
        let detail = parsed
            .as_ref()
            .and_then(|value| value.get("error").and_then(Value::as_str))
            .map(str::to_owned)
            .unwrap_or(body.clone());
        let code = parsed
            .as_ref()
            .and_then(|value| value.get("code").and_then(Value::as_str))
            .map(|code| format!(" [{code}]"))
            .unwrap_or_default();
        return Err(format!(
            "coordinator returned HTTP {status}{code}: {detail}"
        ));
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
        return parse_show(&tokens[1..], options);
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

/// `Ok(None)` hands the statement to the coordinator: `SHOW CREATE TABLE`
/// and any other SHOW form the client does not answer from the catalog API.
fn parse_show(tokens: &[Token], options: &Options) -> Result<Option<MetaCommand>, String> {
    let Some((kind, quote_style)) = tokens.first().and_then(word) else {
        return Err(
            "usage: SHOW CATALOGS | SHOW SCHEMAS [IN catalog] | SHOW TABLES [IN [catalog.]schema] | SHOW CREATE TABLE table"
                .to_owned(),
        );
    };
    if quote_style.is_some() {
        return Err("unsupported SHOW statement".to_owned());
    }
    let Some(kind) = canonical_show_kind(kind)? else {
        return Ok(None);
    };
    parse_show_metadata(kind, &tokens[1..], options).map(Some)
}

fn parse_show_metadata(
    kind: &'static str,
    tokens: &[Token],
    options: &Options,
) -> Result<MetaCommand, String> {
    let (scope, like) = split_like(tokens)?;
    if kind == "COLUMNS" {
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
    if kind == "CATALOGS" {
        if scope.is_empty() {
            return Ok(MetaCommand::Catalogs { like });
        }
        return Err("SHOW CATALOGS takes no scope; use SHOW CATALOGS [LIKE 'pattern']".to_owned());
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
        [Token::Word(connector), ..] if connector.quote_style.is_none() => {
            return Err(format!(
                "SHOW {kind} does not take '{}'; did you mean SHOW {kind} IN ...?",
                connector.value
            ));
        }
        _ => {
            return Err(format!("usage: SHOW {kind} [IN scope] [LIKE 'pattern']"));
        }
    };
    if kind == "SCHEMAS" {
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

const SHOW_KINDS: [&str; 4] = ["CATALOGS", "SCHEMAS", "TABLES", "COLUMNS"];

/// `CATALOG`/`CATALOGS`, `SCHEMA`/`SCHEMAS`, ... in any case are the
/// client's; a near miss is refused with the closest kind as a suggestion;
/// anything else (`SHOW CREATE TABLE`, and whatever the coordinator adds)
/// is `None`: the coordinator's statement.
fn canonical_show_kind(word: &str) -> Result<Option<&'static str>, String> {
    let upper = word.to_ascii_uppercase();
    for kind in SHOW_KINDS {
        if upper == kind || upper == kind.trim_end_matches('S') {
            return Ok(Some(kind));
        }
    }
    if upper == "CREATE" {
        return Ok(None);
    }
    match closest(&upper, &SHOW_KINDS) {
        Some(kind) => Err(format!(
            "unsupported SHOW {upper}; did you mean SHOW {kind}?"
        )),
        None => Ok(None),
    }
}

/// The candidate within a small edit distance of `word`, if one stands out.
pub(crate) fn closest<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let word = word.to_ascii_uppercase();
    candidates
        .iter()
        .map(|candidate| {
            (
                edit_distance(&word, &candidate.to_ascii_uppercase()),
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
    if is_human_format(format)
        && names.len() == 1
        && response.data.len() == 1
        && let Some(text) = response.data[0].first().and_then(Value::as_str)
        && text.contains('\n')
    {
        // `SHOW CREATE TABLE`: the statement itself, not a one-cell table.
        return Ok(format!("{text}\n"));
    }
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

fn metadata_to_string(
    header: &str,
    noun: &'static str,
    names: Vec<String>,
    elapsed: Duration,
    options: &Options,
) -> String {
    if is_human_format(options.output_format) {
        // A one-column list, not a grid: names copy cleanly.
        let mut output = String::new();
        for name in &names {
            output.push_str(&format!("  {name}\n"));
        }
        let summary = crate::render::summary::Summary::metadata(
            elapsed.as_millis() as u64,
            names.len(),
            noun,
        );
        output.push_str(&styled_or_plain(&crate::render::summary::lines(
            &summary,
            &human_theme(options),
        )));
        output.push('\n');
        return output;
    }
    let response = StatementResponse {
        id: String::new(),
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
        next_uri: None,
    };
    let mut output =
        format_result(&response, options.output_format).expect("metadata output is serializable");
    if is_human_format(options.output_format) {
        output.push('\n');
    }
    output
}

fn print_remote_help() {
    let theme = crate::theme::Theme::detect("dark", io::stdout().is_terminal());
    let lines = crate::render::help::help(&theme);
    print!("{}", crate::render::to_ansi(&lines));
    println!();
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
            next_uri: None,
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
                .contains("did you mean SHOW TABLES IN")
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
    fn metadata_http_paths_work_and_failed_use_preserves_context() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let expected = [
                "/v1/catalog/medallion/schema",
                "/v1/catalog/medallion/schema/test/table",
                "/v1/catalog/medallion/schema",
                "/v1/catalog/medallion/schema",
                "/v1/catalog/missing/schema",
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
                let (status, body) = if path.ends_with("/table") {
                    ("200 OK", r#"{"tables":["orders"]}"#)
                } else if path.contains("/missing/") {
                    (
                        "404 Not Found",
                        r#"{"error":"catalog 'missing' not found","code":"CATALOG_NOT_FOUND"}"#,
                    )
                } else {
                    ("200 OK", r#"{"schemas":["test"]}"#)
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
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
        server.join().unwrap();
    }

    #[test]
    fn paged_batch_collects_every_page_for_a_table_format() {
        use crate::client::session::test_server::{fixture, session};
        let page = |rows: &str, next: Option<&str>| {
            let next = next.map_or("null".to_owned(), |next| format!("\"{next}\""));
            format!(r#"{{"id":"q","data":{rows},"next_uri":{next},"row_count":3}}"#)
        };
        let (url, thread) = fixture(vec![
            (
                "POST /v1/statement ",
                200,
                r#"{"id":"q","state":"FINISHED","columns":[{"name":"n","type":"Int64"}],"data":[],"error":null,"elapsed_ms":7,"next_uri":"/v1/query/q/results/0"}"#.into(),
            ),
            (
                "GET /v1/query/q/results/0 ",
                200,
                page("[[1],[2]]", Some("/v1/query/q/results/1")),
            ),
            ("GET /v1/query/q/results/1 ", 200, page("[[3]]", None)),
            (
                "GET /v1/query/q ",
                200,
                r#"{"id":"q","state":"FINISHED","elapsed_ms":7}"#.into(),
            ),
        ]);
        let (client, mut options) = session(&url);
        options.paged = true;
        options.output_format = OutputFormat::Aligned;
        let executed = execute_to_string(&client, &mut options, "SELECT n FROM t").unwrap();
        assert_eq!(
            executed.output,
            "+---+\n| n |\n+---+\n| 1 |\n| 2 |\n| 3 |\n+---+\n(3 rows)\n ✓ 7 ms · 3 rows   q\n\n"
        );
        let bodies = thread.join().unwrap();
        let request: Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(request["result_delivery"], "paged");
    }

    #[test]
    fn inline_batch_keeps_inline_delivery() {
        use crate::client::session::test_server::{fixture, session};
        let (url, thread) = fixture(vec![(
            "POST /v1/statement ",
            200,
            r#"{"id":"q","state":"FINISHED","columns":[{"name":"n","type":"Int64"}],"data":[[1]],"error":null,"elapsed_ms":7}"#.into(),
        )]);
        let (client, mut options) = session(&url);
        options.output_format = OutputFormat::Csv;
        let executed = execute_to_string(&client, &mut options, "SELECT 1").unwrap();
        assert_eq!(executed.output, "n\n1\n");
        let bodies = thread.join().unwrap();
        let request: Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(request["result_delivery"], "inline");
    }

    #[test]
    fn administration_commands_submit_one_catalog_statement() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let bytes = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..bytes]);
                let text = String::from_utf8_lossy(&request);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if body.len() >= length {
                        break;
                    }
                }
            }
            let text = String::from_utf8(request).unwrap();
            assert!(text.starts_with("POST /v1/statement HTTP/1.1"), "{text}");
            let body: Value = serde_json::from_str(text.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(
                body["query"],
                "CREATE TABLE lake.sales.orders WITH (location = 'orders.parquet', format = 'parquet')"
            );
            assert_eq!(body["catalog"], "kaveon");
            let reply = r#"{"id":"q-1","state":"FINISHED","columns":[{"name":"table","type":"Utf8"},{"name":"result","type":"Utf8"}],"data":[["lake.sales.orders","created"]],"elapsed_ms":3}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                reply.len(),
                reply
            )
            .unwrap();
        });
        let mut options = options();
        options.auth = "none".to_owned();
        options.server = format!("http://{address}");
        let (command, _) = crate::admin::split(&[
            "table".to_owned(),
            "register".to_owned(),
            "lake.sales.orders".to_owned(),
            "--location".to_owned(),
            "orders.parquet".to_owned(),
            "--format".to_owned(),
            "parquet".to_owned(),
        ])
        .unwrap();
        run_admin(&options, &command).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn describe_presents_stored_arrow_types_with_their_sql_names() {
        assert_eq!(presented_type(&Value::String("Int64".into())), "bigint");
        assert_eq!(presented_type(&Value::String("Utf8".into())), "varchar");
        assert_eq!(
            presented_type(&serde_json::json!({"Decimal128": [12, 2]})),
            "decimal(12, 2)"
        );
        assert_eq!(presented_type(&Value::String("Mystery".into())), "Mystery");
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
}
