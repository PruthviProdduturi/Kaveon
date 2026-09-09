//! Result formatters for the remote CLI. Lowercase formats retain the CLI's
//! original wire-compatible behavior; exact uppercase spellings use Trino modes.
use serde_json::Value;
use std::cmp;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    // Existing lowercase CLI formats.
    Table,
    Csv,
    Tsv,
    Json,
    // Trino-compatible formats, selected by their exact uppercase spelling.
    Aligned,
    Vertical,
    Auto,
    Markdown,
    TrinoCsv,
    CsvHeader,
    CsvUnquoted,
    CsvHeaderUnquoted,
    TrinoTsv,
    TsvHeader,
    JsonLines,
    Null,
}

impl OutputFormat {
    pub fn parse(value: &str) -> Result<Self, String> {
        let format = match value {
            "ALIGNED" => Self::Aligned,
            "VERTICAL" => Self::Vertical,
            "AUTO" => Self::Auto,
            "MARKDOWN" => Self::Markdown,
            "CSV" => Self::TrinoCsv,
            "CSV_HEADER" => Self::CsvHeader,
            "CSV_UNQUOTED" => Self::CsvUnquoted,
            "CSV_HEADER_UNQUOTED" => Self::CsvHeaderUnquoted,
            "TSV" => Self::TrinoTsv,
            "TSV_HEADER" => Self::TsvHeader,
            "JSON" => Self::JsonLines,
            "NULL" => Self::Null,
            _ if value.eq_ignore_ascii_case("table") => Self::Table,
            _ if value.eq_ignore_ascii_case("csv") => Self::Csv,
            _ if value.eq_ignore_ascii_case("tsv") => Self::Tsv,
            _ if value.eq_ignore_ascii_case("json") => Self::Json,
            _ => return Err(format!("unsupported output format '{value}'")),
        };
        Ok(format)
    }
}

pub fn format_rows(names: &[String], rows: &[Vec<Value>], format: OutputFormat) -> String {
    match format {
        OutputFormat::Table | OutputFormat::Aligned => aligned(names, rows),
        OutputFormat::Auto => auto(names, rows),
        OutputFormat::Vertical => vertical(names, rows),
        OutputFormat::Markdown => markdown(names, rows),
        OutputFormat::Csv => delimited(names, rows, ',', true, true),
        OutputFormat::Tsv => delimited(names, rows, '\t', true, true),
        OutputFormat::TrinoCsv => trino_csv(names, rows, false),
        OutputFormat::CsvHeader => trino_csv(names, rows, true),
        OutputFormat::CsvUnquoted => delimited(names, rows, ',', false, false),
        OutputFormat::CsvHeaderUnquoted => delimited(names, rows, ',', true, false),
        OutputFormat::TrinoTsv => tsv(names, rows, false),
        OutputFormat::TsvHeader => tsv(names, rows, true),
        OutputFormat::Json => serde_json::to_string_pretty(rows)
            .map(|json| format!("{json}\n"))
            .unwrap_or_else(|_| "[]\n".to_owned()),
        OutputFormat::JsonLines => json_lines(names, rows),
        OutputFormat::Null => String::new(),
    }
}

fn cell(value: Option<&Value>) -> String {
    match value.unwrap_or(&Value::Null) {
        Value::Null => "NULL".to_owned(),
        Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}

fn terminal_text(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        match character {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{{{:04x}}}", character as u32));
            }
            character => output.push(character),
        }
    }
    output
}

fn terminal_cell(value: Option<&Value>) -> String {
    terminal_text(&cell(value))
}

fn separator(widths: &[usize]) -> String {
    let mut output = String::from("+");
    for width in widths {
        output.push_str(&"-".repeat(width + 2));
        output.push('+');
    }
    output.push('\n');
    output
}

