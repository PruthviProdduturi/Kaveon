mod args;
mod config;
mod display;
mod planner;
mod remote;

use kaveon_core::{
    AccessPattern, CatalogManager, CatalogProvider, DataFormat, MemoryCatalog, StorageType,
    TableMeta, TableReference, collect_batches,
};
use kaveon_sql::logical_plan::sql_to_logical_plan;
use kaveon_storage::{DeltaTableReader, ParquetReader};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args::parse(&args) {
        Ok(args::Command::Help) => print_usage(),
        Ok(args::Command::Version) => println!("kaveon {VERSION}"),
        Ok(args::Command::Run(options)) if options.local => run_local(*options),
        Ok(args::Command::Run(mut options)) => {
            if let Err(error) = remote::run(&mut options) {
                eprintln!("error: {error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("Run 'kaveon --help' for usage.");
            std::process::exit(2);
        }
    }
}

fn run_local(options: args::Options) {
    let mut catalog_mgr = if let Some(dir) = &options.data_dir {
        let mut mgr = CatalogManager::new("kaveon", "default");
        let catalog = build_local_catalog(dir);
        let table_count = catalog.table_names("default").map(|t| t.len()).unwrap_or(0);
        mgr.register_catalog(Box::new(catalog));
        print_banner(Some(dir), table_count);
        mgr
    } else {
        let cfg_path = options
            .config_path
            .clone()
            .unwrap_or_else(config::default_config_path);
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

    if let Some(sql) = options.execute {
        execute_query(&sql, &catalog_mgr);
    } else {
        repl(&mut catalog_mgr);
    }
}

fn print_usage() {
    println!("Usage: kaveon [OPTIONS]");
    println!();
    println!("Remote coordinator mode is the default.");
    println!();
    println!("Options:");
    println!("      --server <URL>          Coordinator URL (default: http://localhost:8080)");
    println!("      --catalog <NAME>        Session catalog (default: kaveon)");
    println!("      --schema <NAME>         Session schema (default: default)");
    println!("      --user <NAME>           Session user");
    println!("      --source <NAME>         Client source (default: kaveon-cli)");
    println!("      --client-tags <TAGS>    Comma-separated client tags");
    println!("  -e, --execute <SQL>         Execute SQL and exit");
    println!("      --output-format <TYPE>  table, csv, tsv, or json");
    println!("      --timeout <SECONDS>     HTTP request timeout (default: 30)");
    println!("      --local                 Use the embedded local engine");
    println!("  -d, --data-dir <PATH>       Local Parquet/Delta directory (requires --local)");
    println!("  -c, --config <PATH>         Local catalog config (requires --local)");
    println!("  -V, --version               Print version");
    println!("  -h, --help                  Print help");
    println!();
    println!("Meta-commands:");
    println!("  .catalogs              List catalogs");
    println!("  .schemas               List schemas in current catalog");
    println!("  .tables                List tables in current schema");
    println!("  .describe <table>      Show table schema (local mode)");
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
            if path.is_dir() && path.join("_delta_log").is_dir() {
                if let Some(table_name) = path.file_name().and_then(|s| s.to_str()) {
                    match DeltaTableReader::new(&path).metadata() {
                        Ok(meta) => {
                            let _ = catalog.register_table(
                                "default",
                                TableMeta {
                                    name: table_name.to_owned(),
                                    arrow_schema: meta.schema,
                                    location: path
                                        .file_name()
                                        .unwrap()
                                        .to_string_lossy()
                                        .into_owned(),
                                    access: AccessPattern::Shortcut,
                                    format: DataFormat::Delta,
                                },
                            );
                        }
                        Err(e) => eprintln!("warning: skipping {}: {e}", path.display()),
                    }
                }
            } else if path.extension().is_some_and(|e| e == "parquet")
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

fn handle_meta_command(cmd: &str, catalog: &mut CatalogManager) {
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
            if parts.len() != 2 {
                eprintln!("usage: .use <catalog.schema>");
                return;
            }
            use_catalog(parts[1], catalog);
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
            use_catalog(target, catalog);
            true
        }
        _ => false,
    }
}

fn use_catalog(target: &str, catalog: &mut CatalogManager) {
    let parts: Vec<&str> = target.split('.').collect();
    let (catalog_name, schema_name) = match parts.as_slice() {
        [catalog_name, schema_name] => ((*catalog_name).to_owned(), (*schema_name).to_owned()),
        [catalog_name] => (
            (*catalog_name).to_owned(),
            catalog.default_schema().to_owned(),
        ),
        _ => {
            eprintln!("usage: USE catalog.schema");
            return;
        }
    };
    match catalog.set_default(&catalog_name, &schema_name) {
        Ok(()) => println!("Using catalog '{catalog_name}', schema '{schema_name}'"),
        Err(error) => eprintln!("error: {error}"),
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
    let plan = kaveon_optim::rules::push_filter_down(plan);
    let plan = kaveon_optim::rules::push_projection_down(plan);

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
