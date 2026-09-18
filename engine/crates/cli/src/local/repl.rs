//! The stdin REPL and `-e` path for `--local`, unchanged in what it prints.
//! The integrator replaces this with the shared shell and renderers.

use super::catalog::{CatalogCommand, parse_catalog_command};
use super::{LocalEngine, LocalSource, display};
use std::io::{self, BufRead, Write};
use std::time::Instant;

pub fn print_banner(engine: &LocalEngine) {
    println!("Kaveon Engine v{}", crate::VERSION);
    println!("Talk to your data.");
    println!();
    match engine.source() {
        LocalSource::DataDir(dir) => {
            let (catalog, schema) = engine.context();
            let tables = engine
                .tables(&catalog, &schema)
                .map(|tables| tables.len())
                .unwrap_or(0);
            println!(
                "Data directory: {} ({tables} tables discovered)",
                dir.display()
            );
        }
        LocalSource::Config(path) => println!(
            "Config: {} ({} catalogs, {} tables)",
            path.display(),
            engine.catalogs().len(),
            super::catalog::count_tables(&engine.catalog)
        ),
        LocalSource::Empty => {
            println!("No data directory specified. Use --data-dir <path> or .use to configure.");
        }
    }
    println!("Type .help for commands, SQL queries end with ;");
    println!();
}

pub fn run(engine: &mut LocalEngine) -> Result<(), String> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut buffer = String::new();
    let mut collecting = false;

    loop {
        eprint!("{}", if collecting { "     -> " } else { "kaveon> " });
        io::stderr().flush().ok();

        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Err(error) => return Err(format!("error reading input: {error}")),
            _ => {}
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if !collecting && trimmed.starts_with('.') {
            handle_meta_command(trimmed, engine);
            continue;
        }

        buffer.push_str(&line);

        if buffer.trim_end().ends_with(';') {
            let sql = buffer.trim().trim_end_matches(';').trim();
            if !sql.is_empty() {
                run_statement(engine, sql);
            }
            buffer.clear();
            collecting = false;
        } else {
            collecting = true;
        }
    }

    println!();
    Ok(())
}

/// One statement: the catalog statements print their fixed tables, anything
/// else is planned and rendered with the timing line.
pub fn run_statement(engine: &mut LocalEngine, sql: &str) {
    match parse_catalog_command(sql) {
        Some(CatalogCommand::ShowCatalogs) => print_list("Catalog", &engine.catalogs()),
        Some(CatalogCommand::ShowSchemas { catalog }) => {
            let catalog = catalog.unwrap_or_else(|| engine.context().0);
            match engine.schemas(&catalog) {
                Ok(names) => print_list("Schema", &names),
                Err(error) => eprintln!("error: {error}"),
            }
        }
        Some(CatalogCommand::ShowTables { target }) => {
            let (catalog, schema) = match target {
                Some(target) => match super::catalog::split_target(&target, &engine.context().1) {
                    Ok(resolved) => resolved,
                    Err(_) => {
                        eprintln!("usage: SHOW TABLES FROM catalog.schema");
                        return;
                    }
                },
                None => engine.context(),
            };
            match engine.tables(&catalog, &schema) {
                Ok(names) => print_list("Table", &names),
                Err(error) => eprintln!("error: {error}"),
            }
        }
        Some(CatalogCommand::Describe { reference }) => match engine.describe(&reference) {
            Ok(columns) => print_columns(&columns),
            Err(error) => eprintln!("error: {error}"),
        },
        Some(CatalogCommand::Use { target }) => use_context(engine, &target),
        None => {
            let start = Instant::now();
            match engine.execute_batches(sql) {
                Ok((_, batches)) => {
                    print!("{}", display::format_batches(&batches));
                    println!("Time: {:.3}s", start.elapsed().as_secs_f64());
                }
                Err(error) => eprintln!("{error}"),
            }
        }
    }
}

