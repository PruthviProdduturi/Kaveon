use crate::args::{Options, OutputFormat};
use crate::auth::Session;
use reqwest::blocking::Response;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlparser::dialect::GenericDialect;
use sqlparser::tokenizer::{Token, Tokenizer};
use std::io::{self, BufRead, IsTerminal, Write};

#[derive(Clone, Debug, Deserialize)]
struct Column {
    name: String,
    #[serde(rename = "type")]
    data_type: String,
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

#[derive(Debug, PartialEq)]
enum MetaCommand {
    Catalogs,
    Schemas { catalog: String },
    Tables { catalog: String, schema: String },
    Use { catalog: String, schema: String },
}

pub fn run(options: &mut Options) -> Result<(), String> {
    let client = crate::auth::Session::connect(options)?;

    if let Some(sql) = options.execute.clone() {
        execute(&client, options, &sql)?;
        return Ok(());
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
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut sql = String::new();
    loop {
        let prompt = if sql.is_empty() {
            &format!("kaveon:{}> ", options.schema)
        } else {
            "     -> "
        };
        eprint!("{prompt}");
        io::stderr().flush().map_err(|error| error.to_string())?;
        let mut line = String::new();
        if input
            .read_line(&mut line)
            .map_err(|error| error.to_string())?
            == 0
        {
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if sql.is_empty() && is_repl_alias(trimmed, "exit", "quit") {
            return Ok(());
        }
        if sql.is_empty() && is_repl_alias(trimmed, "help", "help") {
            print_remote_help();
            continue;
        }
        if sql.is_empty() && is_repl_alias(trimmed, "clear", "clear") {
            if let Err(error) = clear_terminal() {
                eprintln!("error: {error}");
            }
            continue;
        }
        if sql.is_empty() && trimmed.starts_with('.') {
            match handle_meta_command(client, options, trimmed) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) => eprintln!("error: {error}"),
            }
            continue;
        }
        sql.push_str(&line);
        if sql.trim_end().ends_with(';') {
            let statement = sql.trim().trim_end_matches(';').trim().to_owned();
            sql.clear();
            if !statement.is_empty()
                && let Err(error) = execute(client, options, &statement)
            {
                eprintln!("error: {error}");
            }
        }
    }
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
        [".catalogs"] => MetaCommand::Catalogs,
        [".schemas"] => MetaCommand::Schemas {
            catalog: options.catalog.clone(),
        },
        [".schemas", catalog] => MetaCommand::Schemas {
            catalog: (*catalog).to_owned(),
        },
        [".tables"] => MetaCommand::Tables {
            catalog: options.catalog.clone(),
            schema: options.schema.clone(),
        },
        [".tables", target] => metadata_for_target(target, options, false)?,
        [".use", target] => metadata_for_target(target, options, true)?,
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
    print!("{}", format_result(&response, options.output_format)?);
    if options.output_format == OutputFormat::Table {
        println!(
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
        );
    }
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
) -> Result<(), String> {
    match command {
        MetaCommand::Catalogs => {
            let response: CatalogList = get_json(client, options, "/v1/catalog")?;
            print_metadata("Catalog", response.catalogs, options.output_format)
        }
        MetaCommand::Schemas { catalog } => {
            let url = metadata_url(options, &[&catalog, "schema"])?;
            let response: SchemaList = get_json_url(client, &url)?;
            print_metadata("Schema", response.schemas, options.output_format)
        }
        MetaCommand::Tables { catalog, schema } => {
            let url = metadata_url(options, &[&catalog, "schema", &schema, "table"])?;
            let response: TableList = get_json_url(client, &url)?;
            print_metadata("Table", response.tables, options.output_format)
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
            if options.output_format == OutputFormat::Table {
                println!("Using {}.{}", options.catalog, options.schema);
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
    Ok(None)
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
    if kind.eq_ignore_ascii_case("CATALOGS") {
        if tokens.len() == 1 {
            return Ok(MetaCommand::Catalogs);
        }
        return Err("unsupported SHOW CATALOGS clause".to_owned());
    }
    if !(kind.eq_ignore_ascii_case("SCHEMAS") || kind.eq_ignore_ascii_case("TABLES")) {
        return Err("unsupported SHOW statement".to_owned());
    }
    let names = match &tokens[1..] {
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
            }),
            [catalog] => Ok(MetaCommand::Schemas {
                catalog: catalog.clone(),
            }),
            _ => Err("usage: SHOW SCHEMAS [IN catalog]".to_owned()),
        };
    }
    match names.as_slice() {
        [] => Ok(MetaCommand::Tables {
            catalog: options.catalog.clone(),
            schema: options.schema.clone(),
        }),
        [schema] => Ok(MetaCommand::Tables {
            catalog: options.catalog.clone(),
            schema: schema.clone(),
        }),
        [catalog, schema] => Ok(MetaCommand::Tables {
            catalog: catalog.clone(),
            schema: schema.clone(),
        }),
        _ => Err("usage: SHOW TABLES [IN [catalog.]schema]".to_owned()),
    }
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
    match format {
        OutputFormat::Json => serde_json::to_string_pretty(&response.data)
            .map(|value| format!("{value}\n"))
            .map_err(|error| error.to_string()),
        OutputFormat::Csv => Ok(format_delimited(response, ',')),
        OutputFormat::Tsv => Ok(format_delimited(response, '\t')),
        OutputFormat::Table => Ok(format_table(response)),
    }
}

fn format_delimited(response: &StatementResponse, delimiter: char) -> String {
    let separator = delimiter.to_string();
    let mut output = String::new();
    output.push_str(
        &response
            .columns
            .iter()
            .map(|column| escape_delimited(&column.name, delimiter))
            .collect::<Vec<_>>()
            .join(&separator),
    );
    output.push('\n');
    for row in &response.data {
        output.push_str(
            &row.iter()
                .map(value_text)
                .map(|value| escape_delimited(&value, delimiter))
                .collect::<Vec<_>>()
                .join(&separator),
        );
        output.push('\n');
    }
    output
}

fn escape_delimited(value: &str, delimiter: char) -> String {
    if value.contains(delimiter)
        || value.contains('"')
        || value.contains('\n')
        || value.contains('\r')
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn format_table(response: &StatementResponse) -> String {
    if response.columns.is_empty() {
        return format!("({} rows)\n", response.data.len());
    }
    let mut widths: Vec<usize> = response
        .columns
        .iter()
        .map(|column| column.name.len())
        .collect();
    let rows: Vec<Vec<String>> = response
        .data
        .iter()
        .map(|row| row.iter().map(value_text).collect())
        .collect();
    for row in &rows {
        for (index, value) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(value.len());
            }
        }
    }
    let separator = format!(
        "+{}+\n",
        widths
            .iter()
            .map(|width| "-".repeat(width + 2))
            .collect::<Vec<_>>()
            .join("+")
    );
    let mut output = separator.clone();
    output.push_str(&table_row(
        &response
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>(),
        &widths,
        &vec![false; response.columns.len()],
    ));
    output.push_str(&separator);
    let numeric = response
        .columns
        .iter()
        .map(|column| is_numeric_type(&column.data_type))
        .collect::<Vec<_>>();
    for row in &rows {
        output.push_str(&table_row(row, &widths, &numeric));
    }
    output.push_str(&separator);
    output.push_str(&format!(
        "({} {})\n",
        rows.len(),
        if rows.len() == 1 { "row" } else { "rows" }
    ));
    output
}

