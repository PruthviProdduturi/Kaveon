use kaveon_core::{KaveonError, Result};
use sqlparser::ast::DataType as SqlDataType;
use sqlparser::ast::Statement;
use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, Expr, FromTable, GroupByExpr, MergeAction, MergeClauseKind,
    MergeInsertKind, OnConflictAction, OnInsert, SelectItem, SetExpr, TableFactor, TableWithJoins,
    Value,
};
use sqlparser::dialect::{
    BigQueryDialect, ClickHouseDialect, DatabricksDialect, DuckDbDialect, GenericDialect,
    MsSqlDialect,
};
use sqlparser::parser::Parser;
use std::collections::HashSet;

pub fn parse_sql(sql: &str) -> Result<Vec<Statement>> {
    // DuckDB's grammar is close to the generic SQL surface while enabling
    // standard aggregate FILTER (WHERE ...) clauses. Unsupported execution
    // features are still rejected by the logical planner with typed errors.
    let dialect = DuckDbDialect {};
    // Studio's completion inserts SQL Server-style bracketed identifiers
    // (`[year]`). sqlparser's generic dialect treats those brackets as an
    // array expression, which later reaches the executor as the unsupported
    // scalar function ARRAY. Quote identifier-shaped brackets before parsing;
    // leave numeric/list brackets untouched so ARRAY[...] remains available
    // for the syntax that explicitly supports it.
    let bracket_normalized = normalize_bracket_identifiers(sql);
    let json_exists_normalized = normalize_json_exists_error_clauses(&bracket_normalized)?;
    let json_clauses_normalized = normalize_json_null_clauses(&json_exists_normalized)?;
    let json_unique_normalized = normalize_json_object_unique_keys(&json_clauses_normalized)?;
    let json_returning_normalized = normalize_json_returning(&json_unique_normalized);
    let try_parse_normalized = normalize_try_parse(&json_returning_normalized);
    let parse_normalized = normalize_parse(&try_parse_normalized);
    let (session_normalized, datefirst) =
        normalize_default_tsql_session_prefixes(&parse_normalized)?;
    let session_normalized = rewrite_tsql_datepart_datefirst(&session_normalized, datefirst)?;
    let normalized = normalize_lateral_unnest(&session_normalized);
    // Lower the one recursive shape that has a finite, deterministic
    // equivalent in the existing UNNEST/SEQUENCE execution path.  More
    // general recursive graphs remain explicitly unsupported in the logical
    // planner rather than being accepted with unsafe or incomplete semantics.
    let normalized = normalize_bounded_linear_recursive_cte(&normalized)
        .unwrap_or_else(|| normalized.to_owned());
    let normalized = match normalize_bounded_pivot(&normalized)? {
        Some(pivot) => pivot,
        None => normalized,
    };
    let normalized = match normalize_bounded_unpivot(&normalized)? {
        Some(unpivot) => unpivot,
        None => normalized,
    };
    let normalized = match normalize_bounded_apply(&normalized)? {
        Some(apply) => apply,
        None => normalized,
    };
    reject_unsupported_tsql_relational_forms(&normalized)?;
    // BigQuery's grammar retains a table-shaped UNNEST node when the function
    // is correlated. DuckDB parses the same spelling as a generic function,
    // which cannot carry ordinality metadata.
    if contains_lateral_unnest(sql)
        && let Ok(statements) = Parser::parse_sql(&BigQueryDialect {}, &normalized)
    {
        return Ok(statements);
    }
    // Databricks is the sqlparser dialect that exposes the standard
    // `parameter -> expression` lambda AST used by Trino's comparator
    // functions. Keep this retry narrow so ordinary SQL retains DuckDB's
    // aggregate FILTER and identifier behavior.
    if normalized.contains("->")
        && let Ok(statements) = Parser::parse_sql(&DatabricksDialect {}, &normalized)
    {
        return Ok(statements);
    }
    // DuckDB accepts SQL Server's CONVERT syntax but binds the two arguments
    // in the wrong AST positions (the value becomes the target type). Prefer
    // sqlparser's MSSQL grammar for this narrow surface so CONVERT/TRY_CONVERT
    // can share the Engine's normal CAST semantics.
    let upper = normalized.to_ascii_uppercase();
    if (upper.contains("CONVERT(") || upper.contains("TRY_CONVERT("))
        && let Ok(statements) = Parser::parse_sql(&MsSqlDialect {}, &normalized)
    {
        return Ok(statements);
    }
    // The shared AST models `GROUP BY ... WITH ROLLUP|CUBE|TOTALS` through
    // ClickHouse's dialect. Keep this retry narrow so the normal DuckDB
    // grammar remains authoritative for the rest of the SQL surface.
    if (upper.contains("WITH ROLLUP")
        || upper.contains("WITH CUBE")
        || upper.contains("WITH TOTALS"))
        && let Ok(statements) = Parser::parse_sql(&ClickHouseDialect {}, &normalized)
    {
        return Ok(statements);
    }
    match Parser::parse_sql(&dialect, &normalized) {
        Ok(statements) => Ok(statements),
        Err(primary) if normalized.to_ascii_uppercase().contains("WITH OFFSET") => {
            // sqlparser's DuckDB dialect does not expose BigQuery's
            // zero-based UNNEST offset form. Retry that narrow surface with
            // the dialect that owns the AST fields, leaving all other
            // statements on the normal DuckDB grammar and diagnostics.
            Parser::parse_sql(&BigQueryDialect {}, &normalized)
                .map_err(|_| KaveonError::Sql(primary.to_string()))
        }
        Err(primary) if normalized.to_ascii_uppercase().contains("JSON_OBJECT") => {
            // sqlparser's MSSQL dialect carries the SQL/JSON key-value
            // argument AST (`key : value` and `key VALUE value`) that the
            // DuckDB dialect intentionally does not expose. Retry only this
            // function-specific surface so ordinary statements retain the
            // normal DuckDB grammar and diagnostics.
            Parser::parse_sql(&MsSqlDialect {}, &normalized)
                .map_err(|_| KaveonError::Sql(primary.to_string()))
        }
        Err(primary) => {
            // DuckDB treats MAP(...) as custom type modifiers and cannot
            // parse the parentheses/comma inside DECIMAL(p, s) or ARRAY(T).
            // Alternate grammars expose a typed MAP AST, which uses the existing
            // Decimal128 precision/scale validation in the logical planner.
            // Admit scalar-member decimal maps and maps whose values are
            // one level of scalar ARRAY through this bounded fallback.
            if let Some(statements) = parse_bounded_map_casts(&normalized) {
                return Ok(statements);
            }
            Err(KaveonError::Sql(primary.to_string()))
        }
    }
}

/// SQL Server clients often prepend session prologues before a read. These
/// values are the Engine's fixed semantics, so strip only the explicit
/// defaults. Other values remain fail-closed because honoring mutable session
/// state requires carrying it through planning and every distributed worker.
fn normalize_default_tsql_session_prefixes(sql: &str) -> Result<(String, u8)> {
    const DEFAULTS: [(&str, &str); 5] = [
        ("SET DATEFIRST", "7"),
        ("SET ANSI_NULLS", "ON"),
        ("SET QUOTED_IDENTIFIER", "ON"),
        ("SET ANSI_WARNINGS", "ON"),
        ("SET NOCOUNT", "ON"),
    ];
    let mut remaining = sql.trim_start().to_owned();
    let mut datefirst = 7u8;
    loop {
        let upper = remaining.to_ascii_uppercase();
        let Some((prefix, expected)) = DEFAULTS.iter().find(|(prefix, _)| {
            upper.starts_with(*prefix)
                && !upper
                    .as_bytes()
                    .get(prefix.len())
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        }) else {
            break;
        };
        let Some(separator) = remaining.find(';') else {
            return Err(sql_error(&format!(
                "{prefix} requires a statement terminator before the query"
            )));
        };
        let value = remaining[prefix.len()..separator].trim();
        if *prefix == "SET DATEFIRST" {
            let parsed = value
                .parse::<u8>()
                .map_err(|_| sql_error("SET DATEFIRST requires an integer from 1 through 7"))?;
            if !(1..=7).contains(&parsed) {
                return Err(sql_error(
                    "SET DATEFIRST requires an integer from 1 through 7",
                ));
            }
            datefirst = parsed;
        } else if !value.eq_ignore_ascii_case(expected) {
            return Err(sql_error(&format!(
                "{prefix} supports only the Engine default {expected}; session-specific values are not supported"
            )));
        }
        remaining = remaining[separator + 1..].trim_start().to_owned();
        if remaining.is_empty() {
            return Err(sql_error(&format!(
                "{prefix} {expected} must be followed by a query"
            )));
        }
    }
    Ok((remaining, datefirst))
}

/// Attach the session's DATEFIRST value to each SQL Server DATEPART call.
/// The planner keeps the value as a literal argument, so workers receive the
/// same immutable semantics without relying on coordinator session state.
fn rewrite_tsql_datepart_datefirst(sql: &str, datefirst: u8) -> Result<String> {
    if datefirst == 7 {
        return Ok(sql.to_owned());
    }
    let bytes = sql.as_bytes();
    let keyword = b"DATEPART";
    let mut output = String::with_capacity(sql.len() + 16);
    let mut cursor = 0usize;
    let mut scan = 0usize;
    while scan < bytes.len() {
        if matches!(bytes[scan], b'\'' | b'"' | b'`') {
            let end = quoted_sql_end(sql, scan);
            scan = end;
            continue;
        }
        if scan + keyword.len() <= bytes.len()
            && bytes[scan..scan + keyword.len()].eq_ignore_ascii_case(keyword)
            && (scan == 0 || !is_sql_word_byte(bytes[scan - 1]))
            && (scan + keyword.len() == bytes.len()
                || !is_sql_word_byte(bytes[scan + keyword.len()]))
        {
            let mut open = scan + keyword.len();
            while open < bytes.len() && bytes[open].is_ascii_whitespace() {
                open += 1;
            }
            if open < bytes.len()
                && bytes[open] == b'('
                && let Some(close) = matching_paren(sql, open)
            {
                let args = split_top_level_commas(&sql[open + 1..close])?;
                if args.len() != 2 {
                    return Err(sql_error(
                        "SET DATEFIRST supports only two-argument DATEPART calls",
                    ));
                }
                output.push_str(&sql[cursor..scan]);
                output.push_str("DATEPART_DATEFIRST_");
                output.push_str(&datefirst.to_string());
                output.push_str(&sql[scan + keyword.len()..open + 1]);
                output.push_str(&sql[open + 1..close]);
                output.push(')');
                cursor = close + 1;
                scan = close + 1;
                continue;
            }
        }
        scan += 1;
    }
    output.push_str(&sql[cursor..]);
    Ok(output)
}

/// Lower the bounded SQL Server `TRY_PARSE(value AS type)` spelling to the
/// Engine's existing safe cast path. Numeric and invariant temporal targets
/// are bounded here. Culture-aware `USING` forms are left untouched and
/// therefore fail closed; parsing those would require a locale contract
/// rather than silently applying invariant rules.
fn normalize_try_parse(sql: &str) -> String {
    let upper = sql.to_ascii_uppercase();
    let Some(position) = find_top_level_keyword(&upper, "TRY_PARSE") else {
        return sql.to_owned();
    };
    let mut open = position + "TRY_PARSE".len();
    while open < sql.len() && sql.as_bytes()[open].is_ascii_whitespace() {
        open += 1;
    }
    if sql.as_bytes().get(open) != Some(&b'(') {
        return sql.to_owned();
    }
    let Some(close) = matching_delimiter(sql, open, b'(', b')') else {
        return sql.to_owned();
    };
    let body = sql[open + 1..close].trim();
    let body_upper = body.to_ascii_uppercase();
    let Some(as_position) = find_top_level_keyword(&body_upper, "AS") else {
        return sql.to_owned();
    };
    let value = body[..as_position].trim();
    let target = body[as_position + 2..].trim();
    let target_upper = target.to_ascii_uppercase();
    let numeric_target = [
        "TINYINT",
        "SMALLINT",
        "INT",
        "INTEGER",
        "BIGINT",
        "REAL",
        "FLOAT",
        "DOUBLE",
        "DECIMAL",
        "NUMERIC",
        "MONEY",
        "SMALLMONEY",
    ]
    .iter()
    .any(|candidate| {
        target_upper == *candidate || target_upper.starts_with(&format!("{candidate}("))
    });
    let invariant_temporal_target = matches!(
        target_upper.as_str(),
        "DATE" | "TIME" | "DATETIME" | "DATETIME2" | "DATETIMEOFFSET" | "SMALLDATETIME"
    );
    if value.is_empty()
        || target.is_empty()
        || target_upper.contains(" USING ")
        || (!numeric_target && !invariant_temporal_target)
    {
        return sql.to_owned();
    }
    let replacement = format!(
        "{}TRY_CAST({value} AS {target}){}",
        &sql[..position],
        &sql[close + 1..]
    );
    normalize_try_parse(&replacement)
}

/// Lower the bounded SQL Server `PARSE(value AS type)` spelling to the
/// Engine's strict cast path.  Only invariant numeric and temporal targets
/// are accepted; culture-aware `USING` forms remain untouched so they fail
/// closed rather than silently applying a locale that differs between workers.
fn normalize_parse(sql: &str) -> String {
    let upper = sql.to_ascii_uppercase();
    let Some(position) = find_top_level_keyword(&upper, "PARSE") else {
        return sql.to_owned();
    };
    let mut open = position + "PARSE".len();
    while open < sql.len() && sql.as_bytes()[open].is_ascii_whitespace() {
        open += 1;
    }
    if sql.as_bytes().get(open) != Some(&b'(') {
        return sql.to_owned();
    }
    let Some(close) = matching_delimiter(sql, open, b'(', b')') else {
        return sql.to_owned();
    };
    let body = sql[open + 1..close].trim();
    let body_upper = body.to_ascii_uppercase();
    let Some(as_position) = find_top_level_keyword(&body_upper, "AS") else {
        return sql.to_owned();
    };
    let value = body[..as_position].trim();
    let target = body[as_position + 2..].trim();
    let target_upper = target.to_ascii_uppercase();
    let numeric_target = [
        "TINYINT",
        "SMALLINT",
        "INT",
        "INTEGER",
        "BIGINT",
        "REAL",
        "FLOAT",
        "DOUBLE",
        "DECIMAL",
        "NUMERIC",
        "MONEY",
        "SMALLMONEY",
    ]
    .iter()
    .any(|candidate| {
        target_upper == *candidate || target_upper.starts_with(&format!("{candidate}("))
    });
    let invariant_temporal_target = matches!(
        target_upper.as_str(),
        "DATE" | "TIME" | "DATETIME" | "DATETIME2" | "DATETIMEOFFSET" | "SMALLDATETIME"
    );
    if value.is_empty()
        || target.is_empty()
        || target_upper.contains(" USING ")
        || (!numeric_target && !invariant_temporal_target)
    {
        return sql.to_owned();
    }
    let replacement = format!(
        "{}CAST({value} AS {target}){}",
        &sql[..position],
        &sql[close + 1..]
    );
    normalize_parse(&replacement)
}

/// Keep SQL Server/Fabric relational extensions fail-closed with a useful
/// diagnostic.  The generic parser may reject these forms with a low-level
/// token error, which is not actionable for a client choosing a rewrite.
/// Detection is word-boundary based so identifiers such as `pivot_value` do
/// not trigger it.
fn reject_unsupported_tsql_relational_forms(sql: &str) -> Result<()> {
    let bytes = sql.as_bytes();
    let trimmed = trim_leading_sql_comments(sql);
    let upper = trimmed.to_ascii_uppercase();
    if starts_with_sql_words(trimmed, &["CREATE", "INDEX"])
        || starts_with_sql_words(trimmed, &["CREATE", "UNIQUE", "INDEX"])
        || starts_with_sql_words(trimmed, &["DROP", "INDEX"])
    {
        return Err(sql_error(
            "index DDL is not implemented for lake tables; use CREATE TABLE clustered_by/bloom layout options",
        ));
    }
    if upper.starts_with("ALTER TABLE ")
        && contains_sql_word(bytes, b"ADD")
        && contains_sql_word(bytes, b"CONSTRAINT")
    {
        return Err(sql_error(
            "constraint DDL is not implemented for lake tables; use the qualified KaveonDB typed-row integrity contract",
        ));
    }
    for (word, guidance) in [
        (
            b"PIVOT".as_slice(),
            "PIVOT is not implemented; use conditional aggregation",
        ),
        (
            b"UNPIVOT".as_slice(),
            "UNPIVOT is not implemented; use UNION ALL projections",
        ),
        (
            b"APPLY".as_slice(),
            "CROSS/OUTER APPLY is not implemented; use a supported join or rewrite",
        ),
    ] {
        if contains_sql_word(bytes, word) {
            return Err(sql_error(guidance));
        }
    }
    Ok(())
}

/// Match a leading sequence of SQL keywords while allowing any SQL
/// whitespace between them. This intentionally checks token boundaries so a
/// table or identifier such as `INDEXED_TABLE` is not treated as index DDL.
fn starts_with_sql_words(sql: &str, words: &[&str]) -> bool {
    let bytes = sql.as_bytes();
    let mut offset = 0usize;
    for (position, word) in words.iter().enumerate() {
        while offset < bytes.len() && bytes[offset].is_ascii_whitespace() {
            offset += 1;
        }
        if offset + word.len() > bytes.len()
            || !bytes[offset..offset + word.len()].eq_ignore_ascii_case(word.as_bytes())
        {
            return false;
        }
        offset += word.len();
        if position + 1 < words.len()
            && (offset == bytes.len() || !bytes[offset].is_ascii_whitespace())
        {
            return false;
        }
    }
    true
}

/// Return the first executable portion of a statement, ignoring whitespace
/// and comments before its leading keyword. The fail-closed relational DDL
/// guard uses this so a client cannot bypass its actionable diagnostic by
/// prefixing an otherwise unsupported statement with a comment.
fn trim_leading_sql_comments(mut sql: &str) -> &str {
    loop {
        sql = sql.trim_start();
        if let Some(rest) = sql.strip_prefix("--") {
            sql = rest
                .find('\n')
                .map(|offset| &rest[offset + 1..])
                .unwrap_or("");
            continue;
        }
        if let Some(rest) = sql.strip_prefix("/*") {
            let Some(offset) = rest.find("*/") else {
                return "";
            };
            sql = &rest[offset + 2..];
            continue;
        }
        return sql;
    }
}

/// Lower one deliberately bounded SQL Server PIVOT shape to conditional
/// aggregation before handing the statement to sqlparser.  The rewrite is
/// intentionally narrow: the source must be a derived SELECT with simple
/// columns, the aggregate must be one of the mergeable scalar aggregates, the
/// pivot values must be static literals, and the output width is capped.  A
/// broader PIVOT implementation needs schema-aware binding and a distributed
/// pivot state contract, so unsupported shapes remain fail-closed.
fn normalize_bounded_pivot(sql: &str) -> Result<Option<String>> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let upper = trimmed.to_ascii_uppercase();
    let Some(pivot_marker) = find_top_level_keyword(&upper, "PIVOT") else {
        return Ok(None);
    };
    if !upper.starts_with("SELECT * FROM (") {
        return Ok(None);
    }
    let source_open = upper
        .find('(')
        .ok_or_else(|| sql_error("bounded PIVOT requires a derived source SELECT"))?;
    let source_close = matching_delimiter(trimmed, source_open, b'(', b')')
        .ok_or_else(|| sql_error("bounded PIVOT has an unbalanced source SELECT"))?;
    if pivot_marker <= source_close {
        return Ok(None);
    }
    let source_sql = trimmed[source_open + 1..source_close].trim();
    let after_source = trimmed[source_close + 1..pivot_marker].trim();
    if after_source.is_empty() {
        return Err(sql_error("bounded PIVOT requires a source alias"));
    }
    let pivot_open = trimmed[pivot_marker + "PIVOT".len()..]
        .find('(')
        .map(|offset| pivot_marker + "PIVOT".len() + offset)
        .ok_or_else(|| sql_error("bounded PIVOT requires an aggregate specification"))?;
    let pivot_close = matching_delimiter(trimmed, pivot_open, b'(', b')')
        .ok_or_else(|| sql_error("bounded PIVOT has an unbalanced specification"))?;
    let body = trimmed[pivot_open + 1..pivot_close].trim();
    let pivot_tail = trimmed[pivot_close + 1..].trim();
    let mut pivot_tail_parts = pivot_tail.splitn(2, char::is_whitespace);
    let pivot_alias = pivot_tail_parts.next().unwrap_or("");
    if !is_simple_identifier(pivot_alias) {
        return Ok(None);
    }
    let pivot_order = pivot_tail_parts.next().unwrap_or("").trim();
    if !pivot_order.is_empty() && !pivot_order.to_ascii_uppercase().starts_with("ORDER BY ") {
        return Ok(None);
    }

    let source_upper = source_sql.to_ascii_uppercase();
    if !source_upper.starts_with("SELECT ") {
        return Ok(None);
    }
    let Some(source_from) = find_top_level_keyword(&source_upper, "FROM") else {
        return Ok(None);
    };
    let source_projection = source_sql["SELECT ".len()..source_from].trim();
    let source_relation = source_sql[source_from + "FROM".len()..].trim();
    if source_projection.is_empty() || source_relation.is_empty() {
        return Ok(None);
    }
    let source_columns = split_top_level_commas(source_projection)?;
    if source_columns.is_empty() || source_columns.len() > 128 {
        return Err(sql_error(
            "bounded PIVOT source projection exceeds the 128-column limit",
        ));
    }
    let mut columns = Vec::with_capacity(source_columns.len());
    for expression in source_columns {
        let (expression, alias) = split_simple_alias(expression.trim())
            .ok_or_else(|| sql_error("bounded PIVOT source columns must be simple identifiers"))?;
        let name = alias.unwrap_or(expression);
        if !is_simple_identifier(name) {
            return Ok(None);
        }
        columns.push(name.to_owned());
    }

    let body_upper = body.to_ascii_uppercase();
    let aggregate_open = body
        .find('(')
        .ok_or_else(|| sql_error("bounded PIVOT requires aggregate(column)"))?;
    let aggregate_close = matching_delimiter(body, aggregate_open, b'(', b')')
        .ok_or_else(|| sql_error("bounded PIVOT aggregate is unbalanced"))?;
    let aggregate = body[..aggregate_open].trim().to_ascii_uppercase();
    if !matches!(
        aggregate.as_str(),
        "SUM" | "AVG" | "MIN" | "MAX" | "COUNT" | "COUNT_BIG"
    ) {
        return Ok(None);
    }
    let value_column = body[aggregate_open + 1..aggregate_close].trim();
    let Some(for_pos) = find_top_level_keyword(&body_upper[aggregate_close + 1..], "FOR") else {
        return Err(sql_error("bounded PIVOT requires FOR pivot_column"));
    };
    let for_pos = aggregate_close + 1 + for_pos;
    let Some(in_pos_rel) = find_top_level_keyword(&body_upper[for_pos + "FOR".len()..], "IN")
    else {
        return Err(sql_error("bounded PIVOT requires an explicit IN list"));
    };
    let in_pos = for_pos + "FOR".len() + in_pos_rel;
    let pivot_column = body[for_pos + "FOR".len()..in_pos].trim();
    if (value_column != "*" && !is_simple_identifier(value_column))
        || !is_simple_identifier(pivot_column)
    {
        return Ok(None);
    }
    if (value_column != "*"
        && !columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case(value_column)))
        || !columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case(pivot_column))
    {
        return Err(sql_error(
            "bounded PIVOT columns must be present in the source projection",
        ));
    }
    let list_open = body[in_pos + "IN".len()..]
        .find('(')
        .map(|offset| in_pos + "IN".len() + offset)
        .ok_or_else(|| sql_error("bounded PIVOT requires an IN list"))?;
    let list_close = matching_delimiter(body, list_open, b'(', b')')
        .ok_or_else(|| sql_error("bounded PIVOT IN list is unbalanced"))?;
    if !body[list_close + 1..].trim().is_empty() {
        return Ok(None);
    }
    let items = split_top_level_commas(body[list_open + 1..list_close].trim())?;
    if items.is_empty() || items.len() > 128 {
        return Err(sql_error(
            "bounded PIVOT requires between 1 and 128 IN values",
        ));
    }
    let mut parsed_items = Vec::with_capacity(items.len());
    for item in items {
        let item = item.trim();
        let label = item
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .or_else(|| {
                item.strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
            })
            .unwrap_or(item);
        if label.is_empty() || label.contains(['\r', '\n']) {
            return Err(sql_error(
                "bounded PIVOT IN values must be non-empty literals",
            ));
        }
        let literal = if label.parse::<i128>().is_ok() {
            label.to_owned()
        } else {
            format!("'{}'", label.replace('\'', "''"))
        };
        parsed_items.push((label.to_owned(), literal));
    }
    let group_columns = columns
        .iter()
        .filter(|column| {
            !column.eq_ignore_ascii_case(value_column) && !column.eq_ignore_ascii_case(pivot_column)
        })
        .collect::<Vec<_>>();
    let mut projection = group_columns
        .iter()
        .map(|column| (*column).to_owned())
        .collect::<Vec<_>>();
    for (label, literal) in parsed_items {
        let value = if matches!(aggregate.as_str(), "COUNT" | "COUNT_BIG") {
            if value_column == "*" {
                "1".to_owned()
            } else {
                value_column.to_owned()
            }
        } else {
            value_column.to_owned()
        };
        let aggregate_value = format!(
            "{aggregate}(CASE WHEN {pivot_column} = {literal} THEN {value} END) AS \"{}\"",
            label.replace('"', "\"\"")
        );
        projection.push(aggregate_value);
    }
    let mut rewritten = format!(
        "SELECT {} FROM ({source_sql}) AS __kaveon_pivot_source",
        projection.join(", ")
    );
    if !group_columns.is_empty() {
        rewritten.push_str(" GROUP BY ");
        rewritten.push_str(
            &group_columns
                .iter()
                .map(|column| (*column).to_owned())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if !pivot_order.is_empty() {
        rewritten.push(' ');
        rewritten.push_str(pivot_order);
    }
    Ok(Some(rewritten))
}

/// Lower the bounded derived-source form of SQL Server UNPIVOT to a bounded
/// UNION ALL.  The source and column list are intentionally static so the
/// rewrite cannot invent columns or perform an unbounded schema scan.
fn normalize_bounded_unpivot(sql: &str) -> Result<Option<String>> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let upper = trimmed.to_ascii_uppercase();
    let Some(unpivot_marker) = find_top_level_keyword(&upper, "UNPIVOT") else {
        return Ok(None);
    };
    if !upper.starts_with("SELECT * FROM (") {
        return Ok(None);
    }
    let source_open = upper
        .find('(')
        .ok_or_else(|| sql_error("bounded UNPIVOT requires a derived source SELECT"))?;
    let source_close = matching_delimiter(trimmed, source_open, b'(', b')')
        .ok_or_else(|| sql_error("bounded UNPIVOT has an unbalanced source SELECT"))?;
    if unpivot_marker <= source_close {
        return Ok(None);
    }
    let source_sql = trimmed[source_open + 1..source_close].trim();
    let source_upper = source_sql.to_ascii_uppercase();
    if !source_upper.starts_with("SELECT ") {
        return Ok(None);
    }
    let Some(source_from) = find_top_level_keyword(&source_upper, "FROM") else {
        return Ok(None);
    };
    let source_projection = source_sql["SELECT ".len()..source_from].trim();
    let source_relation = source_sql[source_from + "FROM".len()..].trim();
    if source_projection.is_empty() || source_relation.is_empty() {
        return Ok(None);
    }
    let source_columns = split_top_level_commas(source_projection)?;
    if source_columns.is_empty() || source_columns.len() > 128 {
        return Err(sql_error(
            "bounded UNPIVOT source projection exceeds the 128-column limit",
        ));
    }
    let mut columns = Vec::with_capacity(source_columns.len());
    for expression in source_columns {
        let (expression, alias) = split_simple_alias(expression.trim()).ok_or_else(|| {
            sql_error("bounded UNPIVOT source columns must be simple identifiers")
        })?;
        let name = alias.unwrap_or(expression);
        if !is_simple_identifier(name) {
            return Ok(None);
        }
        columns.push(name.to_owned());
    }

    let body_start = trimmed[unpivot_marker + "UNPIVOT".len()..]
        .find('(')
        .map(|offset| unpivot_marker + "UNPIVOT".len() + offset)
        .ok_or_else(|| sql_error("bounded UNPIVOT requires a value and name specification"))?;
    let body_close = matching_delimiter(trimmed, body_start, b'(', b')')
        .ok_or_else(|| sql_error("bounded UNPIVOT has an unbalanced specification"))?;
    let body = trimmed[body_start + 1..body_close].trim();
    let unpivot_tail = trimmed[body_close + 1..].trim();
    let mut unpivot_tail_parts = unpivot_tail.splitn(2, char::is_whitespace);
    let unpivot_alias = unpivot_tail_parts.next().unwrap_or("");
    if !is_simple_identifier(unpivot_alias) {
        return Ok(None);
    }
    let unpivot_order = unpivot_tail_parts.next().unwrap_or("").trim();
    if !unpivot_order.is_empty() && !unpivot_order.to_ascii_uppercase().starts_with("ORDER BY ") {
        return Ok(None);
    }
    let body_upper = body.to_ascii_uppercase();
    let Some(for_pos) = find_top_level_keyword(&body_upper, "FOR") else {
        return Err(sql_error("bounded UNPIVOT requires FOR name_column"));
    };
    let Some(in_pos_rel) = find_top_level_keyword(&body_upper[for_pos + "FOR".len()..], "IN")
    else {
        return Err(sql_error("bounded UNPIVOT requires an explicit IN list"));
    };
    let in_pos = for_pos + "FOR".len() + in_pos_rel;
    let value_column = body[..for_pos].trim();
    let name_column = body[for_pos + "FOR".len()..in_pos].trim();
    if !is_simple_identifier(value_column) || !is_simple_identifier(name_column) {
        return Ok(None);
    }
    let list_open = body[in_pos + "IN".len()..]
        .find('(')
        .map(|offset| in_pos + "IN".len() + offset)
        .ok_or_else(|| sql_error("bounded UNPIVOT requires an IN list"))?;
    let list_close = matching_delimiter(body, list_open, b'(', b')')
        .ok_or_else(|| sql_error("bounded UNPIVOT IN list is unbalanced"))?;
    if !body[list_close + 1..].trim().is_empty() {
        return Ok(None);
    }
    let items = split_top_level_commas(body[list_open + 1..list_close].trim())?;
    if items.is_empty() || items.len() > 128 {
        return Err(sql_error(
            "bounded UNPIVOT requires between 1 and 128 IN values",
        ));
    }
    let mut columns_to_unpivot = Vec::with_capacity(items.len());
    for item in items {
        let item = item.trim();
        let column = item
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .or_else(|| {
                item.strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
            })
            .unwrap_or(item);
        if !is_simple_identifier(column)
            || !columns
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(column))
        {
            return Err(sql_error(
                "bounded UNPIVOT IN values must name source columns",
            ));
        }
        columns_to_unpivot.push(column.to_owned());
    }
    let group_columns = columns
        .iter()
        .filter(|column| {
            !columns_to_unpivot
                .iter()
                .any(|selected| selected.eq_ignore_ascii_case(column))
        })
        .collect::<Vec<_>>();
    let mut branches = Vec::with_capacity(columns_to_unpivot.len());
    for column in columns_to_unpivot {
        let mut projection = group_columns
            .iter()
            .map(|group| (*group).to_owned())
            .collect::<Vec<_>>();
        projection.push(format!(
            "'{}' AS \"{}\"",
            column.replace('\'', "''"),
            name_column.replace('"', "\"\"")
        ));
        projection.push(format!(
            "{column} AS \"{}\"",
            value_column.replace('"', "\"\"")
        ));
        branches.push(format!(
            "SELECT {} FROM ({source_sql}) AS __kaveon_unpivot_source",
            projection.join(", ")
        ));
    }
    let mut rewritten = branches.join(" UNION ALL ");
    if !unpivot_order.is_empty() {
        rewritten.push(' ');
        rewritten.push_str(unpivot_order);
    }
    Ok(Some(rewritten))
}

