//! `.help` and `help`: the commands the shell accepts, grouped, one screen.
use crate::theme::Theme;
use ratatui::text::{Line, Span};

const COLUMN: usize = 72;

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
        title: "Browse",
        coming_soon: false,
        entries: &[
            Entry {
                command: "SHOW CATALOGS | SCHEMAS [IN catalog] | TABLES [IN [catalog.]schema]",
                description: "list; add LIKE 'pattern'; singular forms work too",
            },
            Entry {
                command: "DESCRIBE table  ·  SHOW COLUMNS FROM table",
                description: "columns, types and nullability",
            },
            Entry {
                command: "SHOW CREATE TABLE table",
                description: "the full definition: columns, location, format, access",
            },
            Entry {
                command: "USE [catalog.]schema  ·  USE catalog",
                description: "switch the session context (a bare catalog name works)",
            },
            Entry {
                command: ".catalogs  .schemas  .tables  .describe  .use",
                description: "the same, without a semicolon",
            },
        ],
    },
    Group {
        title: "Define",
        coming_soon: false,
        entries: &[
            Entry {
                command: "CREATE CATALOG c WITH (storage = 'local' | 'adls' | 's3', ...)",
                description: "admin; a directory, ADLS container or bucket",
            },
            Entry {
                command: "CREATE SCHEMA [catalog.]schema",
                description: "analyst or admin",
            },
            Entry {
                command: "ANALYZE [catalog.][schema.]table",
                description: "exact row count and source identity for the planner (admin)",
            },
            Entry {
                command: "CREATE TABLE t [(cols)] WITH (location = '...', format = '...')",
                description: "parquet, delta, iceberg; columns read from the data; probed first",
            },
            Entry {
                command: "ALTER TABLE t SET LOCATION '...'  ·  DROP TABLE | SCHEMA | CATALOG",
                description: "relocate (probed); remove — RESTRICT by default, or CASCADE",
            },
            Entry {
                command: "CALL system.register_table(schema_name => ..., table_name => ..., ...)",
                description: "Trino-style CREATE TABLE; unregister_table drops",
            },
            Entry {
                command: "kaveon catalog|schema|table <command>",
                description: "the same from the command line; kaveon --help lists them",
            },
        ],
    },
    Group {
        title: "Run",
        coming_soon: false,
        entries: &[
            Entry {
                command: "<sql>;",
                description: "ends with ; and may span lines; Ctrl-Enter forces a run",
            },
            Entry {
                command: "EXPLAIN <statement>",
                description: "the logical plan as a tree",
            },
            Entry {
                command: "SET SESSION key = value; <statement>",
                description: "per-statement settings, or .settings for the session",
            },
            Entry {
                command: ".settings [key value | reset]",
                description: "memory, parallelism, cache, admission_wait for every statement",
            },
            Entry {
                command: ".limit [n | off]",
                description: "rows a query without LIMIT shows (1,000); off pages every row",
            },
            Entry {
                command: "Ctrl-C",
                description: "cancel the running statement, or clear the input",
            },
            Entry {
                command: ".ask <question>  ·  .ask <n>",
                description: "a question in plain language via the DLM (--api); n answers",
            },
            Entry {
                command: ".cluster  .queries  .kill <id>",
                description: "nodes and admission; statements running now; cancel one",
            },
        ],
    },
    Group {
        title: "Shell",
        coming_soon: false,
        entries: &[
            Entry {
                command: "Tab  ·  Up/Down  ·  --editing-mode vi",
                description: "complete keywords and names; history; vi keys",
            },
            Entry {
                command: ".history [n]  .timing  .clear  help  exit",
                description: "recent statements; summary on/off; clear; this; leave (Ctrl-D)",
            },
            Entry {
                command: ".source <file>  .edit  .watch [seconds] <statement>",
                description: "run a file; last statement in $EDITOR; re-run until a key",
            },
        ],
    },
    Group {
        title: "Output",
        coming_soon: false,
        entries: &[
            Entry {
                command: ".format <name>  ·  --output-format",
                description: "ALIGNED, VERTICAL, AUTO, MARKDOWN, CSV, TSV, JSON, NULL",
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
                command: ".tee <file>  .tee off  ·  --paged",
                description: "copy everything shown to a file; page results in scripts",
            },
            Entry {
                command: "--theme dark | light | mono",
                description: "NO_COLOR is honoured",
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
        assert!(text.contains("  Browse\n    SHOW CATALOGS"));
        assert!(text.contains("    SHOW CREATE TABLE table"));
        assert!(text.contains("  Define\n    CREATE CATALOG"));
        assert!(text.contains("CALL system.register_table"));
        assert!(text.contains("  Run\n"));
        assert!(text.contains("    SET SESSION key = value"));
        assert!(text.contains("  Shell\n"));
        assert!(text.contains("  Output\n"));
        assert!(!text.contains("Coming soon"), "{text}");
        assert!(!text.contains("streaming rows"), "{text}");
        assert!(text.contains("    .ask <question>"));
        assert!(text.contains("    .source <file>  .edit  .watch [seconds] <statement>"));
        assert!(text.contains("    .tee <file>  .tee off"));
        assert!(text.contains("    <statement>\\G"));
        let paging = text
            .lines()
            .find(|line| line.starts_with("    Space, Enter  q"))
            .unwrap_or_default();
        assert!(
            paging.ends_with("next page of a result, as soon as it is written; stop"),
            "{paging}"
        );
        assert!(text.lines().count() <= 44, "{}", text.lines().count());
        assert!(
            text.lines().all(|line| line.chars().count() <= 142),
            "{}",
            text.lines().map(|l| l.chars().count()).max().unwrap_or(0)
        );
    }
}
