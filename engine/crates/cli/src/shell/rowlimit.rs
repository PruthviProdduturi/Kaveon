//! The interactive row limit. A `SELECT` or `WITH` statement with no
//! top-level `LIMIT` gets one appended so a stray `SELECT *` over a large
//! table shows its first rows instead of shipping the table; an explicit
//! LIMIT above the ceiling is refused before it is sent. Scripts (`-e`,
//! `-f`, piped input) are never limited.
use sqlparser::dialect::GenericDialect;
use sqlparser::tokenizer::{Token, Tokenizer};

pub const DEFAULT_ROW_LIMIT: usize = 1_000;
/// The most rows the shell will show for one statement.
pub const HARD_ROW_LIMIT: usize = 10_000;

pub enum Limited {
    /// Not a query, or a query the shell leaves alone.
    Unchanged,
    /// The statement with ` LIMIT n` appended.
    Appended(String),
    /// The statement's own top-level LIMIT.
    Explicit(usize),
}

pub fn inspect(statement: &str, limit: usize) -> Limited {
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
    for (index, token) in tokens.iter().enumerate() {
        match token {
            Token::LParen => depth += 1,
            Token::RParen => depth = depth.saturating_sub(1),
            Token::Word(word)
                if depth == 0
                    && word.quote_style.is_none()
                    && word.value.eq_ignore_ascii_case("LIMIT") =>
            {
                let explicit = match tokens.get(index + 1) {
                    Some(Token::Number(value, _)) => value.parse::<usize>().unwrap_or(usize::MAX),
                    _ => usize::MAX,
                };
                return Limited::Explicit(explicit);
            }
            Token::Word(word)
                if depth == 0
                    && word.quote_style.is_none()
                    && word.value.eq_ignore_ascii_case("FETCH") =>
            {
                return Limited::Explicit(usize::MAX);
            }
            _ => {}
        }
    }
    Limited::Appended(format!("{} LIMIT {limit}", statement.trim_end()))
}

/// The message for a statement whose own LIMIT exceeds the ceiling.
pub fn refusal(explicit: usize) -> String {
    let asked = if explicit == usize::MAX {
        "an unbounded LIMIT".to_owned()
    } else {
        format!("LIMIT {}", crate::render::thousands(explicit as i128))
    };
    format!(
        "{asked} exceeds the shell's ceiling of {} rows; lower the LIMIT, or run the statement with -e or -f for the full result",
        crate::render::thousands(HARD_ROW_LIMIT as i128)
    )
}

/// The note under a result that filled the appended limit.
pub fn note(limit: usize) -> String {
    format!(
        "showing the first {} rows of a query without LIMIT · add LIMIT or .limit <n> (up to {}) for more",
        crate::render::thousands(limit as i128),
        crate::render::thousands(HARD_ROW_LIMIT as i128)
    )
}

/// `.limit` → `None` (report); `.limit 500` → the new limit.
pub fn parse_command(argument: Option<&str>) -> Result<Option<usize>, String> {
    let Some(value) = argument else {
        return Ok(None);
    };
    if value.eq_ignore_ascii_case("off") || value == "0" {
        return Err(format!(
            "the shell always limits rows (1 to {}); scripts run with -e or -f are unlimited",
            crate::render::thousands(HARD_ROW_LIMIT as i128)
        ));
    }
    match value.replace(['_', ','], "").parse::<usize>() {
        Ok(limit) if (1..=HARD_ROW_LIMIT).contains(&limit) => Ok(Some(limit)),
        _ => Err(format!(
            "invalid row limit '{value}'; use a number from 1 to {}",
            crate::render::thousands(HARD_ROW_LIMIT as i128)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_a_limit_only_to_queries_without_one() {
        assert!(
            matches!(inspect("SELECT * FROM t", 100), Limited::Appended(sql) if sql == "SELECT * FROM t LIMIT 100")
        );
        assert!(matches!(
            inspect("with c as (select 1) select * from c order by 1", 5),
            Limited::Appended(sql) if sql == "with c as (select 1) select * from c order by 1 LIMIT 5"
        ));
        assert!(matches!(
            inspect("SELECT * FROM t LIMIT 10", 100),
            Limited::Explicit(10)
        ));
        assert!(matches!(
            inspect("SELECT * FROM t ORDER BY a LIMIT 50000 OFFSET 5", 100),
            Limited::Explicit(50_000)
        ));
        assert!(matches!(
            inspect("SELECT * FROM (SELECT * FROM t LIMIT 10) AS s", 100),
            Limited::Appended(_)
        ));
        assert!(matches!(inspect("SHOW TABLES", 100), Limited::Unchanged));
        assert!(matches!(
            inspect("EXPLAIN SELECT 1", 100),
            Limited::Unchanged
        ));
        assert!(matches!(
            inspect("SELECT * FROM \"LIMIT\"", 100),
            Limited::Appended(_)
        ));
    }

    #[test]
    fn limit_command_keeps_within_the_ceiling() {
        assert_eq!(parse_command(None).unwrap(), None);
        assert_eq!(parse_command(Some("2,500")).unwrap(), Some(2_500));
        assert_eq!(parse_command(Some("10000")).unwrap(), Some(10_000));
        assert!(parse_command(Some("off")).unwrap_err().contains("-e or -f"));
        assert!(parse_command(Some("10001")).is_err());
        assert!(parse_command(Some("-1")).is_err());
        assert!(refusal(5_000_000).contains("LIMIT 5,000,000 exceeds"));
        assert!(note(1_000).starts_with("showing the first 1,000 rows"));
    }
}
