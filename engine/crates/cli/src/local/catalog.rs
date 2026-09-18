//! Local catalog discovery and the catalog statements the embedded engine
//! answers itself: `SHOW CATALOGS`, `SHOW SCHEMAS`, `SHOW TABLES`,
//! `DESCRIBE` and `USE`.

use kaveon_core::{
    AccessPattern, CatalogManager, CatalogProvider, DataFormat, MemoryCatalog, StorageType,
    TableMeta,
};
use kaveon_storage::{DeltaTableReader, ParquetReader};
use std::path::Path;

/// A catalog rooted at `dir` whose `default` schema holds every Parquet file
/// (named by its stem) and every Delta table directory found directly in it.
pub fn build_local_catalog(dir: &Path) -> MemoryCatalog {
    let mut catalog = MemoryCatalog::new(
        "kaveon",
        StorageType::Local {
            base_path: dir.to_path_buf(),
        },
    )
    .with_schema("default");
    discover_tables(&mut catalog, dir);
    catalog
}

/// Registers the Parquet files and Delta directories directly under `dir`
/// into the catalog's `default` schema. Files whose metadata cannot be read
/// are reported on stderr and skipped.
pub fn discover_tables(catalog: &mut MemoryCatalog, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let discovered = if path.is_dir() && path.join("_delta_log").is_dir() {
            DeltaTableReader::new(&path)
                .metadata()
                .map(|meta| (file_name.to_owned(), meta.schema, DataFormat::Delta))
        } else if path
            .extension()
            .is_some_and(|extension| extension == "parquet")
        {
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            ParquetReader::new(&path)
                .metadata()
                .map(|meta| (stem.to_owned(), meta.schema, DataFormat::Parquet))
        } else {
            continue;
        };
        match discovered {
            Ok((name, arrow_schema, format)) => {
                let _ = catalog.register_table(
                    "default",
                    TableMeta {
                        name,
                        arrow_schema,
                        location: file_name.to_owned(),
                        access: AccessPattern::Shortcut,
                        format,
                    },
                );
            }
            Err(error) => eprintln!("warning: skipping {}: {error}", path.display()),
        }
    }
}

/// Tables across every catalog and schema the manager knows.
pub fn count_tables(manager: &CatalogManager) -> usize {
    manager
        .catalog_names()
        .iter()
        .filter_map(|name| manager.catalog(name))
        .flat_map(|catalog| {
            catalog
                .schema_names()
                .into_iter()
                .filter_map(move |schema| catalog.table_names(&schema).ok().map(|t| t.len()))
        })
        .sum()
}

/// A statement the embedded engine answers from its catalog rather than by
/// planning SQL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogCommand {
    ShowCatalogs,
    /// `SHOW SCHEMAS [FROM|IN catalog]`
    ShowSchemas {
        catalog: Option<String>,
    },
    /// `SHOW TABLES [FROM|IN catalog[.schema]]`
    ShowTables {
        target: Option<String>,
    },
    /// `DESCRIBE table` / `DESC table`
    Describe {
        reference: String,
    },
    /// `USE catalog[.schema]`
    Use {
        target: String,
    },
}

/// Recognises the catalog statements; anything else is SQL for the planner.
pub fn parse_catalog_command(sql: &str) -> Option<CatalogCommand> {
    let sql = sql.trim().trim_end_matches(';').trim();
    let words: Vec<&str> = sql.split_whitespace().collect();
    let upper: Vec<String> = words.iter().map(|word| word.to_uppercase()).collect();
    let keywords: Vec<&str> = upper.iter().map(String::as_str).collect();

    match keywords.as_slice() {
        ["SHOW", "CATALOGS"] => Some(CatalogCommand::ShowCatalogs),
        ["SHOW", "SCHEMAS"] => Some(CatalogCommand::ShowSchemas { catalog: None }),
        ["SHOW", "SCHEMAS", "FROM" | "IN", _] => Some(CatalogCommand::ShowSchemas {
            catalog: Some(words[3].to_owned()),
        }),
        ["SHOW", "TABLES"] => Some(CatalogCommand::ShowTables { target: None }),
        ["SHOW", "TABLES", "FROM" | "IN", _] => Some(CatalogCommand::ShowTables {
            target: Some(words[3].to_owned()),
        }),
        ["DESCRIBE" | "DESC", _] => Some(CatalogCommand::Describe {
            reference: words[1].to_owned(),
        }),
        ["USE", ..] if words.len() > 1 => Some(CatalogCommand::Use {
            target: sql[3..].trim().to_owned(),
        }),
        _ => None,
    }
}