/// Map the bounded derived-relation forms of SQL Server APPLY onto the
/// Engine's existing lateral-join grammar. Table-valued functions and other
/// arbitrary APPLY expressions are intentionally left for the explicit
/// fail-closed diagnostic below.
fn normalize_bounded_apply(sql: &str) -> Result<Option<String>> {
    let upper = sql.to_ascii_uppercase();
    let mut found = None;
    for keyword in ["CROSS APPLY", "OUTER APPLY"] {
        if let Some(position) = find_top_level_keyword(&upper, keyword) {
            found = Some((position, keyword));
            break;
        }
    }
    let Some((position, keyword)) = found else {
        return Ok(None);
    };
    let after = sql[position + keyword.len()..].trim_start();
    let base = sql.len() - after.len();
    let open = if after.starts_with('(') {
        base
    } else {
        let upper_after = after.to_ascii_uppercase();
        if !upper_after.starts_with("STRING_SPLIT")
            || !after["STRING_SPLIT".len()..].trim_start().starts_with('(')
        {
            return Ok(None);
        }
        base + after.find('(').expect("STRING_SPLIT opening delimiter")
    };
    let close = matching_delimiter(sql, open, b'(', b')')
        .ok_or_else(|| sql_error("bounded APPLY has an unbalanced derived relation"))?;
    let body = sql[open + 1..close].trim();
    let function_apply = !after.starts_with('(');
    if !function_apply {
        let body_upper = body.to_ascii_uppercase();
        if !(body_upper.starts_with("SELECT ")
            || body_upper == "VALUES"
            || body_upper.starts_with("VALUES "))
        {
            return Ok(None);
        }
    }
    let suffix = sql[close + 1..].trim_start();
    if suffix.is_empty() {
        return Ok(None);
    }
    let replacement = if keyword == "CROSS APPLY" {
        "CROSS JOIN LATERAL"
    } else {
        "LEFT JOIN LATERAL"
    };
    let mut rewritten = String::with_capacity(sql.len() + 8);
    rewritten.push_str(&sql[..position]);
    rewritten.push_str(replacement);
    rewritten.push_str(&sql[position + keyword.len()..close + 1]);
    if keyword == "OUTER APPLY" {
        // APPLY has implicit TRUE join semantics; the regular SQL grammar
        // requires an explicit predicate for LEFT JOIN LATERAL. Keep the
        // predicate before any query-level ORDER/LIMIT clauses that follow
        // the APPLY relation.
        let (alias, tail) = split_apply_tail(suffix);
        rewritten.push(' ');
        rewritten.push_str(alias);
        rewritten.push_str(" ON TRUE");
        if !tail.is_empty() {
            rewritten.push(' ');
            rewritten.push_str(tail);
        }
    } else {
        rewritten.push_str(&sql[close + 1..]);
    }
    Ok(Some(rewritten))
}

fn split_apply_tail(suffix: &str) -> (&str, &str) {
    const CLAUSES: [&str; 9] = [
        "WHERE", "GROUP BY", "HAVING", "ORDER BY", "LIMIT", "OFFSET", "FETCH", "UNION", "EXCEPT",
    ];
    let upper = suffix.to_ascii_uppercase();
    let split = CLAUSES
        .iter()
        .filter_map(|clause| find_top_level_keyword(&upper, clause))
        .min();
    split
        .map(|index| (suffix[..index].trim_end(), suffix[index..].trim()))
        .unwrap_or((suffix.trim_end(), ""))
}

fn find_top_level_keyword(sql_upper: &str, keyword: &str) -> Option<usize> {
    let bytes = sql_upper.as_bytes();
    let keyword = keyword.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                if index + 1 < bytes.len() && bytes[index + 1] == delimiter {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
            index += 1;
            continue;
        }
        match byte {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0
            && index + keyword.len() <= bytes.len()
            && bytes[index..index + keyword.len()].eq_ignore_ascii_case(keyword)
            && (index == 0 || !is_sql_word_byte(bytes[index - 1]))
            && (index + keyword.len() == bytes.len()
                || !is_sql_word_byte(bytes[index + keyword.len()]))
        {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn matching_delimiter(sql: &str, open: usize, opening: u8, closing: u8) -> Option<usize> {
    let bytes = sql.as_bytes();
    if bytes.get(open).copied()? != opening {
        return None;
    }
    let mut depth = 0usize;
    let mut quote = None;
    let mut index = open;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                if index + 1 < bytes.len() && bytes[index + 1] == delimiter {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
        } else if byte == opening {
            depth += 1;
        } else if byte == closing {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

fn split_top_level_commas(sql: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    for index in 0..bytes.len() {
        let byte = bytes[index];
        if let Some(delimiter) = quote {
            if byte == delimiter && (index == 0 || bytes[index - 1] != delimiter) {
                quote = None;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
        } else if byte == b'(' || byte == b'[' {
            depth += 1;
        } else if byte == b')' || byte == b']' {
            depth = depth
                .checked_sub(1)
                .ok_or_else(|| sql_error("unbalanced delimiter"))?;
        } else if byte == b',' && depth == 0 {
            parts.push(sql[start..index].trim());
            start = index + 1;
        }
    }
    if quote.is_some() || depth != 0 {
        return Err(sql_error("unbalanced delimiter"));
    }
    let tail = sql[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    Ok(parts)
}

fn split_simple_alias(expression: &str) -> Option<(&str, Option<&str>)> {
    let upper = expression.to_ascii_uppercase();
    if let Some(position) = find_top_level_keyword(&upper, "AS") {
        let left = expression[..position].trim();
        let right = expression[position + 2..].trim();
        return (!left.is_empty() && !right.is_empty()).then_some((left, Some(right)));
    }
    Some((expression, None))
}

fn is_simple_identifier(identifier: &str) -> bool {
    let identifier = identifier.trim().trim_matches('"');
    !identifier.is_empty()
        && identifier.chars().enumerate().all(|(index, character)| {
            character == '_'
                || character == '$'
                || character.is_ascii_alphanumeric() && index > 0
                || character.is_ascii_alphabetic() && index == 0
        })
}

fn parse_bounded_map_casts(sql: &str) -> Option<Vec<Statement>> {
    use sqlparser::ast::{ArrayElemTypeDef, DataType, visit_expressions};
    use std::ops::ControlFlow;

    if !sql.to_ascii_uppercase().contains("MAP") {
        return None;
    }
    // Generic accepts ARRAY<T>; ClickHouse supplies the Trino ARRAY(T)
    // spelling. Both must pass the same bounded AST check below.
    let statements = Parser::parse_sql(&GenericDialect {}, sql)
        .or_else(|_| Parser::parse_sql(&ClickHouseDialect {}, sql))
        .ok()?;
    fn scalar_member(member: &DataType) -> bool {
        !matches!(
            member,
            DataType::Array(_)
                | DataType::Map(_, _)
                | DataType::Struct(_, _)
                | DataType::Tuple(_)
                | DataType::Nested(_)
                | DataType::Union(_)
                | DataType::Nullable(_)
                | DataType::LowCardinality(_)
                | DataType::Custom(_, _)
        )
    }
    // The evaluator already casts nested LIST children recursively.  Admit
    // the bounded nested-array shape here instead of rejecting it merely
    // because DuckDB's grammar cannot carry the Trino MAP type through its
    // normal AST.  Keep the depth cap explicit: arbitrary recursive complex
    // casts still fail closed until their schema and exchange contracts are
    // qualified separately.
    fn bounded_array_member(member: &DataType, depth: usize) -> bool {
        if scalar_member(member) {
            return true;
        }
        if depth >= 1 {
            return false;
        }
        match member {
            DataType::Array(
                ArrayElemTypeDef::AngleBracket(element)
                | ArrayElemTypeDef::Parenthesis(element)
                | ArrayElemTypeDef::SquareBracket(element, None),
            ) => bounded_array_member(element, depth + 1),
            _ => false,
        }
    }
    let mut found_bounded_map = false;
    let bounded = visit_expressions(&statements, |expr| {
        if let Expr::Cast {
            data_type: DataType::Map(key, value),
            ..
        } = expr
        {
            if !scalar_member(key) {
                return ControlFlow::Break(());
            }
            match value.as_ref() {
                DataType::Array(
                    ArrayElemTypeDef::AngleBracket(element)
                    | ArrayElemTypeDef::Parenthesis(element)
                    | ArrayElemTypeDef::SquareBracket(element, None),
                ) if bounded_array_member(element, 0) => found_bounded_map = true,
                member if scalar_member(member) => {
                    found_bounded_map |=
                        matches!(key.as_ref(), DataType::Decimal(_) | DataType::Numeric(_))
                            || matches!(member, DataType::Decimal(_) | DataType::Numeric(_));
                }
                _ => return ControlFlow::Break(()),
            }
        }
        ControlFlow::Continue(())
    });
    (bounded.is_continue() && found_bounded_map).then_some(statements)
}

/// Lower the bounded SQL/JSON `JSON_EXISTS ... ON ERROR` actions to explicit
/// evaluator variants.  The default Trino behavior is FALSE on error; TRUE
/// and UNKNOWN are represented by dedicated names so malformed documents do
/// not get confused with a valid document whose path is absent.
fn normalize_json_exists_error_clauses(sql: &str) -> Result<String> {
    let mut output = String::with_capacity(sql.len());
    let mut cursor = 0;
    while cursor < sql.len() {
        let Some((name_start, open)) = next_json_exists_function(sql, cursor) else {
            output.push_str(&sql[cursor..]);
            break;
        };
        output.push_str(&sql[cursor..name_start]);
        let Some(close) = matching_paren(sql, open) else {
            output.push_str(&sql[name_start..]);
            break;
        };
        let inner = normalize_json_exists_error_clauses(&sql[open + 1..close])?;
        let (inner, action) = strip_json_exists_error_clause(&inner)?;
        let name = match action {
            Some(JsonExistsErrorAction::True) => "JSON_EXISTS_TRUE_ON_ERROR",
            Some(JsonExistsErrorAction::Unknown) => "JSON_EXISTS_UNKNOWN_ON_ERROR",
            None | Some(JsonExistsErrorAction::False) => "JSON_EXISTS",
        };
        output.push_str(name);
        output.push('(');
        output.push_str(&inner);
        output.push(')');
        cursor = close + 1;
    }
    Ok(output)
}

#[derive(Clone, Copy)]
enum JsonExistsErrorAction {
    True,
    False,
    Unknown,
}

fn next_json_exists_function(sql: &str, start: usize) -> Option<(usize, usize)> {
    let bytes = sql.as_bytes();
    let mut cursor = start;
    while cursor < bytes.len() {
        if matches!(bytes[cursor], b'\'' | b'"') {
            cursor = quoted_sql_end(sql, cursor);
            continue;
        }
        let name_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        if cursor == name_start {
            cursor += 1;
            continue;
        }
        if !sql[name_start..cursor].eq_ignore_ascii_case("JSON_EXISTS")
            || bytes
                .get(name_start.wrapping_sub(1))
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            continue;
        }
        let mut open = cursor;
        while open < bytes.len() && bytes[open].is_ascii_whitespace() {
            open += 1;
        }
        if bytes.get(open) == Some(&b'(') {
            return Some((name_start, open));
        }
    }
    None
}

fn strip_json_exists_error_clause(inner: &str) -> Result<(String, Option<JsonExistsErrorAction>)> {
    let bytes = inner.as_bytes();
    let mut cursor = 0;
    let mut depth = 0usize;
    let mut range = None;
    let mut action = None;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\'' | b'"' => cursor = quoted_sql_end(inner, cursor),
            b'(' | b'[' => {
                depth += 1;
                cursor += 1;
            }
            b')' | b']' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
            }
            _ if depth == 0 => {
                let token = ["TRUE", "FALSE", "UNKNOWN"]
                    .into_iter()
                    .find(|token| json_is_sql_word_at(inner, cursor, token));
                if let Some(token) = token {
                    let start = cursor;
                    cursor += token.len();
                    cursor = skip_sql_space(inner, cursor);
                    if !json_is_sql_word_at(inner, cursor, "ON") {
                        continue;
                    }
                    cursor += 2;
                    cursor = skip_sql_space(inner, cursor);
                    if !json_is_sql_word_at(inner, cursor, "ERROR") {
                        return Err(KaveonError::Sql(
                            "invalid JSON_EXISTS error clause; expected ON ERROR".into(),
                        ));
                    }
                    cursor += "ERROR".len();
                    if range.is_some() {
                        return Err(KaveonError::Sql(
                            "duplicate JSON_EXISTS ON ERROR clause".into(),
                        ));
                    }
                    action = Some(match token {
                        "TRUE" => JsonExistsErrorAction::True,
                        "FALSE" => JsonExistsErrorAction::False,
                        "UNKNOWN" => JsonExistsErrorAction::Unknown,
                        _ => unreachable!(),
                    });
                    range = Some((start, cursor));
                    continue;
                }
                cursor += 1;
            }
            _ => cursor += 1,
        }
    }
    let Some((start, end)) = range else {
        return Ok((inner.to_owned(), None));
    };
    let mut result = inner.to_owned();
    result.replace_range(start..end, "");
    Ok((result, action))
}

/// Lower the bounded SQL/JSON `WITH UNIQUE KEYS` constructor clause.
///
/// The evaluator already rejects duplicate object keys deterministically.  The
/// SQL/JSON clause therefore only needs to be removed before the dialect parser
/// sees it; this preserves the existing fail-closed duplicate-key behavior while
/// accepting Trino's explicit spelling. `WITHOUT UNIQUE KEYS` is deliberately
/// rejected because silently selecting a duplicate-key winner would change the
/// result depending on the JSON materializer.
fn normalize_json_object_unique_keys(sql: &str) -> Result<String> {
    let mut output = String::with_capacity(sql.len());
    let mut cursor = 0;
    while cursor < sql.len() {
        let Some((name_start, name, open)) = next_json_object_function(sql, cursor) else {
            output.push_str(&sql[cursor..]);
            break;
        };
        output.push_str(&sql[cursor..name_start]);
        let Some(close) = matching_paren(sql, open) else {
            output.push_str(&sql[name_start..]);
            break;
        };
        let inner = normalize_json_object_unique_keys(&sql[open + 1..close])?;
        let inner = strip_json_object_unique_keys(&inner)?;
        output.push_str(&name);
        output.push('(');
        output.push_str(&inner);
        output.push(')');
        cursor = close + 1;
    }
    Ok(output)
}

fn next_json_object_function(sql: &str, start: usize) -> Option<(usize, String, usize)> {
    let bytes = sql.as_bytes();
    let mut cursor = start;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\'' || bytes[cursor] == b'"' {
            cursor = quoted_sql_end(sql, cursor);
            continue;
        }
        let name_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        if cursor == name_start {
            cursor += 1;
            continue;
        }
        let name = &sql[name_start..cursor];
        if !name.eq_ignore_ascii_case("JSON_OBJECT")
            || bytes
                .get(name_start.wrapping_sub(1))
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            continue;
        }
        let mut open = cursor;
        while open < bytes.len() && bytes[open].is_ascii_whitespace() {
            open += 1;
        }
        if bytes.get(open) == Some(&b'(') {
            return Some((name_start, name.to_owned(), open));
        }
    }
    None
}

fn strip_json_object_unique_keys(inner: &str) -> Result<String> {
    let bytes = inner.as_bytes();
    let mut ranges = Vec::new();
    let mut cursor = 0;
    let mut depth = 0usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\'' | b'"' => cursor = quoted_sql_end(inner, cursor),
            b'(' | b'[' => {
                depth += 1;
                cursor += 1;
            }
            b')' | b']' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
            }
            _ if depth == 0
                && (json_is_sql_word_at(inner, cursor, "WITH")
                    || json_is_sql_word_at(inner, cursor, "WITHOUT")) =>
            {
                let start = cursor;
                let without = json_is_sql_word_at(inner, cursor, "WITHOUT");
                cursor += if without {
                    "WITHOUT".len()
                } else {
                    "WITH".len()
                };
                cursor = skip_sql_space(inner, cursor);
                if !json_is_sql_word_at(inner, cursor, "UNIQUE") {
                    return Err(KaveonError::Sql(
                        "invalid JSON_OBJECT unique-key clause; expected UNIQUE KEYS".into(),
                    ));
                }
                cursor += "UNIQUE".len();
                cursor = skip_sql_space(inner, cursor);
                if !json_is_sql_word_at(inner, cursor, "KEYS") {
                    return Err(KaveonError::Sql(
                        "invalid JSON_OBJECT unique-key clause; expected UNIQUE KEYS".into(),
                    ));
                }
                cursor += "KEYS".len();
                if without {
                    return Err(KaveonError::Sql(
                        "JSON_OBJECT WITHOUT UNIQUE KEYS is not supported; duplicate keys are rejected".into(),
                    ));
                }
                if !ranges.is_empty() {
                    return Err(KaveonError::Sql(
                        "duplicate JSON_OBJECT WITH UNIQUE KEYS clause".into(),
                    ));
                }
                ranges.push((start, cursor));
            }
            _ => cursor += 1,
        }
    }
    let mut result = inner.to_owned();
    for (start, end) in ranges.into_iter().rev() {
        result.replace_range(start..end, "");
    }
    Ok(result)
}

/// Lower the narrow SQL/JSON `ON EMPTY NULL` and `ON ERROR NULL` clauses.
///
/// The current evaluator already returns NULL for a missing path, a
/// non-scalar JSON_VALUE result, and malformed JSON.  Removing these explicit
/// NULL clauses therefore preserves the existing behavior while accepting the
/// useful bounded Trino spelling. Literal DEFAULT actions are lowered to a
/// four-argument evaluator function; arbitrary default expressions remain
/// rejected explicitly. Typed RETURNING combinations are lowered through the
/// default evaluator and the regular CAST path.
fn normalize_json_null_clauses(sql: &str) -> Result<String> {
    let mut output = String::with_capacity(sql.len());
    let mut cursor = 0;
    while cursor < sql.len() {
        let Some((name_start, name, open)) = next_json_function(sql, cursor) else {
            output.push_str(&sql[cursor..]);
            break;
        };
        output.push_str(&sql[cursor..name_start]);
        let Some(close) = matching_paren(sql, open) else {
            output.push_str(&sql[name_start..]);
            break;
        };
        let inner = normalize_json_null_clauses(&sql[open + 1..close])?;
        let (inner, strict, defaults) = strip_json_null_clauses(&inner)?;
        let typed_default = defaults.as_ref().and_then(|_| split_json_returning(&inner));
        let function_name = if defaults.is_some() {
            format!("{name}_DEFAULT")
        } else if strict {
            format!("{name}_STRICT")
        } else {
            name
        };
        if let Some((arguments, _)) = typed_default.as_ref() {
            output.push_str("CAST(");
            output.push_str(&function_name);
            output.push('(');
            output.push_str(arguments.trim_end());
        } else {
            output.push_str(&function_name);
            output.push('(');
            output.push_str(&inner);
        }
        if let Some((empty_default, error_default)) = defaults {
            output.push_str(", ");
            output.push_str(empty_default.as_deref().unwrap_or("NULL"));
            output.push_str(", ");
            output.push_str(error_default.as_deref().unwrap_or("NULL"));
        }
        output.push(')');
        if let Some((_, data_type)) = typed_default {
            output.push_str(" AS ");
            output.push_str(&data_type);
            output.push(')');
        }
        cursor = close + 1;
    }
    Ok(output)
}

type JsonClauseDefaults = Option<(Option<String>, Option<String>)>;

fn strip_json_null_clauses(inner: &str) -> Result<(String, bool, JsonClauseDefaults)> {
    let bytes = inner.as_bytes();
    let mut ranges = Vec::new();
    let mut strict = false;
    let mut null = false;
    let mut empty_default = None;
    let mut error_default = None;
    let mut cursor = 0;
    let mut depth = 0usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\'' | b'"' => cursor = quoted_sql_end(inner, cursor),
            b'(' | b'[' => {
                depth += 1;
                cursor += 1;
            }
            b')' | b']' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
            }
            _ if depth == 0 && json_is_sql_word_at(inner, cursor, "ON") => {
                let start = cursor;
                cursor += 2;
                cursor = skip_sql_space(inner, cursor);
                let kind = if json_is_sql_word_at(inner, cursor, "EMPTY") {
                    "ON EMPTY"
                } else if json_is_sql_word_at(inner, cursor, "ERROR") {
                    "ON ERROR"
                } else {
                    return Err(KaveonError::Sql(
                        "invalid JSON ON clause; expected EMPTY or ERROR".into(),
                    ));
                };
                cursor += kind[3..].len();
                cursor = skip_sql_space(inner, cursor);
                let Some((action, action_end)) = take_sql_word(inner, cursor) else {
                    return Err(KaveonError::Sql(format!(
                        "invalid JSON {kind} clause; expected NULL"
                    )));
                };
                if action.eq_ignore_ascii_case("NULL") {
                    null = true;
                } else if action.eq_ignore_ascii_case("ERROR") {
                    strict = true;
                } else if action.eq_ignore_ascii_case("DEFAULT") {
                    cursor = skip_sql_space(inner, action_end);
                    let Some((literal, literal_end)) = take_json_default_literal(inner, cursor)
                    else {
                        return Err(KaveonError::Sql(format!(
                            "unsupported JSON {kind} DEFAULT expression; only SQL literals are supported"
                        )));
                    };
                    if kind == "ON EMPTY" {
                        if empty_default.is_some() {
                            return Err(KaveonError::Sql("duplicate JSON ON EMPTY clause".into()));
                        }
                        empty_default = Some(literal);
                    } else {
                        if error_default.is_some() {
                            return Err(KaveonError::Sql("duplicate JSON ON ERROR clause".into()));
                        }
                        error_default = Some(literal);
                    }
                    cursor = literal_end;
                    ranges.push((start, cursor));
                    continue;
                } else {
                    return Err(KaveonError::Sql(format!(
                        "unsupported JSON {kind} clause action '{action}'; only NULL, ERROR, or a literal DEFAULT is supported"
                    )));
                }
                cursor = action_end;
                ranges.push((start, cursor));
            }
            _ => cursor += 1,
        }
    }
    if strict && null {
        return Err(KaveonError::Sql(
            "JSON ON EMPTY/ON ERROR clauses must use the same action".into(),
        ));
    }
    if (empty_default.is_some() || error_default.is_some()) && (strict || null) {
        return Err(KaveonError::Sql(
            "JSON DEFAULT clauses cannot be combined with NULL or ERROR clauses".into(),
        ));
    }
    if !ranges.is_empty() {
        let mut result = inner.to_owned();
        for (start, end) in ranges.into_iter().rev() {
            result.replace_range(start..end, "");
        }
        let defaults = if empty_default.is_some() || error_default.is_some() {
            Some((empty_default, error_default))
        } else {
            None
        };
        return Ok((result, strict, defaults));
    }
    Ok((inner.to_owned(), strict, None))
}

/// Consume the narrow literal form accepted after a SQL/JSON DEFAULT action.
/// Arbitrary expressions remain rejected so the rewrite cannot silently change
/// evaluation order or type coercion semantics.
fn take_json_default_literal(sql: &str, start: usize) -> Option<(String, usize)> {
    let bytes = sql.as_bytes();
    let &first = bytes.get(start)?;
    if first == b'\'' || first == b'"' {
        let end = quoted_sql_end(sql, start);
        return (end > start + 1).then(|| (sql[start..end].to_owned(), end));
    }
    let mut end = start;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || b".+-".contains(&bytes[end]))
    {
        end += 1;
    }
    if end == start {
        return None;
    }
    let token = &sql[start..end];
    if token.eq_ignore_ascii_case("TRUE")
        || token.eq_ignore_ascii_case("FALSE")
        || token.eq_ignore_ascii_case("NULL")
        || token.parse::<i64>().is_ok()
        || token.parse::<f64>().is_ok()
    {
        Some((token.to_owned(), end))
    } else {
        None
    }
}

/// Lower the bounded SQL/JSON typed-return form to the existing typed CAST
/// path.  Trino's JSON_VALUE/JSON_QUERY functions accept a `RETURNING` clause;
/// the executor already implements the equivalent conversion after the JSON
/// scalar/document has been extracted.  Keep this rewrite deliberately narrow
/// so unsupported SQL/JSON clauses remain parser errors rather than silently
/// receiving different semantics.
fn normalize_json_returning(sql: &str) -> String {
    let mut output = String::with_capacity(sql.len());
    let mut cursor = 0;
    while cursor < sql.len() {
        let Some((name_start, name, open)) = next_json_function(sql, cursor) else {
            output.push_str(&sql[cursor..]);
            break;
        };
        output.push_str(&sql[cursor..name_start]);
        let Some(close) = matching_paren(sql, open) else {
            output.push_str(&sql[name_start..]);
            break;
        };
        let inner = normalize_json_returning(&sql[open + 1..close]);
        if let Some((arguments, data_type)) = split_json_returning(&inner) {
            output.push_str("CAST(");
            output.push_str(&name);
            output.push('(');
            output.push_str(arguments.trim_end());
            output.push_str(") AS ");
            output.push_str(&data_type);
            output.push(')');
        } else {
            output.push_str(&name);
            output.push('(');
            output.push_str(&inner);
            output.push(')');
        }
        cursor = close + 1;
    }
    output
}

