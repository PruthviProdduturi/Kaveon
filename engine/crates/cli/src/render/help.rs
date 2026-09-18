//! `.help` and `help`: the commands the shell accepts, grouped, one screen.
use crate::theme::Theme;
use ratatui::text::{Line, Span};

const COLUMN: usize = 42;

struct Entry {
    command: &'static str,
    description: &'static str,
}

struct Group {
    title: &'static str,
    entries: &'static [Entry],
    /// Listed so the surface is known; rendered dim until each one ships.
    coming_soon: bool,
}

const GROUPS: &[Group] = &[
    Group {
        title: "Catalog",
        coming_soon: false,
        entries: &[
            Entry {
                command: "SHOW CATALOGS",
                description: "list catalogs",
            },
            Entry {
                command: "SHOW SCHEMAS [IN catalog]",
                description: "list schemas",
            },
            Entry {
                command: "SHOW TABLES [IN [catalog.]schema]",
                description: "list tables; add LIKE 'pattern' to filter",
            },
            Entry {
                command: "DESCRIBE [catalog.][schema.]table",
                description: "columns, types and nullability",
            },
            Entry {
                command: "USE [catalog.]schema",
                description: "switch the session catalog and schema",
            },
            Entry {
                command: ".catalogs  .schemas  .tables  .describe  .use",
                description: "the same, without a semicolon",
            },
        ],
    },
    Group {
        title: "Shell",
        coming_soon: false,
        entries: &[
            Entry {
                command: "help",
                description: "this text",
            },
            Entry {
                command: ".ask <question>",
                description: "a question in plain language through the Kaveon DLM (--api)",
            },
            Entry {
                command: "clear",
                description: "clear the screen",
            },
            Entry {
                command: "exit, quit, Ctrl-D",
                description: "leave",
            },
            Entry {
                command: "Ctrl-C",
                description: "cancel the running statement, or clear the input",
            },
            Entry {
                command: ".limit [n | off]",
                description: "rows a query without LIMIT shows (default 1,000); off pages every row",
            },
            Entry {
                command: "EXPLAIN <statement>",
                description: "the logical plan as a tree",
            },
            Entry {
                command: ".cluster  .queries  .kill <id>",
                description: "nodes and admission; statements running now; cancel one",
            },
            Entry {
                command: ".settings [key value | reset]",
                description: "memory, parallelism, cache, admission_wait for this session",
            },
            Entry {
                command: ".format <name>  .timing  .history [n]",
                description: "result format; summary on/off; recent statements",
            },
            Entry {
                command: "Tab",
                description: "complete keywords, tables, columns, schemas and catalogs",
            },
            Entry {
                command: ".source <file>",
                description: "run the statements in a file, as if typed",
            },
            Entry {
                command: ".edit",
                description: "the last statement in $VISUAL or $EDITOR, back into the editor",
            },
            Entry {
                command: ".watch [seconds] <statement>",
                description: "re-run every N seconds (default 2) until a key is pressed",
            },
        ],
    },
    Group {
        title: "Output",
        coming_soon: false,
        entries: &[
            Entry {
                command: "--output-format <NAME>",
                description: "ALIGNED, VERTICAL, AUTO, MARKDOWN, CSV, TSV, JSON, NULL",
            },
            Entry {
                command: "--theme <NAME>",
                description: "dark, light, or mono; NO_COLOR is honoured",
            },
            Entry {
                command: "Space, Enter  q",
                description: "next page of a result, as soon as it is written; stop",
            },
            Entry {
                command: "<statement>\\G",
                description: "end with \\G instead of ; for the vertical format once",
            },
            Entry {
                command: ".tee <file>  .tee off",
                description: "append everything shown to a file; stop",
            },
            Entry {
                command: "--paged",
                description: "page large results in -e, -f and piped mode instead of the inline limit",
            },
        ],
    },
];

pub fn help(theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (index, group) in GROUPS.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        lines.push(Line::from(Span::styled(
            format!("  {}", group.title),
            theme.title,
        )));
        for entry in group.entries {
            let padding = COLUMN.saturating_sub(entry.command.chars().count()).max(2);
            let command_style = if group.coming_soon {
                theme.dim
            } else {
                theme.accent
            };
            lines.push(Line::from(vec![
                Span::styled(format!("    {}", entry.command), command_style),
                Span::styled(
                    format!("{}{}", " ".repeat(padding), entry.description),
                    theme.dim,
                ),
            ]));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  SQL statements end with ; and may span lines. Metadata commands run in the client over the catalog API.",
        theme.dim,
    )));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_is_grouped_and_nothing_is_still_coming() {
        let text = crate::render::to_plain(&help(&Theme::mono()));
        assert!(text.contains("  Catalog\n    SHOW CATALOGS"));
        assert!(text.contains("  Shell\n"));
        assert!(text.contains("  Output\n"));
        assert!(!text.contains("Coming soon"), "{text}");
        assert!(!text.contains("streaming rows"), "{text}");
        assert!(text.contains("    .ask <question>"));
        assert!(text.contains("    .source <file>"));
        assert!(text.contains("    .tee <file>  .tee off"));
        assert!(text.contains("    .edit "));
        assert!(text.contains("    .watch [seconds] <statement>"));
        assert!(text.contains("    <statement>\\G"));
        let paging = text
            .lines()
            .find(|line| line.starts_with("    Space, Enter  q"))
            .unwrap_or_default();
        assert!(
            paging.ends_with("next page of a result, as soon as it is written; stop"),
            "{paging}"
        );
        assert!(text.lines().count() <= 40, "{}", text.lines().count());
        assert!(text.lines().all(|line| line.chars().count() <= 118));
    }
}
