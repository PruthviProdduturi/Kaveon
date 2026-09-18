//! `kaveon catalog|schema|table …`: catalog administration from the command
//! line. Each command is one catalog statement submitted to the coordinator
//! through `POST /v1/statement` — the same DDL, role checks, revisions and
//! audit trail as an interactive `CREATE TABLE` — except `catalog show`,
//! which reads the durable definition from the catalog API.

use kaveon_sql::ddl::{parse_catalog_statement, quote_identifier};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminCommand {
    CatalogList {
        like: Option<String>,
    },
    CatalogShow {
        name: String,
    },
    CatalogAdd {
        name: String,
        if_not_exists: bool,
        options: Vec<(String, String)>,
    },
    CatalogDrop {
        name: String,
        if_exists: bool,
        cascade: bool,
    },
    SchemaList {
        catalog: Option<String>,
        like: Option<String>,
    },
    SchemaAdd {
        name: String,
        if_not_exists: bool,
    },
    SchemaDrop {
        name: String,
        if_exists: bool,
        cascade: bool,
    },
    TableList {
        schema: Option<String>,
        like: Option<String>,
    },
    TableRegister {
        name: String,
        if_not_exists: bool,
        location: String,
        format: String,
        access: Option<String>,
        columns: Option<String>,
    },
    TableDrop {
        name: String,
        if_exists: bool,
    },
    TableRelocate {
        name: String,
        if_exists: bool,
        location: String,
    },
    TableDescribe {
        name: String,
    },
    TableShowCreate {
        name: String,
    },
}

/// Whether `word` opens an administration command.
pub fn is_noun(word: &str) -> bool {
    matches!(word, "catalog" | "schema" | "table")
}

const BOOLEAN_FLAGS: [&str; 3] = ["--cascade", "--if-exists", "--if-not-exists"];
const VALUE_FLAGS: [&str; 13] = [
    "--location",
    "--format",
    "--access",
    "--columns",
    "--like",
    "--storage",
    "--account",
    "--container",
    "--root",
    "--base-path",
    "--bucket",
    "--region",
    "--prefix",
];
const CREDENTIAL_FLAG: &str = "--credential";

/// Split `args` (everything after the program name, starting with the
/// noun) into the administration command and the connection arguments the
/// ordinary option parser understands.
pub fn split(args: &[String]) -> Result<(AdminCommand, Vec<String>), String> {
    let noun = args.first().map(String::as_str).unwrap_or_default();
    let mut positionals: Vec<String> = Vec::new();
    let mut flags: Vec<(String, Option<String>)> = Vec::new();
    let mut connection = Vec::new();
    let mut index = 1;
    while index < args.len() {
        let arg = args[index].as_str();
        let (key, inline) = match arg.split_once('=') {
            Some((key, value)) if key.starts_with("--") => (key, Some(value.to_owned())),
            _ => (arg, None),
        };
        if BOOLEAN_FLAGS.contains(&key) {
            if inline.is_some() {
                return Err(format!("{key} does not take a value"));
            }
            flags.push((key.to_owned(), None));
            index += 1;
        } else if VALUE_FLAGS.contains(&key) || key == CREDENTIAL_FLAG {
            let value = match inline {
                Some(value) => value,
                None => {
                    index += 1;
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| format!("{key} requires a value"))?
                }
            };
            flags.push((key.to_owned(), Some(value)));
            index += 1;
        } else if arg.starts_with('-') {
            connection.push(arg.to_owned());
            // Connection options take a value unless they are known flags;
            // `args::parse` validates them.
            let is_flag = matches!(
                arg,
                "--local"
                    | "--ignore-errors"
                    | "--no-history"
                    | "--disable-auto-suggestion"
                    | "--help"
                    | "-h"
                    | "--version"
                    | "-V"
            );
            if !is_flag && inline.is_none() {
                index += 1;
                if let Some(value) = args.get(index) {
                    connection.push(value.clone());
                }
            }
            index += 1;
        } else if arg.starts_with("http://") || arg.starts_with("https://") {
            connection.push(arg.to_owned());
            index += 1;
        } else {
            positionals.push(arg.to_owned());
            index += 1;
        }
    }
    let command = build(noun, &positionals, &flags)?;
    Ok((command, connection))
}

