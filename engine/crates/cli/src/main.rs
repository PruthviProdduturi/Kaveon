mod config;
mod display;
mod planner;

use kaveon_core::{
    AccessPattern, CatalogManager, CatalogProvider, DataFormat, MemoryCatalog, StorageType,
    TableMeta, TableReference, collect_batches,
};
use kaveon_sql::logical_plan::sql_to_logical_plan;
use kaveon_storage::ParquetReader;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (data_dir, config_path) = parse_args(&args);

    let mut catalog_mgr = if let Some(dir) = &data_dir {
        let mut mgr = CatalogManager::new("local", "default");
        let catalog = build_local_catalog(dir);
        let table_count = catalog.table_names("default").map(|t| t.len()).unwrap_or(0);
        mgr.register_catalog(Box::new(catalog));
        print_banner(Some(dir), table_count);
        mgr
    } else {
        let cfg_path = config_path.unwrap_or_else(config::default_config_path);
        match config::load_config(&cfg_path) {
            Ok(mgr) => {
                let total_tables: usize = mgr
                    .catalog_names()
                    .iter()
                    .filter_map(|name| mgr.catalog(name))
                    .flat_map(|cat| {
                        cat.schema_names()
                            .into_iter()
                            .filter_map(move |s| cat.table_names(&s).ok().map(|t| t.len()))
                    })
                    .sum();
                println!("Kaveon Engine v{VERSION}");
                println!("Talk to your data.");
                println!();
                println!(
                    "Config: {} ({} catalogs, {} tables)",
                    cfg_path.display(),
                    mgr.catalog_names().len(),
                    total_tables
                );
                println!("Type .help for commands, SQL queries end with ;");
                println!();
                mgr
            }
            Err(_) => {
                let mut mgr = CatalogManager::new("kaveon", "default");
                let catalog = MemoryCatalog::new(
                    "kaveon",
                    StorageType::Local {
                        base_path: PathBuf::from("."),
                    },
                )
                .with_schema("default");
                mgr.register_catalog(Box::new(catalog));
                print_banner(None, 0);
                mgr
            }
        }
    };

    repl(&mut catalog_mgr);
}

fn parse_args(args: &[String]) -> (Option<PathBuf>, Option<PathBuf>) {
    let mut data_dir = None;
    let mut config_path = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--data-dir" | "-d" => {
                if i + 1 < args.len() {
                    data_dir = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                    continue;
                }
                eprintln!("error: --data-dir requires a path");
                std::process::exit(1);
            }
            "--config" | "-c" => {
                if i + 1 < args.len() {
                    config_path = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                    continue;
                }
                eprintln!("error: --config requires a path");
                std::process::exit(1);
            }
            "--version" | "-V" => {
                println!("kaveon {VERSION}");
                std::process::exit(0);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                let path = PathBuf::from(other);
                if path.is_dir() {
                    data_dir = Some(path);
                    i += 1;
                    continue;
                }
                eprintln!("error: unknown argument '{other}'");
                print_usage();
                std::process::exit(1);
            }
        }
    }
    (data_dir, config_path)
}

fn print_usage() {
    println!("Usage: kaveon [OPTIONS] [DATA_DIR]");
    println!();
    println!("Options:");
    println!("  -d, --data-dir <PATH>  Directory containing Parquet files");
    println!("  -c, --config <PATH>    Config file (default: ~/.kaveon/config.toml)");
    println!("  -V, --version          Print version");
    println!("  -h, --help             Print help");
    println!();
    println!("Meta-commands:");
    println!("  .catalogs              List catalogs");
    println!("  .schemas               List schemas in current catalog");
    println!("  .tables                List tables in current schema");
    println!("  .describe <table>      Show table schema");
    println!("  .use <catalog.schema>  Switch default catalog/schema");
    println!("  .quit                  Exit");
}

fn print_banner(data_dir: Option<&Path>, table_count: usize) {
    println!("Kaveon Engine v{VERSION}");
    println!("Talk to your data.");
    println!();
    if let Some(dir) = data_dir {
        println!(
            "Data directory: {} ({} tables discovered)",
            dir.display(),
            table_count
        );
    } else {
        println!("No data directory specified. Use --data-dir <path> or .use to configure.");
    }
    println!("Type .help for commands, SQL queries end with ;");
    println!();
}

fn build_local_catalog(dir: &Path) -> MemoryCatalog {
    let mut catalog = MemoryCatalog::new(
        "kaveon",
        StorageType::Local {
            base_path: dir.to_path_buf(),
        },
    )
    .with_schema("default");

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "parquet")
                && let Some(table_name) = path.file_stem().and_then(|s| s.to_str())
            {
                match ParquetReader::new(&path).metadata() {
                    Ok(meta) => {
                        let _ = catalog.register_table(
                            "default",
                            TableMeta {
                                name: table_name.to_owned(),
                                arrow_schema: meta.schema,
                                location: path.file_name().unwrap().to_string_lossy().into_owned(),
                                access: AccessPattern::Shortcut,
                                format: DataFormat::Parquet,
                            },
                        );
                    }
                    Err(e) => {
                        eprintln!("warning: skipping {}: {e}", path.display());
                    }
                }
            }
        }
    }

    catalog
}