fn handle_meta_command(command: &str, engine: &mut LocalEngine) {
    let parts: Vec<&str> = command.split_whitespace().collect();
    match parts[0] {
        ".quit" | ".exit" | ".q" => std::process::exit(0),
        ".help" | ".h" => crate::print_usage(),
        ".catalogs" => {
            let names = engine.catalogs();
            if names.is_empty() {
                println!("(no catalogs registered)");
            } else {
                let (default_catalog, _) = engine.context();
                println!("Catalogs:");
                for name in names {
                    let marker = if name == default_catalog {
                        " (default)"
                    } else {
                        ""
                    };
                    println!("  {name}{marker}");
                }
            }
        }
        ".schemas" => {
            let (default_catalog, default_schema) = engine.context();
            let catalog = parts.get(1).copied().unwrap_or(default_catalog.as_str());
            match engine.schemas(catalog) {
                Ok(names) => {
                    println!("Schemas in '{catalog}':");
                    for name in names {
                        let marker = if name == default_schema {
                            " (default)"
                        } else {
                            ""
                        };
                        println!("  {name}{marker}");
                    }
                }
                Err(error) => eprintln!("error: {error}"),
            }
        }
        ".tables" => {
            let (catalog, default_schema) = engine.context();
            let schema = parts.get(1).copied().unwrap_or(default_schema.as_str());
            match engine.tables(&catalog, schema) {
                Ok(names) if names.is_empty() => println!("(no tables in '{catalog}.{schema}')"),
                Ok(names) => {
                    println!("Tables in '{catalog}.{schema}':");
                    for name in names {
                        println!("  {name}");
                    }
                }
                Err(error) => eprintln!("error: {error}"),
            }
        }
        ".describe" | ".desc" => {
            let Some(reference) = parts.get(1) else {
                eprintln!("usage: .describe <table>");
                return;
            };
            match engine.table_details(reference) {
                Ok(details) => {
                    println!(
                        "Table: {}.{}.{}",
                        details.catalog, details.schema, details.table
                    );
                    println!("Format: {}", details.format);
                    println!("Access: {}", details.access);
                    println!("Location: {}", details.location);
                    println!();
                    println!("Columns:");
                    for (name, data_type, nullable) in &details.columns {
                        let nullable = if *nullable { "NULL" } else { "NOT NULL" };
                        println!("  {name:<30} {data_type:<15} {nullable}");
                    }
                }
                Err(error) => eprintln!("error: {error}"),
            }
        }
        ".use" => {
            if parts.len() != 2 {
                eprintln!("usage: .use <catalog.schema>");
                return;
            }
            use_context(engine, parts[1]);
        }
        other => {
            eprintln!("unknown command: {other}");
            eprintln!("type .help for available commands");
        }
    }
}

fn use_context(engine: &mut LocalEngine, target: &str) {
    if target.split('.').count() > 2 {
        eprintln!("usage: USE catalog.schema");
        return;
    }
    match engine.use_context(target) {
        Ok((catalog, schema)) => println!("Using catalog '{catalog}', schema '{schema}'"),
        Err(error) => eprintln!("error: {error}"),
    }
}

fn print_list(header: &str, names: &[String]) {
    println!("+{}+", "-".repeat(32));
    println!("| {header:<30} |");
    println!("+{}+", "-".repeat(32));
    for name in names {
        println!("| {name:<30} |");
    }
    println!("+{}+", "-".repeat(32));
    println!("({} rows)", names.len());
}

fn print_columns(columns: &[(String, String, bool)]) {
    let rule = format!("+{}+{}+{}+", "-".repeat(32), "-".repeat(17), "-".repeat(10));
    println!("{rule}");
    println!("| {:<30} | {:<15} | {:<8} |", "Column", "Type", "Nullable");
    println!("{rule}");
    for (name, data_type, nullable) in columns {
        let nullable = if *nullable { "YES" } else { "NO" };
        println!("| {name:<30} | {data_type:<15} | {nullable:<8} |");
    }
    println!("{rule}");
}
