//! The interactive row limit. A `SELECT` or `WITH` statement with no
//! top-level `LIMIT` gets one appended so a stray `SELECT *` over a large
//! table shows its first rows instead of shipping the table. Results page
//! in the shell, so an explicit LIMIT of any size is sent as written and
//! `.limit off` turns the implicit one off. Scripts (`-e`, `-f`, piped
//! input) are never limited.
use sqlparser::dialect::GenericDialect;
use sqlparser::tokenizer::{Token, Tokenizer};

pub const DEFAULT_ROW_LIMIT: usize = 1_000;

/// `None` is `.limit off`: no implicit LIMIT.
pub type RowLimit = Option<usize>;

pub enum Limited {
    /// Not a query, or a query the shell leaves alone.
    Unchanged,
    /// The statement with ` LIMIT n` appended.
    Appended(String),
    /// The statement has its own top-level LIMIT (or FETCH), sent as written.
    Explicit,
}

pub fn inspect(statement: &str, limit: RowLimit) -> Limited {
    let Ok(tokens) = Tokenizer::new(&GenericDialect {}, statement).tokenize() else {
        return Limited::Unchanged;
    };
    let tokens: Vec<Token> = tokens
        .into_iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .collect();
    let first = match tokens.first() {
        Some(Token::Word(word)) if word.quote_style.is_none() => word.value.to_ascii_uppercase(),
        _ => return Limited::Unchanged,
    };
    if first != "SELECT" && first != "WITH" {
        return Limited::Unchanged;
    }
    let mut depth = 0usize;
    for token in &tokens {
        match token {
            Token::LParen => depth += 1,
            Token::RParen => depth = depth.saturating_sub(1),
            Token::Word(word)
                if depth == 0
                    && word.quote_style.is_none()
                    && word.value.eq_ignore_ascii_case("LIMIT") =>
            {
                return Limited::Explicit;
            }
            Token::Word(word)
                if depth == 0
                    && word.quote_style.is_none()
                    && word.value.eq_ignore_ascii_case("FETCH") =>
            {
                return Limited::Explicit;
            }
            _ => {}
        }
    }
    match limit {
        Some(limit) => Limited::Appended(format!("{} LIMIT {limit}", statement.trim_end())),
        None => Limited::Unchanged,
    }
}

/// The note under a result that filled the appended limit.
pub fn note(limit: usize) -> String {
    format!(
        "showing the first {} rows of a query without LIMIT · add LIMIT, .limit <n> or .limit off for more",
        crate::render::thousands(limit as i128)
    )
}

/// How `.limit` reports the current setting.
pub fn report(limit: RowLimit) -> String {
    match limit {
        Some(limit) => format!(
            "row limit {} for queries without LIMIT · .limit <n> to change it, .limit off for none; scripts run with -e or -f are unlimited",
            crate::render::thousands(limit as i128)
        ),
        None => "row limit off: queries without LIMIT return every row, paged · .limit <n> to set one; scripts run with -e or -f are unlimited".to_owned(),
    }
}

/// `.limit` → `None` (report); `.limit 500` → `Some(Some(500))`;
/// `.limit off` → `Some(None)`.
pub fn parse_command(argument: Option<&str>) -> Result<Option<RowLimit>, String> {
    let Some(value) = argument else {
        return Ok(None);
    };
    parse_value(value).map(Some)
}

/// A row limit as `--row-limit` and `.limit` take it: a count of at least
/// one, or `off`.
pub fn parse_value(value: &str) -> Result<RowLimit, String> {
    if value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("none") || value == "0" {
        return Ok(None);
    }
    match value.replace(['_', ','], "").parse::<usize>() {
        Ok(limit) if limit >= 1 => Ok(Some(limit)),
        _ => Err(format!(
            "invalid row limit '{value}'; use a number of at least 1, or off"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_a_limit_only_to_queries_without_one() {
        assert!(
            matches!(inspect("SELECT * FROM t", Some(100)), Limited::Appended(sql) if sql == "SELECT * FROM t LIMIT 100")
        );
        assert!(matches!(
            inspect("with c as (select 1) select * from c order by 1", Some(5)),
            Limited::Appended(sql) if sql == "with c as (select 1) select * from c order by 1 LIMIT 5"
        ));
        assert!(matches!(
            inspect("SELECT * FROM t LIMIT 10", Some(100)),
            Limited::Explicit
        ));
        assert!(matches!(
            inspect("SELECT * FROM t ORDER BY a LIMIT 50000 OFFSET 5", Some(100)),
            Limited::Explicit
        ));
        assert!(matches!(
            inspect("SELECT * FROM (SELECT * FROM t LIMIT 10) AS s", Some(100)),
            Limited::Appended(_)
        ));
        assert!(matches!(
            inspect("SHOW TABLES", Some(100)),
            Limited::Unchanged
        ));
        assert!(matches!(
            inspect("EXPLAIN SELECT 1", Some(100)),
            Limited::Unchanged
        ));
        assert!(matches!(
            inspect("SELECT * FROM \"LIMIT\"", Some(100)),
            Limited::Appended(_)
        ));
    }

    #[test]
    fn limit_off_leaves_queries_alone() {
        assert!(matches!(
            inspect("SELECT * FROM t", None),
            Limited::Unchanged
        ));
        assert!(matches!(
            inspect("SELECT * FROM t LIMIT 10", None),
            Limited::Explicit
        ));
    }

    #[test]
    fn limit_command_takes_a_count_or_off() {
        assert_eq!(parse_command(None).unwrap(), None);
        assert_eq!(parse_command(Some("2,500")).unwrap(), Some(Some(2_500)));
        assert_eq!(parse_command(Some("10000")).unwrap(), Some(Some(10_000)));
        assert_eq!(parse_command(Some("250000")).unwrap(), Some(Some(250_000)));
        assert_eq!(parse_command(Some("off")).unwrap(), Some(None));
        assert_eq!(parse_command(Some("OFF")).unwrap(), Some(None));
        assert_eq!(parse_command(Some("0")).unwrap(), Some(None));
        assert!(parse_command(Some("-1")).is_err());
        assert!(parse_command(Some("many")).is_err());
        assert_eq!(
            note(1_000),
            "showing the first 1,000 rows of a query without LIMIT · add LIMIT, .limit <n> or .limit off for more"
        );
        assert!(report(Some(1_000)).starts_with("row limit 1,000 for queries without LIMIT"));
        assert!(report(None).starts_with("row limit off"));
    }
}