fn repl(catalog: &mut CatalogManager) {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut buffer = String::new();
    let mut collecting = false;

    loop {
        if collecting {
            eprint!("     -> ");
        } else {
            eprint!("kaveon> ");
        }
        io::stderr().flush().ok();

        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Err(e) => {
                eprintln!("error reading input: {e}");
                break;
            }
            _ => {}
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if !collecting && trimmed.starts_with('.') {
            handle_meta_command(trimmed, catalog);
            continue;
        }

        buffer.push_str(&line);

        if buffer.trim_end().ends_with(';') {
            let sql = buffer.trim().trim_end_matches(';').trim();
            if !sql.is_empty() && !try_handle_show_use(sql, catalog) {
                execute_query(sql, catalog);
            }
            buffer.clear();
            collecting = false;
        } else {
            collecting = true;
        }
    }

    println!();
}

fn handle_meta_command(cmd: &str, catalog: &CatalogManager) {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    match parts[0] {
        ".quit" | ".exit" | ".q" => std::process::exit(0),
        ".help" | ".h" => print_usage(),
        ".catalogs" => {
            let names = catalog.catalog_names();
            if names.is_empty() {
                println!("(no catalogs registered)");
            } else {
                println!("Catalogs:");
                for name in names {
                    let marker = if name == catalog.default_catalog() {
                        " (default)"
                    } else {
                        ""
                    };
                    println!("  {name}{marker}");
                }
            }
        }
        ".schemas" => {
            let cat_name = if parts.len() > 1 {
                parts[1]
            } else {
                catalog.default_catalog()
            };
            match catalog.catalog(cat_name) {
                Some(cat) => {
                    let names = cat.schema_names();
                    println!("Schemas in '{cat_name}':");
                    for name in names {
                        let marker = if name == catalog.default_schema() {
                            " (default)"
                        } else {
                            ""
                        };
                        println!("  {name}{marker}");
                    }
                }
                None => eprintln!("error: catalog '{cat_name}' not found"),
            }
        }
        ".tables" => {
            let cat_name = catalog.default_catalog();
            let schema_name = if parts.len() > 1 {
                parts[1]
            } else {
                catalog.default_schema()
            };
            match catalog.catalog(cat_name) {
                Some(cat) => match cat.table_names(schema_name) {
                    Ok(names) => {
                        if names.is_empty() {
                            println!("(no tables in '{cat_name}.{schema_name}')");
                        } else {
                            println!("Tables in '{cat_name}.{schema_name}':");
                            for name in names {
                                println!("  {name}");
                            }
                        }
                    }
                    Err(e) => eprintln!("error: {e}"),
                },
                None => eprintln!("error: catalog '{cat_name}' not found"),
            }
        }
        ".describe" | ".desc" => {
            if parts.len() < 2 {
                eprintln!("usage: .describe <table>");
                return;
            }
            let table_name = parts[1];
            let reference = TableReference::parse(table_name);
            match catalog.resolve_table(&reference) {
                Ok(resolved) => {
                    println!(
                        "Table: {}.{}.{}",
                        resolved.catalog, resolved.schema, resolved.table.name
                    );
                    println!("Format: {:?}", resolved.table.format);
                    println!("Access: {:?}", resolved.table.access);
                    println!("Location: {}", resolved.full_path());
                    println!();
                    println!("Columns:");
                    for field in resolved.table.arrow_schema.fields() {
                        let nullable = if field.is_nullable() {
                            "NULL"
                        } else {
                            "NOT NULL"
                        };
                        println!(
                            "  {:<30} {:<15} {}",
                            field.name(),
                            field.data_type(),
                            nullable
                        );
                    }
                }
                Err(e) => eprintln!("error: {e}"),
            }
        }
        ".use" => {
            eprintln!("note: .use is not yet implemented — restart with --data-dir");
        }
        _ => {
            eprintln!("unknown command: {}", parts[0]);
            eprintln!("type .help for available commands");
        }
    }
}

