//! `SHOW STATS FOR` and `DESCRIBE DETAIL` in the shell. Both are ordinary
//! statements on the coordinator; the shell recognises their results by
//! their column names and shows them as a reader wants them — the table's
//! totals on one line over the per-column statistics, and the detail as a
//! `field | value` list — instead of a raw table of fractions, byte counts
//! and epoch times. Batch mode and the machine formats keep the rows as
//! they came.
use crate::render::table::{Cell, styled_cells};
use crate::render::{human_bytes, thousands};
use crate::theme::Theme;
use ratatui::text::{Line, Span};
use serde_json::Value;

/// The columns of a `SHOW STATS FOR` result: one row per column and a
/// summary row whose `column_name` is null and whose `row_count` is the
/// table's.
pub const STATS_COLUMNS: [&str; 8] = [
    "column_name",
    "data_type",
    "data_size",
    "nulls_fraction",
    "distinct_values_count",
    "low_value",
    "high_value",
    "row_count",
];

/// A column the coordinator may add to the summary row: when the
/// statistics were collected.
const STATS_OPTIONAL_COLUMNS: [&str; 1] = ["analyzed_at"];

/// The single row of a `DESCRIBE DETAIL` result.
pub const DETAIL_COLUMNS: [&str; 11] = [
    "format",
    "location",
    "created_at",
    "last_modified",
    "num_files",
    "size_in_bytes",
    "row_count",
    "delta_version",
    "partition_columns",
    "analyzed_at",
    "catalog_snapshot",
];

/// The per-column table's headings, in the order they are shown.
const COLUMN_HEADINGS: [&str; 7] = ["column", "type", "size", "nulls", "distinct", "low", "high"];

/// Result columns that hold a point in time.
const TIME_COLUMNS: [&str; 3] = ["created_at", "last_modified", "analyzed_at"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Stats,
    Detail,
}

/// Which statistics result these column names are, if either: the exact
/// column set of `SHOW STATS FOR` (with or without `analyzed_at`) or of
/// `DESCRIBE DETAIL`, in any order.
pub fn kind(names: &[String]) -> Option<Kind> {
    if is_column_set(names, &STATS_COLUMNS, &STATS_OPTIONAL_COLUMNS) {
        Some(Kind::Stats)
    } else if is_column_set(names, &DETAIL_COLUMNS, &[]) {
        Some(Kind::Detail)
    } else {
        None
    }
}

fn is_column_set(names: &[String], required: &[&str], optional: &[&str]) -> bool {
    required.iter().all(|name| names.iter().any(|n| n == name))
        && names
            .iter()
            .all(|name| required.contains(&name.as_str()) || optional.contains(&name.as_str()))
        && names.len() <= required.len() + optional.len()
}

/// The table a `SHOW STATS FOR t` or `DESCRIBE DETAIL t` statement names,
/// qualified with the session catalog and schema when the statement did
/// not: what the header line calls the table.
pub fn table_reference(sql: &str, catalog: &str, schema: &str) -> Option<String> {
    let text = sql.trim().trim_end_matches(';').trim();
    let mut words = text.split_whitespace();
    let reference = match (
        words.next()?.to_ascii_uppercase().as_str(),
        words.next()?.to_ascii_uppercase().as_str(),
    ) {
        ("SHOW", "STATS" | "STAT") => {
            if !words.next()?.eq_ignore_ascii_case("FOR") {
                return None;
            }
            words.next()?
        }
        ("DESCRIBE" | "DESC", "DETAIL") => words.next()?,
        _ => return None,
    };
    if words.next().is_some() {
        return None;
    }
    let parts = split_reference(reference)?;
    let qualified: Vec<String> = match parts.len() {
        1 => vec![catalog.to_owned(), schema.to_owned(), parts[0].clone()],
        2 => vec![catalog.to_owned(), parts[0].clone(), parts[1].clone()],
        3 => parts,
        _ => return None,
    };
    Some(qualified.join("."))
}

