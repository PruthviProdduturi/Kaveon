//! The interactive row limit: a `SELECT` or `WITH` statement with no
//! top-level `LIMIT` gets one appended so a stray `SELECT *` over a large
//! table shows its first rows instead of shipping the table.
use sqlparser::dialect::GenericDialect;
use sqlparser::tokenizer::{Token, Tokenizer};

/// The statement with ` LIMIT n` appended when it is a query without a
/// top-level LIMIT; `None` when the statement is left alone.
pub fn apply(statement: &str, limit: usize) -> Option<String> {
    let tokens = Tokenizer::new(&GenericDialect {}, statement)
        .tokenize()
        .ok()?
        .into_iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    let first = match tokens.first() {
        Some(Token::Word(word)) if word.quote_style.is_none() => word.value.to_ascii_uppercase(),
        _ => return None,
    };
    if first != "SELECT" && first != "WITH" {
        return None;
    }
    let mut depth = 0usize;
    for token in &tokens {
        match token {
            Token::LParen => depth += 1,
            Token::RParen => depth = depth.saturating_sub(1),
            Token::Word(word)
                if depth == 0
                    && word.quote_style.is_none()
                    && (word.value.eq_ignore_ascii_case("LIMIT")
                        || word.value.eq_ignore_ascii_case("FETCH")) =>
            {
                return None;
            }
            _ => {}
        }
    }
    Some(format!("{} LIMIT {limit}", statement.trim_end()))
}

/// `.limit`, `.limit 500`, `.limit off`.
pub fn parse_command(argument: Option<&str>) -> Result<Option<Option<usize>>, String> {
    match argument {
        None => Ok(None),
        Some(value) if value.eq_ignore_ascii_case("off") || value == "0" => Ok(Some(None)),
        Some(value) => value
            .replace(['_', ','], "")
            .parse::<usize>()
            .ok()
            .filter(|limit| *limit > 0)
            .map(|limit| Some(Some(limit)))
            .ok_or_else(|| format!("invalid row limit '{value}'; use a positive number or off")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_a_limit_only_to_queries_without_one() {
        assert_eq!(
            apply("SELECT * FROM t", 100).as_deref(),
            Some("SELECT * FROM t LIMIT 100")
        );
        assert_eq!(
            apply("with c as (select 1) select * from c order by 1", 5).as_deref(),
            Some("with c as (select 1) select * from c order by 1 LIMIT 5")
        );
        assert!(apply("SELECT * FROM t LIMIT 10", 100).is_none());
        assert!(apply("SELECT * FROM t ORDER BY a LIMIT 10 OFFSET 5", 100).is_none());
        assert!(apply("SELECT * FROM (SELECT * FROM t LIMIT 10) AS s", 100).is_some());
        assert!(apply("SHOW TABLES", 100).is_none());
        assert!(apply("EXPLAIN SELECT 1", 100).is_none());
        assert!(apply("SELECT * FROM \"LIMIT\"", 100).is_some());
    }

    #[test]
    fn limit_command_parses_numbers_and_off() {
        assert_eq!(parse_command(None).unwrap(), None);
        assert_eq!(parse_command(Some("off")).unwrap(), Some(None));
        assert_eq!(parse_command(Some("2,500")).unwrap(), Some(Some(2500)));
        assert!(parse_command(Some("-1")).is_err());
    }
}