fn next_json_function(sql: &str, start: usize) -> Option<(usize, String, usize)> {
    let bytes = sql.as_bytes();
    let mut cursor = start;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\'' || bytes[cursor] == b'"' {
            cursor = quoted_sql_end(sql, cursor);
            continue;
        }
        let name_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        if cursor == name_start {
            cursor += 1;
            continue;
        }
        let name = &sql[name_start..cursor];
        let upper = name.to_ascii_uppercase();
        if matches!(
            upper.as_str(),
            "JSON_VALUE" | "JSON_QUERY" | "JSON_VALUE_STRICT" | "JSON_QUERY_STRICT"
        ) && bytes
            .get(name_start.wrapping_sub(1))
            .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_')
        {
            let mut open = cursor;
            while open < bytes.len() && bytes[open].is_ascii_whitespace() {
                open += 1;
            }
            if bytes.get(open) == Some(&b'(') {
                return Some((name_start, name.to_owned(), open));
            }
        }
    }
    None
}

fn matching_paren(sql: &str, open: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut cursor = open;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\'' | b'"' => cursor = quoted_sql_end(sql, cursor),
            b'(' => {
                depth += 1;
                cursor += 1;
            }
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(cursor);
                }
                cursor += 1;
            }
            _ => cursor += 1,
        }
    }
    None
}

fn quoted_sql_end(sql: &str, quote: usize) -> usize {
    let bytes = sql.as_bytes();
    let delimiter = bytes[quote];
    let mut cursor = quote + 1;
    while cursor < bytes.len() {
        if bytes[cursor] == delimiter {
            if bytes.get(cursor + 1) == Some(&delimiter) {
                cursor += 2;
                continue;
            }
            return cursor + 1;
        }
        if bytes[cursor] == b'\\' && delimiter == b'\'' {
            cursor = (cursor + 2).min(bytes.len());
        } else {
            cursor += 1;
        }
    }
    bytes.len()
}

fn split_json_returning(inner: &str) -> Option<(&str, String)> {
    let bytes = inner.as_bytes();
    let mut depth = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\'' | b'"' => cursor = quoted_sql_end(inner, cursor),
            b'(' | b'[' => {
                depth += 1;
                cursor += 1;
            }
            b')' | b']' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
            }
            _ if depth == 0 && json_is_sql_word_at(inner, cursor, "RETURNING") => {
                let arguments = &inner[..cursor];
                let type_start = cursor + "RETURNING".len();
                let type_name = inner[type_start..].trim();
                let upper_type = type_name.to_ascii_uppercase();
                let data_type = match upper_type.as_str() {
                    "TINYINT"
                    | "SMALLINT"
                    | "BIGINT"
                    | "INTEGER"
                    | "DOUBLE"
                    | "REAL"
                    | "BOOLEAN"
                    | "VARCHAR"
                    | "DATE"
                    | "TIMESTAMP"
                    | "TIMESTAMP WITH TIME ZONE"
                    | "JSON" => upper_type,
                    _ => parse_json_decimal_type(type_name)?,
                };
                return (!arguments.trim().is_empty()).then_some((arguments, data_type));
            }
            _ => cursor += 1,
        }
    }
    None
}

/// Return a canonical, bounded Decimal128 type for a SQL/JSON RETURNING clause.
///
/// Decimal coercion is routed through the regular CAST evaluator, so accepting
/// only precision/scale pairs representable by Arrow Decimal128 keeps this
/// rewrite exact and avoids silently changing wider Trino decimal semantics.
fn parse_json_decimal_type(type_name: &str) -> Option<String> {
    let trimmed = type_name.trim();
    let upper = trimmed.to_ascii_uppercase();
    let rest = upper.strip_prefix("DECIMAL")?.trim();
    let rest = rest.strip_prefix('(')?.strip_suffix(')')?;
    let (precision, scale) = rest.split_once(',')?;
    let precision = precision.trim().parse::<u8>().ok()?;
    let scale = scale.trim().parse::<u8>().ok()?;
    if !(1..=38).contains(&precision) || scale > precision {
        return None;
    }
    Some(format!("DECIMAL({precision}, {scale})"))
}

fn json_is_sql_word_at(sql: &str, offset: usize, word: &str) -> bool {
    let end = offset.saturating_add(word.len());
    sql.get(offset..end)
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(word))
        && !sql
            .as_bytes()
            .get(offset.wrapping_sub(1))
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        && !sql
            .as_bytes()
            .get(end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

/// Rewrites a single-column, monotonic integer recursive CTE into a bounded
/// `SEQUENCE` relation.  The accepted form is intentionally narrow:
///
/// ```text
/// WITH RECURSIVE n(value) AS (
///   SELECT 1
///   UNION [ALL]
///   SELECT value + 1 FROM n WHERE value < 5
/// ) SELECT * FROM n
/// ```
///
/// The rewrite is valid because the recursive member has one arithmetic step,
/// one source CTE, and a strict/inclusive bound.  Any other shape returns
/// `None`, preserving the planner's explicit unsupported-recursion error.
fn normalize_bounded_linear_recursive_cte(sql: &str) -> Option<String> {
    let trimmed = sql.trim();
    let prefix = "WITH RECURSIVE";
    let prefix_text = trimmed.get(..prefix.len())?;
    if !prefix_text.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let mut cursor = prefix.len();
    cursor = skip_sql_space(trimmed, cursor);
    let (cte_name, next) = take_sql_word(trimmed, cursor)?;
    cursor = skip_sql_space(trimmed, next);
    // Trino permits the recursive column list to be omitted when the seed
    // projection supplies its column name.  Keep this bounded rewrite safe by
    // inferring exactly one identifier from `SELECT <integer> AS <name>`; a
    // nameless or multi-column seed remains on the explicit unsupported path.
    let explicit_column = if trimmed.as_bytes().get(cursor) == Some(&b'(') {
        cursor += 1;
        cursor = skip_sql_space(trimmed, cursor);
        let (column, next) = take_sql_word(trimmed, cursor)?;
        cursor = skip_sql_space(trimmed, next);
        if trimmed.as_bytes().get(cursor) != Some(&b')') {
            return None;
        }
        cursor = skip_sql_space(trimmed, cursor + 1);
        Some(column)
    } else {
        None
    };
    cursor = consume_sql_token(trimmed, cursor, "AS")?;
    cursor = skip_sql_space(trimmed, cursor);
    if trimmed.as_bytes().get(cursor) != Some(&b'(') {
        return None;
    }
    let body_start = cursor + 1;
    let body_end = matching_sql_paren(trimmed, cursor)?;
    let body = trimmed[body_start..body_end].trim();
    let outer = trimmed[body_end + 1..].trim_start();
    if outer.is_empty() || outer.starts_with(',') {
        return None;
    }
    // The generated sequence is unique, so the bounded monotonic shape has
    // identical results for UNION and UNION ALL. Accept both spellings while
    // keeping general recursive UNION graphs explicitly unsupported.
    let (seed_sql, recursive_sql) = split_top_level_union(body)?;
    let (seed, inferred_column) = parse_integer_seed(seed_sql.trim())?;
    if let (Some(expected), Some(actual)) = (&explicit_column, &inferred_column)
        && !expected.eq_ignore_ascii_case(actual)
    {
        return None;
    }
    let column = explicit_column.or(inferred_column)?;
    let tokens = recursive_sql.split_whitespace().collect::<Vec<_>>();
    let from = if tokens.len() == 12 { 6 } else { 4 };
    if !(tokens.len() == 10 || tokens.len() == 12)
        || !tokens[0].eq_ignore_ascii_case("SELECT")
        || !tokens[1].eq_ignore_ascii_case(&column)
        || !matches!(tokens[2], "+" | "-")
        || !tokens[from].eq_ignore_ascii_case("FROM")
        || (tokens.len() == 12
            && (!tokens[4].eq_ignore_ascii_case("AS") || !tokens[5].eq_ignore_ascii_case(&column)))
        || !tokens[from + 1].eq_ignore_ascii_case(&cte_name)
        || !tokens[from + 2].eq_ignore_ascii_case("WHERE")
        || !tokens[from + 3].eq_ignore_ascii_case(&column)
        || !matches!(tokens[from + 4], "<" | "<=" | ">" | ">=")
    {
        return None;
    }
    let magnitude = tokens[3].parse::<i64>().ok()?;
    let step = if tokens[2] == "-" {
        magnitude.checked_neg()?
    } else {
        magnitude
    };
    let bound = tokens[from + 5].parse::<i64>().ok()?;
    if step == 0 {
        return None;
    }
    let stop = match (step.signum(), tokens[from + 4]) {
        (1, "<") => bound,
        (1, "<=") => bound.checked_add(step)?,
        (-1, ">") => bound,
        (-1, ">=") => bound.checked_add(step)?,
        _ => return None,
    };
    Some(format!(
        "WITH {cte_name}({column}) AS (SELECT * FROM UNNEST(SEQUENCE({seed}, {stop}, {step})) AS {cte_name}({column})) {outer}"
    ))
}

fn skip_sql_space(sql: &str, mut cursor: usize) -> usize {
    while sql
        .as_bytes()
        .get(cursor)
        .is_some_and(u8::is_ascii_whitespace)
    {
        cursor += 1;
    }
    cursor
}

fn take_sql_word(sql: &str, cursor: usize) -> Option<(String, usize)> {
    let bytes = sql.as_bytes();
    let mut end = cursor;
    while bytes
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        end += 1;
    }
    (end > cursor).then(|| (sql[cursor..end].to_owned(), end))
}

fn consume_sql_token(sql: &str, cursor: usize, token: &str) -> Option<usize> {
    let end = cursor.checked_add(token.len())?;
    sql.get(cursor..end)?
        .eq_ignore_ascii_case(token)
        .then_some(end)
}

fn matching_sql_paren(sql: &str, open: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut cursor = open;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                if bytes.get(cursor + 1) == Some(&delimiter) {
                    cursor += 2;
                    continue;
                }
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
        } else if byte == b'(' {
            depth += 1;
        } else if byte == b')' {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(cursor);
            }
        }
        cursor += 1;
    }
    None
}

fn split_top_level_union(body: &str) -> Option<(&str, &str)> {
    let bytes = body.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                if bytes.get(cursor + 1) == Some(&delimiter) {
                    cursor += 2;
                    continue;
                }
                quote = None;
            }
            cursor += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
            cursor += 1;
            continue;
        }
        match byte {
            b'(' => depth += 1,
            b')' => depth = depth.checked_sub(1)?,
            _ => {}
        }
        if depth == 0 && body[cursor..].to_ascii_uppercase().starts_with("UNION") {
            let before = if cursor == 0 { b' ' } else { bytes[cursor - 1] };
            let union_end = cursor + 5;
            let after = bytes.get(union_end).copied().unwrap_or(b' ');
            if !before.is_ascii_alphanumeric() && !after.is_ascii_alphanumeric() {
                let mut consumed = 5;
                let whitespace = body
                    .as_bytes()
                    .get(union_end..)
                    .unwrap_or_default()
                    .iter()
                    .take_while(|byte| byte.is_ascii_whitespace())
                    .count();
                let all_start = union_end + whitespace;
                if body
                    .get(all_start..all_start + 3)
                    .is_some_and(|token| token.eq_ignore_ascii_case("ALL"))
                    && !body
                        .as_bytes()
                        .get(all_start + 3)
                        .is_some_and(u8::is_ascii_alphanumeric)
                {
                    consumed = all_start + 3 - cursor;
                }
                return Some((&body[..cursor], &body[cursor + consumed..]));
            }
        }
        cursor += 1;
    }
    None
}

fn parse_integer_seed(sql: &str) -> Option<(i64, Option<String>)> {
    let tokens = sql.split_whitespace().collect::<Vec<_>>();
    match tokens.as_slice() {
        [select, value] if select.eq_ignore_ascii_case("SELECT") => {
            Some((value.parse().ok()?, None))
        }
        [select, value, as_token, alias]
            if select.eq_ignore_ascii_case("SELECT") && as_token.eq_ignore_ascii_case("AS") =>
        {
            Some((value.parse().ok()?, Some((*alias).to_owned())))
        }
        _ => None,
    }
}

fn contains_lateral_unnest(sql: &str) -> bool {
    let upper = sql.to_ascii_uppercase();
    upper
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|window| window[0] == "LATERAL" && window[1].starts_with("UNNEST"))
}

/// The upstream parser represents `LATERAL UNNEST(...)` as a generic table
/// function, which has no `WITH ORDINALITY`/`WITH OFFSET` field. Removing the
/// redundant lateral keyword for this one table-shaped form lets it produce
/// the richer `TableFactor::UNNEST` node; the logical binder still recognizes
/// correlated expressions only when the factor is joined laterally.
fn normalize_lateral_unnest(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut copied = 0usize;
    let mut i = 0usize;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let byte = bytes[i];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                if i + 1 < bytes.len() && bytes[i + 1] == delimiter {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
            i += 1;
            continue;
        }
        if is_sql_word_at(bytes, i, b"LATERAL") {
            let mut next = i + b"LATERAL".len();
            while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                next += 1;
            }
            if is_sql_word_at(bytes, next, b"UNNEST") {
                out.push_str(&sql[copied..i]);
                copied = i + b"LATERAL".len();
                i = copied;
                continue;
            }
        }
        i += 1;
    }
    if copied == 0 {
        sql.to_owned()
    } else {
        out.push_str(&sql[copied..]);
        out
    }
}

fn is_sql_word_at(bytes: &[u8], offset: usize, word: &[u8]) -> bool {
    offset + word.len() <= bytes.len()
        && bytes[offset..offset + word.len()].eq_ignore_ascii_case(word)
        && (offset == 0 || !is_sql_word_byte(bytes[offset - 1]))
        && (offset + word.len() == bytes.len() || !is_sql_word_byte(bytes[offset + word.len()]))
}