/// `a."b.c".d` → `["a", "b.c", "d"]`, quotes removed.
fn split_reference(reference: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in reference.chars() {
        match character {
            '"' => quoted = !quoted,
            '.' if !quoted => parts.push(std::mem::take(&mut current)),
            other => current.push(other),
        }
    }
    if quoted {
        return None;
    }
    parts.push(current);
    if parts.iter().any(String::is_empty) {
        return None;
    }
    Some(parts)
}

/// The `SHOW STATS FOR` result: one line with the table's totals, then the
/// per-column table.
pub fn stats(
    table: Option<&str>,
    names: &[String],
    rows: &[Vec<Value>],
    width: Option<usize>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let column = |name: &str| names.iter().position(|n| n == name);
    let field = |row: &[Value], name: &str| column(name).and_then(|index| row.get(index)).cloned();
    let (summary, columns): (Vec<&Vec<Value>>, Vec<&Vec<Value>>) = rows
        .iter()
        .partition(|row| field(row, "column_name").is_none_or(|value| value.is_null()));
    let summary = summary.first().copied();

    let row_count = summary
        .and_then(|row| field(row, "row_count"))
        .and_then(|value| value.as_u64())
        .or_else(|| {
            rows.iter()
                .filter_map(|row| field(row, "row_count").and_then(|value| value.as_u64()))
                .max()
        });
    let data_size = summary
        .and_then(|row| field(row, "data_size"))
        .and_then(|value| value.as_u64())
        .or_else(|| {
            let sizes: Vec<u64> = columns
                .iter()
                .filter_map(|row| field(row, "data_size").and_then(|value| value.as_u64()))
                .collect();
            (!sizes.is_empty()).then(|| sizes.iter().sum())
        });
    let analyzed = summary
        .and_then(|row| field(row, "analyzed_at"))
        .and_then(|value| human_time(&value))
        .map(|time| time.chars().take(16).collect::<String>());

    let mut pieces: Vec<Span<'static>> = Vec::new();
    if let Some(table) = table {
        pieces.push(Span::styled(table.to_owned(), theme.accent));
    }
    if let Some(rows) = row_count {
        pieces.push(Span::raw(format!("{} rows", thousands(i128::from(rows)))));
    }
    if let Some(bytes) = data_size {
        pieces.push(Span::raw(human_bytes(bytes)));
    }
    if let Some(analyzed) = analyzed {
        pieces.push(Span::styled(format!("analyzed {analyzed}"), theme.dim));
    }
    let mut lines = Vec::with_capacity(rows.len() + 5);
    if !pieces.is_empty() {
        let mut header = vec![Span::raw(" ")];
        for (index, piece) in pieces.into_iter().enumerate() {
            if index > 0 {
                header.push(Span::styled(" · ", theme.dim));
            }
            header.push(piece);
        }
        lines.push(Line::from(header));
    }

    let headings: Vec<String> = COLUMN_HEADINGS.iter().map(|h| (*h).to_owned()).collect();
    let cells: Vec<Vec<Cell>> = columns
        .iter()
        .map(|row| {
            vec![
                text_cell(field(row, "column_name")),
                text_cell(field(row, "data_type")),
                bytes_cell(field(row, "data_size")),
                percent_cell(field(row, "nulls_fraction")),
                count_cell(field(row, "distinct_values_count")),
                text_cell(field(row, "low_value")),
                text_cell(field(row, "high_value")),
            ]
        })
        .collect();
    let (rendered, _) = styled_cells(&headings, &cells, width, theme);
    lines.extend(rendered);
    // Distinct counts are opt-in (one read for the sketches, a scan per
    // column for exact counts); say how to get the missing ones rather
    // than leave a column of dashes to be wondered at.
    let uncounted = columns
        .iter()
        .filter(|row| field(row, "distinct_values_count").is_none_or(|value| value.is_null()))
        .count();
    if uncounted > 0 && !columns.is_empty() {
        let target = table.unwrap_or("<table>");
        let what = if uncounted == columns.len() {
            "distinct values are not counted".to_owned()
        } else {
            format!(
                "distinct values are counted for {} of {} columns",
                columns.len() - uncounted,
                columns.len()
            )
        };
        lines.push(Line::from(Span::styled(
            format!(
                " {what} — ANALYZE {target} WITH (sketches = true) estimates every column in one read, WITH (distinct = true) counts every column exactly, WITH (columns = ARRAY['a', 'b']) some"
            ),
            theme.dim,
        )));
    }
    lines
}