fn build(
    noun: &str,
    positionals: &[String],
    flags: &[(String, Option<String>)],
) -> Result<AdminCommand, String> {
    let verb = positionals.first().map(String::as_str).ok_or_else(|| {
        format!(
            "kaveon {noun} requires a command: {}",
            verbs_for(noun).join(", ")
        )
    })?;
    if !verbs_for(noun).contains(&verb) {
        return Err(format!(
            "unknown command 'kaveon {noun} {verb}'; expected one of: {}",
            verbs_for(noun).join(", ")
        ));
    }
    if positionals.len() > 2 {
        return Err(format!(
            "unexpected argument '{}' after 'kaveon {noun} {verb}'",
            positionals[2]
        ));
    }
    let name = positionals.get(1).cloned();
    let has = |flag: &str| flags.iter().any(|(key, _)| key == flag);
    let value = |flag: &str| -> Option<String> {
        flags
            .iter()
            .find(|(key, _)| key == flag)
            .and_then(|(_, value)| value.clone())
    };
    let allowed: &[&str] = match (noun, verb) {
        ("catalog", "list") | ("schema", "list") | ("table", "list") => &["--like"],
        ("catalog", "show") => &[],
        ("catalog", "add") => &[
            "--if-not-exists",
            "--storage",
            "--account",
            "--container",
            "--root",
            "--base-path",
            "--bucket",
            "--region",
            "--prefix",
            "--credential",
        ],
        ("catalog", "drop") | ("schema", "drop") => &["--if-exists", "--cascade"],
        ("schema", "add") => &["--if-not-exists"],
        ("table", "register") => &[
            "--if-not-exists",
            "--location",
            "--format",
            "--access",
            "--columns",
        ],
        ("table", "drop") => &["--if-exists"],
        ("table", "relocate") => &["--if-exists", "--location"],
        ("table", "describe") | ("table", "show-create") => &[],
        _ => &[],
    };
    if let Some((key, _)) = flags
        .iter()
        .find(|(key, _)| !allowed.contains(&key.as_str()))
    {
        return Err(format!("{key} does not apply to 'kaveon {noun} {verb}'"));
    }
    let require_name = |what: &str| {
        name.clone()
            .ok_or_else(|| format!("kaveon {noun} {verb} requires {what}"))
    };
    let no_name = || {
        if let Some(name) = &name {
            Err(format!(
                "unexpected argument '{name}' after 'kaveon {noun} {verb}'"
            ))
        } else {
            Ok(())
        }
    };
    Ok(match (noun, verb) {
        ("catalog", "list") => {
            no_name()?;
            AdminCommand::CatalogList {
                like: value("--like"),
            }
        }
        ("catalog", "show") => AdminCommand::CatalogShow {
            name: require_name("a catalog name")?,
        },
        ("catalog", "add") => {
            let name = require_name("a catalog name")?;
            let storage = value("--storage")
                .ok_or_else(|| "kaveon catalog add requires --storage adls|local|s3".to_owned())?;
            let mut options = vec![("storage".to_owned(), storage)];
            for flag in [
                "--account",
                "--container",
                "--root",
                "--base-path",
                "--bucket",
                "--region",
                "--prefix",
                "--credential",
            ] {
                if let Some(value) = value(flag) {
                    options.push((flag[2..].replace('-', "_"), value));
                }
            }
            AdminCommand::CatalogAdd {
                name,
                if_not_exists: has("--if-not-exists"),
                options,
            }
        }
        ("catalog", "drop") => AdminCommand::CatalogDrop {
            name: require_name("a catalog name")?,
            if_exists: has("--if-exists"),
            cascade: has("--cascade"),
        },
        ("schema", "list") => AdminCommand::SchemaList {
            catalog: name.clone(),
            like: value("--like"),
        },
        ("schema", "add") => AdminCommand::SchemaAdd {
            name: require_name("a [catalog.]schema name")?,
            if_not_exists: has("--if-not-exists"),
        },
        ("schema", "drop") => AdminCommand::SchemaDrop {
            name: require_name("a [catalog.]schema name")?,
            if_exists: has("--if-exists"),
            cascade: has("--cascade"),
        },
        ("table", "list") => AdminCommand::TableList {
            schema: name.clone(),
            like: value("--like"),
        },
        ("table", "register") => AdminCommand::TableRegister {
            name: require_name("a [catalog.][schema.]table name")?,
            if_not_exists: has("--if-not-exists"),
            location: value("--location")
                .ok_or_else(|| "kaveon table register requires --location".to_owned())?,
            format: value("--format").ok_or_else(|| {
                "kaveon table register requires --format parquet|delta|iceberg".to_owned()
            })?,
            access: value("--access"),
            columns: value("--columns"),
        },
        ("table", "drop") => AdminCommand::TableDrop {
            name: require_name("a [catalog.][schema.]table name")?,
            if_exists: has("--if-exists"),
        },
        ("table", "relocate") => AdminCommand::TableRelocate {
            name: require_name("a [catalog.][schema.]table name")?,
            if_exists: has("--if-exists"),
            location: value("--location")
                .ok_or_else(|| "kaveon table relocate requires --location".to_owned())?,
        },
        ("table", "describe") => AdminCommand::TableDescribe {
            name: require_name("a [catalog.][schema.]table name")?,
        },
        ("table", "show-create") => AdminCommand::TableShowCreate {
            name: require_name("a [catalog.][schema.]table name")?,
        },
        _ => unreachable!("verbs are validated above"),
    })
}