fn aligned(names: &[String], rows: &[Vec<Value>]) -> String {
    if names.is_empty() {
        return format!("({} rows)\n", rows.len());
    }
    let names: Vec<String> = names.iter().map(|name| terminal_text(name)).collect();
    let values: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            (0..names.len())
                .map(|index| terminal_cell(row.get(index)))
                .collect()
        })
        .collect();
    let mut widths: Vec<usize> = names.iter().map(|name| name.chars().count()).collect();
    for row in &values {
        for (index, value) in row.iter().enumerate() {
            widths[index] = cmp::max(widths[index], value.chars().count());
        }
    }
    let row = |cells: &[String]| {
        let mut output = String::from("|");
        for (index, value) in cells.iter().enumerate() {
            output.push_str(&format!(" {value:<width$} |", width = widths[index]));
        }
        output.push('\n');
        output
    };
    let mut output = separator(&widths);
    output.push_str(&row(&names));
    output.push_str(&separator(&widths));
    for values in &values {
        output.push_str(&row(values));
    }
    output.push_str(&separator(&widths));
    output.push_str(&format!(
        "({} {})\n",
        rows.len(),
        if rows.len() == 1 { "row" } else { "rows" }
    ));
    output
}

fn auto(names: &[String], rows: &[Vec<Value>]) -> String {
    let mut widths: Vec<usize> = names
        .iter()
        .map(|name| terminal_text(name).chars().count())
        .collect();
    for row in rows {
        for (index, width) in widths.iter_mut().enumerate() {
            *width = cmp::max(*width, terminal_cell(row.get(index)).chars().count());
        }
    }
    let table_width = widths.iter().sum::<usize>() + (widths.len() * 3) + 1;
    let terminal_width = terminal_size::terminal_size()
        .map(|(terminal_size::Width(width), _)| usize::from(width))
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|width| *width > 0)
        })
        .unwrap_or(120);
    if table_width > terminal_width {
        vertical(names, rows)
    } else {
        aligned(names, rows)
    }
}

fn vertical(names: &[String], rows: &[Vec<Value>]) -> String {
    let mut output = String::new();
    for (row_number, row) in rows.iter().enumerate() {
        output.push_str(&format!("-[ RECORD {} ]-\n", row_number + 1));
        for (index, name) in names.iter().enumerate() {
            output.push_str(&format!(
                "{} | {}\n",
                terminal_text(name),
                terminal_cell(row.get(index))
            ));
        }
    }
    output
}

fn markdown_cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', "<br>")
}

