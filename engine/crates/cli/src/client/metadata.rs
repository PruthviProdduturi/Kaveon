//! Catalog names for completion: catalogs, schemas, tables and columns,
//! fetched from the coordinator on first use and kept until `USE`.
use crate::auth::Session;
use crate::client::session::{endpoint, get, url_with_segments};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
struct CatalogList {
    catalogs: Vec<String>,
}

#[derive(Deserialize)]
struct SchemaList {
    schemas: Vec<String>,
}

#[derive(Deserialize)]
struct TableList {
    tables: Vec<String>,
}

#[derive(Deserialize)]
struct NamedDefinition {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct TableDefinition {
    #[serde(default)]
    columns: Vec<DefinitionColumn>,
}

#[derive(Deserialize)]
struct DefinitionColumn {
    name: String,
}

/// Names by scope. A fetch that fails is remembered as empty so a Tab does
/// not retry a coordinator that is down; `invalidate` forgets everything.
#[derive(Debug, Default)]
pub struct NameCache {
    catalogs: Option<Vec<String>>,
    schemas: HashMap<String, Vec<String>>,
    tables: HashMap<(String, String), Vec<String>>,
    columns: HashMap<(String, String), Vec<String>>,
}

impl NameCache {
    pub fn new() -> NameCache {
        NameCache::default()
    }

    pub fn invalidate(&mut self) {
        *self = NameCache::default();
    }

    /// Tables and columns of `catalog.schema`, then the schemas of
    /// `catalog`, then the catalogs, that start with `prefix` (any case).
    /// Each group is sorted and free of duplicates.
    pub fn candidates(
        &mut self,
        session: &Session,
        server: &str,
        catalog: &str,
        schema: &str,
        prefix: &str,
    ) -> Vec<String> {
        let prefix = prefix.to_lowercase();
        let mut out: Vec<String> = Vec::new();
        let mut push = |names: &[String]| {
            let mut matching: Vec<String> = names
                .iter()
                .filter(|name| name.to_lowercase().starts_with(&prefix))
                .filter(|name| !out.iter().any(|seen| seen == *name))
                .cloned()
                .collect();
            matching.sort_unstable();
            matching.dedup();
            out.extend(matching);
        };
        push(self.tables(session, server, catalog, schema));
        push(self.columns(session, server, catalog, schema));
        push(self.schemas(session, server, catalog));
        push(self.catalogs(session, server));
        out
    }

    fn catalogs(&mut self, session: &Session, server: &str) -> &[String] {
        self.catalogs.get_or_insert_with(|| {
            get::<CatalogList>(session, &endpoint(server, "/v1/catalog"))
                .map(|list| list.catalogs)
                .unwrap_or_default()
        })
    }

    fn schemas(&mut self, session: &Session, server: &str, catalog: &str) -> &[String] {
        if !self.schemas.contains_key(catalog) {
            let schemas = url_with_segments(server, "/v1/catalog", &[catalog, "schema"])
                .and_then(|url| get::<SchemaList>(session, &url))
                .map(|list| list.schemas)
                .unwrap_or_default();
            self.schemas.insert(catalog.to_owned(), schemas);
        }
        &self.schemas[catalog]
    }

    fn tables(
        &mut self,
        session: &Session,
        server: &str,
        catalog: &str,
        schema: &str,
    ) -> &[String] {
        let key = (catalog.to_owned(), schema.to_owned());
        if !self.tables.contains_key(&key) {
            let tables =
                url_with_segments(server, "/v1/catalog", &[catalog, "schema", schema, "table"])
                    .and_then(|url| get::<TableList>(session, &url))
                    .map(|list| list.tables)
                    .unwrap_or_default();
            self.tables.insert(key.clone(), tables);
        }
        &self.tables[&key]
    }