fn verbs_for(noun: &str) -> &'static [&'static str] {
    match noun {
        "catalog" => &["list", "show", "add", "drop"],
        "schema" => &["list", "add", "drop"],
        "table" => &[
            "list",
            "register",
            "drop",
            "relocate",
            "describe",
            "show-create",
        ],
        _ => &[],
    }
}

impl AdminCommand {
    /// The catalog statement this command submits, or `None` for commands
    /// that read the catalog API instead.
    pub fn statement(&self) -> Result<Option<String>, String> {
        let sql = match self {
            Self::CatalogList { like } => format!("SHOW CATALOGS{}", like_clause(like)),
            Self::CatalogShow { .. } => return Ok(None),
            Self::CatalogAdd {
                name,
                if_not_exists,
                options,
            } => format!(
                "CREATE CATALOG {}{} WITH ({})",
                if *if_not_exists { "IF NOT EXISTS " } else { "" },
                quote_identifier(name),
                options
                    .iter()
                    .map(|(key, value)| format!("{key} = {}", literal(value)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::CatalogDrop {
                name,
                if_exists,
                cascade,
            } => format!(
                "DROP CATALOG {}{}{}",
                if *if_exists { "IF EXISTS " } else { "" },
                quote_identifier(name),
                if *cascade { " CASCADE" } else { "" }
            ),
            Self::SchemaList { catalog, like } => format!(
                "SHOW SCHEMAS{}{}",
                match catalog {
                    Some(catalog) => format!(" FROM {}", quote_identifier(catalog)),
                    None => String::new(),
                },
                like_clause(like)
            ),
            Self::SchemaAdd {
                name,
                if_not_exists,
            } => format!(
                "CREATE SCHEMA {}{}",
                if *if_not_exists { "IF NOT EXISTS " } else { "" },
                qualified(name, 2)?
            ),
            Self::SchemaDrop {
                name,
                if_exists,
                cascade,
            } => format!(
                "DROP SCHEMA {}{}{}",
                if *if_exists { "IF EXISTS " } else { "" },
                qualified(name, 2)?,
                if *cascade { " CASCADE" } else { "" }
            ),
            Self::TableList { schema, like } => format!(
                "SHOW TABLES{}{}",
                match schema {
                    Some(schema) => format!(" FROM {}", qualified(schema, 2)?),
                    None => String::new(),
                },
                like_clause(like)
            ),
            Self::TableRegister {
                name,
                if_not_exists,
                location,
                format,
                access,
                columns,
            } => {
                let mut options = vec![
                    format!("location = {}", literal(location)),
                    format!("format = {}", literal(format)),
                ];
                if let Some(access) = access {
                    options.push(format!("access = {}", literal(access)));
                }
                format!(
                    "CREATE TABLE {}{}{} WITH ({})",
                    if *if_not_exists { "IF NOT EXISTS " } else { "" },
                    qualified(name, 3)?,
                    match columns {
                        Some(columns) => format!(" ({columns})"),
                        None => String::new(),
                    },
                    options.join(", ")
                )
            }
            Self::TableDrop { name, if_exists } => format!(
                "DROP TABLE {}{}",
                if *if_exists { "IF EXISTS " } else { "" },
                qualified(name, 3)?
            ),
            Self::TableRelocate {
                name,
                if_exists,
                location,
            } => format!(
                "ALTER TABLE {}{} SET LOCATION {}",
                if *if_exists { "IF EXISTS " } else { "" },
                qualified(name, 3)?,
                literal(location)
            ),
            Self::TableDescribe { name } => format!("DESCRIBE {}", qualified(name, 3)?),
            Self::TableShowCreate { name } => {
                format!("SHOW CREATE TABLE {}", qualified(name, 3)?)
            }
        };
        // The statement is checked here so a mistake is reported by the
        // client, naming the option, before anything reaches the coordinator.
        match parse_catalog_statement(&sql) {
            Ok(Some(_)) => Ok(Some(sql)),
            Ok(None) => Err(format!(
                "internal error: '{sql}' is not a catalog statement"
            )),
            Err(error) => Err(error.to_string().trim_start_matches("sql: ").to_owned()),
        }
    }
}

fn like_clause(like: &Option<String>) -> String {
    match like {
        Some(pattern) => format!(" LIKE {}", literal(pattern)),
        None => String::new(),
    }
}

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// `a.b.c` with each part quoted as needed; a part written in double
/// quotes on the command line keeps its dots.
fn qualified(name: &str, max_parts: usize) -> Result<String, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in name.chars() {
        match character {
            '"' => quoted = !quoted,
            '.' if !quoted => {
                parts.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    if quoted {
        return Err(format!("unbalanced double quote in '{name}'"));
    }
    parts.push(current);
    if parts.iter().any(String::is_empty) {
        return Err(format!("'{name}' has an empty name part"));
    }
    if parts.len() > max_parts {
        return Err(format!(
            "'{name}' has {} parts; at most {max_parts} are allowed",
            parts.len()
        ));
    }
    Ok(parts
        .iter()
        .map(|part| quote_identifier(part))
        .collect::<Vec<_>>()
        .join("."))
}

pub fn print_usage() {
    println!("Usage: kaveon <catalog|schema|table> <command> [ARGS] [connection options]");
    println!();
    println!("Catalog administration through the coordinator's catalog statements.");
    println!("Connection options are the same as for the shell (--server, --catalog,");
    println!("--schema, --auth, --access-token, --ca-cert, --timeout, --output-format).");
    println!("Unqualified names resolve against the session --catalog and --schema.");
    println!();
    println!("Catalogs (admin role):");
    println!("  kaveon catalog list [--like 'pattern']");
    println!("  kaveon catalog show <name>                       Durable definition as JSON");
    println!(
        "  kaveon catalog add <name> --storage adls --account <a> --container <c> [--root <path>]"
    );
    println!(
        "                            [--credential workload-identity:<ref>] [--if-not-exists]"
    );
    println!("  kaveon catalog add <name> --storage local --base-path <absolute path>");
    println!("  kaveon catalog add <name> --storage s3 --bucket <b> --region <r> [--prefix <p>]");
    println!("  kaveon catalog drop <name> [--cascade] [--if-exists]");
    println!();
    println!("Schemas (analyst or admin role):");
    println!("  kaveon schema list [catalog] [--like 'pattern']");
    println!("  kaveon schema add <[catalog.]schema> [--if-not-exists]");
    println!("  kaveon schema drop <[catalog.]schema> [--cascade] [--if-exists]");
    println!();
    println!("Tables (analyst or admin role):");
    println!("  kaveon table list [[catalog.]schema] [--like 'pattern']");
    println!(
        "  kaveon table register <[catalog.][schema.]table> --location <path> --format parquet|delta|iceberg"
    );
    println!(
        "                        [--access shortcut|optimized] [--columns 'id bigint not null, name varchar']"
    );
    println!("                        [--if-not-exists]");
    println!("      Without --columns the coordinator reads them from the table itself.");
    println!("      The location is probed before the table is activated; an unreadable");
    println!("      location fails the command and registers nothing.");
    println!("  kaveon table relocate <name> --location <path> [--if-exists]");
    println!("  kaveon table drop <name> [--if-exists]");
    println!("  kaveon table describe <name>");
    println!("  kaveon table show-create <name>");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn table_register_renders_create_table_and_keeps_connection_options() {
        let (command, connection) = split(&strings(&[
            "table",
            "register",
            "lake.sales.orders",
            "--location",
            "sales/orders",
            "--format=delta",
            "--server",
            "https://engine.example",
            "--if-not-exists",
            "--output-format",
            "json",
        ]))
        .unwrap();
        assert_eq!(
            connection,
            strings(&[
                "--server",
                "https://engine.example",
                "--output-format",
                "json"
            ])
        );
        assert_eq!(
            command.statement().unwrap().unwrap(),
            "CREATE TABLE IF NOT EXISTS lake.sales.orders WITH (location = 'sales/orders', format = 'delta')"
        );
    }

    #[test]
    fn declared_columns_and_quoted_names_are_rendered_into_the_statement() {
        let (command, _) = split(&strings(&[
            "table",
            "register",
            "OpenSource.\"nyc.taxi\".trips",
            "--location",
            "nyc/trips.parquet",
            "--format",
            "parquet",
            "--access",
            "optimized",
            "--columns",
            "id bigint not null, name varchar",
        ]))
        .unwrap();
        assert_eq!(
            command.statement().unwrap().unwrap(),
            "CREATE TABLE \"OpenSource\".\"nyc.taxi\".trips (id bigint not null, name varchar) WITH (location = 'nyc/trips.parquet', format = 'parquet', access = 'optimized')"
        );
        let (command, _) = split(&strings(&[
            "table",
            "register",
            "orders",
            "--location",
            "orders",
            "--format",
            "delta",
            "--columns",
            "id geometry",
        ]))
        .unwrap();
        let error = command.statement().unwrap_err();
        assert!(error.contains("geometry"), "{error}");
    }

    #[test]
    fn catalog_add_renders_storage_options_and_the_credential() {
        let (command, _) = split(&strings(&[
            "catalog",
            "add",
            "Benchmarks",
            "--storage",
            "adls",
            "--account",
            "kvtest",
            "--container",
            "opensource",
            "--root",
            "benchmarks",
            "--credential",
            "workload-identity:kaveon-test-reader",
        ]))
        .unwrap();
        assert_eq!(
            command.statement().unwrap().unwrap(),
            "CREATE CATALOG \"Benchmarks\" WITH (storage = 'adls', account = 'kvtest', container = 'opensource', root = 'benchmarks', credential = 'workload-identity:kaveon-test-reader')"
        );
        let (command, _) = split(&strings(&[
            "catalog",
            "add",
            "local",
            "--storage",
            "local",
            "--base-path",
            "D:\\data",
        ]))
        .unwrap();
        assert_eq!(
            command.statement().unwrap().unwrap(),
            "CREATE CATALOG local WITH (storage = 'local', base_path = 'D:\\data')"
        );
        let (command, _) = split(&strings(&["catalog", "add", "c", "--storage", "adls"])).unwrap();
        let error = command.statement().unwrap_err();
        assert!(error.contains("account"), "{error}");
    }

    #[test]
    fn drop_relocate_describe_and_lists_render_their_statements() {
        let cases: [(&[&str], &str); 8] = [
            (
                &["schema", "add", "lake.sales", "--if-not-exists"],
                "CREATE SCHEMA IF NOT EXISTS lake.sales",
            ),
            (
                &["schema", "drop", "sales", "--cascade", "--if-exists"],
                "DROP SCHEMA IF EXISTS sales CASCADE",
            ),
            (
                &["table", "drop", "lake.sales.orders", "--if-exists"],
                "DROP TABLE IF EXISTS lake.sales.orders",
            ),
            (
                &["table", "relocate", "orders", "--location", "v2/orders"],
                "ALTER TABLE orders SET LOCATION 'v2/orders'",
            ),
            (&["table", "describe", "orders"], "DESCRIBE orders"),
            (
                &["table", "show-create", "sales.orders"],
                "SHOW CREATE TABLE sales.orders",
            ),
            (
                &["table", "list", "lake.sales", "--like", "%_2026"],
                "SHOW TABLES FROM lake.sales LIKE '%_2026'",
            ),
            (
                &["catalog", "drop", "lake", "--cascade"],
                "DROP CATALOG lake CASCADE",
            ),
        ];
        for (args, expected) in cases {
            let (command, connection) = split(&strings(args)).unwrap();
            assert!(connection.is_empty());
            assert_eq!(command.statement().unwrap().unwrap(), expected, "{args:?}");
        }
        let (command, _) = split(&strings(&["catalog", "list"])).unwrap();
        assert_eq!(command.statement().unwrap().unwrap(), "SHOW CATALOGS");
        let (command, _) = split(&strings(&["schema", "list"])).unwrap();
        assert_eq!(command.statement().unwrap().unwrap(), "SHOW SCHEMAS");
        let (command, _) = split(&strings(&["catalog", "show", "lake"])).unwrap();
        assert_eq!(command.statement().unwrap(), None);
    }

    #[test]
    fn errors_name_the_missing_or_misplaced_argument() {
        let cases = [
            (&["table"][..], "requires a command"),
            (
                &["table", "vacuum", "x"][..],
                "unknown command 'kaveon table vacuum'",
            ),
            (&["table", "register", "orders"][..], "--location"),
            (
                &["table", "register", "orders", "--location", "x"][..],
                "--format",
            ),
            (&["table", "describe"][..], "requires a"),
            (
                &["table", "describe", "orders", "--cascade"][..],
                "--cascade does not apply",
            ),
            (&["catalog", "add", "c"][..], "--storage"),
            (
                &["catalog", "list", "extra"][..],
                "unexpected argument 'extra'",
            ),
            (&["schema", "add", "a.b.c"][..], "at most 2"),
            (&["table", "drop", "\"unbalanced"][..], "unbalanced"),
            (&["table", "drop", "a..b"][..], "empty name part"),
            (
                &["table", "drop", "x", "--location"][..],
                "requires a value",
            ),
        ];
        for (args, expected) in cases {
            let error = match split(&strings(args)) {
                Ok((command, _)) => command.statement().unwrap_err(),
                Err(error) => error,
            };
            assert!(error.contains(expected), "{args:?}: {error}");
        }
    }
}