fn table_row(values: &[String], widths: &[usize], numeric: &[bool]) -> String {
    let cells = widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            let value = values.get(index).map_or("", String::as_str);
            if numeric.get(index).copied().unwrap_or(false) {
                format!(" {value:>width$} ")
            } else {
                format!(" {value:<width$} ")
            }
        })
        .collect::<Vec<_>>()
        .join("|");
    format!("|{cells}|\n")
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_owned(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn is_numeric_type(data_type: &str) -> bool {
    let type_name = data_type.to_ascii_lowercase();
    ["int", "float", "double", "decimal", "numeric", "real"]
        .iter()
        .any(|needle| type_name.contains(needle))
}

fn print_metadata(header: &str, names: Vec<String>, format: OutputFormat) {
    let response = StatementResponse {
        id: String::new(),
        state: String::new(),
        columns: vec![Column {
            name: header.to_owned(),
            data_type: "varchar".to_owned(),
        }],
        data: names
            .into_iter()
            .map(|name| vec![Value::String(name)])
            .collect(),
        error: None,
        elapsed_ms: 0,
    };
    print!(
        "{}",
        format_result(&response, format).expect("metadata output is serializable")
    );
}

fn print_remote_help() {
    println!(
        "SQL metadata: SHOW CATALOGS; SHOW SCHEMAS [IN catalog]; SHOW TABLES [IN [catalog.]schema]; USE [catalog.]schema;"
    );
    println!(
        ".catalogs  .schemas [catalog]  .tables [[catalog.]schema]  .use [catalog.]schema  .clear  .quit"
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
                    data_type: "Utf8".to_owned(),
                },
                Column {
                    name: "count".to_owned(),
                    data_type: "Int64".to_owned(),
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
            })
        );
    }

    #[test]
    fn rejects_unsupported_metadata_clauses_and_multiple_statements() {
        let options = options();
        assert!(
            parse_sql_metadata("SHOW TABLES LIKE 'orders'", &options)
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
        let output = format_table(&response());
        assert!(output.contains("| a,b  |     2 |"));
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
                let body = if path.ends_with("/table") {
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
            },
        )
        .unwrap();
        run_meta_command(
            &client,
            &mut options,
            MetaCommand::Tables {
                catalog: "medallion".to_owned(),
                schema: "test".to_owned(),
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
