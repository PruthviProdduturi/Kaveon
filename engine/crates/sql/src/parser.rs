use kaveon_core::{KaveonError, Result};
use sqlparser::ast::Statement;
use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, Expr, FromTable, SetExpr, TableFactor, TableWithJoins, Value,
};
use sqlparser::dialect::{BigQueryDialect, DuckDbDialect, MsSqlDialect};
use sqlparser::parser::Parser;

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
    let json_clauses_normalized = normalize_json_null_clauses(&bracket_normalized)?;
    let json_unique_normalized = normalize_json_object_unique_keys(&json_clauses_normalized)?;
    let json_returning_normalized = normalize_json_returning(&json_unique_normalized);
    let normalized = normalize_lateral_unnest(&json_returning_normalized);
    // Lower the one recursive shape that has a finite, deterministic
    // equivalent in the existing UNNEST/SEQUENCE execution path.  More
    // general recursive graphs remain explicitly unsupported in the logical
    // planner rather than being accepted with unsafe or incomplete semantics.
    let normalized = normalize_bounded_linear_recursive_cte(&normalized)
        .unwrap_or_else(|| normalized.to_owned());
    // BigQuery's grammar retains a table-shaped UNNEST node when the function
    // is correlated. DuckDB parses the same spelling as a generic function,
    // which cannot carry ordinality metadata.
    if contains_lateral_unnest(sql)
        && let Ok(statements) = Parser::parse_sql(&BigQueryDialect {}, &normalized)
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
        Err(error) => Err(KaveonError::Sql(error.to_string())),
    }
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
                    "TINYINT" | "SMALLINT" | "BIGINT" | "INTEGER" | "DOUBLE" | "REAL"
                    | "BOOLEAN" | "VARCHAR" | "DATE" | "TIMESTAMP" | "JSON" => upper_type,
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
            if identifier {
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativeDmlStatement {
    pub kind: NativeDmlKind,
    pub table: String,
    pub ast: Statement,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NativeTransactionalStatement {
    Begin,
    Commit,
    Rollback,
    Dml(Box<NativeDmlStatement>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductDmlCommand {
    Create {
        kind: String,
        id: String,
        document_json: String,
    },
    Update {
        kind: String,
        id: String,
        expected_revision: u64,
        document_json: String,
    },
    Delete {
        kind: String,
        id: String,
        expected_revision: u64,
    },
}

/// Adapt only Kaveon's typed product-record SQL facade. Arbitrary row DML has
/// no storage contract and is rejected instead of being silently emulated.
pub fn adapt_product_dml(dml: &NativeDmlStatement) -> Result<ProductDmlCommand> {
    let mut commands = adapt_product_dml_batch(dml)?;
    if commands.len() != 1 {
        return Err(sql_error(
            "product INSERT accepts one record per statement; use the transaction batch API for multiple records",
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
            if columns.len() != 2
                || !columns[0].eq_ignore_ascii_case("id")
                || !columns[1].eq_ignore_ascii_case("document_json")
            {
                return Err(sql_error(
                    "product INSERT columns must be (id, document_json)",
                ));
            }
            let Some(SetExpr::Values(values)) =
                insert.source.as_deref().map(|query| query.body.as_ref())
            else {
                return Err(sql_error("product INSERT requires VALUES"));
            };
            if values.rows.is_empty() || values.rows.iter().any(|row| row.len() != 2) {
                return Err(sql_error(
                    "product INSERT rows must contain id and document_json",
                ));
            }
            values
                .rows
                .iter()
                .map(|row| {
                    Ok(ProductDmlCommand::Create {
                        kind: kind.clone(),
                        id: string_literal(&row[0], "product ID")?,
                        document_json: string_literal(&row[1], "product document_json")?,
                    })
                })
                .collect()
        }
        Statement::Update {
            assignments,
            selection,
            ..
        } => {
            if assignments.len() != 1 {
                return Err(sql_error("product UPDATE may set document_json only"));
            }
            let AssignmentTarget::ColumnName(column) = &assignments[0].target else {
                return Err(sql_error("product UPDATE may set document_json only"));
            };
            if column.0.len() != 1 || !column.0[0].value.eq_ignore_ascii_case("document_json") {
                return Err(sql_error("product UPDATE may set document_json only"));
            }
            let document_json = string_literal(&assignments[0].value, "product document_json")?;
            let (id, expected_revision) = product_key_predicate(selection.as_ref())?;
            Ok(vec![ProductDmlCommand::Update {
                kind,
                id,
                expected_revision,
                document_json,
            }])
        }
        Statement::Delete(delete) => {
            let (id, expected_revision) = product_key_predicate(delete.selection.as_ref())?;
            Ok(vec![ProductDmlCommand::Delete {
                kind,
                id,
                expected_revision,
            }])
        }
        _ => Err(sql_error("statement is not product DML")),
    }
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

fn product_key_predicate(selection: Option<&Expr>) -> Result<(String, u64)> {
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
    let mut id = None;
    let mut revision = None;
    for expression in [left.as_ref(), right.as_ref()] {
        let Expr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } = expression
        else {
            return Err(sql_error(
                "product UPDATE/DELETE requires equality predicates",
            ));
        };
        let Expr::Identifier(column) = left.as_ref() else {
            return Err(sql_error(
                "product key predicates require unqualified columns",
            ));
        };
        if column.value.eq_ignore_ascii_case("id") {
            id = Some(string_literal(right, "product ID")?);
        } else if column.value.eq_ignore_ascii_case("revision") {
            revision = Some(integer_literal(right, "expected revision")?);
        } else {
            return Err(sql_error(
                "product key predicates support id and revision only",
            ));
        }
    }
    match (id, revision) {
        (Some(id), Some(revision)) if revision > 0 => Ok((id, revision)),
        _ => Err(sql_error(
            "product UPDATE/DELETE requires one id and one positive revision",
        )),
    }
}

fn string_literal(expression: &Expr, label: &str) -> Result<String> {
    match expression {
        Expr::Value(Value::SingleQuotedString(value)) => Ok(value.clone()),
        _ => Err(sql_error(&format!("{label} must be a string literal"))),
    }
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
    let mut statements = parse_sql(sql)?;
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
        Statement::Rollback { .. } => Err(sql_error(
            "ROLLBACK TO SAVEPOINT and ROLLBACK AND CHAIN are not supported",
        )),
        Statement::Savepoint { .. } | Statement::ReleaseSavepoint { .. } => Err(sql_error(
            "SAVEPOINT and RELEASE SAVEPOINT are not supported",
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
                Some(SetExpr::Values(_))
            ) {
                return Err(sql_error("INSERT supports VALUES only"));
            }
            if insert.table_alias.is_some()
                || insert.or.is_some()
                || insert.ignore
                || insert.overwrite
                || insert.partitioned.is_some()
                || !insert.after_columns.is_empty()
                || insert.table
                || insert.on.is_some()
                || insert.returning.is_some()
                || insert.replace_into
                || insert.priority.is_some()
                || insert.insert_alias.is_some()
            {
                return Err(sql_error(
                    "INSERT aliases, conflict clauses, modifiers, partitions, and RETURNING are not supported",
                ));
            }
            let table = validated_table_name(&insert.table_name)?;
            Ok(NativeTransactionalStatement::Dml(Box::new(
                NativeDmlStatement {
                    kind: NativeDmlKind::Insert,
                    table,
                    ast: statement,
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
            if from.is_some() || returning.is_some() || or.is_some() {
                return Err(sql_error(
                    "UPDATE FROM, conflict clauses, and RETURNING are not supported",
                ));
            }
            if selection.is_none() {
                return Err(sql_error("UPDATE requires a WHERE predicate"));
            }
            if assignments.is_empty()
                || assignments.iter().any(|assignment| {
                    !matches!(&assignment.target, AssignmentTarget::ColumnName(name) if name.0.len() == 1)
                })
            {
                return Err(sql_error("UPDATE requires simple single-column assignments"));
            }
            Ok(NativeTransactionalStatement::Dml(Box::new(
                NativeDmlStatement {
                    kind: NativeDmlKind::Update,
                    table: table_name,
                    ast: statement,
                },
            )))
        }
        Statement::Delete(delete) => {
            let tables = match &delete.from {
                FromTable::WithFromKeyword(tables) => tables,
                FromTable::WithoutKeyword(_) => {
                    return Err(sql_error("DELETE requires the FROM keyword"));
                }
            };
            if tables.len() != 1
                || !delete.tables.is_empty()
                || delete.using.is_some()
                || delete.returning.is_some()
                || !delete.order_by.is_empty()
                || delete.limit.is_some()
            {
                return Err(sql_error(
                    "DELETE supports one table and no USING, RETURNING, ORDER BY, or LIMIT",
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
                },
            )))
        }
        _ => Err(sql_error(
            "native transaction parser accepts INSERT, UPDATE, DELETE, BEGIN, COMMIT, or ROLLBACK only",
        )),
    }
}

fn simple_table(table: &TableWithJoins) -> Result<String> {
    if !table.joins.is_empty() {
        return Err(sql_error("write targets cannot contain joins"));
    }
    match &table.relation {
        TableFactor::Table {
            name,
            alias: None,
            args: None,
            with_hints,
            version: None,
            with_ordinality: false,
            partitions,
            json_path: None,
        } if with_hints.is_empty() && partitions.is_empty() => validated_table_name(name),
        _ => Err(sql_error("write target must be an unaliased base table")),
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
        // A bracket inside a string literal is data, not an identifier.
        assert_eq!(
            normalize_bracket_identifiers("SELECT '[year]' FROM t"),
            "SELECT '[year]' FROM t"
        );
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
    fn rejects_unbounded_or_extended_write_forms() {
        let cases = [
            "INSERT INTO users VALUES (1)",
            "INSERT INTO users (id) SELECT id FROM old_users",
            "INSERT INTO users (id) VALUES (1) RETURNING id",
            "UPDATE users SET name = 'Ada'",
            "UPDATE users SET name = old.name FROM old_users old WHERE users.id = old.id",
            "DELETE FROM users",
            "DELETE FROM users USING old_users WHERE users.id = old_users.id",
            "ROLLBACK TO SAVEPOINT before_write",
            "COMMIT AND CHAIN",
            "SAVEPOINT before_write",
            "RELEASE SAVEPOINT before_write",
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
                "SAVEPOINT checkpoint",
                "SAVEPOINT and RELEASE SAVEPOINT are not supported",
            ),
            (
                "RELEASE SAVEPOINT checkpoint",
                "SAVEPOINT and RELEASE SAVEPOINT are not supported",
            ),
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
            "INSERT INTO product.datasets (document_json, id) VALUES ('{}', 'd-1')",
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
}