/// Find an unsupported SQL word in executable text, ignoring quoted values,
/// quoted identifiers, and comments. A substring scan is not sufficient here:
/// a valid query such as `SELECT 'pivot'` must not be rejected merely because
/// its literal contains the word.
fn contains_sql_word(sql: &[u8], word: &[u8]) -> bool {
    let mut index = 0usize;
    while index < sql.len() {
        match sql[index] {
            b'\'' | b'"' | b'`' => {
                let quote = sql[index];
                index += 1;
                while index < sql.len() {
                    if sql[index] == quote {
                        if index + 1 < sql.len() && sql[index + 1] == quote {
                            index += 2;
                            continue;
                        }
                        index += 1;
                        break;
                    }
                    index += 1;
                }
            }
            b'[' => {
                index += 1;
                while index < sql.len() {
                    if sql[index] == b']' {
                        if index + 1 < sql.len() && sql[index + 1] == b']' {
                            index += 2;
                            continue;
                        }
                        index += 1;
                        break;
                    }
                    index += 1;
                }
            }
            b'-' if sql.get(index + 1) == Some(&b'-') => {
                index += 2;
                while index < sql.len() && sql[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if sql.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < sql.len() {
                    if sql[index] == b'*' && sql[index + 1] == b'/' {
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            _ => {
                if index + word.len() <= sql.len()
                    && sql[index..index + word.len()].eq_ignore_ascii_case(word)
                    && is_sql_word_at(sql, index, word)
                {
                    return true;
                }
                index += 1;
            }
        }
    }
    false
}

fn is_sql_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn normalize_bracket_identifiers(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    // Everything before `copied` has been written out. The statement is copied
    // as string slices and only the bracket spans this rewrites are emitted
    // character by character.
    //
    // It used to rebuild the whole statement with `out.push(b as char)` per
    // byte. In Rust `u8 as char` is a Latin-1 conversion: byte 0xF0 becomes
    // U+00F0, which the String then re-encodes as 0xC3 0xB0. So every
    // multi-byte UTF-8 sequence in any statement came out as one character per
    // byte -- in identifiers and string literals alike -- and because product
    // records are written as SQL literals, the corruption was stored and
    // compounded on each rewrite. A chart named with an en dash reached the
    // screen as "New Chart ÃÂ¢ÃÂÃÂ ..." after three saves.
    //
    // Byte indexing stays valid because every delimiter scanned for here is
    // ASCII, and a UTF-8 continuation byte can never equal an ASCII byte, so
    // `copied` and the slice bounds always land on character boundaries.
    let mut copied = 0usize;
    let mut i = 0usize;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                if i + 1 < bytes.len() && bytes[i + 1] == q {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        if matches!(b, b'\'' | b'"' | b'`') {
            quote = Some(b);
            i += 1;
            continue;
        }
        if b == b'['
            && let Some(end_rel) = bytes[i + 1..].iter().position(|c| *c == b']')
        {
            let end = i + 1 + end_rel;
            let inner = &sql[i + 1..end];
            let mut chars = inner.chars();
            let identifier = chars
                .next()
                .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                && chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric());
            // `ARRAY[NULL]` is a list literal, not a bracketed identifier.
            // Identifier-shaped list elements (especially `NULL`) must stay
            // in the AST so the binder can infer the list type from context.
            // Keep the existing bracket-identifier normalization everywhere
            // else, including `[year]` and `table.[year]`.
            let preceded_by_array = sql[..i]
                .trim_end()
                .rsplit_once(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .map(|(_, word)| word.eq_ignore_ascii_case("ARRAY"))
                .unwrap_or_else(|| sql[..i].trim_end().eq_ignore_ascii_case("ARRAY"));
            if identifier && !preceded_by_array {
                out.push_str(&sql[copied..i]);
                out.push('"');
                out.push_str(inner);
                out.push('"');
                i = end + 1;
                copied = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&sql[copied..]);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeDmlKind {
    Insert,
    Update,
    Delete,
    Merge,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativeDmlStatement {
    pub kind: NativeDmlKind,
    pub table: String,
    pub ast: Statement,
    pub returning: Vec<String>,
    pub output_pseudo: Option<String>,
    pub on_conflict_do_nothing: bool,
    pub on_conflict_do_update: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NativeTransactionalStatement {
    Begin,
    Commit,
    Rollback,
    Savepoint(String),
    RollbackToSavepoint(String),
    ReleaseSavepoint(String),
    Dml(Box<NativeDmlStatement>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductDmlCommand {
    Create {
        kind: String,
        id: String,
        document_json: String,
    },
    CreateIgnore {
        kind: String,
        id: String,
        document_json: String,
    },
    Upsert {
        kind: String,
        id: String,
        document_json: String,
    },
    /// Bounded MERGE semantics: delete the matched product row, or create the
    /// source document when no row matches. The transaction layer resolves
    /// the current revision immediately before staging the CAS change.
    MergeDeleteOrInsert {
        kind: String,
        id: String,
        document_json: String,
    },
    /// Bounded MERGE delete-or-insert semantics with a literal source-id
    /// predicate on the matched clause. Existing rows are deleted only when
    /// their ids satisfy `matched_ids`; absent source rows still insert.
    MergeDeleteOrCreateFiltered {
        kind: String,
        id: String,
        document_json: String,
        matched_ids: Vec<String>,
    },
    /// Bounded MERGE semantics: delete a matched product row.  An unmatched
    /// source row is a transactional no-op, matching SQL MERGE semantics.
    MergeDelete { kind: String, id: String },
    /// Bounded MERGE semantics: update a matched product row. An unmatched
    /// source row is a transactional no-op, matching SQL MERGE semantics.
    MergeUpdate {
        kind: String,
        id: String,
        document_json: String,
    },
    /// Bounded MERGE update-or-insert semantics with a literal source-id
    /// predicate on the matched clause.  The transaction layer updates only
    /// existing target rows whose ids satisfy `matched_ids`; absent source
    /// rows still follow the ordinary NOT MATCHED insert action.
    MergeUpdateOrCreateFiltered {
        kind: String,
        id: String,
        document_json: String,
        matched_ids: Vec<String>,
    },
    /// Bounded MERGE semantics: create a source product row when unmatched;
    /// an already-present key is a transactional no-op.
    MergeCreateIfAbsent {
        kind: String,
        id: String,
        document_json: String,
    },
    /// Bounded MERGE semantics: delete target rows owned by the actor whose
    /// ids are absent from the source VALUES batch. The transaction layer
    /// expands this against the pinned snapshot before staging CAS deletes.
    MergeDeleteNotMatched {
        kind: String,
        source_ids: Vec<String>,
        target_ids: Option<Vec<String>>,
    },
    /// Bounded MERGE semantics: update target rows owned by the actor whose
    /// ids are absent from the source VALUES batch. The transaction layer
    /// expands this against the pinned snapshot before staging CAS updates.
    MergeUpdateNotMatched {
        kind: String,
        source_ids: Vec<String>,
        target_ids: Option<Vec<String>>,
        document_json: String,
    },
    /// Copy product records from another product table in the pinned
    /// transactional snapshot.  The server expands this bounded form into
    /// ordinary create/update commands; it is deliberately not a lake-table
    /// INSERT ... SELECT implementation.
    InsertSelect {
        kind: String,
        source_kind: String,
        source_ids: Option<Vec<String>>,
        on_conflict_do_nothing: bool,
        on_conflict_do_update: bool,
        returning: Vec<String>,
    },
    Update {
        kind: String,
        id: String,
        expected_revision: u64,
        document_json: String,
    },
    /// Bounded source-backed UPDATE ... FROM semantics. The parser admits one
    /// same-kind product source, a literal source id or identity join, and at
    /// most 1,000 literal target ids; the transaction layer resolves each
    /// document from the pinned snapshot before staging normal CAS updates.
    UpdateFrom {
        kind: String,
        id: String,
        expected_revision: u64,
        source_id: String,
        source_kind: String,
    },
    Delete {
        kind: String,
        id: String,
        expected_revision: u64,
    },
    /// Bounded same-kind source-backed DELETE ... USING semantics. The source
    /// is owner-checked before each revision/CAS delete is staged.
    DeleteFrom {
        kind: String,
        id: String,
        expected_revision: u64,
        source_id: String,
        source_kind: String,
    },
}

/// Adapt only Kaveon's typed product-record SQL facade. Arbitrary row DML has
/// no storage contract and is rejected instead of being silently emulated.
pub fn adapt_product_dml(dml: &NativeDmlStatement) -> Result<ProductDmlCommand> {
    let mut commands = adapt_product_dml_batch(dml)?;
    if commands.len() != 1 {
        return Err(sql_error(
            "product DML statement expands to multiple records; use the transaction batch API",
        ));
    }
    Ok(commands.remove(0))
}

/// Adapt a product DML statement into one or more transactional commands.
/// Multi-row INSERT is intentionally expanded here so the transaction layer
/// can stage every row under one commit boundary without interpolating SQL.
pub fn adapt_product_dml_batch(dml: &NativeDmlStatement) -> Result<Vec<ProductDmlCommand>> {
    let kind = if dml.table.eq_ignore_ascii_case("kaveon.product.typed_rows")
        || dml.table.eq_ignore_ascii_case("product.typed_rows")
    {
        "typed_row".to_owned()
    } else {
        product_kind(&dml.table)?
    };
    match &dml.ast {
        Statement::Insert(insert) => {
            let columns: Vec<_> = insert
                .columns
                .iter()
                .map(|column| column.value.as_str())
                .collect();
            // SQL INSERT column order is part of the statement contract, not
            // the product-record storage layout. Resolve both supported
            // columns by name so clients can emit `(document_json, id)` as
            // well as the canonical `(id, document_json)` order.
            let id_index = columns
                .iter()
                .position(|column| column.eq_ignore_ascii_case("id"));
            let document_index = columns
                .iter()
                .position(|column| column.eq_ignore_ascii_case("document_json"));
            if columns.len() != 2
                || id_index.is_none()
                || document_index.is_none()
                || id_index == document_index
            {
                return Err(sql_error(
                    "product INSERT columns must contain exactly id and document_json",
                ));
            }
            let id_index = id_index.expect("validated product INSERT id column");
            let document_index = document_index.expect("validated product INSERT document column");
            let Some(query) = insert.source.as_deref() else {
                return Err(sql_error("product INSERT requires VALUES or SELECT"));
            };
            match query.body.as_ref() {
                SetExpr::Values(values) => {
                    if values.rows.is_empty() || values.rows.iter().any(|row| row.len() != 2) {
                        return Err(sql_error(
                            "product INSERT rows must contain id and document_json",
                        ));
                    }
                    let conflict_override = if dml.on_conflict_do_update {
                        conflict_document_override(insert.on.as_ref())?
                    } else {
                        None
                    };
                    values
                        .rows
                        .iter()
                        .map(|row| {
                            Ok(if dml.on_conflict_do_update {
                                ProductDmlCommand::Upsert {
                                    kind: kind.clone(),
                                    id: string_literal(&row[id_index], "product ID")?,
                                    document_json: conflict_override.clone().unwrap_or(
                                        string_literal(
                                            &row[document_index],
                                            "product document_json",
                                        )?,
                                    ),
                                }
                            } else if dml.on_conflict_do_nothing {
                                ProductDmlCommand::CreateIgnore {
                                    kind: kind.clone(),
                                    id: string_literal(&row[id_index], "product ID")?,
                                    document_json: string_literal(
                                        &row[document_index],
                                        "product document_json",
                                    )?,
                                }
                            } else {
                                ProductDmlCommand::Create {
                                    kind: kind.clone(),
                                    id: string_literal(&row[id_index], "product ID")?,
                                    document_json: string_literal(
                                        &row[document_index],
                                        "product document_json",
                                    )?,
                                }
                            })
                        })
                        .collect()
                }
                SetExpr::Select(select) => {
                    if dml.on_conflict_do_update
                        && conflict_document_override(insert.on.as_ref())?.is_some()
                    {
                        return Err(sql_error(
                            "literal ON CONFLICT document assignments require VALUES",
                        ));
                    }
                    if query.with.is_some()
                        || query.order_by.is_some()
                        || query.limit.is_some()
                        || !query.limit_by.is_empty()
                        || query.offset.is_some()
                        || query.fetch.is_some()
                        || !query.locks.is_empty()
                        || query.for_clause.is_some()
                        || query.settings.is_some()
                        || query.format_clause.is_some()
                        || select.distinct.is_some()
                        || select.top.is_some()
                        || select.into.is_some()
                        || (!select.from.is_empty() && !is_bounded_product_source(select, &kind))
                        || !select.lateral_views.is_empty()
                        || select.prewhere.is_some()
                        || (select.selection.is_some()
                            && (select.from.is_empty()
                                || !is_bounded_product_source(select, &kind)))
                        || !matches!(
                            &select.group_by,
                            GroupByExpr::Expressions(expressions, modifiers)
                                if expressions.is_empty() && modifiers.is_empty()
                        )
                        || !select.cluster_by.is_empty()
                        || !select.distribute_by.is_empty()
                        || !select.sort_by.is_empty()
                        || select.having.is_some()
                        || !select.named_window.is_empty()
                        || select.qualify.is_some()
                        || select.value_table_mode.is_some()
                        || select.connect_by.is_some()
                    {
                        return Err(sql_error(
                            "product INSERT SELECT supports one constant SELECT or a bounded product-table copy",
                        ));
                    }
                    if !select.from.is_empty() {
                        let (source_kind, source_ids) = bounded_product_source_kind(select, &kind)?;
                        if !dml.returning.is_empty()
                            && dml.returning.iter().any(|column| {
                                !column.eq_ignore_ascii_case("id")
                                    && !column.eq_ignore_ascii_case("revision")
                                    && !column.eq_ignore_ascii_case("document_json")
                            })
                        {
                            return Err(sql_error(
                                "source-backed product INSERT SELECT RETURNING supports only id, revision, or document_json",
                            ));
                        }
                        return Ok(vec![ProductDmlCommand::InsertSelect {
                            kind,
                            source_kind,
                            source_ids,
                            on_conflict_do_nothing: dml.on_conflict_do_nothing,
                            on_conflict_do_update: dml.on_conflict_do_update,
                            returning: dml.returning.clone(),
                        }]);
                    }
                    let mut values = [None, None];
                    if select.projection.len() != 2 {
                        return Err(sql_error(
                            "product INSERT SELECT must project id and document_json",
                        ));
                    }
                    for (index, item) in select.projection.iter().enumerate() {
                        let SelectItem::UnnamedExpr(expression) = item else {
                            return Err(sql_error(
                                "product INSERT SELECT does not accept projection aliases",
                            ));
                        };
                        values[index] = Some(expression);
                    }
                    let values = values.map(|value| value.expect("projection length checked"));
                    Ok(vec![if dml.on_conflict_do_update {
                        ProductDmlCommand::Upsert {
                            kind,
                            id: string_literal(values[id_index], "product ID")?,
                            document_json: string_literal(
                                values[document_index],
                                "product document_json",
                            )?,
                        }
                    } else if dml.on_conflict_do_nothing {
                        ProductDmlCommand::CreateIgnore {
                            kind,
                            id: string_literal(values[id_index], "product ID")?,
                            document_json: string_literal(
                                values[document_index],
                                "product document_json",
                            )?,
                        }
                    } else {
                        ProductDmlCommand::Create {
                            kind,
                            id: string_literal(values[id_index], "product ID")?,
                            document_json: string_literal(
                                values[document_index],
                                "product document_json",
                            )?,
                        }
                    }])
                }
                _ => Err(sql_error(
                    "product INSERT supports VALUES or one constant SELECT only",
                )),
            }
        }
        Statement::Update {
            table,
            assignments,
            from,
            selection,
            ..
        } => {
            if let Some(source) = from {
                let (target_alias, source_kind, source_alias) =
                    product_update_from_tables(table, source, &kind)?;
                if assignments.len() != 1 {
                    return Err(sql_error("product UPDATE FROM may set document_json only"));
                }
                let AssignmentTarget::ColumnName(column) = &assignments[0].target else {
                    return Err(sql_error("product UPDATE FROM may set document_json only"));
                };
                if !((column.0.len() == 1
                    && column.0[0].value.eq_ignore_ascii_case("document_json"))
                    || (column.0.len() == 2
                        && column.0[0].value.eq_ignore_ascii_case(&target_alias)
                        && column.0[1].value.eq_ignore_ascii_case("document_json")))
                    || !merge_source_document_ref(&assignments[0].value, &source_alias)
                {
                    return Err(sql_error(
                        "product UPDATE FROM must set target.document_json from source.document_json",
                    ));
                }
                let updates = product_update_from_predicate(
                    selection.as_ref(),
                    &target_alias,
                    &source_alias,
                )?;
                return Ok(updates
                    .into_iter()
                    .map(
                        |(id, expected_revision, source_id)| ProductDmlCommand::UpdateFrom {
                            kind: kind.clone(),
                            id,
                            expected_revision,
                            source_id,
                            source_kind: source_kind.clone(),
                        },
                    )
                    .collect());
            }
            if assignments.len() != 1 {
                return Err(sql_error("product UPDATE may set document_json only"));
            }
            let AssignmentTarget::ColumnName(column) = &assignments[0].target else {
                return Err(sql_error("product UPDATE may set document_json only"));
            };
            if column.0.len() != 1 || !column.0[0].value.eq_ignore_ascii_case("document_json") {
                return Err(sql_error("product UPDATE may set document_json only"));
            }
            let document_json = product_document_literal(&assignments[0].value)?;
            let (ids, expected_revision) = product_key_predicate(selection.as_ref())?;
            Ok(ids
                .into_iter()
                .map(|id| ProductDmlCommand::Update {
                    kind: kind.clone(),
                    id,
                    expected_revision,
                    document_json: document_json.clone(),
                })
                .collect())
        }
        Statement::Delete(delete) => {
            let using = delete.using.as_ref();
            if let Some(using) = using {
                let tables = match &delete.from {
                    FromTable::WithFromKeyword(tables) if tables.len() == 1 => tables,
                    _ => {
                        return Err(sql_error(
                            "product DELETE USING supports one same-kind source table",
                        ));
                    }
                };
                if using.len() != 1 || !delete.tables.is_empty() {
                    return Err(sql_error(
                        "product DELETE USING supports one same-kind source table",
                    ));
                }
                let (target_alias, source_kind, source_alias) =
                    product_update_from_tables(&tables[0], &using[0], &kind)?;
                let ids = product_update_from_predicate(
                    delete.selection.as_ref(),
                    &target_alias,
                    &source_alias,
                )?;
                return Ok(ids
                    .into_iter()
                    .map(
                        |(id, expected_revision, source_id)| ProductDmlCommand::DeleteFrom {
                            kind: kind.clone(),
                            id,
                            expected_revision,
                            source_id,
                            source_kind: source_kind.clone(),
                        },
                    )
                    .collect());
            }
            let (ids, expected_revision) = product_key_predicate(delete.selection.as_ref())?;
            Ok(ids
                .into_iter()
                .map(|id| ProductDmlCommand::Delete {
                    kind: kind.clone(),
                    id,
                    expected_revision,
                })
                .collect())
        }
        Statement::Merge { .. } => adapt_product_merge(&dml.ast),
        _ => Err(sql_error("statement is not product DML")),
    }
}

fn adapt_product_merge(statement: &Statement) -> Result<Vec<ProductDmlCommand>> {
    let Statement::Merge {
        table,
        source,
        on,
        clauses,
        ..
    } = statement
    else {
        return Err(sql_error("statement is not MERGE"));
    };
    let (target_name, target_alias) = match table {
        TableFactor::Table {
            name,
            alias,
            args: None,
            with_hints,
            version: None,
            with_ordinality: false,
            partitions,
            json_path: None,
        } if with_hints.is_empty()
            && partitions.is_empty()
            && alias.as_ref().is_none_or(|alias| alias.columns.is_empty()) =>
        {
            let target_name = validated_table_name(name)?;
            let target_alias = alias
                .as_ref()
                .map(|alias| alias.name.value.clone())
                .or_else(|| target_name.rsplit('.').next().map(str::to_owned))
                .ok_or_else(|| sql_error("MERGE target must have a table name"))?;
            (target_name, target_alias)
        }
        _ => {
            return Err(sql_error(
                "MERGE target must be one base product table with an optional simple alias",
            ));
        }
    };
    let kind = product_kind(&target_name)?;
    let (source_alias, source_columns, source_values) = match source {
        TableFactor::Derived {
            lateral: false,
            subquery,
            alias: Some(alias),
        } if alias.columns.len() == 2 => {
            let SetExpr::Values(values) = subquery.body.as_ref() else {
                return Err(sql_error("MERGE source must be a bounded VALUES batch"));
            };
            if subquery.with.is_some()
                || subquery.order_by.is_some()
                || subquery.limit.is_some()
                || values.rows.is_empty()
                || values.rows.len() > 1000
                || values.rows.iter().any(|row| row.len() != 2)
            {
                return Err(sql_error(
                    "MERGE source must be a bounded VALUES batch of at most 1000 two-column rows",
                ));
            }
            (
                alias.name.value.clone(),
                [
                    alias.columns[0].name.value.clone(),
                    alias.columns[1].name.value.clone(),
                ],
                values.rows.clone(),
            )
        }
        _ => {
            return Err(sql_error(
                "MERGE source must be an aliased VALUES batch with id and document_json columns",
            ));
        }
    };
    if !source_columns[0].eq_ignore_ascii_case("id")
        || !source_columns[1].eq_ignore_ascii_case("document_json")
    {
        return Err(sql_error(
            "MERGE source columns must be id and document_json",
        ));
    }
    let mut source_ids = Vec::with_capacity(source_values.len());
    for row in &source_values {
        let id = string_literal(&row[0], "MERGE source id")?;
        if source_ids.iter().any(|existing| existing == &id) {
            return Err(sql_error(
                "MERGE source ids must be unique within the bounded batch",
            ));
        }
        source_ids.push(id);
    }
    if !merge_id_predicate(on, &target_alias, &source_alias) {
        return Err(sql_error("MERGE ON must equate target.id with source.id"));
    }
    let matched_filter_allowed = clauses.len() == 1
        || (clauses.len() == 2 && merge_is_not_matched_target(&clauses[1].clause_kind));
    let matched_source_filter = if matched_filter_allowed
        && matches!(
            clauses.first().map(|clause| &clause.clause_kind),
            Some(MergeClauseKind::Matched)
        ) {
        if clauses[0]
            .predicate
            .as_ref()
            .is_some_and(|predicate| merge_id_predicate(predicate, &target_alias, &source_alias))
        {
            None
        } else {
            merge_source_filter(clauses[0].predicate.as_ref(), &target_alias, &source_alias)?
        }
    } else {
        None
    };
    let not_matched_source_filter =
        if clauses.len() == 1 && merge_is_not_matched_target(&clauses[0].clause_kind) {
            clauses[0]
                .predicate
                .as_ref()
                .map(|predicate| {
                    merge_insert_source_filter(Some(predicate), &target_alias, &source_alias)
                })
                .transpose()?
                .flatten()
        } else {
            None
        };
    let single_matched_clause =
        clauses.len() == 1 && matches!(clauses[0].clause_kind, MergeClauseKind::Matched);
    let has_not_matched_by_source_delete = clauses.len() == 3
        && matches!(clauses[2].clause_kind, MergeClauseKind::NotMatchedBySource)
        && matches!(clauses[2].action, MergeAction::Delete)
        && matches!(clauses[0].clause_kind, MergeClauseKind::Matched)
        && merge_is_not_matched_target(&clauses[1].clause_kind);
    let has_not_matched_by_source_update = clauses.len() == 3
        && matches!(clauses[2].clause_kind, MergeClauseKind::NotMatchedBySource)
        && matches!(clauses[2].action, MergeAction::Update { .. })
        && matches!(clauses[0].clause_kind, MergeClauseKind::Matched)
        && merge_is_not_matched_target(&clauses[1].clause_kind);
    let not_matched_by_source_target_filter =
        if has_not_matched_by_source_delete || has_not_matched_by_source_update {
            merge_target_filter(clauses[2].predicate.as_ref(), &target_alias)?
        } else {
            None
        };
    if clauses.iter().enumerate().any(|(index, clause)| {
        clause.predicate.as_ref().is_some_and(|predicate| {
            if single_matched_clause && index == 0 {
                return false;
            }
            if index == 0
                && matches!(clause.clause_kind, MergeClauseKind::Matched)
                && matched_source_filter.is_some()
            {
                return false;
            }
            if index == 2
                && (has_not_matched_by_source_delete || has_not_matched_by_source_update)
                && not_matched_by_source_target_filter.is_some()
            {
                return false;
            }
            !((index == 0
                && matches!(clause.clause_kind, MergeClauseKind::Matched)
                && merge_id_predicate(predicate, &target_alias, &source_alias))
                || (index == 0
                    && clauses.len() == 1
                    && merge_is_not_matched_target(&clause.clause_kind)
                    && not_matched_source_filter.is_some()))
        })
    }) {
        return Err(sql_error(
            "MERGE clauses support only the redundant target.id = source.id predicate",
        ));
    }
    if has_not_matched_by_source_delete || has_not_matched_by_source_update {
        let MergeAction::Update { assignments } = &clauses[0].action else {
            return Err(sql_error(
                "MERGE WHEN MATCHED must update before WHEN NOT MATCHED BY SOURCE",
            ));
        };
        if assignments.len() != 1
            || !merge_document_target(&assignments[0].target, &target_alias)
            || !merge_source_document_ref(&assignments[0].value, &source_alias)
        {
            return Err(sql_error(
                "MERGE WHEN MATCHED must set document_json from source",
            ));
        }
        let MergeAction::Insert(insert) = &clauses[1].action else {
            return Err(sql_error(
                "MERGE WHEN NOT MATCHED must insert id and document_json",
            ));
        };
        let MergeInsertKind::Values(values) = &insert.kind else {
            return Err(sql_error("MERGE WHEN NOT MATCHED must use VALUES"));
        };
        if insert.columns.len() != 2
            || !insert.columns[0].value.eq_ignore_ascii_case("id")
            || !insert.columns[1]
                .value
                .eq_ignore_ascii_case("document_json")
            || values.rows.len() != 1
            || values.rows[0].len() != 2
            || !merge_source_ref(&values.rows[0][0], &source_alias, "id")
            || merge_document_assignment(&values.rows[0][1], &source_alias).is_none()
        {
            return Err(sql_error(
                "MERGE WHEN NOT MATCHED must insert source id and document_json",
            ));
        }
        let mut commands = source_values
            .iter()
            .map(|row| {
                Ok(ProductDmlCommand::Upsert {
                    kind: kind.clone(),
                    id: string_literal(&row[0], "MERGE source id")?,
                    document_json: merge_document_value(
                        &values.rows[0][1],
                        &source_alias,
                        &row[1],
                    )?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if has_not_matched_by_source_delete {
            commands.push(ProductDmlCommand::MergeDeleteNotMatched {
                kind: kind.clone(),
                source_ids,
                target_ids: not_matched_by_source_target_filter.clone(),
            });
        } else {
            let MergeAction::Update { assignments } = &clauses[2].action else {
                unreachable!("not-matched-by-source update was checked above");
            };
            if assignments.len() != 1
                || !merge_document_target(&assignments[0].target, &target_alias)
            {
                return Err(sql_error(
                    "MERGE WHEN NOT MATCHED BY SOURCE must set document_json to a string literal",
                ));
            }
            let document_json = product_document_literal(&assignments[0].value).map_err(|_| {
                sql_error(
                    "MERGE WHEN NOT MATCHED BY SOURCE document_json must be a string literal or CAST(string AS JSON)",
                )
            })?;
            commands.push(ProductDmlCommand::MergeUpdateNotMatched {
                kind: kind.clone(),
                source_ids,
                target_ids: not_matched_by_source_target_filter.clone(),
                document_json,
            });
        }
        return Ok(commands);
    }
    let matched_delete = matches!(clauses[0].action, MergeAction::Delete);
    let matched_only_delete = clauses.len() == 1
        && matches!(clauses[0].clause_kind, MergeClauseKind::Matched)
        && matched_delete;
    if matched_only_delete {
        return source_values
            .iter()
            .filter(|row| {
                matched_source_filter.as_ref().is_none_or(|ids| {
                    string_literal(&row[0], "MERGE source id")
                        .map(|id| ids.contains(&id))
                        .unwrap_or(false)
                })
            })
            .map(|row| {
                Ok(ProductDmlCommand::MergeDelete {
                    kind: kind.clone(),
                    id: string_literal(&row[0], "MERGE source id")?,
                })
            })
            .collect();
    }
    let matched_only_update = clauses.len() == 1
        && matches!(clauses[0].clause_kind, MergeClauseKind::Matched)
        && matches!(clauses[0].action, MergeAction::Update { .. });
    if matched_only_update {
        let MergeAction::Update { assignments } = &clauses[0].action else {
            unreachable!("matched_only_update checks the MERGE action");
        };
        let document_assignment = if assignments.len() != 1
            || !merge_document_target(&assignments[0].target, &target_alias)
        {
            None
        } else {
            merge_document_assignment(&assignments[0].value, &source_alias)
        };
        let Some(document_assignment) = document_assignment else {
            return Err(sql_error(
                "MERGE WHEN MATCHED must set document_json from source or a string literal",
            ));
        };
        return source_values
            .iter()
            .filter(|row| {
                matched_source_filter.as_ref().is_none_or(|ids| {
                    string_literal(&row[0], "MERGE source id")
                        .map(|id| ids.contains(&id))
                        .unwrap_or(false)
                })
            })
            .map(|row| {
                let document_json = match &document_assignment {
                    MergeDocumentAssignment::Source => {
                        string_literal(&row[1], "MERGE source document_json")?
                    }
                    MergeDocumentAssignment::Literal(value) => value.clone(),
                };
                Ok(ProductDmlCommand::MergeUpdate {
                    kind: kind.clone(),
                    id: string_literal(&row[0], "MERGE source id")?,
                    document_json,
                })
            })
            .collect();
    }
    let not_matched_only_insert =
        clauses.len() == 1 && merge_is_not_matched_target(&clauses[0].clause_kind);
    if not_matched_only_insert {
        let MergeAction::Insert(insert) = &clauses[0].action else {
            return Err(sql_error(
                "MERGE WHEN NOT MATCHED must insert id and document_json",
            ));
        };
        let MergeInsertKind::Values(values) = &insert.kind else {
            return Err(sql_error("MERGE WHEN NOT MATCHED must use VALUES"));
        };
        if insert.columns.len() != 2
            || !insert.columns[0].value.eq_ignore_ascii_case("id")
            || !insert.columns[1]
                .value
                .eq_ignore_ascii_case("document_json")
            || values.rows.len() != 1
            || values.rows[0].len() != 2
            || !merge_source_ref(&values.rows[0][0], &source_alias, "id")
            || merge_document_assignment(&values.rows[0][1], &source_alias).is_none()
        {
            return Err(sql_error(
                "MERGE WHEN NOT MATCHED must insert source id and document_json",
            ));
        }
        return source_values
            .iter()
            .filter(|row| {
                not_matched_source_filter.as_ref().is_none_or(|ids| {
                    string_literal(&row[0], "MERGE source id")
                        .map(|id| ids.contains(&id))
                        .unwrap_or(false)
                })
            })
            .map(|row| {
                Ok(ProductDmlCommand::MergeCreateIfAbsent {
                    kind: kind.clone(),
                    id: string_literal(&row[0], "MERGE source id")?,
                    document_json: merge_document_value(
                        &values.rows[0][1],
                        &source_alias,
                        &row[1],
                    )?,
                })
            })
            .collect();
    }
    if clauses.len() != 2
        || !matches!(clauses[0].clause_kind, MergeClauseKind::Matched)
        || !merge_is_not_matched_target(&clauses[1].clause_kind)
    {
        return Err(sql_error(
            "MERGE requires WHEN MATCHED UPDATE or DELETE followed by WHEN NOT MATCHED INSERT, or matched DELETE only",
        ));
    }
    if !matched_delete {
        let MergeAction::Update { assignments } = &clauses[0].action else {
            return Err(sql_error("MERGE WHEN MATCHED must update or delete"));
        };
        if assignments.len() != 1
            || !merge_document_target(&assignments[0].target, &target_alias)
            || merge_document_assignment(&assignments[0].value, &source_alias).is_none()
        {
            return Err(sql_error(
                "MERGE WHEN MATCHED must set document_json from source or a string literal",
            ));
        }
    }
    let MergeAction::Insert(insert) = &clauses[1].action else {
        return Err(sql_error(
            "MERGE WHEN NOT MATCHED must insert id and document_json",
        ));
    };
    let MergeInsertKind::Values(values) = &insert.kind else {
        return Err(sql_error("MERGE WHEN NOT MATCHED must use VALUES"));
    };
    if insert.columns.len() != 2
        || !insert.columns[0].value.eq_ignore_ascii_case("id")
        || !insert.columns[1]
            .value
            .eq_ignore_ascii_case("document_json")
        || values.rows.len() != 1
        || values.rows[0].len() != 2
        || !merge_source_ref(&values.rows[0][0], &source_alias, "id")
        || merge_document_assignment(&values.rows[0][1], &source_alias).is_none()
    {
        return Err(sql_error(
            "MERGE WHEN NOT MATCHED must insert source id and document_json",
        ));
    }
    let matched_document_assignment = if matched_delete {
        None
    } else {
        let MergeAction::Update { assignments } = &clauses[0].action else {
            unreachable!("matched delete was handled above");
        };
        Some(merge_document_assignment(&assignments[0].value, &source_alias).unwrap())
    };
    let matched_ids = matched_source_filter.map(|ids| ids.into_iter().collect::<Vec<_>>());
    source_values
        .iter()
        .map(|row| {
            Ok(if matched_delete {
                if let Some(matched_ids) = &matched_ids {
                    ProductDmlCommand::MergeDeleteOrCreateFiltered {
                        kind: kind.clone(),
                        id: string_literal(&row[0], "MERGE source id")?,
                        document_json: merge_document_value(
                            &values.rows[0][1],
                            &source_alias,
                            &row[1],
                        )?,
                        matched_ids: matched_ids.clone(),
                    }
                } else {
                    ProductDmlCommand::MergeDeleteOrInsert {
                        kind: kind.clone(),
                        id: string_literal(&row[0], "MERGE source id")?,
                        document_json: merge_document_value(
                            &values.rows[0][1],
                            &source_alias,
                            &row[1],
                        )?,
                    }
                }
            } else if let Some(matched_ids) = &matched_ids {
                ProductDmlCommand::MergeUpdateOrCreateFiltered {
                    kind: kind.clone(),
                    id: string_literal(&row[0], "MERGE source id")?,
                    document_json: match matched_document_assignment.as_ref().unwrap() {
                        MergeDocumentAssignment::Source => {
                            string_literal(&row[1], "MERGE source document_json")?
                        }
                        MergeDocumentAssignment::Literal(value) => value.clone(),
                    },
                    matched_ids: matched_ids.clone(),
                }
            } else {
                let document_json = match matched_document_assignment.as_ref().unwrap() {
                    MergeDocumentAssignment::Source => {
                        string_literal(&row[1], "MERGE source document_json")?
                    }
                    MergeDocumentAssignment::Literal(value) => value.clone(),
                };
                ProductDmlCommand::Upsert {
                    kind: kind.clone(),
                    id: string_literal(&row[0], "MERGE source id")?,
                    document_json,
                }
            })
        })
        .collect()
}

/// SQL Server exposes the target-side insert arm as either `WHEN NOT MATCHED`
/// or the explicit `WHEN NOT MATCHED BY TARGET`. Both spellings have the same
/// semantics; keep the native product-DML surface bounded while accepting the
/// explicit form used by T-SQL clients.
fn merge_is_not_matched_target(kind: &MergeClauseKind) -> bool {
    matches!(
        kind,
        MergeClauseKind::NotMatched | MergeClauseKind::NotMatchedByTarget
    )
}

fn merge_id_predicate(expression: &Expr, target: &str, source: &str) -> bool {
    let Expr::BinaryOp { left, op, right } = expression else {
        return false;
    };
    if *op != BinaryOperator::Eq {
        return false;
    }
    (merge_source_ref(left, target, "id") && merge_source_ref(right, source, "id"))
        || (merge_source_ref(right, target, "id") && merge_source_ref(left, source, "id"))
}

/// Return the source ids selected by the bounded, matched-only MERGE
/// predicate.  The transaction layer still resolves each selected id against
/// its pinned snapshot and applies the normal revision/CAS command, so this is
/// only a deterministic source-row filter, not a second mutation path.
fn merge_source_filter(
    predicate: Option<&Expr>,
    target_alias: &str,
    source_alias: &str,
) -> Result<Option<HashSet<String>>> {
    let Some(predicate) = predicate else {
        return Ok(None);
    };
    if let Expr::BinaryOp { left, op, right } = predicate
        && *op == BinaryOperator::Eq
    {
        let value = if merge_source_ref(left, source_alias, "id")
            || merge_source_ref(left, target_alias, "id")
        {
            string_literal(right, "MERGE matched source id")?
        } else if merge_source_ref(right, source_alias, "id")
            || merge_source_ref(right, target_alias, "id")
        {
            string_literal(left, "MERGE matched source id")?
        } else {
            return Err(sql_error(
                "MERGE matched predicates must filter source.id or target.id by a string literal or IN list",
            ));
        };
        return Ok(Some(HashSet::from([value])));
    }
    if let Expr::InList {
        expr,
        list,
        negated: false,
    } = predicate
    {
        if !merge_source_ref(expr, source_alias, "id")
            && !merge_source_ref(expr, target_alias, "id")
        {
            return Err(sql_error(
                "MERGE matched IN predicates must reference source.id or target.id",
            ));
        }
        let mut ids = HashSet::with_capacity(list.len());
        for value in list {
            ids.insert(string_literal(value, "MERGE matched source id")?);
        }
        return Ok(Some(ids));
    }
    Err(sql_error(
        "MERGE matched predicates support only source.id/target.id = 'literal' or an IN list",
    ))
}

/// Return the target ids selected by a bounded `WHEN NOT MATCHED BY SOURCE`
/// predicate. Unlike matched-clause filters, this arm cannot reference a
/// source row, so only the declared target alias and literal string ids are
/// accepted.
fn merge_target_filter(
    predicate: Option<&Expr>,
    target_alias: &str,
) -> Result<Option<Vec<String>>> {
    let Some(predicate) = predicate else {
        return Ok(None);
    };
    if let Expr::BinaryOp { left, op, right } = predicate
        && *op == BinaryOperator::Eq
    {
        let value = if merge_source_ref(left, target_alias, "id") {
            string_literal(right, "MERGE target id")?
        } else if merge_source_ref(right, target_alias, "id") {
            string_literal(left, "MERGE target id")?
        } else {
            return Err(sql_error(
                "MERGE NOT MATCHED BY SOURCE predicates must filter target.id by a string literal or IN list",
            ));
        };
        return Ok(Some(vec![value]));
    }
    if let Expr::InList {
        expr,
        list,
        negated: false,
    } = predicate
    {
        if !merge_source_ref(expr, target_alias, "id") {
            return Err(sql_error(
                "MERGE NOT MATCHED BY SOURCE IN predicates must reference target.id",
            ));
        }
        let mut ids = Vec::with_capacity(list.len());
        for value in list {
            let id = string_literal(value, "MERGE target id")?;
            if !ids.iter().any(|known| known == &id) {
                ids.push(id);
            }
        }
        return Ok(Some(ids));
    }
    Err(sql_error(
        "MERGE NOT MATCHED BY SOURCE predicates support only target.id = 'literal' or an IN list",
    ))
}

/// The insert-only `WHEN NOT MATCHED` form has no target row, so its bounded
/// predicate must select source rows only.  Keep this stricter than the
/// matched-clause helper, which also permits the equivalent target-id filter.
fn merge_insert_source_filter(
    predicate: Option<&Expr>,
    target_alias: &str,
    source_alias: &str,
) -> Result<Option<HashSet<String>>> {
    let Some(predicate) = predicate else {
        return Ok(None);
    };
    let references_target = match predicate {
        Expr::BinaryOp { left, right, .. } => {
            merge_source_ref(left, target_alias, "id")
                || merge_source_ref(right, target_alias, "id")
        }
        Expr::InList { expr, .. } => merge_source_ref(expr, target_alias, "id"),
        _ => false,
    };
    if references_target {
        return Err(sql_error(
            "MERGE insert-only predicates must filter source.id; target.id is not available for WHEN NOT MATCHED",
        ));
    }
    merge_source_filter(Some(predicate), target_alias, source_alias)
}

fn merge_source_ref(expression: &Expr, alias: &str, column: &str) -> bool {
    matches!(expression, Expr::CompoundIdentifier(parts)
        if parts.len() == 2
            && parts[0].value.eq_ignore_ascii_case(alias)
            && parts[1].value.eq_ignore_ascii_case(column))
}

fn merge_source_document_ref(expression: &Expr, alias: &str) -> bool {
    merge_source_ref(expression, alias, "document_json")
        || matches!(
            expression,
            Expr::Cast {
                expr,
                data_type: SqlDataType::JSON,
                ..
            } if merge_source_ref(expr, alias, "document_json")
        )
}

/// Accept the ordinary SQL Server spelling for a target assignment while
/// keeping MERGE writes bounded to the one product document column. The
/// unqualified form remains valid; a two-part name is valid only when its
/// qualifier is the target alias from the MERGE header.
fn merge_document_target(target: &AssignmentTarget, target_alias: &str) -> bool {
    match target {
        AssignmentTarget::ColumnName(name) if name.0.len() == 1 => {
            name.0[0].value.eq_ignore_ascii_case("document_json")
        }
        AssignmentTarget::ColumnName(name) if name.0.len() == 2 => {
            name.0[0].value.eq_ignore_ascii_case(target_alias)
                && name.0[1].value.eq_ignore_ascii_case("document_json")
        }
        _ => false,
    }
}

enum MergeDocumentAssignment {
    Source,
    Literal(String),
}

fn merge_document_assignment(
    expression: &Expr,
    source_alias: &str,
) -> Option<MergeDocumentAssignment> {
    if merge_source_document_ref(expression, source_alias) {
        Some(MergeDocumentAssignment::Source)
    } else {
        product_document_literal(expression)
            .ok()
            .map(MergeDocumentAssignment::Literal)
    }
}

fn merge_document_value(
    expression: &Expr,
    source_alias: &str,
    source_document: &Expr,
) -> Result<String> {
    match merge_document_assignment(expression, source_alias) {
        Some(MergeDocumentAssignment::Source) => {
            string_literal(source_document, "MERGE source document_json")
        }
        Some(MergeDocumentAssignment::Literal(value)) => Ok(value),
        None => Err(sql_error(
            "MERGE document_json must be source.document_json or a string literal/CAST(string AS JSON)",
        )),
    }
}

fn bounded_product_source_kind(
    select: &sqlparser::ast::Select,
    target_kind: &str,
) -> Result<(String, Option<Vec<String>>)> {
    if select.from.len() != 1 {
        return Err(sql_error(
            "product INSERT SELECT requires exactly one source table",
        ));
    }
    let Some(table) = select.from.first() else {
        return Err(sql_error("product INSERT SELECT requires one source table"));
    };
    if !table.joins.is_empty() {
        return Err(sql_error("product INSERT SELECT does not support joins"));
    }
    let TableFactor::Table { name, alias, .. } = &table.relation else {
        return Err(sql_error(
            "product INSERT SELECT source must be a product table",
        ));
    };
    if alias.is_some() {
        return Err(sql_error(
            "product INSERT SELECT does not support source aliases",
        ));
    }
    let source_kind = product_kind(&name.to_string())?;
    if source_kind != target_kind {
        return Err(sql_error(
            "product INSERT SELECT source and target kinds must match",
        ));
    }
    let source_ids = match select.selection.as_ref() {
        None => None,
        Some(Expr::Value(Value::Boolean(true))) => None,
        Some(Expr::InList {
            expr,
            list,
            negated: false,
        }) => {
            if !matches!(expr.as_ref(), Expr::Identifier(identifier) if identifier.value.eq_ignore_ascii_case("id"))
            {
                return Err(sql_error(
                    "product INSERT SELECT WHERE may filter only the id column",
                ));
            }
            if list.is_empty() || list.len() > 1_000 {
                return Err(sql_error(
                    "product INSERT SELECT id filter must contain 1 to 1000 values",
                ));
            }
            Some(
                list.iter()
                    .map(|value| string_literal(value, "product ID"))
                    .collect::<Result<Vec<_>>>()?,
            )
        }
        Some(_) => {
            return Err(sql_error(
                "product INSERT SELECT WHERE supports only id IN (literal, ...)",
            ));
        }
    };
    if select.projection.len() != 2
        || !select.projection.iter().all(|item| match item {
            SelectItem::UnnamedExpr(Expr::Identifier(identifier)) => {
                identifier.value.eq_ignore_ascii_case("id")
                    || identifier.value.eq_ignore_ascii_case("document_json")
            }
            SelectItem::UnnamedExpr(Expr::Cast {
                expr,
                data_type: SqlDataType::JSON,
                ..
            }) => matches!(
                expr.as_ref(),
                Expr::Identifier(identifier)
                    if identifier.value.eq_ignore_ascii_case("document_json")
            ),
            _ => false,
        })
    {
        return Err(sql_error(
            "product INSERT SELECT must project only id and document_json",
        ));
    }
    Ok((source_kind, source_ids))
}

fn is_bounded_product_source(select: &sqlparser::ast::Select, target_kind: &str) -> bool {
    bounded_product_source_kind(select, target_kind).is_ok()
}

fn product_kind(table: &str) -> Result<String> {
    let normalized = table.to_ascii_lowercase();
    let leaf = normalized
        .strip_prefix("kaveon.product.")
        .or_else(|| normalized.strip_prefix("product."));
    match leaf {
        Some("datasets") => Ok("dataset".into()),
        Some("charts") => Ok("chart".into()),
        Some("dashboards") => Ok("dashboard".into()),
        Some("saved_queries") => Ok("saved_query".into()),
        Some("user_themes") => Ok("user_theme".into()),
        Some("dlm_definitions") => Ok("dlm_definition".into()),
        Some("dlm_runs") => Ok("dlm_run".into()),
        Some("favorites") => Ok("favorite".into()),
        Some("sources") => Ok("source".into()),
        Some("user_recents") => Ok("user_recent".into()),
        Some("query_history") => Ok("query_history".into()),
        Some("activity") => Ok("activity".into()),
        Some("chat_sessions") => Ok("chat_session".into()),
        Some("chat_messages") => Ok("chat_message".into()),
        _ => Err(sql_error(
            "row DML is unsupported; target a supported kaveon.product table",
        )),
    }
}

fn product_key_predicate(selection: Option<&Expr>) -> Result<(Vec<String>, u64)> {
    let Some(Expr::BinaryOp {
        left,
        op: BinaryOperator::And,
        right,
    }) = selection
    else {
        return Err(sql_error(
            "product UPDATE/DELETE requires WHERE id = <string> AND revision = <integer>",
        ));
    };
    let mut ids = None;
    let mut revision = None;
    for expression in [left.as_ref(), right.as_ref()] {
        match expression {
            Expr::BinaryOp {
                left,
                op: BinaryOperator::Eq,
                right,
            } => {
                let Expr::Identifier(column) = left.as_ref() else {
                    return Err(sql_error(
                        "product key predicates require unqualified columns",
                    ));
                };
                if column.value.eq_ignore_ascii_case("id") {
                    if ids.is_some() {
                        return Err(sql_error(
                            "product key predicates may contain one id predicate",
                        ));
                    }
                    ids = Some(vec![string_literal(right, "product ID")?]);
                } else if column.value.eq_ignore_ascii_case("revision") {
                    if revision.is_some() {
                        return Err(sql_error(
                            "product key predicates may contain one revision predicate",
                        ));
                    }
                    revision = Some(integer_literal(right, "expected revision")?);
                } else {
                    return Err(sql_error(
                        "product key predicates support id and revision only",
                    ));
                }
            }
            Expr::InList {
                expr,
                list,
                negated: false,
            } => {
                let Expr::Identifier(column) = expr.as_ref() else {
                    return Err(sql_error(
                        "product key predicates require unqualified columns",
                    ));
                };
                if !column.value.eq_ignore_ascii_case("id") {
                    return Err(sql_error("product key IN predicates support id only"));
                }
                if list.is_empty() || list.len() > 1_000 {
                    return Err(sql_error("product key IN predicates require 1 to 1000 IDs"));
                }
                if ids.is_some() {
                    return Err(sql_error(
                        "product key predicates may contain one id predicate",
                    ));
                }
                let values = list
                    .iter()
                    .map(|value| string_literal(value, "product ID"))
                    .collect::<Result<Vec<_>>>()?;
                let unique = values.iter().collect::<HashSet<_>>().len() == values.len();
                if !unique {
                    return Err(sql_error(
                        "product key IN predicates cannot contain duplicate IDs",
                    ));
                }
                ids = Some(values);
            }
            _ => {
                return Err(sql_error(
                    "product UPDATE/DELETE requires equality predicates or id IN (...) AND revision = <integer>",
                ));
            }
        }
    }
    match (ids, revision) {
        (Some(ids), Some(revision)) if revision > 0 => Ok((ids, revision)),
        _ => Err(sql_error(
            "product UPDATE/DELETE requires one or more IDs and one positive revision",
        )),
    }
}

fn string_literal(expression: &Expr, label: &str) -> Result<String> {
    match expression {
        Expr::Value(Value::SingleQuotedString(value)) => Ok(value.clone()),
        _ => Err(sql_error(&format!("{label} must be a string literal"))),
    }
}

/// Accept a bounded JSON-typed literal in product UPDATE assignments. The
/// transaction layer still validates the resulting bytes as an object before
/// publication; this only widens the SQL spelling from a bare string literal
/// to `CAST('<json>' AS JSON)` without evaluating arbitrary expressions.
fn product_document_literal(expression: &Expr) -> Result<String> {
    match expression {
        Expr::Value(Value::SingleQuotedString(value)) => Ok(value.clone()),
        Expr::Cast {
            expr,
            data_type: SqlDataType::JSON,
            ..
        } => string_literal(expr, "product document_json"),
        _ => Err(sql_error(
            "product document_json must be a string literal or CAST(string AS JSON)",
        )),
    }
}

fn is_conflict_identity_predicate(expression: &Expr) -> bool {
    let Expr::BinaryOp { left, op, right } = expression else {
        return false;
    };
    if !matches!(op, BinaryOperator::Eq) {
        return false;
    }

    fn is_target_id(expression: &Expr) -> bool {
        match expression {
            Expr::Identifier(identifier) => identifier.value.eq_ignore_ascii_case("id"),
            Expr::CompoundIdentifier(parts) => {
                parts.len() == 1 && parts[0].value.eq_ignore_ascii_case("id")
            }
            _ => false,
        }
    }

    fn is_excluded_id(expression: &Expr) -> bool {
        let Expr::CompoundIdentifier(parts) = expression else {
            return false;
        };
        parts.len() == 2
            && parts[0].value.eq_ignore_ascii_case("excluded")
            && parts[1].value.eq_ignore_ascii_case("id")
    }

    (is_target_id(left) && is_excluded_id(right)) || (is_excluded_id(left) && is_target_id(right))
}

/// Product tables expose one stable identity constraint for `id`. Accept the
/// conventional `<table>_pkey` named form; arbitrary named constraints could
/// identify a different uniqueness rule and remain fail-closed until
/// catalog-backed constraint metadata is available.
fn conflict_target_is_product_id(
    target: Option<&sqlparser::ast::ConflictTarget>,
    table: &str,
) -> bool {
    match target {
        None => true,
        Some(sqlparser::ast::ConflictTarget::Columns(columns)) => {
            columns.len() == 1 && columns[0].value.eq_ignore_ascii_case("id")
        }
        Some(sqlparser::ast::ConflictTarget::OnConstraint(name)) => {
            let table_name = table.rsplit('.').next().unwrap_or(table);
            let Some(constraint) = name.0.last().map(|part| part.value.as_str()) else {
                return false;
            };
            constraint.eq_ignore_ascii_case(&format!("{table_name}_pkey"))
        }
    }
}

fn is_conflict_document_assignment(expression: &Expr) -> bool {
    match expression {
        Expr::CompoundIdentifier(parts) => {
            parts.len() == 2
                && parts[0].value.eq_ignore_ascii_case("excluded")
                && parts[1].value.eq_ignore_ascii_case("document_json")
        }
        Expr::Cast {
            expr,
            data_type: SqlDataType::JSON,
            ..
        } => is_conflict_document_assignment(expr),
        _ => product_document_literal(expression).is_ok(),
    }
}

/// Return a literal document override for a bounded `ON CONFLICT DO UPDATE`.
/// The override applies only when the inserted key already exists; the
/// ordinary VALUES document remains the create path.
fn conflict_document_override(on: Option<&OnInsert>) -> Result<Option<String>> {
    let Some(OnInsert::OnConflict(conflict)) = on else {
        return Ok(None);
    };
    let OnConflictAction::DoUpdate(update) = &conflict.action else {
        return Ok(None);
    };
    if update.assignments.len() != 1 {
        return Ok(None);
    }
    let value = &update.assignments[0].value;
    if !product_document_literal(value).is_ok() {
        // `excluded.document_json` (including its JSON cast spelling) means
        // the incoming VALUES document, not a literal override.
        return Ok(None);
    }
    Ok(Some(product_document_literal(value)?))
}

fn integer_literal(expression: &Expr, label: &str) -> Result<u64> {
    match expression {
        Expr::Value(Value::Number(value, false)) => value
            .parse()
            .map_err(|_| sql_error(&format!("{label} must be an unsigned integer literal"))),
        _ => Err(sql_error(&format!(
            "{label} must be an unsigned integer literal"
        ))),
    }
}

/// Parse the deliberately small native write contract.
///
/// This validates syntax only. It does not imply that the coordinator can
/// execute, isolate, commit, or recover the returned statement.
pub fn parse_native_transactional(sql: &str) -> Result<NativeTransactionalStatement> {
    let normalized = normalize_transaction_control_aliases(sql);
    let (normalized, merge_returning) = normalize_product_output_with_merge(&normalized)?;
    let normalized = normalize_unfiltered_product_source_returning(&normalized);
    let mut statements = parse_sql(&normalized)?;
    if statements.len() != 1 {
        return Err(sql_error(
            "native transactions accept exactly one statement per request",
        ));
    }
    let statement = statements.pop().expect("length checked");
    match &statement {
        Statement::StartTransaction {
            modes, modifier, ..
        } if modes.is_empty() && modifier.is_none() => Ok(NativeTransactionalStatement::Begin),
        Statement::StartTransaction { .. } => Err(sql_error(
            "BEGIN isolation, access-mode, and dialect modifiers are not supported",
        )),
        Statement::Commit { chain: false } => Ok(NativeTransactionalStatement::Commit),
        Statement::Commit { chain: true } => Err(sql_error("COMMIT AND CHAIN is not supported")),
        Statement::Rollback {
            chain: false,
            savepoint: None,
        } => Ok(NativeTransactionalStatement::Rollback),
        Statement::Rollback {
            chain: false,
            savepoint: Some(name),
        } => Ok(NativeTransactionalStatement::RollbackToSavepoint(
            name.value.clone(),
        )),
        Statement::Rollback { .. } => Err(sql_error("ROLLBACK AND CHAIN is not supported")),
        Statement::Savepoint { name } => {
            Ok(NativeTransactionalStatement::Savepoint(name.value.clone()))
        }
        Statement::ReleaseSavepoint { name } => Ok(NativeTransactionalStatement::ReleaseSavepoint(
            name.value.clone(),
        )),
        Statement::SetTransaction { .. } => Err(sql_error(
            "SET TRANSACTION isolation and access modes are not supported",
        )),
        Statement::Insert(insert) => {
            if insert.columns.is_empty() {
                return Err(sql_error("INSERT requires an explicit column list"));
            }
            if !matches!(
                insert.source.as_deref().map(|query| query.body.as_ref()),
                Some(SetExpr::Values(_)) | Some(SetExpr::Select(_))
            ) {
                return Err(sql_error("INSERT supports VALUES or SELECT only"));
            }
            let table = validated_table_name(&insert.table_name)?;
            let (on_conflict_do_nothing, on_conflict_do_update) = match insert.on.as_ref() {
                None => (false, false),
                Some(OnInsert::OnConflict(conflict))
                    if conflict_target_is_product_id(conflict.conflict_target.as_ref(), &table)
                        && matches!(conflict.action, OnConflictAction::DoNothing) =>
                {
                    (true, false)
                }
                Some(OnInsert::OnConflict(conflict))
                    if conflict_target_is_product_id(conflict.conflict_target.as_ref(), &table)
                        && matches!(
                            &conflict.action,
                            OnConflictAction::DoUpdate(update)
                                if update.selection.as_ref().is_none_or(|selection| {
                                    matches!(selection, Expr::Value(Value::Boolean(true)))
                                        || is_conflict_identity_predicate(selection)
                                })
                                    && update.assignments.len() == 1
                                    && matches!(
                                        &update.assignments[0].target,
                                        AssignmentTarget::ColumnName(column)
                                            if column.0.len() == 1
                                                && column.0[0].value.eq_ignore_ascii_case("document_json")
                                    )
                                    && is_conflict_document_assignment(&update.assignments[0].value)
                        ) =>
                {
                    (false, true)
                }
                Some(_) => {
                    return Err(sql_error(
                        "INSERT supports only bounded ON CONFLICT DO NOTHING or DO UPDATE of document_json on the product id",
                    ));
                }
            };
            let returning = parse_product_returning(insert.returning.as_ref())?;
            if !returning.is_empty()
                && !table.to_ascii_lowercase().starts_with("product.")
                && !table.to_ascii_lowercase().starts_with("kaveon.product.")
            {
                return Err(sql_error(
                    "RETURNING is supported only for kaveon.product records",
                ));
            }
            if insert.table_alias.is_some()
                || insert.or.is_some()
                || insert.ignore
                || insert.overwrite
                || insert.partitioned.is_some()
                || !insert.after_columns.is_empty()
                || insert.table
                || (!on_conflict_do_nothing && !on_conflict_do_update && insert.on.is_some())
                || insert.replace_into
                || insert.priority.is_some()
                || insert.insert_alias.is_some()
            {
                return Err(sql_error(
                    "INSERT aliases, conflict clauses, modifiers, partitions, and RETURNING are not supported",
                ));
            }
            Ok(NativeTransactionalStatement::Dml(Box::new(
                NativeDmlStatement {
                    kind: NativeDmlKind::Insert,
                    table,
                    ast: statement,
                    returning,
                    output_pseudo: None,
                    on_conflict_do_nothing,
                    on_conflict_do_update,
                },
            )))
        }
        Statement::Update {
            table,
            assignments,
            from,
            selection,
            returning,
            or,
        } => {
            let table_name = simple_table(table)?;
            let returning = parse_product_returning(returning.as_ref())?;
            if or.is_some() {
                return Err(sql_error("UPDATE conflict clauses are not supported"));
            }
            if from.is_some() && product_kind(&table_name).is_err() {
                return Err(sql_error(
                    "UPDATE FROM is supported only for bounded product tables",
                ));
            }
            if selection.is_none() {
                return Err(sql_error("UPDATE requires a WHERE predicate"));
            }
            if assignments.is_empty()
                || assignments.iter().any(|assignment| {
                    !matches!(&assignment.target, AssignmentTarget::ColumnName(name)
                        if name.0.len() == 1 || (from.is_some() && name.0.len() == 2))
                })
            {
                return Err(sql_error(
                    "UPDATE requires simple single-column assignments",
                ));
            }
            Ok(NativeTransactionalStatement::Dml(Box::new(
                NativeDmlStatement {
                    kind: NativeDmlKind::Update,
                    table: table_name,
                    ast: statement,
                    returning,
                    output_pseudo: None,
                    on_conflict_do_nothing: false,
                    on_conflict_do_update: false,
                },
            )))
        }
        Statement::Delete(delete) => {
            let returning = parse_product_returning(delete.returning.as_ref())?;
            let tables = match &delete.from {
                FromTable::WithFromKeyword(tables) => tables,
                FromTable::WithoutKeyword(_) => {
                    return Err(sql_error("DELETE requires the FROM keyword"));
                }
            };
            let bounded_using = delete.using.as_ref().is_some_and(|using| {
                using.len() == 1
                    && tables.len() == 1
                    && delete.tables.is_empty()
                    && delete.order_by.is_empty()
                    && delete.limit.is_none()
            });
            if tables.len() != 1
                || !delete.tables.is_empty()
                || (!bounded_using && delete.using.is_some())
                || !delete.order_by.is_empty()
                || delete.limit.is_some()
            {
                return Err(sql_error(
                    "DELETE supports one table and no USING, ORDER BY, or LIMIT",
                ));
            }
            if delete.selection.is_none() {
                return Err(sql_error("DELETE requires a WHERE predicate"));
            }
            let table = simple_table(&tables[0])?;
            Ok(NativeTransactionalStatement::Dml(Box::new(
                NativeDmlStatement {
                    kind: NativeDmlKind::Delete,
                    table,
                    ast: statement,
                    returning,
                    output_pseudo: None,
                    on_conflict_do_nothing: false,
                    on_conflict_do_update: false,
                },
            )))
        }
        Statement::Merge { table, .. } => {
            let table = merge_target_table(table)?;
            product_kind(&table)?;
            let (returning, output_pseudo) = merge_returning.map_or_else(
                || (Vec::new(), None),
                |(columns, pseudo)| (columns, Some(pseudo)),
            );
            if output_pseudo.as_deref() != Some("action")
                && returning.iter().any(|column| {
                    !column.eq_ignore_ascii_case("id")
                        && !column.eq_ignore_ascii_case("revision")
                        && !column.eq_ignore_ascii_case("document_json")
                })
            {
                return Err(sql_error(
                    "MERGE OUTPUT supports only id, revision, or document_json columns",
                ));
            }
            Ok(NativeTransactionalStatement::Dml(Box::new(
                NativeDmlStatement {
                    kind: NativeDmlKind::Merge,
                    table,
                    ast: statement,
                    returning,
                    output_pseudo,
                    on_conflict_do_nothing: false,
                    on_conflict_do_update: false,
                },
            )))
        }
        _ => Err(sql_error(
            "native transaction parser accepts INSERT, UPDATE, DELETE, MERGE, BEGIN, COMMIT, or ROLLBACK only",
        )),
    }
}

/// sqlparser requires a SELECT query's optional WHERE boundary before an
/// INSERT ... SELECT RETURNING clause in the source-backed shape. Preserve
/// the user's unfiltered product copy semantics by inserting a tautological
/// predicate only for that narrow form; the product adapter treats `WHERE
/// TRUE` as no source filter and still applies its 1,000-row expansion cap.
fn normalize_unfiltered_product_source_returning(sql: &str) -> String {
    let trimmed = sql.trim();
    let (body, terminator) = match trimmed.strip_suffix(';') {
        Some(body) => (body.trim_end(), ";"),
        None => (trimmed, ""),
    };
    let upper = body.to_ascii_uppercase();
    if !upper.starts_with("INSERT") {
        return sql.to_owned();
    }
    let Some(returning_position) = find_top_level_keyword(&upper, "RETURNING") else {
        return sql.to_owned();
    };
    let before_returning = &upper[..returning_position];
    let Some(select_position) = find_top_level_keyword(before_returning, "SELECT") else {
        return sql.to_owned();
    };
    if find_top_level_keyword(
        &before_returning[select_position + "SELECT".len()..],
        "FROM",
    )
    .is_none()
        || find_top_level_keyword(before_returning, "WHERE").is_some()
    {
        return sql.to_owned();
    }
    let insertion_position =
        find_top_level_keyword(before_returning, "ON").unwrap_or(returning_position);
    let mut rewritten = String::with_capacity(body.len() + 12);
    rewritten.push_str(body[..insertion_position].trim_end());
    rewritten.push_str(" WHERE TRUE ");
    rewritten.push_str(body[insertion_position..].trim_start());
    rewritten.push_str(terminator);
    rewritten
}

/// Normalize the bounded SQL Server `OUTPUT` projection into the existing
/// product-record `RETURNING` contract. This handles simple
/// inserted/deleted column references for INSERT/UPDATE/DELETE; the bounded
/// update-or-insert and matched-update-only MERGE projections are normalized
/// separately so mixed-action batches remain fail-closed.
type MergeOutputProjection = Option<(Vec<String>, String)>;

fn normalize_product_output_with_merge(sql: &str) -> Result<(String, MergeOutputProjection)> {
    let trimmed = sql.trim();
    let (body, terminator) = match trimmed.strip_suffix(';') {
        Some(body) => (body.trim_end(), ";"),
        None => (trimmed, ""),
    };
    let upper = body.to_ascii_uppercase();
    if upper.starts_with("MERGE") {
        let Some(output_position) = find_top_level_keyword(&upper, "OUTPUT") else {
            return Ok((sql.to_owned(), None));
        };
        let expression_start = output_position + "OUTPUT".len();
        let projection = body[expression_start..].trim();
        if projection.is_empty() {
            return Err(sql_error(
                "MERGE OUTPUT requires a bounded column projection",
            ));
        }
        if projection.eq_ignore_ascii_case("$action") {
            let mut rewritten = String::with_capacity(body.len());
            rewritten.push_str(body[..output_position].trim_end());
            rewritten.push_str(terminator);
            return Ok((
                rewritten,
                Some((vec!["action".to_owned()], "action".to_owned())),
            ));
        }
        let pseudo = projection
            .split(',')
            .next()
            .and_then(|item| item.trim().split('.').next())
            .filter(|value| {
                value.eq_ignore_ascii_case("inserted") || value.eq_ignore_ascii_case("deleted")
            })
            .ok_or_else(|| {
                sql_error("MERGE OUTPUT supports only inserted/deleted column projections")
            })?
            .to_ascii_lowercase();
        let returning = parse_sql_server_output_projection(projection, &pseudo)?;
        let mut rewritten = String::with_capacity(body.len());
        rewritten.push_str(body[..output_position].trim_end());
        rewritten.push_str(terminator);
        return Ok((rewritten, Some((returning, pseudo))));
    }
    Ok((normalize_product_output(sql)?, None))
}

fn normalize_product_output(sql: &str) -> Result<String> {
    let trimmed = sql.trim();
    let (body, terminator) = match trimmed.strip_suffix(';') {
        Some(body) => (body.trim_end(), ";"),
        None => (trimmed, ""),
    };
    let upper = body.to_ascii_uppercase();
    let statement_kind = if upper.starts_with("INSERT") {
        "INSERT"
    } else if upper.starts_with("UPDATE") {
        "UPDATE"
    } else if upper.starts_with("DELETE") {
        "DELETE"
    } else {
        return Ok(sql.to_owned());
    };
    let Some(output_position) = find_top_level_keyword(&upper, "OUTPUT") else {
        return Ok(sql.to_owned());
    };
    let expression_start = output_position + "OUTPUT".len();
    let boundary_keywords: &[&str] = match statement_kind {
        "INSERT" => &["VALUES", "SELECT", "DEFAULT", "ON", "RETURNING"],
        "UPDATE" | "DELETE" => &["WHERE", "ORDER", "LIMIT", "RETURNING"],
        _ => unreachable!(),
    };
    let tail_start = boundary_keywords
        .iter()
        .filter_map(|keyword| {
            find_top_level_keyword(&upper[expression_start..], keyword)
                .map(|position| expression_start + position)
        })
        .min()
        .unwrap_or(body.len());
    let projection = body[expression_start..tail_start].trim();
    if projection.is_empty() {
        return Err(sql_error("OUTPUT requires a bounded column projection"));
    }
    let expected_pseudo = if statement_kind == "DELETE" {
        "deleted"
    } else {
        "inserted"
    };
    let returning = parse_sql_server_output_projection(projection, expected_pseudo)?;
    let mut rewritten = String::with_capacity(body.len() + 16);
    rewritten.push_str(body[..output_position].trim_end());
    if tail_start < body.len() {
        rewritten.push(' ');
        rewritten.push_str(body[tail_start..].trim_start());
    }
    rewritten.push_str(" RETURNING ");
    rewritten.push_str(&returning.join(", "));
    rewritten.push_str(terminator);
    Ok(rewritten)
}

fn parse_sql_server_output_projection(
    projection: &str,
    expected_pseudo: &str,
) -> Result<Vec<String>> {
    if projection.to_ascii_uppercase().contains(" INTO ") {
        return Err(sql_error("OUTPUT INTO is not supported"));
    }
    let items = split_top_level_commas(projection)?;
    let wildcard_parts = items
        .first()
        .map(|item| item.trim().split('.').collect::<Vec<_>>());
    let wildcard = items.len() == 1
        && wildcard_parts.as_ref().is_some_and(|parts| {
            parts.len() == 2
                && parts[1].trim() == "*"
                && parts[0].trim().eq_ignore_ascii_case(expected_pseudo)
        });
    if wildcard {
        return Ok(["id", "revision", "document_json"]
            .map(str::to_owned)
            .to_vec());
    }
    let mut returning = Vec::with_capacity(items.len());
    for item in items {
        let parts = item.trim().split('.').collect::<Vec<_>>();
        if parts.len() != 2 {
            return Err(sql_error(
                "OUTPUT supports only inserted/deleted.column projections",
            ));
        }
        let pseudo = parts[0].trim();
        let column = parts[1].trim();
        if !pseudo.eq_ignore_ascii_case(expected_pseudo) || !is_simple_identifier(column) {
            return Err(sql_error(
                "OUTPUT supports only the statement's inserted/deleted column projections",
            ));
        }
        returning.push(column.to_owned());
    }
    if returning.is_empty() {
        return Err(sql_error("OUTPUT requires at least one column"));
    }
    Ok(returning)
}

/// SQL Server clients commonly spell the transaction controls with an
/// optional `TRAN`/`TRANSACTION` keyword.  They are aliases for the same
/// single-statement controls; normalize only the complete statement so a
/// keyword inside a DML statement is never rewritten.
fn normalize_transaction_control_aliases(sql: &str) -> String {
    let trimmed = sql.trim();
    let (body, terminator) = match trimmed.strip_suffix(';') {
        Some(body) => (body.trim_end(), ";"),
        None => (trimmed, ""),
    };
    let canonical = match body.to_ascii_uppercase().as_str() {
        "BEGIN TRAN" | "BEGIN TRANSACTION" => Some("BEGIN"),
        "COMMIT TRAN" | "COMMIT TRANSACTION" => Some("COMMIT"),
        "ROLLBACK TRAN" | "ROLLBACK TRANSACTION" => Some("ROLLBACK"),
        _ => None,
    };
    canonical
        .map(|statement| format!("{statement}{terminator}"))
        .unwrap_or_else(|| sql.to_owned())
}

/// The product transaction contract can return stable identity, revision, and
/// the staged document. Arbitrary row RETURNING remains rejected until
/// typed-table DML has a durable row contract.
fn parse_product_returning(returning: Option<&Vec<SelectItem>>) -> Result<Vec<String>> {
    let Some(items) = returning else {
        return Ok(Vec::new());
    };
    if items.is_empty() {
        return Err(sql_error("RETURNING requires id or revision"));
    }
    if items.len() == 1 && matches!(items[0], SelectItem::Wildcard(_)) {
        return Ok(vec![
            "id".to_owned(),
            "revision".to_owned(),
            "document_json".to_owned(),
        ]);
    }
    items
        .iter()
        .map(|item| {
            let SelectItem::UnnamedExpr(Expr::Identifier(identifier)) = item else {
                return Err(sql_error(
                    "product RETURNING supports only unqualified id and revision",
                ));
            };
            if !identifier.value.eq_ignore_ascii_case("id")
                && !identifier.value.eq_ignore_ascii_case("revision")
                && !identifier.value.eq_ignore_ascii_case("document_json")
            {
                return Err(sql_error(
                    "product RETURNING supports only id, revision, or document_json",
                ));
            }
            Ok(identifier.value.to_ascii_lowercase())
        })
        .collect()
}

fn simple_table(table: &TableWithJoins) -> Result<String> {
    if !table.joins.is_empty() {
        return Err(sql_error("write targets cannot contain joins"));
    }
    match &table.relation {
        TableFactor::Table {
            name,
            alias,
            args: None,
            with_hints,
            version: None,
            with_ordinality: false,
            partitions,
            json_path: None,
        } if with_hints.is_empty()
            && partitions.is_empty()
            && alias.as_ref().is_none_or(|alias| alias.columns.is_empty()) =>
        {
            validated_table_name(name)
        }
        _ => Err(sql_error("write target must be an unaliased base table")),
    }
}

fn product_update_from_tables(
    target: &TableWithJoins,
    source: &TableWithJoins,
    target_kind: &str,
) -> Result<(String, String, String)> {
    if !target.joins.is_empty() || !source.joins.is_empty() {
        return Err(sql_error(
            "product UPDATE FROM supports one target and one source table without joins",
        ));
    }
    let TableFactor::Table {
        name,
        alias,
        args: None,
        with_hints,
        version: None,
        with_ordinality: false,
        partitions,
        json_path: None,
    } = &target.relation
    else {
        return Err(sql_error(
            "product UPDATE FROM target must be a base product table",
        ));
    };
    if !with_hints.is_empty()
        || !partitions.is_empty()
        || alias.as_ref().is_some_and(|a| !a.columns.is_empty())
    {
        return Err(sql_error(
            "product UPDATE FROM target must use a simple alias",
        ));
    }
    let target_name = validated_table_name(name)?;
    let parsed_target_kind = product_kind(&target_name)?;
    if parsed_target_kind != target_kind {
        return Err(sql_error("product UPDATE FROM target kind is inconsistent"));
    }
    let target_alias = alias
        .as_ref()
        .map(|value| value.name.value.clone())
        .or_else(|| target_name.rsplit('.').next().map(str::to_owned))
        .ok_or_else(|| sql_error("product UPDATE FROM target must have a table name"))?;

    let TableFactor::Table {
        name: source_name,
        alias: Some(source_alias),
        args: None,
        with_hints: source_hints,
        version: None,
        with_ordinality: false,
        partitions: source_partitions,
        json_path: None,
    } = &source.relation
    else {
        return Err(sql_error(
            "product UPDATE FROM source must be an aliased base product table",
        ));
    };
    if !source_hints.is_empty() || !source_partitions.is_empty() || !source_alias.columns.is_empty()
    {
        return Err(sql_error(
            "product UPDATE FROM source must use a simple alias",
        ));
    }
    let source_kind = product_kind(&validated_table_name(source_name)?)?;
    if source_kind != target_kind {
        return Err(sql_error(
            "product UPDATE FROM source and target kinds must match",
        ));
    }
    Ok((target_alias, source_kind, source_alias.name.value.clone()))
}

fn product_update_from_predicate(
    selection: Option<&Expr>,
    target_alias: &str,
    source_alias: &str,
) -> Result<Vec<(String, u64, String)>> {
    let Some(selection) = selection else {
        return Err(sql_error(
            "product UPDATE FROM requires target id, revision, and source id predicates",
        ));
    };
    let mut predicates = Vec::new();
    fn flatten<'a>(expression: &'a Expr, predicates: &mut Vec<&'a Expr>) {
        if let Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } = expression
        {
            flatten(left, predicates);
            flatten(right, predicates);
        } else {
            predicates.push(expression);
        }
    }
    flatten(selection, &mut predicates);
    if !(2..=3).contains(&predicates.len()) {
        return Err(sql_error(
            "product UPDATE FROM requires target.id, target.revision, and an optional source.id predicate",
        ));
    }
    let mut ids = None;
    let mut revision = None;
    let mut source_id = None;
    let mut identity_join = false;
    for expression in predicates {
        if let Expr::InList {
            expr,
            list,
            negated: false,
        } = expression
        {
            let Expr::CompoundIdentifier(parts) = expr.as_ref() else {
                return Err(sql_error(
                    "product UPDATE FROM target.id IN must qualify the target column",
                ));
            };
            if parts.len() != 2
                || !parts[0].value.eq_ignore_ascii_case(target_alias)
                || !parts[1].value.eq_ignore_ascii_case("id")
            {
                return Err(sql_error(
                    "product UPDATE FROM target.id IN must qualify the target column",
                ));
            }
            if ids.is_some() || identity_join || list.is_empty() || list.len() > 1000 {
                return Err(sql_error(
                    "product UPDATE FROM target.id IN must contain 1 to 1000 literals",
                ));
            }
            let target_ids = list
                .iter()
                .map(|value| string_literal(value, "product target ID"))
                .collect::<Result<Vec<_>>>()?;
            let mut unique_ids = std::collections::HashSet::with_capacity(target_ids.len());
            if target_ids.iter().any(|value| !unique_ids.insert(value)) {
                return Err(sql_error(
                    "product UPDATE FROM target.id IN values must be unique",
                ));
            }
            ids = Some(target_ids);
            continue;
        }
        let Expr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } = expression
        else {
            return Err(sql_error(
                "product UPDATE FROM predicates must use equality with string or integer literals",
            ));
        };
        let Expr::CompoundIdentifier(parts) = left.as_ref() else {
            return Err(sql_error(
                "product UPDATE FROM predicates must qualify target and source columns",
            ));
        };
        if parts.len() != 2 {
            return Err(sql_error(
                "product UPDATE FROM predicates must qualify target and source columns",
            ));
        }
        let qualifier = parts[0].value.as_str();
        let column = parts[1].value.as_str();
        if qualifier.eq_ignore_ascii_case(target_alias) && column.eq_ignore_ascii_case("id") {
            if let Expr::CompoundIdentifier(source_parts) = right.as_ref() {
                if identity_join {
                    return Err(sql_error(
                        "product UPDATE FROM may contain one target.id join predicate",
                    ));
                }
                if source_parts.len() != 2
                    || !source_parts[0].value.eq_ignore_ascii_case(source_alias)
                    || !source_parts[1].value.eq_ignore_ascii_case("id")
                {
                    return Err(sql_error(
                        "product UPDATE FROM target.id joins must reference source.id",
                    ));
                }
                identity_join = true;
                continue;
            }
            if ids.is_some() || identity_join {
                return Err(sql_error(
                    "product UPDATE FROM may contain one target.id predicate",
                ));
            }
            let target_ids = vec![string_literal(right, "product target ID")?];
            let mut unique_ids = std::collections::HashSet::with_capacity(target_ids.len());
            if target_ids.iter().any(|value| !unique_ids.insert(value)) {
                return Err(sql_error(
                    "product UPDATE FROM target.id IN values must be unique",
                ));
            }
            ids = Some(target_ids);
        } else if qualifier.eq_ignore_ascii_case(target_alias)
            && column.eq_ignore_ascii_case("revision")
        {
            if revision.is_some() {
                return Err(sql_error(
                    "product UPDATE FROM may contain one target.revision predicate",
                ));
            }
            revision = Some(integer_literal(right, "expected revision")?);
        } else if qualifier.eq_ignore_ascii_case(source_alias) && column.eq_ignore_ascii_case("id")
        {
            if source_id.is_some() {
                return Err(sql_error(
                    "product UPDATE FROM may contain one source.id predicate",
                ));
            }
            source_id = Some(string_literal(right, "product source ID")?);
        } else {
            return Err(sql_error(
                "product UPDATE FROM predicates support target.id, target.revision, and source.id only",
            ));
        }
    }
    let revision =
        revision.ok_or_else(|| sql_error("product UPDATE FROM requires target.revision"))?;
    let ids = ids.ok_or_else(|| sql_error("product UPDATE FROM requires target.id"))?;
    if source_id.is_none() && !identity_join {
        return Err(sql_error(
            "product UPDATE FROM requires source.id or target.id = source.id",
        ));
    }
    Ok(ids
        .into_iter()
        .map(|id| {
            let source_id = source_id.as_ref().map_or_else(|| id.clone(), Clone::clone);
            (id, revision, source_id)
        })
        .collect())
}

fn merge_target_table(table: &TableFactor) -> Result<String> {
    match table {
        TableFactor::Table {
            name,
            alias,
            args: None,
            with_hints,
            version: None,
            with_ordinality: false,
            partitions,
            json_path: None,
        } if with_hints.is_empty()
            && partitions.is_empty()
            && alias.as_ref().is_none_or(|alias| alias.columns.is_empty()) =>
        {
            validated_table_name(name)
        }
        _ => Err(sql_error(
            "MERGE target must be one base product table with an optional simple alias",
        )),
    }
}

fn validated_table_name(name: &sqlparser::ast::ObjectName) -> Result<String> {
    if !(1..=3).contains(&name.0.len()) {
        return Err(sql_error(
            "write target must be table, schema.table, or catalog.schema.table",
        ));
    }
    Ok(name.to_string())
}

fn sql_error(message: &str) -> KaveonError {
    KaveonError::Sql(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every statement passes through `normalize_bracket_identifiers`, and it
    /// used to rebuild the string with `u8 as char` -- a Latin-1 conversion --
    /// so every multi-byte UTF-8 sequence came out as one character per byte.
    /// Product records are written as SQL literals, so the corruption was
    /// stored and compounded on each rewrite.
    #[test]
    fn non_ascii_survives_the_bracket_pass_unchanged() {
        for sql in [
            "SELECT 'caf\u{e9}' AS x FROM t",
            "SELECT 'Probe \u{1f695} en\u{2013}dash' FROM t",
            "SELECT * FROM t WHERE country = 'K\u{f8}benhavn'",
            "SELECT \"na\u{ef}ve\" FROM t",
            "SELECT '\u{4f60}\u{597d}' FROM t",
            // A statement with no brackets at all must come back byte-identical.
            "SELECT a, b FROM t WHERE c = 'plain'",
        ] {
            assert_eq!(normalize_bracket_identifiers(sql), sql, "mangled: {sql}");
        }
    }

    /// The rewrite the function exists for still happens, and still only for
    /// identifier-shaped brackets.
    #[test]
    fn bracket_identifiers_are_quoted_and_other_brackets_are_not() {
        assert_eq!(
            normalize_bracket_identifiers("SELECT [year] FROM t"),
            "SELECT \"year\" FROM t"
        );
        assert_eq!(
            normalize_bracket_identifiers("SELECT [_a$1] FROM t"),
            "SELECT \"_a$1\" FROM t"
        );
        // Numeric and list brackets stay, so ARRAY[...] keeps working.
        assert_eq!(
            normalize_bracket_identifiers("SELECT ARRAY[1, 2] FROM t"),
            "SELECT ARRAY[1, 2] FROM t"
        );
        assert_eq!(
            normalize_bracket_identifiers("SELECT ARRAY[NULL] FROM t"),
            "SELECT ARRAY[NULL] FROM t"
        );
        // A bracket inside a string literal is data, not an identifier.
        assert_eq!(
            normalize_bracket_identifiers("SELECT '[year]' FROM t"),
            "SELECT '[year]' FROM t"
        );
    }

    #[test]
    fn null_array_literals_remain_array_expressions() {
        let statements = parse_sql("SELECT ARRAY[NULL], ARRAY[1, NULL] FROM t").unwrap();
        assert_eq!(statements.len(), 1);
        assert!(statements[0].to_string().contains("ARRAY[NULL]"));
    }

    #[test]
    fn parses_two_parameter_lambda_for_array_sort() {
        let statements = parse_sql(
            "SELECT ARRAY_SORT(ARRAY[3, 1, 2], (x, y) -> CASE WHEN x < y THEN -1 WHEN x > y THEN 1 ELSE 0 END) FROM t",
        )
        .expect("Databricks lambda fallback should parse comparator syntax");
        assert_eq!(statements.len(), 1);
        assert!(statements[0].to_string().contains("->"));
    }

    /// Both together: a rewrite in the same statement as non-ASCII text, which
    /// is where a slice-copying implementation could lose or duplicate a span.
    #[test]
    fn a_bracket_rewrite_and_non_ascii_text_coexist() {
        assert_eq!(
            normalize_bracket_identifiers(
                "SELECT [year] FROM t WHERE city = 'K\u{f8}benhavn' AND note = '\u{2013}'"
            ),
            "SELECT \"year\" FROM t WHERE city = 'K\u{f8}benhavn' AND note = '\u{2013}'"
        );
        assert_eq!(
            normalize_bracket_identifiers("SELECT '\u{1f695}', [year], '\u{2013}' FROM t"),
            "SELECT '\u{1f695}', \"year\", '\u{2013}' FROM t"
        );
    }

    /// An escaped quote inside a literal must not end the literal, or a later
    /// bracket would be rewritten inside what is still a string.
    #[test]
    fn a_doubled_quote_keeps_the_literal_open() {
        assert_eq!(
            normalize_bracket_identifiers("SELECT 'it''s [year]' FROM t"),
            "SELECT 'it''s [year]' FROM t"
        );
    }

    #[test]
    fn accepts_constrained_dml_and_transaction_controls() {
        let cases = [
            ("BEGIN", None),
            ("COMMIT", None),
            ("ROLLBACK", None),
            (
                "INSERT INTO kaveon.app.users (id, name) VALUES (1, 'Ada')",
                Some((NativeDmlKind::Insert, "kaveon.app.users")),
            ),
            (
                "UPDATE app.users SET name = 'Grace' WHERE id = 1",
                Some((NativeDmlKind::Update, "app.users")),
            ),
            (
                "DELETE FROM users WHERE id = 1",
                Some((NativeDmlKind::Delete, "users")),
            ),
        ];
        for (sql, expected) in cases {
            let parsed = parse_native_transactional(sql).unwrap();
            if let Some((kind, table)) = expected {
                let NativeTransactionalStatement::Dml(dml) = parsed else {
                    panic!("expected DML")
                };
                assert_eq!(dml.kind, kind);
                assert_eq!(dml.table, table);
            }
        }
    }

    #[test]
    fn accepts_identity_and_document_returning_for_product_dml() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') RETURNING id, revision",
        )
        .unwrap()
        else {
            panic!("expected product DML");
        };
        assert_eq!(dml.returning, ["id", "revision"]);
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') RETURNING document_json"
        )
        .unwrap()
        else {
            panic!("expected product DML");
        };
        assert_eq!(dml.returning, ["document_json"]);
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') RETURNING *",
        )
        .unwrap() else {
            panic!("expected product DML");
        };
        assert_eq!(dml.returning, ["id", "revision", "document_json"]);
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "DELETE FROM product.datasets WHERE id = 'ds-1' AND revision = 1 RETURNING document_json"
        )
        .expect("product DELETE document projection should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(dml.returning, ["document_json"]);
    }

    #[test]
    fn accepts_sql_server_count_big_as_read_compatible_count() {
        let statements = parse_sql("SELECT COUNT_BIG(*) AS row_count FROM t").unwrap();
        assert_eq!(statements.len(), 1);
        assert!(format!("{:?}", statements[0]).contains("COUNT_BIG"));
    }

    #[test]
    fn accepts_sql_server_count_big_distinct_as_read_compatible_count() {
        let statements = parse_sql("SELECT COUNT_BIG(DISTINCT user_id) AS users FROM t").unwrap();
        assert_eq!(statements.len(), 1);
        assert!(format!("{:?}", statements[0]).contains("COUNT_BIG"));
    }

    #[test]
    fn bracketed_identifiers_are_not_parsed_as_arrays() {
        let statements = parse_sql("SELECT DISTINCT [year] FROM climate_energy.energy_annual")
            .expect("bracketed identifier should parse");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("\"year\""));
        assert!(!rendered.contains("ARRAY"));
    }

    #[test]
    fn json_returning_clause_lowers_to_a_typed_cast() {
        let statements = parse_sql(
            "SELECT JSON_VALUE(JSON_PARSE('{\"count\":7}'), '$.count' RETURNING BIGINT) AS count, JSON_QUERY(JSON_PARSE('{\"items\":[1]}'), '$.items' RETURNING JSON) AS items FROM t",
        )
        .expect("bounded JSON RETURNING forms should parse");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains("CAST(JSON_VALUE(JSON_PARSE('{\"count\":7}'), '$.count') AS BIGINT)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("CAST(JSON_QUERY(JSON_PARSE('{\"items\":[1]}'), '$.items') AS JSON)"),
            "{rendered}"
        );
    }

    #[test]
    fn json_returning_decimal_lowers_to_a_bounded_decimal128_cast() {
        let statements = parse_sql(
            "SELECT JSON_VALUE(JSON_PARSE('{\"amount\":12.34}'), '$.amount' RETURNING DECIMAL(10, 2)) FROM t",
        )
        .expect("bounded decimal JSON RETURNING form should parse");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains(
                "CAST(JSON_VALUE(JSON_PARSE('{\"amount\":12.34}'), '$.amount') AS DECIMAL(10,2))"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn json_returning_rewrite_preserves_nested_calls_and_literals() {
        let sql = "SELECT JSON_VALUE(JSON_PARSE('{\"label\":\"RETURNING BIGINT\"}'), '$.label' RETURNING VARCHAR) FROM t WHERE note = 'JSON_VALUE(x RETURNING BIGINT)'";
        let normalized = normalize_json_returning(sql);
        assert_eq!(
            normalized,
            "SELECT CAST(JSON_VALUE(JSON_PARSE('{\"label\":\"RETURNING BIGINT\"}'), '$.label') AS VARCHAR) FROM t WHERE note = 'JSON_VALUE(x RETURNING BIGINT)'"
        );
    }

    #[test]
    fn json_null_clauses_lower_to_existing_null_behavior() {
        let statements = parse_sql(
            "SELECT JSON_VALUE(payload, '$.missing' RETURNING BIGINT ON EMPTY NULL ON ERROR NULL), JSON_QUERY(payload, '$.missing' ON EMPTY NULL) FROM t",
        )
        .expect("bounded JSON NULL clauses should parse");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains("CAST(JSON_VALUE(payload, '$.missing') AS BIGINT)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("JSON_QUERY(payload, '$.missing')"),
            "{rendered}"
        );
        assert!(!rendered.contains("ON EMPTY"), "{rendered}");
        assert!(!rendered.contains("ON ERROR"), "{rendered}");
    }

    #[test]
    fn json_default_literal_lowers_to_bounded_evaluator_function() {
        let statements = parse_sql(
            "SELECT JSON_VALUE(payload, '$.missing' ON EMPTY DEFAULT 'fallback' ON ERROR DEFAULT 'bad'), JSON_QUERY(payload, '$.missing' ON EMPTY DEFAULT NULL) FROM t",
        )
        .expect("literal JSON DEFAULT clauses should parse");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains("JSON_VALUE_DEFAULT(payload, '$.missing', 'fallback', 'bad')"),
            "{rendered}"
        );
        assert!(
            rendered.contains("JSON_QUERY_DEFAULT(payload, '$.missing', NULL, NULL)"),
            "{rendered}"
        );
    }

    #[test]
    fn unsupported_json_default_expression_fails_closed() {
        let error = parse_sql(
            "SELECT JSON_VALUE(payload, '$.missing' ON EMPTY DEFAULT other_column) FROM t",
        )
        .expect_err("non-literal JSON default must be rejected");
        assert!(
            error
                .to_string()
                .contains("only SQL literals are supported")
        );
    }

    #[test]
    fn json_default_literal_keeps_returning_text_inside_quotes_literal() {
        let statements = parse_sql(
            "SELECT JSON_VALUE(payload, '$.missing' ON EMPTY DEFAULT 'RETURNING BIGINT') FROM t",
        )
        .expect("RETURNING text inside a default string is not a clause");
        assert!(statements[0].to_string().contains("'RETURNING BIGINT'"));
    }

    #[test]
    fn json_default_with_returning_lowers_through_cast() {
        let statements = parse_sql(
            "SELECT JSON_VALUE(payload, '$.missing' RETURNING BIGINT ON EMPTY DEFAULT '0') FROM t",
        )
        .expect("typed JSON DEFAULT should lower through the existing cast path");
        let rendered = statements[0].to_string();
        assert!(
            rendered
                .contains("CAST(JSON_VALUE_DEFAULT(payload, '$.missing', '0', NULL) AS BIGINT)"),
            "{rendered}"
        );
    }

    #[test]
    fn json_path_mode_prefixes_remain_in_path_literals() {
        let statements = parse_sql(
            "SELECT JSON_VALUE(payload, 'lax $.x'), JSON_QUERY(payload, 'strict $.items') FROM t",
        )
        .expect("SQL/JSON path mode prefixes should parse as path text");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("'lax $.x'"), "{rendered}");
        assert!(rendered.contains("'strict $.items'"), "{rendered}");
    }

    #[test]
    fn json_path_surrogate_pairs_remain_in_path_literals() {
        let statements = parse_sql(
            r#"SELECT JSON_VALUE(payload, '$."\uD83D\uDE00".value' RETURNING BIGINT), JSON_QUERY(payload, '$["\ud83d\ude00"]') FROM t"#,
        )
        .expect("quoted SQL/JSON surrogate escapes should remain path text");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains(r#"'$."\uD83D\uDE00".value'"#),
            "{rendered}"
        );
        assert!(rendered.contains(r#"'$["\ud83d\ude00"]'"#), "{rendered}");
        assert!(rendered.contains("AS BIGINT"), "{rendered}");
    }

    #[test]
    fn json_path_mode_prefixes_accept_whitespace_separators() {
        let statements = parse_sql(
            "SELECT JSON_VALUE(payload, 'lax   $.x'), JSON_QUERY(payload, 'strict\t$.items') FROM t",
        )
        .expect("SQL/JSON mode prefixes should allow whitespace separators");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("'lax   $.x'"), "{rendered}");
        assert!(rendered.contains("'strict\t$.items'"), "{rendered}");
    }

    #[test]
    fn json_query_typed_error_default_lowers_through_cast() {
        let statements = parse_sql(
            "SELECT JSON_QUERY('not-json', '$.missing' RETURNING JSON ON ERROR DEFAULT '{\"fallback\":2}') FROM t",
        )
        .expect("typed JSON_QUERY error defaults should lower through the cast path");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains(
                "CAST(JSON_QUERY_DEFAULT('not-json', '$.missing', NULL, '{\"fallback\":2}') AS JSON)"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn json_value_date_returning_lowers_through_cast() {
        let statements = parse_sql("SELECT JSON_VALUE(payload, '$.day' RETURNING DATE) FROM t")
            .expect("JSON_VALUE DATE RETURNING should use the existing cast path");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains("CAST(JSON_VALUE(payload, '$.day') AS DATE)"),
            "{rendered}"
        );
    }

    #[test]
    fn json_value_timestamp_returning_lowers_through_cast() {
        let statements =
            parse_sql("SELECT JSON_VALUE(payload, '$.created' RETURNING TIMESTAMP) FROM t")
                .expect("JSON_VALUE TIMESTAMP RETURNING should use the existing cast path");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains("CAST(JSON_VALUE(payload, '$.created') AS TIMESTAMP)"),
            "{rendered}"
        );
    }

    #[test]
    fn json_value_timestamp_with_timezone_returning_lowers_through_cast() {
        let statements = parse_sql(
            "SELECT JSON_VALUE(payload, '$.created' RETURNING TIMESTAMP WITH TIME ZONE) FROM t",
        )
        .expect("JSON_VALUE TIMESTAMP WITH TIME ZONE should use the existing cast path");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains("CAST(JSON_VALUE(payload, '$.created') AS TIMESTAMP WITH TIME ZONE)"),
            "{rendered}"
        );
    }

    #[test]
    fn json_object_with_unique_keys_lowers_to_existing_duplicate_key_guard() {
        let statements = parse_sql(
            "SELECT JSON_OBJECT('answer' : 7, 'label' VALUE 'x' WITH UNIQUE KEYS) FROM t",
        )
        .expect("WITH UNIQUE KEYS should use the existing fail-closed object evaluator");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("JSON_OBJECT"), "{rendered}");
        assert!(!rendered.contains("UNIQUE"), "{rendered}");
    }

    #[test]
    fn json_object_without_unique_keys_remains_rejected() {
        let error = parse_sql("SELECT JSON_OBJECT('a' : 1, 'a' : 2 WITHOUT UNIQUE KEYS) FROM t")
            .expect_err("WITHOUT UNIQUE KEYS must not silently choose a duplicate-key winner");
        assert!(error.to_string().contains("WITHOUT UNIQUE KEYS"));
    }

    #[test]
    fn json_object_unique_key_rewrite_ignores_quoted_words_and_nested_objects() {
        let sql = "SELECT JSON_OBJECT('note' : 'WITH UNIQUE KEYS', 'nested' : JSON_OBJECT('x' : 1 WITH UNIQUE KEYS) WITH UNIQUE KEYS) FROM t";
        let normalized = normalize_json_object_unique_keys(sql).unwrap();
        assert_eq!(normalized.matches("WITH UNIQUE KEYS").count(), 1);
        assert!(normalized.contains("'WITH UNIQUE KEYS'"));
        assert!(normalized.contains("JSON_OBJECT('x' : 1 )"));
    }

    #[test]
    fn json_exists_error_actions_lower_to_bounded_variants() {
        let statements = parse_sql(
            "SELECT JSON_EXISTS(payload, '$.x' TRUE ON ERROR), JSON_EXISTS(payload, '$.y' UNKNOWN ON ERROR), JSON_EXISTS(payload, '$.z' FALSE ON ERROR) FROM t",
        )
        .expect("JSON_EXISTS ON ERROR actions should parse");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("JSON_EXISTS_TRUE_ON_ERROR"), "{rendered}");
        assert!(
            rendered.contains("JSON_EXISTS_UNKNOWN_ON_ERROR"),
            "{rendered}"
        );
        assert!(!rendered.contains("ON ERROR"), "{rendered}");
    }

    #[test]
    fn json_exists_duplicate_error_action_fails_closed() {
        let error =
            parse_sql("SELECT JSON_EXISTS(payload, '$.x' TRUE ON ERROR FALSE ON ERROR) FROM t")
                .expect_err("duplicate JSON_EXISTS ON ERROR actions must be rejected");
        assert!(error.to_string().contains("duplicate JSON_EXISTS"));
    }

    #[test]
    fn json_error_clause_lowers_to_strict_function() {
        let statements = parse_sql(
            "SELECT JSON_VALUE(payload, '$.missing' RETURNING BIGINT ON EMPTY ERROR), JSON_QUERY(payload, '$.missing' ON ERROR ERROR) FROM t",
        )
        .expect("bounded JSON ERROR clauses should parse");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains("CAST(JSON_VALUE_STRICT(payload, '$.missing') AS BIGINT)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("JSON_QUERY_STRICT(payload, '$.missing')"),
            "{rendered}"
        );
    }

    #[test]
    fn unsupported_json_returning_decimal_bounds_remain_rejected() {
        let sql = "SELECT JSON_VALUE(payload, '$.count' RETURNING DECIMAL(39, 2)) FROM t";
        assert!(parse_sql(sql).is_err());

        let sql = "SELECT JSON_VALUE(payload, '$.count' RETURNING DECIMAL(10, 11)) FROM t";
        assert!(parse_sql(sql).is_err());
    }

    #[test]
    fn bounded_linear_recursive_cte_uses_sequence_lowering() {
        let statements = parse_sql(
            "WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM numbers WHERE n < 5) SELECT * FROM numbers",
        )
        .expect("bounded recursive CTE should parse");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("SEQUENCE(1, 5, 1)"), "{rendered}");
        assert!(!rendered.contains("WITH RECURSIVE"), "{rendered}");
    }

    #[test]
    fn bounded_recursive_cte_supports_descending_and_inclusive_bounds() {
        let source = "WITH RECURSIVE countdown(n) AS (SELECT 5 UNION ALL SELECT n - 1 FROM countdown WHERE n > 1) SELECT * FROM countdown";
        let rewritten = normalize_bounded_linear_recursive_cte(source);
        assert!(rewritten.is_some(), "{rewritten:?}");
        let statements = parse_sql(source).expect("descending recursive CTE should parse");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("SEQUENCE(5, 1, -1)"), "{rendered}");

        let statements = parse_sql(
            "WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n + 2 FROM numbers WHERE n <= 5) SELECT * FROM numbers",
        )
        .expect("inclusive recursive CTE should parse");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("SEQUENCE(1, 7, 2)"), "{rendered}");
    }

    #[test]
    fn bounded_linear_recursive_cte_accepts_distinct_union() {
        let source = "WITH RECURSIVE numbers(n) AS (SELECT 1 UNION SELECT n + 1 FROM numbers WHERE n < 3) SELECT * FROM numbers";
        let statements = parse_sql(source).expect("bounded UNION should parse");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("SEQUENCE(1, 3, 1)"), "{rendered}");
        assert!(!rendered.contains("WITH RECURSIVE"), "{rendered}");
    }

    #[test]
    fn bounded_linear_recursive_cte_accepts_matching_seed_and_step_aliases() {
        let source = "WITH RECURSIVE numbers(n) AS (SELECT 1 AS n UNION ALL SELECT n + 1 AS n FROM numbers WHERE n < 3) SELECT n FROM numbers ORDER BY n";
        assert!(
            normalize_bounded_linear_recursive_cte(source).is_some(),
            "rewrite should match"
        );
        let statements = parse_sql(source).expect("aliased bounded recursive CTE should parse");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("SEQUENCE(1, 3, 1)"), "{rendered}");
        assert!(!rendered.contains("WITH RECURSIVE"), "{rendered}");
        let mismatched = source.replace("AS n FROM numbers", "AS other FROM numbers");
        assert!(normalize_bounded_linear_recursive_cte(&mismatched).is_none());
    }

    #[test]
    fn bounded_recursive_cte_infers_single_column_from_seed_alias() {
        let source = "WITH RECURSIVE numbers AS (SELECT 1 AS n UNION ALL SELECT n + 1 FROM numbers WHERE n < 3) SELECT n FROM numbers ORDER BY n";
        let rewritten = normalize_bounded_linear_recursive_cte(source)
            .expect("seed alias should supply the recursive column");
        assert!(rewritten.contains("numbers(n)") && rewritten.contains("SEQUENCE(1, 3, 1)"));
        let statements = parse_sql(source).expect("inferred recursive column should parse");
        assert!(!statements[0].to_string().contains("WITH RECURSIVE"));
    }

    #[test]
    fn recursive_cte_without_column_list_still_requires_seed_alias() {
        let source = "WITH RECURSIVE numbers AS (SELECT 1 UNION ALL SELECT n + 1 FROM numbers WHERE n < 3) SELECT n FROM numbers";
        assert!(normalize_bounded_linear_recursive_cte(source).is_none());
    }

    #[test]
    fn unsupported_recursive_cte_is_not_rewritten() {
        let sql = "WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM numbers WHERE n < other) SELECT * FROM numbers";
        assert!(normalize_bounded_linear_recursive_cte(sql).is_none());
    }

    #[test]
    fn recursive_cte_probe_is_utf8_boundary_safe() {
        assert!(normalize_bounded_linear_recursive_cte("😀 SELECT 1").is_none());
    }

    #[test]
    fn accepts_only_default_tsql_read_session_prefixes() {
        let statements = parse_sql(
            "SET ANSI_NULLS ON; SET QUOTED_IDENTIFIER ON; SET ANSI_WARNINGS ON; SET NOCOUNT ON; SET DATEFIRST 7; SELECT DATEPART(dw, event_date) FROM t",
        )
        .expect("Engine-default session prefixes should be no-ops");
        assert_eq!(statements.len(), 1);
        assert!(statements[0].to_string().contains("DATEPART"));

        let statements = parse_sql("SET DATEFIRST 1; SELECT DATEPART(dw, event_date) FROM t")
            .expect("DATEFIRST 1 should lower to an immutable DATEPART function");
        assert!(
            statements[0]
                .to_string()
                .contains("DATEPART_DATEFIRST_1(dw, event_date)")
        );
        let error = parse_sql("SET DATEFIRST 0; SELECT 1")
            .expect_err("out-of-range DATEFIRST must remain fail-closed");
        assert!(error.to_string().contains("1 through 7"));
        let error = parse_sql("SET NOCOUNT OFF; SELECT 1")
            .expect_err("non-default NOCOUNT must remain fail-closed");
        assert!(error.to_string().contains("only the Engine default ON"));
    }

    #[test]
    fn rejects_unbounded_or_extended_write_forms() {
        let cases = [
            "INSERT INTO users VALUES (1)",
            "INSERT INTO users (id) SELECT id FROM old_users RETURNING id",
            "INSERT INTO users (id) VALUES (1) RETURNING id",
            "UPDATE users SET name = 'Ada'",
            "UPDATE users SET name = old.name FROM old_users old WHERE users.id = old.id",
            "DELETE FROM users",
            "COMMIT AND CHAIN",
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        ];
        for sql in cases {
            assert!(
                parse_native_transactional(sql).is_err(),
                "unexpectedly accepted {sql}"
            );
        }
    }

    #[test]
    fn transaction_control_rejections_explain_the_unsupported_feature() {
        let cases = [
            (
                "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
                "SET TRANSACTION isolation and access modes are not supported",
            ),
            (
                "BEGIN ISOLATION LEVEL REPEATABLE READ",
                "BEGIN isolation, access-mode, and dialect modifiers are not supported",
            ),
        ];
        for (sql, expected) in cases {
            assert!(
                parse_native_transactional(sql)
                    .unwrap_err()
                    .to_string()
                    .contains(expected),
                "{sql} did not report {expected}"
            );
        }
    }

    #[test]
    fn accepts_savepoint_controls() {
        assert!(matches!(
            parse_native_transactional("SAVEPOINT checkpoint").unwrap(),
            NativeTransactionalStatement::Savepoint(name) if name == "checkpoint"
        ));
        assert!(matches!(
            parse_native_transactional("ROLLBACK TO SAVEPOINT checkpoint").unwrap(),
            NativeTransactionalStatement::RollbackToSavepoint(name) if name == "checkpoint"
        ));
        assert!(matches!(
            parse_native_transactional("RELEASE SAVEPOINT checkpoint").unwrap(),
            NativeTransactionalStatement::ReleaseSavepoint(name) if name == "checkpoint"
        ));
    }

    #[test]
    fn rejects_batches_and_non_transactional_sql() {
        for sql in [
            "BEGIN; DELETE FROM users WHERE id = 1",
            "SELECT * FROM users",
            "CREATE TABLE users (id BIGINT)",
        ] {
            assert!(
                parse_native_transactional(sql).is_err(),
                "unexpectedly accepted {sql}"
            );
        }
    }

    #[test]
    fn rejects_unsupported_tsql_relational_forms_with_actionable_errors() {
        for (sql, message) in [
            (
                "SELECT * FROM t UNPIVOT (v FOR k IN (a)) u",
                "UNPIVOT is not implemented",
            ),
            (
                "SELECT * FROM t CROSS APPLY f(t.id)",
                "CROSS/OUTER APPLY is not implemented",
            ),
        ] {
            let error = parse_sql(sql).expect_err("unsupported T-SQL form must fail closed");
            assert!(error.to_string().contains(message), "{sql}: {error}");
        }
        assert!(parse_sql("SELECT pivot_value FROM t").is_ok());
    }

    #[test]
    fn unsupported_lake_index_and_constraint_ddl_fails_closed() {
        for (sql, expected) in [
            (
                "CREATE INDEX orders_customer_idx ON orders (customer_id)",
                "index DDL is not implemented",
            ),
            (
                "CREATE UNIQUE INDEX orders_id_idx ON orders (id)",
                "index DDL is not implemented",
            ),
            (
                "DROP INDEX orders_customer_idx",
                "index DDL is not implemented",
            ),
            (
                "ALTER TABLE orders ADD CONSTRAINT orders_id_key UNIQUE (id)",
                "constraint DDL is not implemented",
            ),
            (
                "-- generated by migration\nCREATE INDEX orders_customer_idx ON orders (customer_id)",
                "index DDL is not implemented",
            ),
            (
                "/* migration step */ ALTER TABLE orders ADD CONSTRAINT orders_id_key UNIQUE (id)",
                "constraint DDL is not implemented",
            ),
            (
                "CREATE\nINDEX orders_customer_idx ON orders (customer_id)",
                "index DDL is not implemented",
            ),
            (
                "DROP\tINDEX orders_customer_idx",
                "index DDL is not implemented",
            ),
        ] {
            let error = parse_sql(sql).expect_err("unsupported lake DDL must fail closed");
            assert!(error.to_string().contains(expected), "{sql}: {error}");
        }
        assert!(parse_sql("SELECT * FROM indexed_table").is_ok());
    }

    #[test]
    fn bounded_pivot_lowers_to_conditional_aggregation() {
        let sql = "SELECT * FROM (SELECT region, month, amount FROM sales) src PIVOT (SUM(amount) FOR month IN ([Jan], [Feb])) p";
        let statements = parse_sql(sql).expect("bounded PIVOT should lower");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains("SUM(CASE WHEN month = 'Jan' THEN amount END)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("SUM(CASE WHEN month = 'Feb' THEN amount END)"),
            "{rendered}"
        );
        assert!(rendered.contains("GROUP BY region"), "{rendered}");
    }

    #[test]
    fn bounded_pivot_preserves_outer_ordering() {
        let sql = "SELECT * FROM (SELECT region, month, amount FROM sales) src PIVOT (SUM(amount) FOR month IN ([Jan], [Feb])) p ORDER BY region";
        let statements = parse_sql(sql).expect("ordered bounded PIVOT should lower");
        let rendered = statements[0].to_string();
        assert!(rendered.ends_with("ORDER BY region"), "{rendered}");
    }

    #[test]
    fn bounded_pivot_count_star_lowers_to_conditional_aggregation() {
        let sql = "SELECT * FROM (SELECT region, month FROM sales) src PIVOT (COUNT(*) FOR month IN ([Jan], [Feb])) p";
        let statements = parse_sql(sql).expect("COUNT(*) PIVOT should lower");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains("COUNT(CASE WHEN month = 'Jan' THEN 1 END)"),
            "{rendered}"
        );
        assert!(rendered.contains("GROUP BY region"), "{rendered}");
    }

    #[test]
    fn bounded_pivot_count_big_star_lowers_to_conditional_aggregation() {
        let sql = "SELECT * FROM (SELECT region, month FROM sales) src PIVOT (COUNT_BIG(*) FOR month IN ([Jan], [Feb])) p";
        let statements = parse_sql(sql).expect("COUNT_BIG(*) PIVOT should lower");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains("COUNT_BIG(CASE WHEN month = 'Jan' THEN 1 END)"),
            "{rendered}"
        );
        assert!(rendered.contains("GROUP BY region"), "{rendered}");
    }

    #[test]
    fn bounded_pivot_count_big_expression_preserves_null_counting() {
        let sql = "SELECT * FROM (SELECT region, month, amount FROM sales) src PIVOT (COUNT_BIG(amount) FOR month IN ([Jan], [Feb])) p";
        let statements = parse_sql(sql).expect("COUNT_BIG(expression) PIVOT should lower");
        let rendered = statements[0].to_string();
        assert!(
            rendered.contains("COUNT_BIG(CASE WHEN month = 'Jan' THEN amount END)"),
            "{rendered}"
        );
        assert!(rendered.contains("GROUP BY region"), "{rendered}");
    }

    #[test]
    fn pivot_keeps_unbounded_shapes_fail_closed() {
        let error = parse_sql("SELECT * FROM t PIVOT (SUM(v) FOR k IN ([a])) p")
            .expect_err("a non-derived PIVOT must remain unsupported");
        assert!(error.to_string().contains("PIVOT is not implemented"));
    }

    #[test]
    fn bounded_unpivot_lowers_to_union_all() {
        let sql = "SELECT * FROM (SELECT region, Jan, Feb FROM sales) src UNPIVOT (amount FOR month IN ([Jan], [Feb])) p";
        let statements = parse_sql(sql).expect("bounded UNPIVOT should lower");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("'Jan' AS \"month\""), "{rendered}");
        assert!(rendered.contains("'Feb' AS \"month\""), "{rendered}");
        assert!(rendered.contains("UNION ALL"), "{rendered}");
    }

    #[test]
    fn bounded_unpivot_preserves_outer_ordering() {
        let sql = "SELECT * FROM (SELECT region, Jan, Feb FROM sales) src UNPIVOT (amount FOR month IN ([Jan], [Feb])) p ORDER BY region, month";
        let statements = parse_sql(sql).expect("ordered bounded UNPIVOT should lower");
        let rendered = statements[0].to_string();
        assert!(rendered.ends_with("ORDER BY region, month"), "{rendered}");
    }

    #[test]
    fn unpivot_keeps_unbounded_shapes_fail_closed() {
        let error = parse_sql("SELECT * FROM t UNPIVOT (v FOR k IN (a)) u")
            .expect_err("a non-derived UNPIVOT must remain unsupported");
        assert!(error.to_string().contains("UNPIVOT is not implemented"));
    }

    #[test]
    fn bounded_cross_apply_lowers_to_lateral_join() {
        let sql = "SELECT t.id, a.next_id FROM t CROSS APPLY (VALUES (1)) AS a(next_id)";
        let statements = parse_sql(sql).expect("bounded CROSS APPLY should lower");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("CROSS JOIN LATERAL"), "{rendered}");
    }

    #[test]
    fn bounded_outer_apply_lowers_to_left_lateral_join() {
        let sql = "SELECT t.id, a.next_id FROM t OUTER APPLY (VALUES (1)) AS a(next_id)";
        let statements = parse_sql(sql).expect("bounded OUTER APPLY should lower");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("LEFT JOIN LATERAL"), "{rendered}");
        assert!(
            rendered.to_ascii_uppercase().contains("ON TRUE"),
            "{rendered}"
        );
    }

    #[test]
    fn correlated_cross_apply_lowers_to_lateral_join() {
        let statements = parse_sql(
            "SELECT t.id, a.next_id FROM t CROSS APPLY (VALUES (t.id + 1)) AS a(next_id)",
        )
        .expect("correlated derived APPLY should lower");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("CROSS JOIN LATERAL"), "{rendered}");
    }

    #[test]
    fn correlated_outer_apply_lowers_to_left_lateral_join() {
        let statements = parse_sql(
            "SELECT t.id, a.next_id FROM t OUTER APPLY (VALUES (t.id + 1)) AS a(next_id)",
        )
        .expect("correlated derived OUTER APPLY should lower");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("LEFT JOIN LATERAL"), "{rendered}");
    }

    #[test]
    fn string_split_cross_apply_lowers_to_lateral_join() {
        let statements = parse_sql(
            "SELECT t.id, parts.value FROM t CROSS APPLY STRING_SPLIT(t.csv, ',') AS parts(value)",
        )
        .expect("STRING_SPLIT CROSS APPLY should lower");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("CROSS JOIN LATERAL"), "{rendered}");
        assert!(rendered.contains("STRING_SPLIT"), "{rendered}");
    }

    #[test]
    fn string_split_outer_apply_lowers_to_left_lateral_join() {
        let statements = parse_sql(
            "SELECT t.id, parts.value FROM t OUTER APPLY STRING_SPLIT(t.csv, ',') AS parts(value)",
        )
        .expect("STRING_SPLIT OUTER APPLY should lower");
        let rendered = statements[0].to_string();
        assert!(rendered.contains("LEFT JOIN LATERAL"), "{rendered}");
        assert!(rendered.contains("STRING_SPLIT"), "{rendered}");
        assert!(
            rendered.to_ascii_uppercase().contains("ON TRUE"),
            "{rendered}"
        );
    }

    #[test]
    fn apply_function_keeps_fail_closed_boundary() {
        let error = parse_sql("SELECT * FROM t CROSS APPLY f(t.id)")
            .expect_err("table-valued APPLY functions remain unsupported");
        assert!(
            error
                .to_string()
                .contains("CROSS/OUTER APPLY is not implemented")
        );
    }

    #[test]
    fn accepts_semicolon_terminated_single_transaction_statements() {
        for sql in [
            "BEGIN;",
            "COMMIT;",
            "ROLLBACK;",
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}');",
            "UPDATE product.datasets SET document_json = '{}' WHERE id = 'ds-1' AND revision = 1;",
            "DELETE FROM product.datasets WHERE id = 'ds-1' AND revision = 1;",
        ] {
            assert!(
                parse_native_transactional(sql).is_ok(),
                "expected supported single statement: {sql}"
            );
        }
    }

    #[test]
    fn accepts_string_agg_syntax_for_planner_validation() {
        assert!(parse_sql("SELECT STRING_AGG(name, ',') FROM events").is_ok());
    }

    #[test]
    fn unsupported_word_guard_ignores_literals_comments_and_quoted_identifiers() {
        for sql in [
            "SELECT 'pivot', 'string_agg', 'apply' FROM events",
            "SELECT 1 /* pivot unpivot apply string_agg */ FROM events",
            "SELECT 1 -- pivot string_agg apply\nFROM events",
            "SELECT [pivot], [string_agg] FROM events",
        ] {
            parse_sql(sql).unwrap_or_else(|error| {
                panic!("text inside a literal/comment/identifier was rejected: {sql}: {error}")
            });
        }
    }

    #[test]
    fn accepts_sql_server_transaction_control_aliases() {
        for sql in [
            "BEGIN TRANSACTION",
            "BEGIN TRAN",
            "COMMIT TRANSACTION",
            "COMMIT TRAN",
            "ROLLBACK TRANSACTION",
            "ROLLBACK TRAN",
        ] {
            parse_native_transactional(sql).unwrap_or_else(|error| {
                panic!("SQL Server transaction alias rejected: {sql}: {error}")
            });
        }
    }

    #[test]
    fn accepts_merge_not_matched_by_target_alias() {
        let sql = "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED BY TARGET THEN INSERT (id, document_json) VALUES (s.id, s.document_json)";
        parse_native_transactional(sql)
            .unwrap_or_else(|error| panic!("MERGE BY TARGET alias rejected: {error}"));
    }

    #[test]
    fn rejects_duplicate_merge_source_ids() {
        let sql = "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}'), ('ds-1', '{\"duplicate\":true}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json)";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("duplicate ids remain syntactically valid")
        else {
            panic!("expected MERGE DML");
        };
        let error =
            adapt_product_dml_batch(&dml).expect_err("duplicate MERGE ids must fail closed");
        assert!(error.to_string().contains("source ids must be unique"));
    }

    #[test]
    fn accepts_identity_returning_for_product_update_and_delete() {
        for sql in [
            "UPDATE product.datasets AS target SET document_json = '{}' WHERE id = 'ds-1' AND revision = 1 RETURNING id, revision",
            "DELETE FROM product.datasets AS target WHERE id = 'ds-1' AND revision = 1 RETURNING id, revision, document_json",
        ] {
            let NativeTransactionalStatement::Dml(dml) =
                parse_native_transactional(sql).expect("product DML should parse")
            else {
                panic!("expected product DML")
            };
            if sql.starts_with("DELETE") {
                assert_eq!(dml.returning, ["id", "revision", "document_json"]);
            } else {
                assert_eq!(dml.returning, ["id", "revision"]);
            }
            assert!(adapt_product_dml(&dml).is_ok());
        }
    }

    #[test]
    fn accepts_bounded_product_update_from_and_rejects_wider_shapes() {
        let sql = "UPDATE product.datasets AS target SET target.document_json = source.document_json FROM product.datasets AS source WHERE target.id = 'target-1' AND target.revision = 2 AND source.id = 'source-1' RETURNING id, revision, document_json";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("bounded UPDATE FROM should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::UpdateFrom {
                kind: "dataset".into(),
                id: "target-1".into(),
                expected_revision: 2,
                source_id: "source-1".into(),
                source_kind: "dataset".into(),
            }
        );
        let sql = "UPDATE product.datasets AS target SET target.document_json = source.document_json FROM product.datasets AS source WHERE target.id IN ('target-1', 'target-2') AND target.revision = 2 AND source.id = 'source-1'";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("bounded batch UPDATE FROM should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![
                ProductDmlCommand::UpdateFrom {
                    kind: "dataset".into(),
                    id: "target-1".into(),
                    expected_revision: 2,
                    source_id: "source-1".into(),
                    source_kind: "dataset".into(),
                },
                ProductDmlCommand::UpdateFrom {
                    kind: "dataset".into(),
                    id: "target-2".into(),
                    expected_revision: 2,
                    source_id: "source-1".into(),
                    source_kind: "dataset".into(),
                },
            ]
        );
        let sql = "UPDATE product.datasets AS target SET target.document_json = source.document_json FROM product.datasets AS source WHERE target.id IN ('target-1', 'target-2') AND target.revision = 2 AND target.id = source.id";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("identity UPDATE FROM should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![
                ProductDmlCommand::UpdateFrom {
                    kind: "dataset".into(),
                    id: "target-1".into(),
                    expected_revision: 2,
                    source_id: "target-1".into(),
                    source_kind: "dataset".into(),
                },
                ProductDmlCommand::UpdateFrom {
                    kind: "dataset".into(),
                    id: "target-2".into(),
                    expected_revision: 2,
                    source_id: "target-2".into(),
                    source_kind: "dataset".into(),
                },
            ]
        );
        for sql in [
            "UPDATE product.datasets AS target SET document_json = source.document_json FROM product.charts AS source WHERE target.id = 'target-1' AND target.revision = 2 AND source.id = 'source-1'",
            "UPDATE product.datasets AS target SET document_json = '{}' FROM product.datasets AS source WHERE target.id = 'target-1' AND target.revision = 2 AND source.id = 'source-1'",
            "UPDATE product.datasets AS target SET document_json = source.document_json FROM product.datasets AS source WHERE target.id IN ('target-1', 'target-1') AND target.revision = 2 AND source.id = 'source-1'",
            "UPDATE product.datasets AS target SET document_json = source.document_json FROM product.datasets AS source WHERE target.id IN ('target-1', 'target-2') AND target.revision = 2 AND source.id = source.id",
        ] {
            let NativeTransactionalStatement::Dml(dml) =
                parse_native_transactional(sql).expect("shape should parse before adaptation")
            else {
                panic!("expected product DML");
            };
            assert!(
                adapt_product_dml(&dml).is_err(),
                "unsupported UPDATE FROM shape was accepted: {sql}"
            );
        }
    }

    #[test]
    fn product_update_from_accepts_a_typed_source_document_assignment() {
        let sql = "UPDATE product.datasets AS target SET target.document_json = CAST(source.document_json AS JSON) FROM product.datasets AS source WHERE target.id = 'target-1' AND target.revision = 2 AND source.id = 'source-1'";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("typed UPDATE FROM should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::UpdateFrom {
                kind: "dataset".into(),
                id: "target-1".into(),
                expected_revision: 2,
                source_id: "source-1".into(),
                source_kind: "dataset".into(),
            }
        );
    }

    #[test]
    fn accepts_bounded_product_delete_using_and_rejects_wider_shapes() {
        let sql = "DELETE FROM product.datasets AS target USING product.datasets AS source WHERE target.id IN ('target-1', 'target-2') AND target.revision = 3 AND source.id = 'source-1' RETURNING id, revision";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("bounded DELETE USING should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![
                ProductDmlCommand::DeleteFrom {
                    kind: "dataset".into(),
                    id: "target-1".into(),
                    expected_revision: 3,
                    source_id: "source-1".into(),
                    source_kind: "dataset".into(),
                },
                ProductDmlCommand::DeleteFrom {
                    kind: "dataset".into(),
                    id: "target-2".into(),
                    expected_revision: 3,
                    source_id: "source-1".into(),
                    source_kind: "dataset".into(),
                },
            ]
        );
        for sql in [
            "DELETE FROM product.datasets AS target USING product.charts AS source WHERE target.id = 'target-1' AND target.revision = 3 AND source.id = 'source-1'",
            "DELETE FROM product.datasets AS target USING product.datasets AS source WHERE target.id = 'target-1' AND target.revision = 3",
            "DELETE FROM product.datasets AS target USING product.datasets AS source WHERE target.id IN ('target-1', 'target-1') AND target.revision = 3 AND source.id = 'source-1'",
        ] {
            let NativeTransactionalStatement::Dml(dml) =
                parse_native_transactional(sql).expect("shape should parse before adaptation")
            else {
                panic!("expected product DML");
            };
            assert!(
                adapt_product_dml(&dml).is_err(),
                "unsupported DELETE USING shape was accepted: {sql}"
            );
        }
    }

    #[test]
    fn sql_server_output_lowers_to_bounded_product_returning() {
        for sql in [
            "INSERT INTO product.datasets (id, document_json) OUTPUT inserted.id, inserted.revision VALUES ('ds-1', '{}')",
            "UPDATE product.datasets SET document_json = '{}' OUTPUT inserted.id, inserted.revision WHERE id = 'ds-1' AND revision = 1",
            "DELETE FROM product.datasets OUTPUT deleted.id, deleted.revision, deleted.document_json WHERE id = 'ds-1' AND revision = 1",
        ] {
            let NativeTransactionalStatement::Dml(dml) =
                parse_native_transactional(sql).expect("bounded OUTPUT should parse")
            else {
                panic!("expected product DML");
            };
            assert!(
                dml.returning.iter().all(|column| {
                    matches!(column.as_str(), "id" | "revision" | "document_json")
                })
            );
            assert!(adapt_product_dml(&dml).is_ok());
        }
    }

    #[test]
    fn sql_server_output_wildcard_expands_to_product_fields() {
        for sql in [
            "INSERT INTO product.datasets (id, document_json) OUTPUT inserted.* VALUES ('ds-1', '{}')",
            "UPDATE product.datasets SET document_json = '{}' OUTPUT inserted.* WHERE id = 'ds-1' AND revision = 1",
            "DELETE FROM product.datasets OUTPUT deleted.* WHERE id = 'ds-1' AND revision = 1",
        ] {
            let NativeTransactionalStatement::Dml(dml) =
                parse_native_transactional(sql).expect("bounded OUTPUT wildcard should parse")
            else {
                panic!("expected product DML");
            };
            assert_eq!(dml.returning, ["id", "revision", "document_json"]);
            assert!(adapt_product_dml(&dml).is_ok());
        }
        for sql in [
            "UPDATE product.datasets SET document_json = '{}' OUTPUT deleted.* WHERE id = 'ds-1' AND revision = 1",
            "UPDATE product.datasets SET document_json = '{}' OUTPUT inserted.*, inserted.id WHERE id = 'ds-1' AND revision = 1",
        ] {
            assert!(
                parse_native_transactional(sql).is_err(),
                "unsafe OUTPUT wildcard was accepted: {sql}"
            );
        }
    }

    #[test]
    fn sql_server_merge_output_lowers_to_inserted_product_returning() {
        let sql = "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json) OUTPUT inserted.id, inserted.revision";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("bounded MERGE OUTPUT should parse")
        else {
            panic!("expected MERGE DML");
        };
        assert_eq!(dml.returning, ["id", "revision"]);
        let commands = adapt_product_dml_batch(&dml).expect("MERGE should adapt");
        assert!(
            commands
                .iter()
                .all(|command| matches!(command, ProductDmlCommand::Upsert { .. }))
        );
    }

    #[test]
    fn sql_server_matched_update_output_lowers_to_inserted_product_returning() {
        let sql = "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}'), ('missing', '{\"ignored\":true}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json OUTPUT inserted.id, inserted.revision, inserted.document_json";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("matched MERGE OUTPUT should parse")
        else {
            panic!("expected MERGE DML");
        };
        assert_eq!(dml.returning, ["id", "revision", "document_json"]);
        let commands = adapt_product_dml_batch(&dml).expect("MERGE should adapt");
        assert!(
            commands
                .iter()
                .all(|command| matches!(command, ProductDmlCommand::MergeUpdate { .. }))
        );
    }

    #[test]
    fn sql_server_unmatched_insert_output_lowers_to_inserted_product_returning() {
        let sql = "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.id = s.id WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json) OUTPUT inserted.id, inserted.revision";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("unmatched MERGE OUTPUT should parse")
        else {
            panic!("expected MERGE DML");
        };
        assert_eq!(dml.returning, ["id", "revision"]);
        let commands = adapt_product_dml_batch(&dml).expect("MERGE should adapt");
        assert!(
            commands
                .iter()
                .all(|command| matches!(command, ProductDmlCommand::MergeCreateIfAbsent { .. }))
        );
    }

    #[test]
    fn sql_server_merge_output_rejects_unsupported_projections() {
        let sql = "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN DELETE OUTPUT inserted.id";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("MERGE should parse before action validation")
        else {
            panic!("expected MERGE DML");
        };
        let commands = adapt_product_dml_batch(&dml).expect("MERGE should adapt");
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, ProductDmlCommand::MergeDelete { .. }))
        );
        let deleted_output = "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN DELETE OUTPUT deleted.id, deleted.revision";
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(deleted_output)
            .expect("bounded deleted OUTPUT should parse")
        else {
            panic!("expected MERGE DML");
        };
        assert_eq!(dml.output_pseudo.as_deref(), Some("deleted"));
        assert_eq!(dml.returning, ["id", "revision"]);
        let action_sql = "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN DELETE OUTPUT $action";
        let NativeTransactionalStatement::Dml(action) =
            parse_native_transactional(action_sql).expect("bounded $action output should parse")
        else {
            panic!("expected MERGE DML");
        };
        assert_eq!(action.output_pseudo.as_deref(), Some("action"));
        assert_eq!(action.returning, ["action"]);
        let projection = "inserted.id INTO audit.rows";
        let sql = format!(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{{}}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN DELETE OUTPUT {projection}"
        );
        assert!(
            parse_native_transactional(&sql).is_err(),
            "unsafe MERGE OUTPUT shape was accepted: {projection}"
        );
    }

    #[test]
    fn sql_server_output_rejects_expressions_and_output_into() {
        for sql in [
            "UPDATE product.datasets SET document_json = '{}' OUTPUT inserted.id AS key WHERE id = 'ds-1' AND revision = 1",
            "DELETE FROM product.datasets OUTPUT deleted.id INTO audit.rows WHERE id = 'ds-1' AND revision = 1",
        ] {
            assert!(
                parse_native_transactional(sql).is_err(),
                "unsafe OUTPUT shape was accepted: {sql}"
            );
        }
    }

    #[test]
    fn product_update_accepts_a_target_alias_without_widening_the_contract() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "UPDATE product.datasets AS target SET document_json = '{\"name\":\"Orders\"}' WHERE id = 'ds-1' AND revision = 1 RETURNING id, revision",
        )
        .expect("an unambiguous target alias should parse")
        else {
            panic!("expected product UPDATE");
        };
        assert_eq!(dml.returning, ["id", "revision"]);
        assert!(adapt_product_dml(&dml).is_ok());
    }

    #[test]
    fn product_update_accepts_a_json_typed_document_literal() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "UPDATE product.datasets SET document_json = CAST('{\"name\":\"Orders\"}' AS JSON) WHERE id = 'ds-1' AND revision = 1",
        )
        .expect("JSON-typed document literals should parse")
        else {
            panic!("expected product UPDATE");
        };
        let commands = adapt_product_dml_batch(&dml).expect("JSON-typed document should adapt");
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            &commands[0],
            ProductDmlCommand::Update { document_json, .. }
                if document_json == r#"{"name":"Orders"}"#
        ));
    }

    #[test]
    fn product_update_and_delete_expand_bounded_id_lists_with_one_cas_revision() {
        let cases = [
            (
                "UPDATE product.datasets SET document_json = '{}' WHERE id IN ('one', 'two') AND revision = 3",
                2,
            ),
            (
                "DELETE FROM product.datasets WHERE revision = 3 AND id IN ('one', 'two')",
                2,
            ),
        ];
        for (sql, expected) in cases {
            let NativeTransactionalStatement::Dml(dml) =
                parse_native_transactional(sql).expect("bounded id-list DML should parse")
            else {
                panic!("expected product DML");
            };
            assert_eq!(
                adapt_product_dml_batch(&dml)
                    .expect("bounded id-list DML should adapt")
                    .len(),
                expected
            );
        }
        for sql in [
            "UPDATE product.datasets SET document_json = '{}' WHERE id IN ('one', 'one') AND revision = 3",
            "DELETE FROM product.datasets WHERE id IN ('one') AND revision IN (3)",
        ] {
            let NativeTransactionalStatement::Dml(dml) =
                parse_native_transactional(sql).expect("DML should parse before adaptation")
            else {
                panic!("expected product DML");
            };
            assert!(
                adapt_product_dml_batch(&dml).is_err(),
                "unsafe shape accepted: {sql}"
            );
        }
    }

    #[test]
    fn product_adapter_rejects_unbound_parameter_values() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ($1, $2)",
        )
        .unwrap() else {
            panic!("expected product DML")
        };
        assert!(adapt_product_dml(&dml).is_err());
    }

    #[test]
    fn adapts_only_exact_product_record_dml() {
        let parsed = parse_native_transactional(
            "INSERT INTO kaveon.product.datasets (id, document_json) VALUES ('ds-1', '{\"name\":\"Orders\"}')",
        ).unwrap();
        let NativeTransactionalStatement::Dml(dml) = parsed else {
            panic!("expected DML")
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Create {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"name\":\"Orders\"}".into(),
            }
        );
        for sql in [
            "UPDATE product.charts SET document_json = '{}' WHERE id = 'c-1' AND revision = 2",
            "DELETE FROM product.dashboards WHERE id = 'd-1' AND revision = 3",
            "INSERT INTO product.dlm_definitions (id, document_json) VALUES ('ds-1', '{}')",
            "INSERT INTO product.dlm_runs (id, document_json) VALUES ('run-1', '{}')",
            "INSERT INTO kaveon.product.typed_rows (id, document_json) VALUES ('row-1', '{}')",
        ] {
            let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(sql).unwrap()
            else {
                panic!("expected DML")
            };
            assert!(adapt_product_dml(&dml).is_ok());
        }
    }

    #[test]
    fn product_adapter_rejects_generic_rows_and_unsafe_shapes() {
        for sql in [
            "INSERT INTO app.users (id, document_json) VALUES ('u-1', '{}')",
            "INSERT INTO product.datasets (id, id) VALUES ('d-1', 'd-1')",
            "UPDATE product.datasets SET document_json = '{}' WHERE id = 'd-1' AND owner = 'alice'",
            "DELETE FROM product.datasets WHERE revision = 1 AND revision = 2",
        ] {
            let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(sql).unwrap()
            else {
                panic!("expected DML")
            };
            assert!(
                adapt_product_dml(&dml).is_err(),
                "unexpectedly adapted {sql}"
            );
        }
    }

    #[test]
    fn product_insert_resolves_columns_by_name() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (document_json, id) VALUES ('{\"name\":\"Orders\"}', 'ds-1')",
        )
        .unwrap() else {
            panic!("expected product DML");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Create {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"name\":\"Orders\"}".into(),
            }
        );
    }

    #[test]
    fn typed_row_insert_supports_bounded_conflict_ignore() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO kaveon.product.typed_rows (id, document_json) VALUES ('row-1', '{\"table\":\"app.users\",\"primary_key\":\"row-1\",\"revision\":1,\"owner_principal\":\"alice\",\"columns\":{}}') ON CONFLICT DO NOTHING",
        )
        .expect("typed-row conflict-ignore should parse")
        else {
            panic!("expected typed-row DML");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::CreateIgnore {
                kind: "typed_row".into(),
                id: "row-1".into(),
                document_json: "{\"table\":\"app.users\",\"primary_key\":\"row-1\",\"revision\":1,\"owner_principal\":\"alice\",\"columns\":{}}".into(),
            }
        );
    }

    #[test]
    fn typed_row_insert_supports_bounded_conflict_update() {
        let document = "{\"table\":\"app.users\",\"primary_key\":\"row-1\",\"revision\":1,\"owner_principal\":\"alice\",\"columns\":{}}";
        let sql = format!(
            "INSERT INTO kaveon.product.typed_rows (id, document_json) VALUES ('row-1', '{document}') ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json"
        );
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(&sql).expect("typed-row conflict-update should parse")
        else {
            panic!("expected typed-row DML");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Upsert {
                kind: "typed_row".into(),
                id: "row-1".into(),
                document_json: document.into(),
            }
        );
    }

    #[test]
    fn product_insert_supports_bounded_on_conflict_do_nothing() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') ON CONFLICT DO NOTHING",
        )
        .expect("bounded conflict-ignore insert should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::CreateIgnore {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{}".into(),
            }
        );
        let NativeTransactionalStatement::Dml(returning) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') ON CONFLICT DO NOTHING RETURNING id, revision, document_json"
        )
        .expect("bounded conflict-ignore returning should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(returning.returning, ["id", "revision", "document_json"]);
        assert!(parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') ON CONFLICT (owner_principal) DO NOTHING"
        )
        .is_err());
        assert!(parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') ON CONFLICT ON CONSTRAINT datasets_pkey DO NOTHING"
        )
        .is_ok());
        assert!(parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') ON CONFLICT ON CONSTRAINT owner_unique DO NOTHING"
        )
        .is_err());
    }

    #[test]
    fn product_insert_supports_bounded_on_conflict_do_update() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{\"name\":\"new\"}') ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json",
        )
        .expect("bounded conflict upsert should parse")
        else {
            panic!("expected product DML");
        };
        assert!(dml.on_conflict_do_update);
        assert!(!dml.on_conflict_do_nothing);
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Upsert {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"name\":\"new\"}".into(),
            }
        );
        let named = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') ON CONFLICT ON CONSTRAINT datasets_pkey DO UPDATE SET document_json = excluded.document_json",
        )
        .expect("named primary-key conflict target should remain bounded");
        let NativeTransactionalStatement::Dml(named) = named else {
            panic!("expected product DML");
        };
        assert!(named.on_conflict_do_update);
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{\"name\":\"new\"}') ON CONFLICT (id) DO UPDATE SET document_json = CAST(excluded.document_json AS JSON)",
        )
        .expect("typed conflict upsert should parse")
        else {
            panic!("expected product DML");
        };
        assert!(dml.on_conflict_do_update);
        assert!(matches!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Upsert { .. }
        ));
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{\"name\":\"new\"}') ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json WHERE TRUE",
        )
        .expect("literal TRUE conflict predicate should remain bounded")
        else {
            panic!("expected product DML");
        };
        assert!(dml.on_conflict_do_update);
        assert!(matches!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Upsert { .. }
        ));
        for sql in [
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json WHERE id = excluded.id",
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json WHERE excluded.id = id",
        ] {
            let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(sql)
                .expect("conflict identity predicate should remain bounded")
            else {
                panic!("expected product DML");
            };
            assert!(dml.on_conflict_do_update);
        }
        for sql in [
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') ON CONFLICT (owner_principal) DO UPDATE SET document_json = excluded.document_json",
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{}') ON CONFLICT (id) DO UPDATE SET document_json = excluded.document_json WHERE id = 'ds-1'",
        ] {
            assert!(
                parse_native_transactional(sql).is_err(),
                "unexpectedly accepted unsafe upsert: {sql}"
            );
        }
        let NativeTransactionalStatement::Dml(literal) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('ds-1', '{\"name\":\"insert\"}') ON CONFLICT (id) DO UPDATE SET document_json = '{\"name\":\"conflict\"}'",
        )
        .expect("literal conflict document should remain bounded")
        else {
            panic!("expected product DML");
        };
        assert_eq!(
            adapt_product_dml(&literal).unwrap(),
            ProductDmlCommand::Upsert {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"name\":\"conflict\"}".into(),
            }
        );
    }

    #[test]
    fn product_merge_supports_bounded_update_or_insert() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{\"name\":\"Orders\"}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
        )
        .expect("bounded MERGE should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(dml.kind, NativeDmlKind::Merge);
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Upsert {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"name\":\"Orders\"}".into(),
            }
        );
    }

    #[test]
    fn product_merge_accepts_typed_literal_not_matched_document() {
        let sql = "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.id = s.id WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, CAST('{\"created\":true}' AS JSON))";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("typed literal MERGE insert should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::MergeCreateIfAbsent {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"created\":true}".into(),
            }
        );
    }

    #[test]
    fn product_merge_accepts_a_typed_source_document_assignment() {
        let sql = "MERGE INTO product.datasets AS target USING (VALUES ('ds-1', '{\"name\":\"Orders\"}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED THEN UPDATE SET target.document_json = CAST(source.document_json AS JSON) WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, CAST(source.document_json AS JSON))";
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(sql)
            .expect("typed source document assignment should remain bounded MERGE")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Upsert {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"name\":\"Orders\"}".into(),
            }
        );
    }

    #[test]
    fn product_merge_accepts_typed_literal_document_assignments() {
        let matched = "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = CAST('{\"archived\":true}' AS JSON)";
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(matched)
            .expect("typed literal matched assignment should remain bounded MERGE")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::MergeUpdate {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"archived\":true}".into(),
            }
        );

        let not_matched = "MERGE INTO product.datasets AS t USING (VALUES ('keep', '{\"name\":\"kept\"}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json) WHEN NOT MATCHED BY SOURCE THEN UPDATE SET document_json = CAST('{\"archived\":true}' AS JSON)";
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(not_matched)
            .expect("typed literal not-matched assignment should remain bounded MERGE")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap().last(),
            Some(&ProductDmlCommand::MergeUpdateNotMatched {
                kind: "dataset".into(),
                source_ids: vec!["keep".into()],
                target_ids: None,
                document_json: "{\"archived\":true}".into(),
            })
        );
    }

    #[test]
    fn product_merge_accepts_a_qualified_target_document_assignment() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{\"name\":\"Orders\"}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
        )
        .expect("qualified target assignment should remain bounded MERGE") else {
            panic!("expected product MERGE DML");
        };
        assert!(matches!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Upsert { .. }
        ));
    }

    #[test]
    fn product_merge_accepts_an_unaliased_target_with_derived_qualifier() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets USING (VALUES ('ds-1', '{\"name\":\"Orders\"}')) AS s (id, document_json) ON datasets.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
        )
        .expect("an unaliased bounded MERGE target should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Upsert {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"name\":\"Orders\"}".into(),
            }
        );
    }

    #[test]
    fn product_merge_expands_a_bounded_values_batch() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('one', '{}'), ('two', '{\"n\":2}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
        )
        .expect("bounded MERGE batch should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![
                ProductDmlCommand::Upsert {
                    kind: "dataset".into(),
                    id: "one".into(),
                    document_json: "{}".into(),
                },
                ProductDmlCommand::Upsert {
                    kind: "dataset".into(),
                    id: "two".into(),
                    document_json: "{\"n\":2}".into(),
                },
            ]
        );
    }

    #[test]
    fn product_merge_supports_bounded_delete_or_insert() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{\"name\":\"Orders\"}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN DELETE WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
        )
        .expect("bounded delete-or-insert MERGE should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::MergeDeleteOrInsert {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"name\":\"Orders\"}".into(),
            }
        );
    }

    #[test]
    fn product_merge_supports_bounded_matched_delete() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}'), ('ds-2', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN DELETE",
        )
        .expect("bounded matched-delete MERGE should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![
                ProductDmlCommand::MergeDelete {
                    kind: "dataset".into(),
                    id: "ds-1".into(),
                },
                ProductDmlCommand::MergeDelete {
                    kind: "dataset".into(),
                    id: "ds-2".into(),
                },
            ]
        );
    }

    #[test]
    fn product_merge_supports_bounded_matched_update() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{\"name\":\"Orders v2\"}'), ('ds-2', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json",
        )
        .expect("bounded matched-update MERGE should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![
                ProductDmlCommand::MergeUpdate {
                    kind: "dataset".into(),
                    id: "ds-1".into(),
                    document_json: "{\"name\":\"Orders v2\"}".into(),
                },
                ProductDmlCommand::MergeUpdate {
                    kind: "dataset".into(),
                    id: "ds-2".into(),
                    document_json: "{}".into(),
                },
            ]
        );
    }

    #[test]
    fn product_merge_supports_bounded_literal_matched_update() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}'), ('ds-2', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = '{\"archived\":true}'",
        )
        .expect("bounded literal matched-update MERGE should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![
                ProductDmlCommand::MergeUpdate {
                    kind: "dataset".into(),
                    id: "ds-1".into(),
                    document_json: "{\"archived\":true}".into(),
                },
                ProductDmlCommand::MergeUpdate {
                    kind: "dataset".into(),
                    id: "ds-2".into(),
                    document_json: "{\"archived\":true}".into(),
                },
            ]
        );
    }

    #[test]
    fn product_merge_supports_bounded_matched_source_filter() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}'), ('ds-2', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED AND s.id IN ('ds-2') THEN UPDATE SET document_json = s.document_json",
        )
        .expect("bounded matched source filter should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![ProductDmlCommand::MergeUpdate {
                kind: "dataset".into(),
                id: "ds-2".into(),
                document_json: "{}".into(),
            }]
        );
    }

    #[test]
    fn product_merge_supports_bounded_matched_target_filter() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}'), ('ds-2', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED AND t.id = 'ds-2' THEN UPDATE SET document_json = s.document_json",
        )
        .expect("bounded matched target filter should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![ProductDmlCommand::MergeUpdate {
                kind: "dataset".into(),
                id: "ds-2".into(),
                document_json: "{}".into(),
            }]
        );
    }

    #[test]
    fn product_merge_supports_bounded_matched_delete_source_filter() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}'), ('ds-2', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED AND s.id = 'ds-1' THEN DELETE",
        )
        .expect("bounded matched delete source filter should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![ProductDmlCommand::MergeDelete {
                kind: "dataset".into(),
                id: "ds-1".into(),
            }]
        );
    }

    #[test]
    fn product_merge_supports_bounded_unmatched_insert() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{\"name\":\"Orders\"}')) AS s (id, document_json) ON t.id = s.id WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
        )
        .expect("bounded unmatched-insert MERGE should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::MergeCreateIfAbsent {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"name\":\"Orders\"}".into(),
            }
        );
        let NativeTransactionalStatement::Dml(identity_predicate) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{\"name\":\"Orders\"}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED AND t.id = s.id THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
        )
        .expect("redundant MERGE identity predicate should remain bounded")
        else {
            panic!("expected product MERGE");
        };
        assert!(matches!(
            adapt_product_dml(&identity_predicate).unwrap(),
            ProductDmlCommand::Upsert { .. }
        ));

        let NativeTransactionalStatement::Dml(filtered) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('skip', '{}'), ('keep', '{\"name\":\"Kept\"}')) AS s (id, document_json) ON t.id = s.id WHEN NOT MATCHED AND s.id = 'keep' THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
        )
        .expect("bounded unmatched-insert source filter should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&filtered).unwrap(),
            vec![ProductDmlCommand::MergeCreateIfAbsent {
                kind: "dataset".into(),
                id: "keep".into(),
                document_json: "{\"name\":\"Kept\"}".into(),
            }]
        );
        let NativeTransactionalStatement::Dml(target_predicate) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('keep', '{}')) AS s (id, document_json) ON t.id = s.id WHEN NOT MATCHED AND t.id = 'keep' THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
        )
        .expect("target predicate should parse before bounded adaptation")
        else {
            panic!("expected product MERGE");
        };
        let target_predicate = adapt_product_dml(&target_predicate)
            .expect_err("insert-only MERGE must not expose a target row");
        assert!(
            target_predicate
                .to_string()
                .contains("target.id is not available")
        );
    }

    #[test]
    fn product_merge_accepts_explicit_not_matched_by_target() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{\"name\":\"Orders\"}')) AS s (id, document_json) ON t.id = s.id WHEN NOT MATCHED BY TARGET THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
        )
        .expect("explicit SQL Server target arm should parse")
        else {
            panic!("expected native DML");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::MergeCreateIfAbsent {
                kind: "dataset".to_owned(),
                id: "ds-1".to_owned(),
                document_json: "{\"name\":\"Orders\"}".to_owned(),
            }
        );
    }

    #[test]
    fn product_merge_supports_bounded_not_matched_by_source_delete() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('keep', '{\"name\":\"kept\"}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json) WHEN NOT MATCHED BY SOURCE THEN DELETE",
        )
        .expect("bounded not-matched-by-source MERGE should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![
                ProductDmlCommand::Upsert {
                    kind: "dataset".into(),
                    id: "keep".into(),
                    document_json: "{\"name\":\"kept\"}".into(),
                },
                ProductDmlCommand::MergeDeleteNotMatched {
                    kind: "dataset".into(),
                    source_ids: vec!["keep".into()],
                    target_ids: None,
                },
            ]
        );
    }

    #[test]
    fn product_merge_supports_bounded_not_matched_by_source_update() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS t USING (VALUES ('keep', '{\"name\":\"kept\"}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json) WHEN NOT MATCHED BY SOURCE THEN UPDATE SET document_json = '{\"archived\":true}'",
        )
        .expect("bounded not-matched-by-source update MERGE should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![
                ProductDmlCommand::Upsert {
                    kind: "dataset".into(),
                    id: "keep".into(),
                    document_json: "{\"name\":\"kept\"}".into(),
                },
                ProductDmlCommand::MergeUpdateNotMatched {
                    kind: "dataset".into(),
                    source_ids: vec!["keep".into()],
                    target_ids: None,
                    document_json: "{\"archived\":true}".into(),
                },
            ]
        );
    }

    #[test]
    fn product_merge_supports_bounded_not_matched_by_source_target_filter() {
        let sql = "MERGE INTO product.datasets AS t USING (VALUES ('keep', '{\"name\":\"kept\"}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json) WHEN NOT MATCHED BY SOURCE AND t.id IN ('stale-1', 'stale-2') THEN DELETE";
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(sql)
            .expect("bounded target-filtered not-matched-by-source MERGE should parse")
        else {
            panic!("expected product MERGE");
        };
        assert!(matches!(
            adapt_product_dml_batch(&dml).unwrap().last(),
            Some(ProductDmlCommand::MergeDeleteNotMatched {
                target_ids: Some(ids),
                ..
            }) if ids == &vec!["stale-1".to_owned(), "stale-2".to_owned()]
        ));
    }

    #[test]
    fn product_merge_supports_filtered_matched_update_with_insert_fallback() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS target USING (VALUES ('update-me', '{\"changed\":true}'), ('new-row', '{\"created\":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED AND source.id IN ('update-me') THEN UPDATE SET document_json = source.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json)",
        )
        .expect("filtered matched MERGE should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![
                ProductDmlCommand::MergeUpdateOrCreateFiltered {
                    kind: "dataset".into(),
                    id: "update-me".into(),
                    document_json: "{\"changed\":true}".into(),
                    matched_ids: vec!["update-me".into()],
                },
                ProductDmlCommand::MergeUpdateOrCreateFiltered {
                    kind: "dataset".into(),
                    id: "new-row".into(),
                    document_json: "{\"created\":true}".into(),
                    matched_ids: vec!["update-me".into()],
                },
            ]
        );
    }

    #[test]
    fn product_merge_supports_filtered_matched_delete_with_insert_fallback() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "MERGE INTO product.datasets AS target USING (VALUES ('delete-me', '{\"deleted\":true}'), ('keep-existing', '{\"keep\":true}'), ('new-row', '{\"created\":true}')) AS source (id, document_json) ON target.id = source.id WHEN MATCHED AND source.id IN ('delete-me') THEN DELETE WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (source.id, source.document_json)",
        )
        .expect("filtered matched DELETE MERGE should parse")
        else {
            panic!("expected product MERGE");
        };
        assert_eq!(
            adapt_product_dml_batch(&dml).unwrap(),
            vec![
                ProductDmlCommand::MergeDeleteOrCreateFiltered {
                    kind: "dataset".into(),
                    id: "delete-me".into(),
                    document_json: "{\"deleted\":true}".into(),
                    matched_ids: vec!["delete-me".into()],
                },
                ProductDmlCommand::MergeDeleteOrCreateFiltered {
                    kind: "dataset".into(),
                    id: "keep-existing".into(),
                    document_json: "{\"keep\":true}".into(),
                    matched_ids: vec!["delete-me".into()],
                },
                ProductDmlCommand::MergeDeleteOrCreateFiltered {
                    kind: "dataset".into(),
                    id: "new-row".into(),
                    document_json: "{\"created\":true}".into(),
                    matched_ids: vec!["delete-me".into()],
                },
            ]
        );
    }

    #[test]
    fn sql_server_filtered_delete_merge_output_lowers_to_deleted_returning() {
        let sql = "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED AND s.id IN ('ds-1') THEN DELETE WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json) OUTPUT deleted.id, deleted.revision, deleted.document_json";
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(sql).expect("filtered MERGE OUTPUT should parse")
        else {
            panic!("expected MERGE DML");
        };
        assert_eq!(dml.output_pseudo.as_deref(), Some("deleted"));
        assert_eq!(dml.returning, ["id", "revision", "document_json"]);
        let commands = adapt_product_dml_batch(&dml).expect("filtered MERGE should adapt");
        assert!(commands.iter().all(|command| matches!(
            command,
            ProductDmlCommand::MergeDeleteOrCreateFiltered { .. }
        )));
    }

    #[test]
    fn product_merge_rejects_unbounded_or_ambiguous_shapes() {
        for sql in [
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.owner_principal = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED AND t.id = s.id AND s.id = 'ds-1' THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json)",
            "MERGE INTO product.datasets AS t USING (VALUES ('ds-1', '{}')) AS s (id, document_json) ON t.id = s.id WHEN MATCHED AND s.id IN ('ds-1') THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json) WHEN NOT MATCHED BY SOURCE THEN DELETE",
        ] {
            let NativeTransactionalStatement::Dml(dml) =
                parse_native_transactional(sql).expect("MERGE shape should parse")
            else {
                panic!("expected product MERGE");
            };
            assert!(
                adapt_product_dml(&dml).is_err(),
                "unexpectedly accepted unsupported MERGE: {sql}"
            );
        }
        let rows = (0..=1000)
            .map(|i| format!("('ds-{i}', '{{}}')"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "MERGE INTO product.datasets AS t USING (VALUES {rows}) AS s (id, document_json) ON t.id = s.id WHEN MATCHED THEN UPDATE SET document_json = s.document_json WHEN NOT MATCHED THEN INSERT (id, document_json) VALUES (s.id, s.document_json)"
        );
        let NativeTransactionalStatement::Dml(dml) =
            parse_native_transactional(&sql).expect("large MERGE shape should parse")
        else {
            panic!("expected product MERGE");
        };
        let error =
            adapt_product_dml_batch(&dml).expect_err("MERGE source over the cap must fail closed");
        assert!(error.to_string().contains("at most 1000"), "{error}");
    }

    #[test]
    fn product_insert_batch_expands_rows_without_sql_interpolation() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) VALUES ('one', '{}'), ('two', '{\"n\":2}')",
        )
        .unwrap() else {
            panic!("expected product DML");
        };
        let commands = adapt_product_dml_batch(&dml).unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[0],
            ProductDmlCommand::Create {
                kind: "dataset".into(),
                id: "one".into(),
                document_json: "{}".into(),
            }
        );
        assert_eq!(
            commands[1],
            ProductDmlCommand::Create {
                kind: "dataset".into(),
                id: "two".into(),
                document_json: "{\"n\":2}".into(),
            }
        );
        assert!(adapt_product_dml(&dml).is_err());
    }

    #[test]
    fn product_insert_select_accepts_one_constant_row() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (document_json, id) SELECT '{\"name\":\"Orders\"}', 'ds-1'",
        )
        .expect("constant product INSERT SELECT should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::Create {
                kind: "dataset".into(),
                id: "ds-1".into(),
                document_json: "{\"name\":\"Orders\"}".into(),
            }
        );
    }

    #[test]
    fn product_insert_select_rejects_source_queries_and_modifiers() {
        for sql in [
            "INSERT INTO product.datasets (id, document_json) SELECT id, document_json FROM source.datasets",
            "INSERT INTO product.datasets (id, document_json) SELECT DISTINCT 'ds-1', '{}'",
            "INSERT INTO product.datasets (id, document_json) SELECT 'ds-1', '{}' LIMIT 1",
            "INSERT INTO product.datasets (id, document_json) SELECT a.id, a.document_json FROM product.datasets a, product.datasets b",
        ] {
            let NativeTransactionalStatement::Dml(dml) =
                parse_native_transactional(sql).expect("SELECT shape should parse")
            else {
                panic!("expected product DML");
            };
            let error = adapt_product_dml(&dml).expect_err("unsupported SELECT must fail closed");
            assert!(
                error.to_string().contains("constant SELECT")
                    || error.to_string().contains("product INSERT SELECT"),
                "{sql}: {error}"
            );
        }
    }

    #[test]
    fn product_insert_select_accepts_bounded_product_copy() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) SELECT id, CAST(document_json AS JSON) FROM product.datasets ON CONFLICT DO NOTHING",
        )
        .expect("product source copy should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::InsertSelect {
                kind: "dataset".into(),
                source_kind: "dataset".into(),
                source_ids: None,
                on_conflict_do_nothing: true,
                on_conflict_do_update: false,
                returning: Vec::new(),
            }
        );
    }

    #[test]
    fn product_insert_select_rejects_joins() {
        let sql = "INSERT INTO product.datasets (id, document_json) SELECT a.id, a.document_json FROM product.datasets a JOIN product.datasets b ON a.id = b.id";
        let parsed = parse_native_transactional(sql);
        if let Ok(NativeTransactionalStatement::Dml(dml)) = parsed {
            assert!(adapt_product_dml(&dml).is_err(), "{sql}");
        }
    }

    #[test]
    fn product_insert_select_accepts_bounded_unfiltered_returning() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) SELECT id, document_json FROM product.datasets RETURNING id, revision, document_json",
        )
        .expect("bounded source RETURNING should parse without an id filter")
        else {
            panic!("expected product DML");
        };
        assert_eq!(dml.returning, ["id", "revision", "document_json"]);
        let commands = adapt_product_dml_batch(&dml).expect("source RETURNING should adapt");
        assert!(matches!(
            &commands[0],
            ProductDmlCommand::InsertSelect {
                source_ids: None,
                returning,
                ..
            } if returning == &["id".to_owned(), "revision".to_owned(), "document_json".to_owned()]
        ));
    }

    #[test]
    fn product_insert_select_accepts_bounded_identity_returning() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) SELECT id, document_json FROM product.datasets WHERE id IN ('one') RETURNING *",
        )
        .expect("bounded source RETURNING should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(dml.returning, ["id", "revision", "document_json"]);
        let commands =
            adapt_product_dml_batch(&dml).expect("bounded source RETURNING should adapt");
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            &commands[0],
            ProductDmlCommand::InsertSelect {
                source_ids: Some(ids),
                returning,
                ..
            } if ids == &["one".to_owned()]
                && returning == &["id".to_owned(), "revision".to_owned(), "document_json".to_owned()]
        ));
    }

    #[test]
    fn product_insert_select_accepts_bounded_id_filter() {
        let NativeTransactionalStatement::Dml(dml) = parse_native_transactional(
            "INSERT INTO product.datasets (id, document_json) SELECT id, document_json FROM product.datasets WHERE id IN ('one', 'two') ON CONFLICT DO NOTHING",
        )
        .expect("bounded source filter should parse")
        else {
            panic!("expected product DML");
        };
        assert_eq!(
            adapt_product_dml(&dml).unwrap(),
            ProductDmlCommand::InsertSelect {
                kind: "dataset".into(),
                source_kind: "dataset".into(),
                source_ids: Some(vec!["one".into(), "two".into()]),
                on_conflict_do_nothing: true,
                on_conflict_do_update: false,
                returning: Vec::new(),
            }
        );
    }
}