/// Splits a `catalog[.schema]` target, filling the schema from the session.
pub fn split_target(target: &str, default_schema: &str) -> Result<(String, String), String> {
    match target.split('.').collect::<Vec<_>>().as_slice() {
        [catalog, schema] if !catalog.is_empty() && !schema.is_empty() => {
            Ok(((*catalog).to_owned(), (*schema).to_owned()))
        }
        [catalog] if !catalog.is_empty() => Ok(((*catalog).to_owned(), default_schema.to_owned())),
        _ => Err(format!(
            "expected catalog or catalog.schema, got '{target}'"
        )),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    /// A fresh directory under the system temp dir holding `events.parquet`
    /// with three rows.
    pub(crate) fn parquet_fixture() -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "kaveon-cli-local-{}-{unique}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("fixture directory should be created");

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("kind", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec![Some("click"), None, Some("view")])),
            ],
        )
        .expect("fixture batch should build");
        let file =
            std::fs::File::create(dir.join("events.parquet")).expect("fixture file should open");
        let mut writer = parquet::arrow::ArrowWriter::try_new(file, schema, None)
            .expect("parquet writer should open");
        writer
            .write(&batch)
            .expect("fixture batch should be written");
        writer.close().expect("fixture file should close");
        std::fs::write(dir.join("notes.txt"), "not a table").expect("stray file should be written");
        dir
    }

    #[test]
    fn discovers_parquet_files_by_stem() {
        let dir = parquet_fixture();
        let catalog = build_local_catalog(&dir);
        assert_eq!(catalog.table_names("default").unwrap(), vec!["events"]);
        let table = catalog.table("default", "events").unwrap().unwrap();
        assert_eq!(table.format, DataFormat::Parquet);
        assert_eq!(table.location, "events.parquet");
        assert_eq!(table.arrow_schema.fields().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_directory_yields_an_empty_catalog() {
        let catalog = build_local_catalog(Path::new("does-not-exist-anywhere"));
        assert!(catalog.table_names("default").unwrap().is_empty());
    }

    #[test]
    fn parses_catalog_statements() {
        assert_eq!(
            parse_catalog_command("show catalogs;"),
            Some(CatalogCommand::ShowCatalogs)
        );
        assert_eq!(
            parse_catalog_command("SHOW SCHEMAS in Lake"),
            Some(CatalogCommand::ShowSchemas {
                catalog: Some("Lake".to_owned())
            })
        );
        assert_eq!(
            parse_catalog_command("SHOW TABLES FROM lake.raw"),
            Some(CatalogCommand::ShowTables {
                target: Some("lake.raw".to_owned())
            })
        );
        assert_eq!(
            parse_catalog_command("desc lake.raw.events"),
            Some(CatalogCommand::Describe {
                reference: "lake.raw.events".to_owned()
            })
        );
        assert_eq!(
            parse_catalog_command("use lake.raw;"),
            Some(CatalogCommand::Use {
                target: "lake.raw".to_owned()
            })
        );
        assert_eq!(parse_catalog_command("USE"), None);
        assert_eq!(parse_catalog_command("SELECT 1"), None);
        assert_eq!(parse_catalog_command("SHOW TABLES FROM a b"), None);
    }

    #[test]
    fn splits_targets() {
        assert_eq!(
            split_target("lake", "default").unwrap(),
            ("lake".to_owned(), "default".to_owned())
        );
        assert_eq!(
            split_target("lake.raw", "default").unwrap(),
            ("lake".to_owned(), "raw".to_owned())
        );
        assert!(split_target("a.b.c", "default").is_err());
        assert!(split_target("", "default").is_err());
    }
}