/// The `DESCRIBE DETAIL` result as a `field | value` list, one field per
/// line, with sizes, counts and times humanised.
pub fn detail(
    names: &[String],
    rows: &[Vec<Value>],
    width: Option<usize>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let headings = vec!["field".to_owned(), "value".to_owned()];
    let Some(row) = rows.first() else {
        return styled_cells(&headings, &[], width, theme).0;
    };
    let cells: Vec<Vec<Cell>> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let value = row.get(index).cloned();
            let shown = match name.as_str() {
                "size_in_bytes" => bytes_cell(value),
                "num_files" | "row_count" => count_cell(value),
                "partition_columns" => list_cell(value),
                name if TIME_COLUMNS.contains(&name) => time_cell(value),
                _ => text_cell(value),
            };
            // A list reads down its left edge: no right-aligned numbers.
            let shown = match shown {
                Cell::Number(text) => Cell::Text(text),
                other => other,
            };
            vec![Cell::plain(name), shown]
        })
        .collect();
    styled_cells(&headings, &cells, width, theme).0
}

fn text_cell(value: Option<Value>) -> Cell {
    match value {
        None | Some(Value::Null) => Cell::Missing,
        Some(Value::String(text)) => Cell::plain(&text),
        Some(other) => Cell::plain(&other.to_string()),
    }
}

fn bytes_cell(value: Option<Value>) -> Cell {
    match value.as_ref().and_then(Value::as_u64) {
        Some(bytes) => Cell::Number(human_bytes(bytes)),
        None => Cell::Missing,
    }
}

fn count_cell(value: Option<Value>) -> Cell {
    match value.as_ref().and_then(Value::as_i64) {
        Some(count) => Cell::Number(thousands(i128::from(count))),
        None => Cell::Missing,
    }
}

/// A fraction in `0..=1` as a percentage with one decimal: `0.0 %`.
fn percent_cell(value: Option<Value>) -> Cell {
    match value.as_ref().and_then(Value::as_f64) {
        Some(fraction) => Cell::Number(format!("{:.1} %", fraction * 100.0)),
        None => Cell::Missing,
    }
}

fn list_cell(value: Option<Value>) -> Cell {
    match value {
        Some(Value::Array(items)) if !items.is_empty() => Cell::plain(
            &items
                .iter()
                .map(|item| match item {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(", "),
        ),
        Some(Value::Array(_)) => Cell::Missing,
        other => text_cell(other),
    }
}

fn time_cell(value: Option<Value>) -> Cell {
    match value {
        None | Some(Value::Null) => Cell::Missing,
        Some(value) => match human_time(&value) {
            Some(time) => Cell::plain(&time),
            None => text_cell(Some(value)),
        },
    }
}

/// A point in time as `YYYY-MM-DD HH:MM:SS`: an RFC 3339 string loses its
/// `T`, fractional seconds and `Z` (another offset is kept); a number is
/// Unix seconds, or milliseconds when it is too large to be seconds. Any
/// other text is left as it is.
fn human_time(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => {
            let seconds = if let Some(int) = number.as_i64() {
                int
            } else {
                number.as_f64()? as i64
            };
            // Milliseconds since 1970 reach this in 1973; seconds, in 5138.
            let seconds = if seconds.abs() >= 100_000_000_000 {
                seconds.div_euclid(1000)
            } else {
                seconds
            };
            Some(unix_to_civil(seconds))
        }
        Value::String(text) => Some(rfc3339_to_civil(text)),
        _ => None,
    }
}

fn rfc3339_to_civil(text: &str) -> String {
    let text = text.trim();
    let bytes = text.as_bytes();
    let is_date = bytes.len() >= 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit);
    if !is_date {
        return text.to_owned();
    }
    let (date, rest) = text.split_at(10);
    let rest = rest.strip_prefix(['T', 't', ' ']).unwrap_or(rest);
    if rest.is_empty() {
        return date.to_owned();
    }
    let (clock, offset) = match rest.find(['Z', 'z', '+', '-']) {
        Some(index) => rest.split_at(index),
        None => (rest, ""),
    };
    let clock = clock.split('.').next().unwrap_or(clock);
    let offset = match offset {
        "" | "Z" | "z" => String::new(),
        other => format!(" {other}"),
    };
    format!("{date} {clock}{offset}")
}

