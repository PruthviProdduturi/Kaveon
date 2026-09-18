//! The embedded engine behind `--local`.
//!
//! `LocalEngine` owns the catalog and plans and runs statements in-process,
//! handing back the same `columns` / `rows` / `elapsed` shape a coordinator
//! serves so the shell and renderers need not know which side ran the query.
//! `run` is today's stdin REPL and `-e` path over that engine.

pub mod catalog;
pub mod config;
pub mod display;
pub mod planner;
mod repl;
// The JSON row shape is consumed by the shared shell once `--local` runs
// through it; until then the stdin REPL drives the engine via `execute_batches`.
#[allow(dead_code)]
pub mod rows;

use crate::args::Options;
use arrow::record_batch::RecordBatch;
use catalog::{CatalogCommand, split_target};
use kaveon_core::{CatalogManager, CatalogProvider, MemoryCatalog, StorageType, TableReference};
use kaveon_sql::logical_plan::sql_to_logical_plan;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Where the engine's catalog came from; drives the banner and `description`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalSource {
    /// `--data-dir`: one catalog discovered from a directory.
    DataDir(PathBuf),
    /// `--config` or `~/.kaveon/config.toml`.
    Config(PathBuf),
    /// No data directory and no config file: an empty `kaveon.default`.
    Empty,
}

/// A finished statement in the coordinator's shape.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub struct LocalResult {
    /// `(name, type)` per output column, types spelled as Arrow presents them.
    pub columns: Vec<(String, String)>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub elapsed_ms: u64,
}

/// One table as `.describe` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableDetails {
    pub catalog: String,
    pub schema: String,
    pub table: String,
    pub format: String,
    pub access: String,
    pub location: String,
    /// `(name, type, nullable)` per column.
    pub columns: Vec<(String, String, bool)>,
}

pub struct LocalEngine {
    catalog: CatalogManager,
    source: LocalSource,
}

impl LocalEngine {
    /// Builds the catalog the way `--local` always has: from `--data-dir`
    /// when given, else from `--config` or the default config path, else an
    /// empty catalog when the default config does not exist.
    pub fn open(options: &Options) -> Result<LocalEngine, String> {
        if let Some(dir) = &options.data_dir {
            return Self::from_data_dir(dir);
        }
        match &options.config_path {
            Some(path) => Self::from_config(path),
            None => {
                let path = config::default_config_path();
                if path.exists() || path.parent().is_some_and(|p| p.join("catalogs").is_dir()) {
                    Self::from_config(&path)
                } else {
                    Ok(Self::empty())
                }
            }
        }
    }

    pub fn from_data_dir(dir: &Path) -> Result<LocalEngine, String> {
        if !dir.is_dir() {
            return Err(format!("data directory not found: {}", dir.display()));
        }
        let mut catalog = CatalogManager::new("kaveon", "default");
        catalog.register_catalog(Box::new(catalog::build_local_catalog(dir)));
        Ok(LocalEngine {
            catalog,
            source: LocalSource::DataDir(dir.to_path_buf()),
        })
    }

    pub fn from_config(path: &Path) -> Result<LocalEngine, String> {
        let catalog = config::load_config(path).map_err(|error| error.to_string())?;
        Ok(LocalEngine {
            catalog,
            source: LocalSource::Config(path.to_path_buf()),
        })
    }

    fn empty() -> LocalEngine {
        let mut catalog = CatalogManager::new("kaveon", "default");
        catalog.register_catalog(Box::new(
            MemoryCatalog::new(
                "kaveon",
                StorageType::Local {
                    base_path: PathBuf::from("."),
                },
            )
            .with_schema("default"),
        ));
        LocalEngine {
            catalog,
            source: LocalSource::Empty,
        }
    }

    pub fn source(&self) -> &LocalSource {
        &self.source
    }

