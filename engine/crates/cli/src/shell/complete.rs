//! Tab completion: SQL keywords, dot commands, and the catalog names the
//! `NameCache` knows.
use crate::shell::commands::DOT_COMMANDS;

/// The keywords the engine's parser accepts, in the order they are offered.
pub const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "GROUP",
    "BY",
    "ORDER",
    "LIMIT",
    "OFFSET",
    "HAVING",
    "JOIN",
    "LEFT",
    "RIGHT",
    "FULL",
    "INNER",
    "CROSS",
    "ON",
    "AS",
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS",
    "NULL",
    "DISTINCT",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "CAST",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "SHOW",
    "CATALOGS",
    "SCHEMAS",
    "TABLES",
    "COLUMNS",
    "DESCRIBE",
    "USE",
    "EXPLAIN",
    "SET",
    "SESSION",
];

/// The metadata dot commands, completed alongside the ones `commands.rs`
/// parses.
const METADATA_DOT_COMMANDS: &[&str] = &[
    ".catalogs",
    ".schemas",
    ".tables",
    ".describe",
    ".use",
    ".limit",
];

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// The byte offset where the word under completion starts. A qualified
/// name (`schema.table`) completes its last part; a dot command keeps its
/// leading dot.
fn word_start(line: &str, cursor: usize) -> usize {
    let head = &line[..cursor];
    let mut start = head
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_word_char(*ch) || *ch == '.')
        .last()
        .map_or(cursor, |(index, _)| index);
    let word = &head[start..];
    let dot_command = word.starts_with('.') && head[..start].trim().is_empty();
    if !dot_command && let Some(dot) = word.rfind('.') {
        start += dot + 1;
    }
    start
}

/// Candidates for the word ending at `cursor`, and the offset the editor
/// replaces from. Names match any case and are offered as-is; keywords take
/// the case the user typed (upper unless the prefix is all lowercase); dot
/// commands complete only at the start of the line. One candidate is the
/// expansion itself.
pub fn complete(line: &str, cursor: usize, names: &[String]) -> (usize, Vec<String>) {
    let cursor = cursor.min(line.len());
    let start = word_start(line, cursor);
    let prefix = &line[start..cursor];
    if prefix.is_empty() {
        return (start, Vec::new());
    }
    let lower = prefix.to_lowercase();
    let mut candidates: Vec<String> = Vec::new();
    let mut offer = |candidate: String| {
        if !candidates
            .iter()
            .any(|seen| seen.eq_ignore_ascii_case(&candidate))
        {
            candidates.push(candidate);
        }
    };
    if prefix.starts_with('.') {
        for command in DOT_COMMANDS.iter().chain(METADATA_DOT_COMMANDS) {
            if command.starts_with(&lower) {
                offer((*command).to_owned());
            }
        }
        return (start, candidates);
    }
    for name in names {
        if name.to_lowercase().starts_with(&lower) {
            offer(name.clone());
        }
    }
    let lowercase = prefix.chars().all(|ch| !ch.is_uppercase());
    for keyword in KEYWORDS {
        if keyword.to_lowercase().starts_with(&lower) {
            offer(if lowercase {
                keyword.to_lowercase()
            } else {
                (*keyword).to_owned()
            });
        }
    }
    (start, candidates)
}

/// The longest prefix all candidates share beyond what was typed, for the
/// shell-style partial expansion when several remain (any case).
pub fn common_prefix(candidates: &[String]) -> String {
    let Some(first) = candidates.first() else {
        return String::new();
    };
    let mut length = first.chars().count();
    for candidate in &candidates[1..] {
        length = first
            .chars()
            .zip(candidate.chars())
            .take(length)
            .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
            .count();
    }
    first.chars().take(length).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        [
            "kaveon_events",
            "kaveon_events_enriched",
            "region",
            "Population",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn keywords_keep_the_case_the_user_typed() {
        let line = "sel";
        assert_eq!(complete(line, 3, &[]), (0, strings(&["select"])));
        assert_eq!(complete("SEL", 3, &[]), (0, strings(&["SELECT"])));
        assert_eq!(complete("Sel", 3, &[]), (0, strings(&["SELECT"])));
        assert_eq!(
            complete("SELECT * FROM t GR", 18, &[]),
            (16, strings(&["GROUP"]))
        );
    }

    #[test]
    fn names_are_offered_as_is_before_keywords() {
        let (start, candidates) = complete("SELECT * FROM kav", 17, &names());
        assert_eq!(start, 14);
        assert_eq!(
            candidates,
            strings(&["kaveon_events", "kaveon_events_enriched"])
        );
        let (_, candidates) = complete("select po", 9, &names());
        assert_eq!(candidates, strings(&["Population"]));
        let (_, candidates) = complete("select c", 8, &strings(&["city"]));
        assert_eq!(
            candidates,
            strings(&[
                "city", "cross", "count", "case", "cast", "catalogs", "columns"
            ])
        );
    }

    #[test]
    fn qualified_names_complete_their_last_part() {
        let (start, candidates) = complete("SELECT * FROM kaveon_product.kav", 32, &names());
        assert_eq!(start, 29);
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn dot_commands_complete_at_the_start_of_the_line() {
        assert_eq!(
            complete(".cl", 3, &[]),
            (0, strings(&[".cluster", ".clear"]))
        );
        assert_eq!(complete("  .se", 5, &[]), (2, strings(&[".settings"])));
        assert_eq!(complete(".ta", 3, &[]), (0, strings(&[".tables"])));
        assert!(complete("SELECT .cl", 10, &[]).1.is_empty());
    }

    #[test]
    fn nothing_to_complete_without_a_word() {
        assert_eq!(complete("SELECT ", 7, &names()), (7, vec![]));
        assert_eq!(complete("", 0, &names()), (0, vec![]));
        assert!(complete("SELECT zz", 9, &names()).1.is_empty());
    }

    #[test]
    fn a_column_named_like_a_keyword_is_offered_once() {
        let (_, candidates) = complete("select cou", 10, &strings(&["count"]));
        assert_eq!(candidates, strings(&["count"]));
    }

    #[test]
    fn common_prefix_expands_what_all_candidates_share() {
        assert_eq!(
            common_prefix(&strings(&["kaveon_events", "kaveon_events_enriched"])),
            "kaveon_events"
        );
        assert_eq!(common_prefix(&strings(&["GROUP", "GRANT"])), "GR");
        assert_eq!(common_prefix(&strings(&["only"])), "only");
        assert_eq!(common_prefix(&[]), "");
    }
}