fn try_handle_show_use(sql: &str, catalog: &mut CatalogManager) -> bool {
    let upper = sql.trim().to_uppercase();
    let parts: Vec<&str> = upper.split_whitespace().collect();

    match parts.as_slice() {
        ["SHOW", "CATALOGS"] => {
            let names = catalog.catalog_names();
            println!("+{}+", "-".repeat(32));
            println!("| {:<30} |", "Catalog");
            println!("+{}+", "-".repeat(32));
            for name in &names {
                println!("| {:<30} |", name);
            }
            println!("+{}+", "-".repeat(32));
            println!("({} rows)", names.len());
            true
        }
        ["SHOW", "SCHEMAS"] => {
            show_schemas(catalog.default_catalog(), catalog);
            true
        }
        ["SHOW", "SCHEMAS", "FROM", _catalog_name] | ["SHOW", "SCHEMAS", "IN", _catalog_name] => {
            let name = sql.split_whitespace().last().unwrap();
            show_schemas(name, catalog);
            true
        }
        ["SHOW", "TABLES"] => {
            show_tables(catalog.default_catalog(), catalog.default_schema(), catalog);
            true
        }
        ["SHOW", "TABLES", "FROM", _qualified] | ["SHOW", "TABLES", "IN", _qualified] => {
            let name = sql.split_whitespace().last().unwrap();
            let parts: Vec<&str> = name.split('.').collect();
            match parts.len() {
                2 => show_tables(parts[0], parts[1], catalog),
                1 => show_tables(parts[0], catalog.default_schema(), catalog),
                _ => eprintln!("usage: SHOW TABLES FROM catalog.schema"),
            }
            true
        }
        ["DESCRIBE", _table] | ["DESC", _table] => {
            let name = sql.split_whitespace().last().unwrap();
            let reference = TableReference::parse(name);
            match catalog.resolve_table(&reference) {
                Ok(resolved) => {
                    println!("+{}+{}+{}+", "-".repeat(32), "-".repeat(17), "-".repeat(10));
                    println!("| {:<30} | {:<15} | {:<8} |", "Column", "Type", "Nullable");
                    println!("+{}+{}+{}+", "-".repeat(32), "-".repeat(17), "-".repeat(10));
                    for field in resolved.table.arrow_schema.fields() {
                        println!(
                            "| {:<30} | {:<15} | {:<8} |",
                            field.name(),
                            format!("{}", field.data_type()),
                            if field.is_nullable() { "YES" } else { "NO" }
                        );
                    }
                    println!("+{}+{}+{}+", "-".repeat(32), "-".repeat(17), "-".repeat(10));
                }
                Err(e) => eprintln!("error: {e}"),
            }
            true
        }
        _ if upper.starts_with("USE ") => {
            let target = sql.trim()[4..].trim().trim_end_matches(';');
            let parts: Vec<&str> = target.split('.').collect();
            match parts.len() {
                2 => {
                    *catalog = CatalogManager::new(parts[0], parts[1]);
                    println!("Using catalog '{}', schema '{}'", parts[0], parts[1]);
                }
                1 => {
                    let current_schema = catalog.default_schema().to_owned();
                    *catalog = CatalogManager::new(parts[0], current_schema);
                    println!("Using catalog '{}'", parts[0]);
                }
                _ => eprintln!("usage: USE catalog.schema"),
            }
            true
        }
        _ => false,
    }
}

fn show_schemas(catalog_name: &str, catalog: &CatalogManager) {
    match catalog.catalog(catalog_name) {
        Some(cat) => {
            let names = cat.schema_names();
            println!("+{}+", "-".repeat(32));
            println!("| {:<30} |", "Schema");
            println!("+{}+", "-".repeat(32));
            for name in &names {
                println!("| {:<30} |", name);
            }
            println!("+{}+", "-".repeat(32));
            println!("({} rows)", names.len());
        }
        None => eprintln!("error: catalog '{catalog_name}' not found"),
    }
}

fn show_tables(catalog_name: &str, schema_name: &str, catalog: &CatalogManager) {
    match catalog.catalog(catalog_name) {
        Some(cat) => match cat.table_names(schema_name) {
            Ok(names) => {
                println!("+{}+", "-".repeat(32));
                println!("| {:<30} |", "Table");
                println!("+{}+", "-".repeat(32));
                for name in &names {
                    println!("| {:<30} |", name);
                }
                println!("+{}+", "-".repeat(32));
                println!("({} rows)", names.len());
            }
            Err(e) => eprintln!("error: {e}"),
        },
        None => eprintln!("error: catalog '{catalog_name}' not found"),
    }
}

fn execute_query(sql: &str, catalog: &CatalogManager) {
    let start = Instant::now();

    let plan = match sql_to_logical_plan(sql) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("SQL error: {e}");
            return;
        }
    };

    let mut operator = match planner::plan_to_operator(&plan, catalog) {
        Ok(op) => op,
        Err(e) => {
            eprintln!("Planning error: {e}");
            return;
        }
    };

    let batches = match collect_batches(&mut *operator) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Execution error: {e}");
            return;
        }
    };

    let elapsed = start.elapsed();
    print!("{}", display::format_batches(&batches));
    println!("Time: {:.3}s", elapsed.as_secs_f64());
}
