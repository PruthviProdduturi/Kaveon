use crate::args::{Options, OutputFormat};
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, Write};

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

pub fn run(options: &mut Options) -> Result<(), String> {
    let client = Client::builder()
        .timeout(options.timeout)
        .build()
        .map_err(|error| format!("cannot initialize HTTP client: {error}"))?;

    if let Some(sql) = options.execute.clone() {
        execute(&client, options, &sql)?;
        return Ok(());
    }

    println!("Kaveon CLI v{}", env!("CARGO_PKG_VERSION"));
    println!("Connected to {}", options.server);
    println!("Catalog: {}  Schema: {}", options.catalog, options.schema);
    println!("Type .help for commands; SQL statements end with semicolons.");
    println!();
    repl(&client, options)
}

fn repl(client: &Client, options: &mut Options) -> Result<(), String> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut sql = String::new();
    loop {
        let prompt = if sql.is_empty() {
            "kaveon> "
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
        if sql.is_empty() && trimmed.starts_with('.') {
            if handle_meta_command(client, options, trimmed)? {
                return Ok(());
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
    client: &Client,
    options: &mut Options,
    command: &str,
) -> Result<bool, String> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    match parts.as_slice() {
        [".quit" | ".exit" | ".q"] => return Ok(true),
        [".help" | ".h"] => print_remote_help(),
        [".catalogs"] => {
            let response: CatalogList = get_json(client, options, "/v1/catalog")?;
            print_names("Catalog", &response.catalogs);
        }
        [".schemas"] => {
            let path = format!("/v1/catalog/{}/schema", options.catalog);
            let response: SchemaList = get_json(client, options, &path)?;
            print_names("Schema", &response.schemas);
        }
        [".tables"] => {
            let path = format!(
                "/v1/catalog/{}/schema/{}/table",
                options.catalog, options.schema
            );
            let response: TableList = get_json(client, options, &path)?;
            print_names("Table", &response.tables);
        }
        [".use", target] => {
            let values: Vec<&str> = target.split('.').collect();
            match values.as_slice() {
                [catalog, schema] => {
                    options.catalog = (*catalog).to_owned();
                    options.schema = (*schema).to_owned();
                }
                [schema] => options.schema = (*schema).to_owned(),
                _ => return Err("usage: .use [catalog.]schema".to_owned()),
            }
            println!("Using {}.{}", options.catalog, options.schema);
        }
        _ => return Err(format!("unknown command '{command}'")),
    }
    Ok(false)
}

fn execute(client: &Client, options: &Options, sql: &str) -> Result<(), String> {
    let url = endpoint(options, "/v1/statement");
    let request = StatementRequest {
        query: sql,
        catalog: &options.catalog,
        schema: &options.schema,
        user: &options.user,
        source: &options.source,
        client: "kaveon-cli",
        client_tags: &options.client_tags,
        result_delivery: "direct",
    };
    let response = client
        .post(url)
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
            "Query {} {} in {} ms",
            response.id, response.state, response.elapsed_ms
        );
    }
    Ok(())
}

fn get_json<T: for<'de> Deserialize<'de>>(
    client: &Client,
    options: &Options,
    path: &str,
) -> Result<T, String> {
    let response = client
        .get(endpoint(options, path))
        .send()
        .map_err(connection_error)?;
    decode_response(response)
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
        .map(|column| column.name.len().max(column.data_type.len()))
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
    ));
    output.push_str(&separator);
    for row in &rows {
        output.push_str(&table_row(row, &widths));
    }
    output.push_str(&separator);
    output.push_str(&format!("({} rows)\n", rows.len()));
    output
}

fn table_row(values: &[String], widths: &[usize]) -> String {
    let cells = widths
        .iter()
        .enumerate()
        .map(|(index, width)| format!(" {:width$} ", values.get(index).map_or("", String::as_str)))
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

fn print_names(header: &str, names: &[String]) {
    println!("{header}");
    for name in names {
        println!("{name}");
    }
    println!("({} rows)", names.len());
}

fn print_remote_help() {
    println!(".catalogs              List catalogs");
    println!(".schemas               List schemas in the current catalog");
    println!(".tables                List tables in the current schema");
    println!(".use [catalog.]schema  Change the session context");
    println!(".quit                  Exit");
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(output.contains("(1 rows)"));
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