    /// Column names across the schema's tables, through the definitions
    /// routes `DESCRIBE` uses.
    fn columns(
        &mut self,
        session: &Session,
        server: &str,
        catalog: &str,
        schema: &str,
    ) -> &[String] {
        let key = (catalog.to_owned(), schema.to_owned());
        if !self.columns.contains_key(&key) {
            let columns = fetch_columns(session, server, catalog, schema).unwrap_or_default();
            self.columns.insert(key.clone(), columns);
        }
        &self.columns[&key]
    }
}

fn fetch_columns(
    session: &Session,
    server: &str,
    catalog: &str,
    schema: &str,
) -> Option<Vec<String>> {
    let definitions: Vec<NamedDefinition> =
        get(session, &endpoint(server, "/v1/catalog/definitions")).ok()?;
    let catalog = definitions
        .into_iter()
        .find(|definition| definition.name == catalog)?;
    let url =
        url_with_segments(server, "/v1/catalog/definitions", &[&catalog.id, "schemas"]).ok()?;
    let schemas: Vec<NamedDefinition> = get(session, &url).ok()?;
    let schema = schemas
        .into_iter()
        .find(|definition| definition.name == schema)?;
    let url = url_with_segments(server, "/v1/catalog/schemas", &[&schema.id, "tables"]).ok()?;
    let tables: Vec<TableDefinition> = get(session, &url).ok()?;
    Some(
        tables
            .into_iter()
            .flat_map(|table| table.columns.into_iter().map(|column| column.name))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::session::test_server::{fixture, session};

    /// In the order `candidates` asks: tables, columns (through the
    /// definitions), schemas, catalogs.
    fn responses() -> Vec<(&'static str, u16, String)> {
        vec![
            (
                "GET /v1/catalog/OpenSource/schema/kaveon_product/table ",
                200,
                r#"{"tables":["kaveon_events","kaveon_events_enriched","regions"]}"#.into(),
            ),
            (
                "GET /v1/catalog/definitions ",
                200,
                r#"[{"id":"cat-1","name":"OpenSource"},{"id":"cat-2","name":"Lakehouse"}]"#
                    .into(),
            ),
            (
                "GET /v1/catalog/definitions/cat-1/schemas ",
                200,
                r#"[{"id":"sch-9","name":"kaveon_product"}]"#.into(),
            ),
            (
                "GET /v1/catalog/schemas/sch-9/tables ",
                200,
                r#"[{"name":"kaveon_events","columns":[{"name":"region","data_type":"varchar","nullable":true},{"name":"kaveon_version","data_type":"varchar","nullable":true}]},{"name":"regions","columns":[{"name":"region","data_type":"varchar","nullable":false},{"name":"Population","data_type":"bigint","nullable":true}]}]"#.into(),
            ),
            (
                "GET /v1/catalog/OpenSource/schema ",
                200,
                r#"{"schemas":["kaveon_product","kaveon_ops"]}"#.into(),
            ),
            (
                "GET /v1/catalog ",
                200,
                r#"{"catalogs":["OpenSource","Lakehouse"]}"#.into(),
            ),
        ]
    }

    #[test]
    fn candidates_fetch_once_and_match_case_insensitively() {
        let (url, thread) = fixture(responses());
        let (session, options) = session(&url);
        let mut cache = NameCache::new();
        let first = cache.candidates(
            &session,
            &options.server,
            "OpenSource",
            "kaveon_product",
            "KAV",
        );
        assert_eq!(
            first,
            vec![
                "kaveon_events".to_owned(),
                "kaveon_events_enriched".into(),
                "kaveon_version".into(),
                "kaveon_ops".into(),
                "kaveon_product".into(),
            ]
        );
        // Every group is cached: these go to the server no further.
        let second = cache.candidates(
            &session,
            &options.server,
            "OpenSource",
            "kaveon_product",
            "re",
        );
        assert_eq!(second, vec!["regions".to_owned(), "region".into()]);
        let catalogs = cache.candidates(
            &session,
            &options.server,
            "OpenSource",
            "kaveon_product",
            "l",
        );
        assert_eq!(catalogs, vec!["Lakehouse".to_owned()]);
        let population = cache.candidates(
            &session,
            &options.server,
            "OpenSource",
            "kaveon_product",
            "pop",
        );
        assert_eq!(population, vec!["Population".to_owned()]);
        assert!(
            cache
                .candidates(
                    &session,
                    &options.server,
                    "OpenSource",
                    "kaveon_product",
                    "zzz"
                )
                .is_empty()
        );
        thread.join().unwrap();
    }

    #[test]
    fn a_failed_fetch_is_empty_until_invalidated() {
        let (url, thread) = fixture(vec![
            ("GET /v1/catalog/c/schema/s/table ", 404, "{}".into()),
            ("GET /v1/catalog/definitions ", 404, "{}".into()),
            ("GET /v1/catalog/c/schema ", 404, "{}".into()),
            (
                "GET /v1/catalog ",
                500,
                r#"{"error":"catalog unavailable"}"#.into(),
            ),
            (
                "GET /v1/catalog/c/schema/s/table ",
                200,
                r#"{"tables":["t"]}"#.into(),
            ),
            ("GET /v1/catalog/definitions ", 200, "[]".into()),
            (
                "GET /v1/catalog/c/schema ",
                200,
                r#"{"schemas":["s"]}"#.into(),
            ),
            ("GET /v1/catalog ", 200, r#"{"catalogs":["c"]}"#.into()),
        ]);
        let (session, options) = session(&url);
        let mut cache = NameCache::new();
        assert!(
            cache
                .candidates(&session, &options.server, "c", "s", "")
                .is_empty()
        );
        assert!(
            cache
                .candidates(&session, &options.server, "c", "s", "")
                .is_empty()
        );
        cache.invalidate();
        assert_eq!(
            cache.candidates(&session, &options.server, "c", "s", ""),
            vec!["t".to_owned(), "s".into(), "c".into()]
        );
        thread.join().unwrap();
    }
}