    /// Plans and runs one statement. `SHOW CATALOGS` / `SHOW SCHEMAS` /
    /// `SHOW TABLES` / `DESCRIBE` are answered from the catalog; `USE` must go
    /// through `use_context` because it changes the session.
    #[allow(dead_code)]
    pub fn execute(&self, sql: &str) -> Result<LocalResult, String> {
        let start = Instant::now();
        let (columns, rows) = match catalog::parse_catalog_command(sql) {
            Some(CatalogCommand::ShowCatalogs) => (
                vec![("Catalog".to_owned(), "Utf8".to_owned())],
                self.catalogs().into_iter().map(single).collect(),
            ),
            Some(CatalogCommand::ShowSchemas { catalog }) => {
                let catalog = catalog.unwrap_or_else(|| self.catalog.default_catalog().to_owned());
                (
                    vec![("Schema".to_owned(), "Utf8".to_owned())],
                    self.schemas(&catalog)?.into_iter().map(single).collect(),
                )
            }
            Some(CatalogCommand::ShowTables { target }) => {
                let (catalog, schema) = match target {
                    Some(target) => split_target(&target, self.catalog.default_schema())?,
                    None => self.context(),
                };
                (
                    vec![("Table".to_owned(), "Utf8".to_owned())],
                    self.tables(&catalog, &schema)?
                        .into_iter()
                        .map(single)
                        .collect(),
                )
            }
            Some(CatalogCommand::Describe { reference }) => (
                vec![
                    ("Column".to_owned(), "Utf8".to_owned()),
                    ("Type".to_owned(), "Utf8".to_owned()),
                    ("Nullable".to_owned(), "Boolean".to_owned()),
                ],
                self.describe(&reference)?
                    .into_iter()
                    .map(|(name, data_type, nullable)| {
                        vec![name.into(), data_type.into(), nullable.into()]
                    })
                    .collect(),
            ),
            Some(CatalogCommand::Use { .. }) => {
                return Err("USE changes the session; run it through use_context".to_owned());
            }
            None => {
                let (schema, batches) = self.execute_batches(sql)?;
                (rows::columns_of(&schema), rows::batches_to_rows(&batches))
            }
        };
        Ok(LocalResult {
            columns,
            rows,
            elapsed_ms: elapsed_ms(start),
        })
    }

    /// The SQL path with Arrow batches kept intact, for the local table
    /// renderer. Returns the operator's output schema (present even when no
    /// batch is produced) and the collected batches.
    pub fn execute_batches(
        &self,
        sql: &str,
    ) -> Result<(arrow::datatypes::SchemaRef, Vec<RecordBatch>), String> {
        let plan = sql_to_logical_plan(sql).map_err(|error| format!("SQL error: {error}"))?;
        let plan = kaveon_optim::rules::push_filter_down(plan);
        let plan = kaveon_optim::rules::push_projection_down(plan);
        let plan = kaveon_optim::statistics::optimize_join_builds(plan, &self.catalog);
        let mut operator = planner::plan_to_operator(&plan, &self.catalog)
            .map_err(|error| format!("Planning error: {error}"))?;
        let schema = operator.schema().clone();
        let batches = kaveon_core::collect_batches(&mut *operator)
            .map_err(|error| format!("Execution error: {error}"))?;
        Ok((schema, batches))
    }

    pub fn catalogs(&self) -> Vec<String> {
        let mut names = self.catalog.catalog_names();
        names.sort();
        names
    }

    pub fn schemas(&self, catalog: &str) -> Result<Vec<String>, String> {
        let mut names = self.provider(catalog)?.schema_names();
        names.sort();
        Ok(names)
    }

    pub fn tables(&self, catalog: &str, schema: &str) -> Result<Vec<String>, String> {
        let mut names = self
            .provider(catalog)?
            .table_names(schema)
            .map_err(|error| error.to_string())?;
        names.sort();
        Ok(names)
    }

    /// `(name, type, nullable)` for each column of `reference`, resolved
    /// against the session's catalog and schema when unqualified.
    pub fn describe(&self, reference: &str) -> Result<Vec<(String, String, bool)>, String> {
        Ok(self.table_details(reference)?.columns)
    }

    pub fn table_details(&self, reference: &str) -> Result<TableDetails, String> {
        let resolved = self
            .catalog
            .resolve_table(&TableReference::parse(reference))
            .map_err(|error| error.to_string())?;
        let columns = resolved
            .table
            .arrow_schema
            .fields()
            .iter()
            .map(|field| {
                (
                    field.name().clone(),
                    rows::presented_type(field.data_type()),
                    field.is_nullable(),
                )
            })
            .collect();
        Ok(TableDetails {
            catalog: resolved.catalog.clone(),
            schema: resolved.schema.clone(),
            table: resolved.table.name.clone(),
            format: format!("{:?}", resolved.table.format),
            access: format!("{:?}", resolved.table.access),
            location: resolved.full_path(),
            columns,
        })
    }

    /// `USE catalog[.schema]`; a bare catalog keeps the session's schema.
    pub fn use_context(&mut self, target: &str) -> Result<(String, String), String> {
        let (catalog, schema) = split_target(target, self.catalog.default_schema())?;
        self.catalog
            .set_default(&catalog, &schema)
            .map_err(|error| error.to_string())?;
        Ok((catalog, schema))
    }

    /// `(catalog, schema)` the session resolves unqualified names against.
    pub fn context(&self) -> (String, String) {
        (
            self.catalog.default_catalog().to_owned(),
            self.catalog.default_schema().to_owned(),
        )
    }