fn markdown(names: &[String], rows: &[Vec<Value>]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let mut output = format!(
        "| {} |\n",
        names
            .iter()
            .map(|name| markdown_cell(&terminal_text(name)))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    output.push_str(&format!(
        "|{}|\n",
        names.iter().map(|_| " --- ").collect::<String>()
    ));
    for row in rows {
        let cells = (0..names.len())
            .map(|index| markdown_cell(&terminal_cell(row.get(index))))
            .collect::<Vec<_>>()
            .join(" | ");
        output.push_str(&format!("| {cells} |\n"));
    }
    output
}

fn csv_field(value: &str, delimiter: char) -> String {
    if value.contains(delimiter) || value.contains('"') || value.contains(['\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn delimited(
    names: &[String],
    rows: &[Vec<Value>],
    delimiter: char,
    header: bool,
    quoted: bool,
) -> String {
    let field = |value: String| {
        if quoted {
            csv_field(&value, delimiter)
        } else {
            value
        }
    };
    let mut output = String::new();
    if header {
        output.push_str(
            &names
                .iter()
                .cloned()
                .map(field)
                .collect::<Vec<_>>()
                .join(&delimiter.to_string()),
        );
        output.push('\n');
    }
    for row in rows {
        output.push_str(
            &(0..names.len())
                .map(|index| field(cell(row.get(index))))
                .collect::<Vec<_>>()
                .join(&delimiter.to_string()),
        );
        output.push('\n');
    }
    output
}

fn trino_csv(names: &[String], rows: &[Vec<Value>], header: bool) -> String {
    let quoted = |value: &str| format!("\"{}\"", value.replace('"', "\"\""));
    let mut output = String::new();
    if header {
        output.push_str(
            &names
                .iter()
                .map(|name| quoted(name))
                .collect::<Vec<_>>()
                .join(","),
        );
        output.push('\n');
    }
    for row in rows {
        output.push_str(
            &(0..names.len())
                .map(|index| quoted(&cell(row.get(index))))
                .collect::<Vec<_>>()
                .join(","),
        );
        output.push('\n');
    }
    output
}

fn tsv_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn tsv(names: &[String], rows: &[Vec<Value>], header: bool) -> String {
    let mut output = String::new();
    if header {
        output.push_str(
            &names
                .iter()
                .map(|name| tsv_field(name))
                .collect::<Vec<_>>()
                .join("\t"),
        );
        output.push('\n');
    }
    for row in rows {
        output.push_str(
            &(0..names.len())
                .map(|index| tsv_field(&cell(row.get(index))))
                .collect::<Vec<_>>()
                .join("\t"),
        );
        output.push('\n');
    }
    output
}

fn json_lines(names: &[String], rows: &[Vec<Value>]) -> String {
    let mut output = String::new();
    for row in rows {
        let mut object = serde_json::Map::new();
        for (index, name) in names.iter().enumerate() {
            object.insert(name.clone(), row.get(index).cloned().unwrap_or(Value::Null));
        }
        output.push_str(
            &serde_json::to_string(&Value::Object(object)).unwrap_or_else(|_| "{}".to_owned()),
        );
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        vec!["name".into(), "count".into()]
    }
    fn rows() -> Vec<Vec<Value>> {
        vec![
            vec![Value::String("a,b\"\n🙂".into()), Value::Null],
            vec![Value::String("".into()), Value::from(2)],
        ]
    }

    #[test]
    fn parses_exact_trino_and_case_insensitive_legacy_formats() {
        assert_eq!(OutputFormat::parse("CSV"), Ok(OutputFormat::TrinoCsv));
        assert_eq!(OutputFormat::parse("csv"), Ok(OutputFormat::Csv));
        assert_eq!(OutputFormat::parse("JsOn"), Ok(OutputFormat::Json));
        assert_eq!(OutputFormat::parse("JSON"), Ok(OutputFormat::JsonLines));
    }

    #[test]
    fn csv_escaping_and_headers_are_distinct() {
        let values = rows();
        assert_eq!(
            format_rows(&names(), &values, OutputFormat::Csv),
            "name,count\n\"a,b\"\"\n🙂\",NULL\n,2\n"
        );
        assert_eq!(
            format_rows(&names(), &values, OutputFormat::TrinoCsv),
            "\"a,b\"\"\n🙂\",\"NULL\"\n\"\",\"2\"\n"
        );
        assert_eq!(
            format_rows(&names(), &values, OutputFormat::CsvHeader),
            "\"name\",\"count\"\n\"a,b\"\"\n🙂\",\"NULL\"\n\"\",\"2\"\n"
        );
        assert_eq!(
            format_rows(&names(), &values, OutputFormat::CsvHeaderUnquoted),
            "name,count\na,b\"\n🙂,NULL\n,2\n"
        );
    }

    #[test]
    fn json_lines_is_an_object_per_row_and_null_discards_rows() {
        let values = rows();
        let lines = format_rows(&names(), &values, OutputFormat::JsonLines);
        let objects: Vec<Value> = lines
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(objects[0]["name"], "a,b\"\n🙂");
        assert!(objects[0]["count"].is_null());
        assert_eq!(format_rows(&names(), &values, OutputFormat::Null), "");
    }

    #[test]
    fn aligned_vertical_and_markdown_preserve_null_empty_and_unicode() {
        let values = rows();
        assert!(format_rows(&names(), &values, OutputFormat::Aligned).contains("NULL"));
        assert!(format_rows(&names(), &values, OutputFormat::Vertical).contains("RECORD 1"));
        assert!(format_rows(&names(), &values, OutputFormat::Markdown).contains("🙂"));
    }

    #[test]
    fn trino_tsv_escapes_control_characters_without_changing_legacy_tsv() {
        let names = vec!["value".into()];
        let rows = vec![vec![Value::String("tab\tline\nslash\\".into())]];
        assert_eq!(
            format_rows(&names, &rows, OutputFormat::TrinoTsv),
            "tab\\tline\\nslash\\\\\n"
        );
        assert_eq!(
            format_rows(&names, &rows, OutputFormat::Tsv),
            "value\n\"tab\tline\nslash\\\"\n"
        );
    }

    #[test]
    fn human_formats_escape_terminal_controls_without_changing_unicode() {
        let names = vec!["na\x1bme".into()];
        let rows = vec![vec![Value::String("ok\x1b[2J🙂\nnext".into())]];
        let output = format_rows(&names, &rows, OutputFormat::Aligned);
        assert!(!output.contains('\x1b'));
        assert!(output.contains("\\u{001b}[2J🙂\\nnext"));
    }
}