/// Unix seconds as `YYYY-MM-DD HH:MM:SS` in UTC.
fn unix_to_civil(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        of_day / 3600,
        (of_day % 3600) / 60,
        of_day % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::to_plain;
    use serde_json::json;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn results_are_recognised_by_their_column_sets() {
        assert_eq!(kind(&strings(&STATS_COLUMNS)), Some(Kind::Stats));
        let mut with_time = strings(&STATS_COLUMNS);
        with_time.push("analyzed_at".to_owned());
        assert_eq!(kind(&with_time), Some(Kind::Stats));
        let mut reordered = strings(&DETAIL_COLUMNS);
        reordered.reverse();
        assert_eq!(kind(&reordered), Some(Kind::Detail));
        assert_eq!(kind(&strings(&["column_name", "data_type"])), None);
        let mut extra = strings(&DETAIL_COLUMNS);
        extra.push("owner".to_owned());
        assert_eq!(kind(&extra), None);
        assert_eq!(kind(&[]), None);
    }

    #[test]
    fn the_statement_names_the_table_qualified_by_the_session() {
        assert_eq!(
            table_reference("SHOW STATS FOR orders;", "lake", "sales").as_deref(),
            Some("lake.sales.orders")
        );
        assert_eq!(
            table_reference("show stat for gold.orders", "lake", "sales").as_deref(),
            Some("lake.gold.orders")
        );
        assert_eq!(
            table_reference("DESCRIBE DETAIL \"Lake\".\"gold.v2\".orders", "x", "y").as_deref(),
            Some("Lake.gold.v2.orders")
        );
        assert_eq!(
            table_reference("desc detail orders", "lake", "sales").as_deref(),
            Some("lake.sales.orders")
        );
        assert_eq!(table_reference("SELECT 1", "lake", "sales"), None);
        assert_eq!(table_reference("SHOW STATS orders", "lake", "sales"), None);
        assert_eq!(
            table_reference("SHOW STATS FOR a.b.c.d", "lake", "sales"),
            None
        );
    }

    #[test]
    fn stats_show_the_totals_then_the_columns_humanised() {
        let names = strings(&STATS_COLUMNS);
        let rows = vec![
            vec![
                json!("id"),
                json!("bigint"),
                json!(24_000_000),
                json!(0.0),
                json!(null),
                json!(1),
                json!(3_000_000),
                json!(null),
            ],
            vec![
                json!("city"),
                json!("varchar"),
                json!(18_140_000),
                json!(0.0134),
                json!(412),
                json!("Aachen"),
                json!("Zürich"),
                json!(null),
            ],
            vec![
                json!(null),
                json!(null),
                json!(42_140_000),
                json!(null),
                json!(null),
                json!(null),
                json!(null),
                json!(3_000_000),
            ],
        ];
        let lines = stats(
            Some("lake.sales.orders"),
            &names,
            &rows,
            None,
            &Theme::mono(),
        );
        let text = to_plain(&lines);
        assert_eq!(
            text,
            " lake.sales.orders · 3,000,000 rows · 40.2 MiB\n\
┌────────┬─────────┬──────────┬───────┬──────────┬────────┬─────────┐\n\
│ column │ type    │     size │ nulls │ distinct │ low    │ high    │\n\
├────────┼─────────┼──────────┼───────┼──────────┼────────┼─────────┤\n\
│ id     │ bigint  │ 22.9 MiB │ 0.0 % │ —        │ 1      │ 3000000 │\n\
│ city   │ varchar │ 17.3 MiB │ 1.3 % │      412 │ Aachen │ Zürich  │\n\
└────────┴─────────┴──────────┴───────┴──────────┴────────┴─────────┘\n\
\x20distinct values are counted for 1 of 2 columns — ANALYZE lake.sales.orders WITH (sketches = true) estimates every column in one read, WITH (distinct = true) counts every column exactly, WITH (columns = ARRAY['a', 'b']) some\n"
        );
    }

    #[test]
    fn stats_header_takes_the_analyzed_time_and_sums_sizes_without_a_summary() {
        let mut names = strings(&STATS_COLUMNS);
        names.push("analyzed_at".to_owned());
        let rows = vec![
            vec![
                json!("id"),
                json!("bigint"),
                json!(1024),
                json!(0.0),
                json!(7),
                json!(null),
                json!(null),
                json!(7),
                json!(null),
            ],
            vec![
                json!(null),
                json!(null),
                json!(null),
                json!(null),
                json!(null),
                json!(null),
                json!(null),
                json!(7),
                json!("2026-09-18T19:04:31.250Z"),
            ],
        ];
        let text = to_plain(&stats(None, &names, &rows, None, &Theme::mono()));
        assert!(
            text.starts_with(" 7 rows · 1.0 KiB · analyzed 2026-09-18 19:04\n┌"),
            "{text}"
        );
        assert!(
            text.contains("│ id     │ bigint │ 1.0 KiB │ 0.0 % │        7 │ —   │ —    │"),
            "{text}"
        );
    }

    #[test]
    fn detail_is_a_field_value_list_with_humanised_values() {
        let names = strings(&DETAIL_COLUMNS);
        let rows = vec![vec![
            json!("delta"),
            json!("tpch/sf100/lineitem"),
            json!(1_726_000_000),
            json!("2026-09-18T19:04:31+02:00"),
            json!(1_204),
            json!(41_943_040),
            json!(600_037_902),
            json!(12),
            json!(["l_shipdate"]),
            json!(null),
            json!("sha256:0c8e1f"),
        ]];
        let text = to_plain(&detail(&names, &rows, None, &Theme::mono()));
        assert_eq!(
            text,
            "┌───────────────────┬────────────────────────────┐\n\
│ field             │ value                      │\n\
├───────────────────┼────────────────────────────┤\n\
│ format            │ delta                      │\n\
│ location          │ tpch/sf100/lineitem        │\n\
│ created_at        │ 2024-09-10 20:26:40        │\n\
│ last_modified     │ 2026-09-18 19:04:31 +02:00 │\n\
│ num_files         │ 1,204                      │\n\
│ size_in_bytes     │ 40.0 MiB                   │\n\
│ row_count         │ 600,037,902                │\n\
│ delta_version     │ 12                         │\n\
│ partition_columns │ l_shipdate                 │\n\
│ analyzed_at       │ —                          │\n\
│ catalog_snapshot  │ sha256:0c8e1f              │\n\
└───────────────────┴────────────────────────────┘\n"
        );
    }

    #[test]
    fn times_read_as_civil_utc() {
        assert_eq!(unix_to_civil(0), "1970-01-01 00:00:00");
        assert_eq!(unix_to_civil(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(unix_to_civil(1_789_758_271), "2026-09-18 19:04:31");
        assert_eq!(
            human_time(&json!(1_789_758_271_250_i64)).as_deref(),
            Some("2026-09-18 19:04:31")
        );
        assert_eq!(rfc3339_to_civil("2026-09-18"), "2026-09-18");
        assert_eq!(
            rfc3339_to_civil("2026-09-18 19:04:31"),
            "2026-09-18 19:04:31"
        );
        assert_eq!(rfc3339_to_civil("yesterday"), "yesterday");
    }
}