    /// One line for the shell header, e.g. `embedded · D:\data (3 tables)` or
    /// `embedded · ~/.kaveon/config.toml (2 catalogs, 5 tables)`.
    #[allow(dead_code)]
    pub fn description(&self) -> String {
        let tables = catalog::count_tables(&self.catalog);
        let plural = |count: usize, noun: &str| {
            if count == 1 {
                format!("{count} {noun}")
            } else {
                format!("{count} {noun}s")
            }
        };
        match &self.source {
            LocalSource::DataDir(dir) => {
                format!("embedded · {} ({})", dir.display(), plural(tables, "table"))
            }
            LocalSource::Config(path) => format!(
                "embedded · {} ({}, {})",
                path.display(),
                plural(self.catalog.catalog_names().len(), "catalog"),
                plural(tables, "table")
            ),
            LocalSource::Empty => {
                format!("embedded · no data directory ({})", plural(tables, "table"))
            }
        }
    }

    fn provider(&self, catalog: &str) -> Result<&dyn CatalogProvider, String> {
        self.catalog
            .catalog(catalog)
            .ok_or_else(|| format!("catalog '{catalog}' not found"))
    }
}

/// Today's `--local` entry point: banner, then `-e` or the stdin REPL.
pub fn run(options: Options) -> Result<(), String> {
    let mut engine = LocalEngine::open(&options)?;
    repl::print_banner(&engine);
    match options.execute {
        Some(sql) => {
            repl::run_statement(&mut engine, &sql);
            Ok(())
        }
        None => repl::run(&mut engine),
    }
}

fn single(value: String) -> Vec<serde_json::Value> {
    vec![serde_json::Value::String(value)]
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;
    use serde_json::json;

    fn open_fixture() -> (LocalEngine, PathBuf) {
        let dir = catalog::tests::parquet_fixture();
        let args = [
            "kaveon".to_owned(),
            "--local".to_owned(),
            "--data-dir".to_owned(),
            dir.to_string_lossy().into_owned(),
        ];
        let args::Command::Run(options) = args::parse(&args).expect("args should parse") else {
            panic!("expected a run command");
        };
        let engine = LocalEngine::open(&options).expect("engine should open");
        (engine, dir)
    }

    #[test]
    fn counts_rows_of_a_discovered_table() {
        let (engine, dir) = open_fixture();
        let result = engine
            .execute("SELECT COUNT(*) FROM events")
            .expect("count should run");
        assert_eq!(result.columns.len(), 1);
        assert_eq!(result.columns[0].1, "UInt64");
        assert_eq!(result.rows, vec![vec![json!(3)]]);
        assert!(result.rows[0][0].is_u64());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn projects_rows_with_nulls_as_json() {
        let (engine, dir) = open_fixture();
        let result = engine
            .execute("SELECT id, kind FROM events ORDER BY id")
            .expect("select should run");
        assert_eq!(
            result.columns,
            vec![
                ("id".to_owned(), "Int64".to_owned()),
                ("kind".to_owned(), "Utf8".to_owned())
            ]
        );
        assert_eq!(
            result.rows,
            vec![
                vec![json!(1), json!("click")],
                vec![json!(2), serde_json::Value::Null],
                vec![json!(3), json!("view")],
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn answers_catalog_statements_and_switches_context() {
        let (mut engine, dir) = open_fixture();
        assert_eq!(engine.catalogs(), vec!["kaveon"]);
        assert_eq!(engine.schemas("kaveon").unwrap(), vec!["default"]);
        assert_eq!(engine.tables("kaveon", "default").unwrap(), vec!["events"]);
        assert!(engine.schemas("missing").is_err());
        assert_eq!(
            engine.describe("events").unwrap(),
            vec![
                ("id".to_owned(), "Int64".to_owned(), false),
                ("kind".to_owned(), "Utf8".to_owned(), true),
            ]
        );
        assert_eq!(
            engine.execute("SHOW TABLES").unwrap().rows,
            vec![vec![json!("events")]]
        );
        assert_eq!(engine.execute("DESCRIBE events").unwrap().rows.len(), 2);
        assert!(engine.execute("USE kaveon").is_err());
        assert_eq!(
            engine.use_context("kaveon.default").unwrap(),
            ("kaveon".to_owned(), "default".to_owned())
        );
        assert!(engine.use_context("kaveon.nope").is_err());
        assert_eq!(
            engine.context(),
            ("kaveon".to_owned(), "default".to_owned())
        );
        assert_eq!(
            engine.description(),
            format!("embedded · {} (1 table)", dir.display())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn errors_surface_as_strings() {
        let (engine, dir) = open_fixture();
        assert!(
            engine
                .execute("SELECT * FROM nowhere")
                .unwrap_err()
                .contains("nowhere")
        );
        assert!(engine.execute("SELEC 1").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_data_dir_is_an_error() {
        assert!(LocalEngine::from_data_dir(Path::new("no-such-dir-for-kaveon")).is_err());
    }
}
