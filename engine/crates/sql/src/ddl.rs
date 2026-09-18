//! Catalog statements: the Trino-shaped DDL and metadata surface over the
//! Engine's durable catalog (`CREATE SCHEMA`, `CREATE TABLE … WITH (…)`,
//! `DROP`, `ALTER TABLE … SET LOCATION`, `SHOW …`, `DESCRIBE`, `CALL
//! system.register_table`). This module only recognises and validates the
//! statement shape; the coordinator lowers it onto catalog definitions.
//!
//! [`parse_catalog_statement`] answers `Ok(None)` for anything that is not a
//! catalog statement, so a caller can try it first and fall through to the
//! query pipeline. A statement that starts like a catalog statement but does
//! not parse is an error naming what was expected.

use arrow::datatypes::{DataType, TimeUnit};
use kaveon_core::{AccessPattern, DataFormat, KaveonError, Result};
use sqlparser::ast::{self, ObjectName};
use sqlparser::dialect::GenericDialect;
use sqlparser::keywords::Keyword;
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::Token;

/// A dotted object name as written, without case folding: the Engine's
/// catalog names are case-sensitive (`OpenSource`, `Benchmarks`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedName(pub Vec<String>);

impl QualifiedName {
    pub fn parts(&self) -> &[String] {
        &self.0
    }
}

impl std::fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, part) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(".")?;
            }
            f.write_str(&quote_identifier(part))?;
        }
        Ok(())
    }
}

/// A column as declared in `CREATE TABLE (…)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSpec {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
}

/// How a catalog stores its tables, as given by `CREATE CATALOG … WITH (…)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogStorageSpec {
    Local {
        base_path: String,
    },
    AdlsGen2 {
        account: String,
        container: String,
        root_path: String,
    },
    S3 {
        bucket: String,
        region: String,
        prefix: String,
    },
}

/// A credential reference for a catalog: `kind:reference`. Never a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialSpec {
    pub kind: String,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogStatement {
    CreateCatalog {
        name: String,
        if_not_exists: bool,
        storage: CatalogStorageSpec,
        credential: Option<CredentialSpec>,
    },
    DropCatalog {
        name: String,
        if_exists: bool,
        cascade: bool,
    },
    CreateSchema {
        name: QualifiedName,
        if_not_exists: bool,
    },
    DropSchema {
        name: QualifiedName,
        if_exists: bool,
        cascade: bool,
    },
    CreateTable {
        name: QualifiedName,
        if_not_exists: bool,
        /// `None` asks the coordinator to infer the columns from the table
        /// itself (Delta log, Iceberg metadata, Parquet footer).
        columns: Option<Vec<ColumnSpec>>,
        location: String,
        format: DataFormat,
        access: AccessPattern,
    },
    DropTable {
        name: QualifiedName,
        if_exists: bool,
    },
    AlterTableSetLocation {
        name: QualifiedName,
        if_exists: bool,
        location: String,
    },
    ShowCreateTable {
        name: QualifiedName,
    },
    Describe {
        name: QualifiedName,
    },
    ShowCatalogs {
        like: Option<String>,
    },
    ShowSchemas {
        catalog: Option<String>,
        like: Option<String>,
    },
    ShowTables {
        schema: Option<QualifiedName>,
        like: Option<String>,
    },
}

impl CatalogStatement {
    /// Whether the statement changes catalog definitions (as opposed to
    /// reading them).
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::CreateCatalog { .. }
                | Self::DropCatalog { .. }
                | Self::CreateSchema { .. }
                | Self::DropSchema { .. }
                | Self::CreateTable { .. }
                | Self::DropTable { .. }
                | Self::AlterTableSetLocation { .. }
        )
    }

    /// Whether the statement creates or drops a whole catalog.
    pub fn is_catalog_mutation(&self) -> bool {
        matches!(self, Self::CreateCatalog { .. } | Self::DropCatalog { .. })
    }
}

/// Recognise a catalog statement. `Ok(None)` means the text is not one and
/// belongs to the query pipeline.
pub fn parse_catalog_statement(sql: &str) -> Result<Option<CatalogStatement>> {
    let sql = sql.trim().trim_end_matches(';').trim();
    let mut parser = Parser::new(&GenericDialect {})
        .try_with_sql(sql)
        .map_err(parse_error)?;
    let Some(leading) = leading_keyword(&parser) else {
        return Ok(None);
    };
    let statement = match leading {
        Keyword::CREATE => parse_create(&mut parser)?,
        Keyword::DROP => parse_drop(&mut parser)?,
        Keyword::ALTER => parse_alter(&mut parser)?,
        Keyword::SHOW => parse_show(&mut parser)?,
        Keyword::DESCRIBE | Keyword::DESC => parse_describe(&mut parser)?,
        Keyword::CALL => parse_call(&mut parser)?,
        _ => return Ok(None),
    };
    let Some(statement) = statement else {
        return Ok(None);
    };
    expect_end(&mut parser)?;
    Ok(Some(statement))
}

fn leading_keyword(parser: &Parser<'_>) -> Option<Keyword> {
    match parser.peek_token().token {
        Token::Word(word) if word.quote_style.is_none() => Some(word.keyword),
        _ => None,
    }
}

fn parse_create(parser: &mut Parser<'_>) -> Result<Option<CatalogStatement>> {
    parser
        .expect_keyword(Keyword::CREATE)
        .map_err(parse_error)?;
    if parser.parse_keyword(Keyword::CATALOG) {
        let if_not_exists = parser.parse_keywords(&[Keyword::IF, Keyword::NOT, Keyword::EXISTS]);
        let name = identifier(parser, "a catalog name")?;
        parser
            .expect_keyword(Keyword::WITH)
            .map_err(|_| sql_error("CREATE CATALOG requires WITH (storage = '…', …)"))?;
        let options = parse_with_options(parser, "CREATE CATALOG")?;
        let (storage, credential) = catalog_options(options)?;
        return Ok(Some(CatalogStatement::CreateCatalog {
            name,
            if_not_exists,
            storage,
            credential,
        }));
    }
    if parser.parse_keyword(Keyword::SCHEMA) {
        let if_not_exists = parser.parse_keywords(&[Keyword::IF, Keyword::NOT, Keyword::EXISTS]);
        let name = qualified_name(parser, 1..=2, "CREATE SCHEMA expects [catalog.]schema")?;
        return Ok(Some(CatalogStatement::CreateSchema {
            name,
            if_not_exists,
        }));
    }
    if parser.parse_keyword(Keyword::TABLE) {
        let if_not_exists = parser.parse_keywords(&[Keyword::IF, Keyword::NOT, Keyword::EXISTS]);
        let name = qualified_name(
            parser,
            1..=3,
            "CREATE TABLE expects [catalog.][schema.]table",
        )?;
        let columns = if parser.consume_token(&Token::LParen) {
            let columns = parser
                .parse_comma_separated(parse_column)
                .map_err(parse_error)?;
            parser.expect_token(&Token::RParen).map_err(parse_error)?;
            if columns.is_empty() {
                return Err(sql_error(
                    "CREATE TABLE column list cannot be empty; omit it to infer the columns",
                ));
            }
            let mut names = std::collections::HashSet::new();
            if let Some(duplicate) = columns.iter().find(|column| !names.insert(&column.name)) {
                return Err(sql_error(&format!(
                    "CREATE TABLE declares column '{}' twice",
                    duplicate.name
                )));
            }
            Some(columns)
        } else {
            None
        };
        parser.expect_keyword(Keyword::WITH).map_err(|_| {
            sql_error(
                "CREATE TABLE requires WITH (location = '…', format = 'parquet'|'delta'|'iceberg')",
            )
        })?;
        let options = parse_with_options(parser, "CREATE TABLE")?;
        let (location, format, access) = table_options(options)?;
        return Ok(Some(CatalogStatement::CreateTable {
            name,
            if_not_exists,
            columns,
            location,
            format,
            access,
        }));
    }
    // CREATE VIEW, CREATE INDEX, … are not catalog statements; the query
    // pipeline refuses them by name.
    Ok(None)
}

fn parse_drop(parser: &mut Parser<'_>) -> Result<Option<CatalogStatement>> {
    parser.expect_keyword(Keyword::DROP).map_err(parse_error)?;
    if parser.parse_keyword(Keyword::CATALOG) {
        let if_exists = parser.parse_keywords(&[Keyword::IF, Keyword::EXISTS]);
        let name = identifier(parser, "a catalog name")?;
        let cascade = parse_cascade(parser)?;
        return Ok(Some(CatalogStatement::DropCatalog {
            name,
            if_exists,
            cascade,
        }));
    }
    if parser.parse_keyword(Keyword::SCHEMA) {
        let if_exists = parser.parse_keywords(&[Keyword::IF, Keyword::EXISTS]);
        let name = qualified_name(parser, 1..=2, "DROP SCHEMA expects [catalog.]schema")?;
        let cascade = parse_cascade(parser)?;
        return Ok(Some(CatalogStatement::DropSchema {
            name,
            if_exists,
            cascade,
        }));
    }
    if parser.parse_keyword(Keyword::TABLE) {
        let if_exists = parser.parse_keywords(&[Keyword::IF, Keyword::EXISTS]);
        let name = qualified_name(parser, 1..=3, "DROP TABLE expects [catalog.][schema.]table")?;
        return Ok(Some(CatalogStatement::DropTable { name, if_exists }));
    }
    Ok(None)
}

fn parse_cascade(parser: &mut Parser<'_>) -> Result<bool> {
    if parser.parse_keyword(Keyword::CASCADE) {
        return Ok(true);
    }
    let _ = parser.parse_keyword(Keyword::RESTRICT);
    Ok(false)
}

fn parse_alter(parser: &mut Parser<'_>) -> Result<Option<CatalogStatement>> {
    parser.expect_keyword(Keyword::ALTER).map_err(parse_error)?;
    if !parser.parse_keyword(Keyword::TABLE) {
        return Ok(None);
    }
    let if_exists = parser.parse_keywords(&[Keyword::IF, Keyword::EXISTS]);
    let name = qualified_name(
        parser,
        1..=3,
        "ALTER TABLE expects [catalog.][schema.]table",
    )?;
    if !parser.parse_keywords(&[Keyword::SET, Keyword::LOCATION]) {
        return Err(sql_error(
            "ALTER TABLE supports SET LOCATION '…' only; columns come from the table itself",
        ));
    }
    let location = string_literal(parser, "SET LOCATION expects a quoted location")?;
    validate_location(&location)?;
    Ok(Some(CatalogStatement::AlterTableSetLocation {
        name,
        if_exists,
        location,
    }))
}

fn parse_show(parser: &mut Parser<'_>) -> Result<Option<CatalogStatement>> {
    parser.expect_keyword(Keyword::SHOW).map_err(parse_error)?;
    if parser.parse_keywords(&[Keyword::CREATE, Keyword::TABLE]) {
        let name = qualified_name(
            parser,
            1..=3,
            "SHOW CREATE TABLE expects [catalog.][schema.]table",
        )?;
        return Ok(Some(CatalogStatement::ShowCreateTable { name }));
    }
    if parser.parse_keyword(Keyword::COLUMNS) {
        if parser
            .parse_one_of_keywords(&[Keyword::FROM, Keyword::IN])
            .is_none()
        {
            return Err(sql_error(
                "SHOW COLUMNS expects FROM [catalog.][schema.]table",
            ));
        }
        let name = qualified_name(
            parser,
            1..=3,
            "SHOW COLUMNS expects FROM [catalog.][schema.]table",
        )?;
        return Ok(Some(CatalogStatement::Describe { name }));
    }
    if parser.parse_keyword(Keyword::SCHEMAS) {
        let catalog = if parser
            .parse_one_of_keywords(&[Keyword::FROM, Keyword::IN])
            .is_some()
        {
            Some(identifier(parser, "a catalog name")?)
        } else {
            None
        };
        let like = parse_like(parser)?;
        return Ok(Some(CatalogStatement::ShowSchemas { catalog, like }));
    }
    if parser.parse_keyword(Keyword::TABLES) {
        let schema = if parser
            .parse_one_of_keywords(&[Keyword::FROM, Keyword::IN])
            .is_some()
        {
            Some(qualified_name(
                parser,
                1..=2,
                "SHOW TABLES expects FROM [catalog.]schema",
            )?)
        } else {
            None
        };
        let like = parse_like(parser)?;
        return Ok(Some(CatalogStatement::ShowTables { schema, like }));
    }
    if matches!(&parser.peek_token().token, Token::Word(word) if word.quote_style.is_none() && word.value.eq_ignore_ascii_case("CATALOGS"))
    {
        parser.next_token();
        let like = parse_like(parser)?;
        return Ok(Some(CatalogStatement::ShowCatalogs { like }));
    }
    // SHOW SESSION, SHOW FUNCTIONS, … are not catalog statements.
    Ok(None)
}

fn parse_like(parser: &mut Parser<'_>) -> Result<Option<String>> {
    if !parser.parse_keyword(Keyword::LIKE) {
        return Ok(None);
    }
    string_literal(parser, "LIKE expects a single-quoted pattern").map(Some)
}

fn parse_describe(parser: &mut Parser<'_>) -> Result<Option<CatalogStatement>> {
    if parser
        .parse_one_of_keywords(&[Keyword::DESCRIBE, Keyword::DESC])
        .is_none()
    {
        return Ok(None);
    }
    let _ = parser.parse_keyword(Keyword::TABLE);
    let name = qualified_name(parser, 1..=3, "DESCRIBE expects [catalog.][schema.]table")?;
    Ok(Some(CatalogStatement::Describe { name }))
}

/// `CALL [catalog.]system.register_table(schema_name => '…', table_name =>
/// '…', table_location => '…' [, format => '…'])` and
/// `CALL [catalog.]system.unregister_table(schema_name => '…', table_name =>
/// '…')`: Trino's procedure spelling of `CREATE TABLE` with inferred columns
/// and `DROP TABLE`.
fn parse_call(parser: &mut Parser<'_>) -> Result<Option<CatalogStatement>> {
    parser.expect_keyword(Keyword::CALL).map_err(parse_error)?;
    let procedure = qualified_name(
        parser,
        2..=3,
        "CALL expects [catalog.]system.register_table(…) or system.unregister_table(…)",
    )?;
    let (catalog, procedure_name) = match procedure.parts() {
        [schema, name] if schema.eq_ignore_ascii_case("system") => (None, name.as_str()),
        [catalog, schema, name] if schema.eq_ignore_ascii_case("system") => {
            (Some(catalog.clone()), name.as_str())
        }
        _ => {
            return Err(sql_error(
                "CALL expects [catalog.]system.register_table(…) or system.unregister_table(…)",
            ));
        }
    };
    let register = if procedure_name.eq_ignore_ascii_case("register_table") {
        true
    } else if procedure_name.eq_ignore_ascii_case("unregister_table") {
        false
    } else {
        return Err(sql_error(&format!(
            "procedure system.{procedure_name} is not available; use register_table or unregister_table"
        )));
    };
    parser
        .expect_token(&Token::LParen)
        .map_err(|_| sql_error("CALL expects a parenthesised argument list"))?;
    let arguments = if parser.consume_token(&Token::RParen) {
        Vec::new()
    } else {
        let arguments = parser
            .parse_comma_separated(parse_named_argument)
            .map_err(parse_error)?;
        parser.expect_token(&Token::RParen).map_err(parse_error)?;
        arguments
    };
    let mut schema_name = None;
    let mut table_name = None;
    let mut table_location = None;
    let mut format = None;
    for (key, value) in arguments {
        let slot = match key.to_ascii_lowercase().as_str() {
            "schema_name" => &mut schema_name,
            "table_name" => &mut table_name,
            "table_location" if register => &mut table_location,
            "format" if register => &mut format,
            _ => {
                return Err(sql_error(&format!(
                    "procedure argument '{key}' is not recognised"
                )));
            }
        };
        if slot.replace(value).is_some() {
            return Err(sql_error(&format!(
                "procedure argument '{key}' is given twice"
            )));
        }
    }
    let schema_name = schema_name.ok_or_else(|| sql_error("CALL requires schema_name => '…'"))?;
    let table_name = table_name.ok_or_else(|| sql_error("CALL requires table_name => '…'"))?;
    let mut parts = Vec::new();
    parts.extend(catalog);
    parts.push(schema_name);
    parts.push(table_name);
    let name = QualifiedName(parts);
    if !register {
        return Ok(Some(CatalogStatement::DropTable {
            name,
            if_exists: false,
        }));
    }
    let location =
        table_location.ok_or_else(|| sql_error("register_table requires table_location => '…'"))?;
    validate_location(&location)?;
    let format = match format {
        Some(value) => parse_format(&value)?,
        None => DataFormat::Delta,
    };
    Ok(Some(CatalogStatement::CreateTable {
        name,
        if_not_exists: false,
        columns: None,
        location,
        format,
        access: AccessPattern::Shortcut,
    }))
}

fn parse_named_argument(
    parser: &mut Parser<'_>,
) -> std::result::Result<(String, String), ParserError> {
    let name = parser.parse_identifier(false)?.value;
    parser.expect_token(&Token::RArrow)?;
    let value = parser.parse_literal_string()?;
    Ok((name, value))
}

fn parse_column(parser: &mut Parser<'_>) -> std::result::Result<ColumnSpec, ParserError> {
    let name = parser.parse_identifier(false)?.value;
    let data_type = parser.parse_data_type()?;
    let data_type =
        arrow_type(&data_type).map_err(|error| ParserError::ParserError(error.to_string()))?;
    let nullable = if parser.parse_keywords(&[Keyword::NOT, Keyword::NULL]) {
        false
    } else {
        let _ = parser.parse_keyword(Keyword::NULL);
        true
    };
    Ok(ColumnSpec {
        name,
        data_type,
        nullable,
    })
}

fn parse_with_options(parser: &mut Parser<'_>, statement: &str) -> Result<Vec<(String, String)>> {
    parser.expect_token(&Token::LParen).map_err(|_| {
        sql_error(&format!(
            "{statement} WITH expects a parenthesised option list"
        ))
    })?;
    let options = parser
        .parse_comma_separated(|parser| {
            let key = parser.parse_identifier(false)?.value;
            parser.expect_token(&Token::Eq)?;
            let value = parser.parse_literal_string()?;
            Ok((key, value))
        })
        .map_err(|_| {
            sql_error(&format!(
                "{statement} WITH options are key = 'value' pairs separated by commas"
            ))
        })?;
    parser.expect_token(&Token::RParen).map_err(parse_error)?;
    let mut seen = std::collections::HashSet::new();
    for (key, _) in &options {
        if !seen.insert(key.to_ascii_lowercase()) {
            return Err(sql_error(&format!(
                "{statement} option '{key}' is given twice"
            )));
        }
    }
    Ok(options)
}

fn table_options(options: Vec<(String, String)>) -> Result<(String, DataFormat, AccessPattern)> {
    let mut location = None;
    let mut format = None;
    let mut access = AccessPattern::Shortcut;
    for (key, value) in options {
        match key.to_ascii_lowercase().as_str() {
            "location" => location = Some(value),
            "format" => format = Some(parse_format(&value)?),
            "access" => {
                access = match value.to_ascii_lowercase().as_str() {
                    "shortcut" => AccessPattern::Shortcut,
                    "optimized" => AccessPattern::Optimized,
                    other => {
                        return Err(sql_error(&format!(
                            "access '{other}' is not supported; use 'shortcut' or 'optimized'"
                        )));
                    }
                }
            }
            other => {
                return Err(sql_error(&format!(
                    "table option '{other}' is not supported; use location, format and access"
                )));
            }
        }
    }
    let location =
        location.ok_or_else(|| sql_error("CREATE TABLE requires the location = '…' option"))?;
    validate_location(&location)?;
    let format = format.ok_or_else(|| {
        sql_error("CREATE TABLE requires the format = 'parquet'|'delta'|'iceberg' option")
    })?;
    Ok((location, format, access))
}

fn catalog_options(
    options: Vec<(String, String)>,
) -> Result<(CatalogStorageSpec, Option<CredentialSpec>)> {
    let mut storage = None;
    let mut fields = std::collections::BTreeMap::new();
    let mut credential = None;
    for (key, value) in options {
        match key.to_ascii_lowercase().as_str() {
            "storage" => storage = Some(value.to_ascii_lowercase()),
            "credential" => {
                let (kind, reference) = value.split_once(':').ok_or_else(|| {
                    sql_error(
                        "credential expects 'kind:reference', for example 'workload-identity:kaveon-test-reader'",
                    )
                })?;
                if reference.trim().is_empty() {
                    return Err(sql_error("credential reference cannot be empty"));
                }
                credential = Some(CredentialSpec {
                    kind: kind.trim().to_ascii_lowercase(),
                    reference: reference.trim().to_owned(),
                });
            }
            "account" | "container" | "root" | "base_path" | "bucket" | "region" | "prefix" => {
                fields.insert(key.to_ascii_lowercase(), value);
            }
            other => {
                return Err(sql_error(&format!(
                    "catalog option '{other}' is not supported; use storage, account, container, root, base_path, bucket, region, prefix and credential"
                )));
            }
        }
    }
    let storage = storage.ok_or_else(|| {
        sql_error("CREATE CATALOG requires the storage = 'adls'|'local'|'s3' option")
    })?;
    let take = |fields: &mut std::collections::BTreeMap<String, String>, key: &str| {
        fields.remove(key).ok_or_else(|| {
            sql_error(&format!(
                "storage '{storage}' requires the {key} = '…' option"
            ))
        })
    };
    let spec = match storage.as_str() {
        "adls" | "adlsgen2" | "adls_gen2" => CatalogStorageSpec::AdlsGen2 {
            account: take(&mut fields, "account")?,
            container: take(&mut fields, "container")?,
            root_path: fields.remove("root").unwrap_or_default(),
        },
        "local" => CatalogStorageSpec::Local {
            base_path: take(&mut fields, "base_path")?,
        },
        "s3" => CatalogStorageSpec::S3 {
            bucket: take(&mut fields, "bucket")?,
            region: take(&mut fields, "region")?,
            prefix: fields.remove("prefix").unwrap_or_default(),
        },
        other => {
            return Err(sql_error(&format!(
                "storage '{other}' is not supported; use 'adls', 'local' or 's3'"
            )));
        }
    };
    if let Some((key, _)) = fields.iter().next() {
        return Err(sql_error(&format!(
            "catalog option '{key}' does not apply to storage '{storage}'"
        )));
    }
    Ok((spec, credential))
}

fn parse_format(value: &str) -> Result<DataFormat> {
    match value.to_ascii_lowercase().as_str() {
        "parquet" => Ok(DataFormat::Parquet),
        "delta" => Ok(DataFormat::Delta),
        "iceberg" => Ok(DataFormat::Iceberg),
        other => Err(sql_error(&format!(
            "format '{other}' is not supported; use 'parquet', 'delta' or 'iceberg'"
        ))),
    }
}

/// A table location is a path within its catalog's storage root: never
/// empty, never absolute, never climbing out of the root.
fn validate_location(location: &str) -> Result<()> {
    let trimmed = location.trim();
    if trimmed.is_empty() {
        return Err(sql_error("location cannot be empty"));
    }
    if trimmed.contains("://") {
        return Err(sql_error(
            "location is a path within the catalog's storage root, not a URI",
        ));
    }
    if trimmed.split(['/', '\\']).any(|segment| segment == "..") {
        return Err(sql_error("location cannot contain '..' segments"));
    }
    Ok(())
}

fn identifier(parser: &mut Parser<'_>, expected: &str) -> Result<String> {
    parser
        .parse_identifier(false)
        .map(|ident| ident.value)
        .map_err(|_| sql_error(&format!("expected {expected}")))
}

fn qualified_name(
    parser: &mut Parser<'_>,
    parts: std::ops::RangeInclusive<usize>,
    usage: &str,
) -> Result<QualifiedName> {
    let name: ObjectName = parser
        .parse_object_name(false)
        .map_err(|_| sql_error(usage))?;
    if !parts.contains(&name.0.len()) {
        return Err(sql_error(usage));
    }
    if name.0.iter().any(|ident| ident.value.trim().is_empty()) {
        return Err(sql_error(usage));
    }
    Ok(QualifiedName(
        name.0.into_iter().map(|ident| ident.value).collect(),
    ))
}

fn string_literal(parser: &mut Parser<'_>, expected: &str) -> Result<String> {
    match parser.next_token().token {
        Token::SingleQuotedString(value) => Ok(value),
        _ => Err(sql_error(expected)),
    }
}

fn expect_end(parser: &mut Parser<'_>) -> Result<()> {
    match parser.peek_token().token {
        Token::EOF => Ok(()),
        Token::SemiColon => {
            parser.next_token();
            match parser.peek_token().token {
                Token::EOF => Ok(()),
                _ => Err(sql_error(
                    "catalog statements accept one statement per request",
                )),
            }
        }
        token => Err(sql_error(&format!(
            "unexpected '{token}' after the catalog statement"
        ))),
    }
}

/// The Arrow type a declared SQL type stores as. Trino's spellings and the
/// Arrow display names (`Int64`, `Utf8`) are both accepted, so `SHOW CREATE
/// TABLE` output and `DESCRIBE` output are both valid declarations.
pub fn arrow_type(data_type: &ast::DataType) -> Result<DataType> {
    use ast::DataType as Sql;
    Ok(match data_type {
        Sql::Boolean | Sql::Bool => DataType::Boolean,
        Sql::TinyInt(_) | Sql::Int8(_) => DataType::Int8,
        Sql::SmallInt(_) | Sql::Int16 => DataType::Int16,
        Sql::Int(_) | Sql::Integer(_) | Sql::Int32 => DataType::Int32,
        Sql::BigInt(_) | Sql::Int64 => DataType::Int64,
        Sql::UnsignedTinyInt(_) | Sql::UInt8 => DataType::UInt8,
        Sql::UnsignedSmallInt(_) | Sql::UInt16 => DataType::UInt16,
        Sql::UnsignedInt(_) | Sql::UnsignedInteger(_) | Sql::UInt32 => DataType::UInt32,
        Sql::UnsignedBigInt(_) | Sql::UInt64 => DataType::UInt64,
        Sql::Real | Sql::Float32 | Sql::Float4 => DataType::Float32,
        Sql::Double | Sql::DoublePrecision | Sql::Float64 | Sql::Float8 => DataType::Float64,
        Sql::Float(precision) => match precision.unwrap_or(53) {
            0..=24 => DataType::Float32,
            _ => DataType::Float64,
        },
        Sql::Varchar(_)
        | Sql::Char(_)
        | Sql::Character(_)
        | Sql::CharacterVarying(_)
        | Sql::Text
        | Sql::String(_) => DataType::Utf8,
        Sql::Varbinary(_) | Sql::Binary(_) | Sql::Bytea | Sql::Blob(_) => DataType::Binary,
        Sql::Date => DataType::Date32,
        Sql::Timestamp(_, ast::TimezoneInfo::None) => {
            DataType::Timestamp(TimeUnit::Microsecond, None)
        }
        Sql::Timestamp(_, _) => DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        Sql::Decimal(info) | Sql::Numeric(info) | Sql::Dec(info) => {
            let (precision, scale) = match info {
                ast::ExactNumberInfo::None => (38, 0),
                ast::ExactNumberInfo::Precision(precision) => (*precision, 0),
                ast::ExactNumberInfo::PrecisionAndScale(precision, scale) => (*precision, *scale),
            };
            let precision = u8::try_from(precision)
                .ok()
                .filter(|precision| (1..=38).contains(precision))
                .ok_or_else(|| sql_error("decimal precision must be between 1 and 38"))?;
            let scale = i8::try_from(scale)
                .ok()
                .filter(|scale| *scale >= 0 && u8::try_from(*scale).is_ok_and(|s| s <= precision))
                .ok_or_else(|| sql_error("decimal scale must be between 0 and the precision"))?;
            DataType::Decimal128(precision, scale)
        }
        Sql::Custom(name, _) => {
            let spelled = name.to_string();
            arrow_type_by_display(&spelled)
                .ok_or_else(|| sql_error(&format!("column type '{spelled}' is not supported")))?
        }
        other => {
            return Err(sql_error(&format!(
                "column type '{other}' is not supported"
            )));
        }
    })
}

fn arrow_type_by_display(spelled: &str) -> Option<DataType> {
    let candidates = [
        DataType::Boolean,
        DataType::Int8,
        DataType::Int16,
        DataType::Int32,
        DataType::Int64,
        DataType::UInt8,
        DataType::UInt16,
        DataType::UInt32,
        DataType::UInt64,
        DataType::Float32,
        DataType::Float64,
        DataType::Utf8,
        DataType::LargeUtf8,
        DataType::Binary,
        DataType::LargeBinary,
        DataType::Date32,
        DataType::Date64,
    ];
    candidates
        .into_iter()
        .find(|candidate| candidate.to_string().eq_ignore_ascii_case(spelled))
}

/// The SQL spelling of a stored Arrow type, as `SHOW CREATE TABLE` and
/// `DESCRIBE` present it; a type without a SQL name keeps its Arrow name,
/// which [`arrow_type`] accepts back.
pub fn sql_type_name(data_type: &DataType) -> String {
    match data_type {
        DataType::Boolean => "boolean".into(),
        DataType::Int8 => "tinyint".into(),
        DataType::Int16 => "smallint".into(),
        DataType::Int32 => "integer".into(),
        DataType::Int64 => "bigint".into(),
        DataType::Float32 => "real".into(),
        DataType::Float64 => "double".into(),
        DataType::Utf8 | DataType::LargeUtf8 => "varchar".into(),
        DataType::Binary | DataType::LargeBinary => "varbinary".into(),
        DataType::Date32 => "date".into(),
        DataType::Timestamp(_, None) => "timestamp".into(),
        DataType::Timestamp(_, Some(_)) => "timestamp with time zone".into(),
        DataType::Decimal128(precision, scale) => format!("decimal({precision}, {scale})"),
        DataType::Dictionary(_, values) => sql_type_name(values),
        other => other.to_string(),
    }
}

/// Quote an identifier for SQL output when it is not a plain lowercase
/// word, so `SHOW CREATE TABLE` output parses back to the same names.
pub fn quote_identifier(name: &str) -> String {
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit());
    if plain {
        name.to_owned()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// Render a definition as the `CREATE TABLE` statement that recreates it.
pub fn render_create_table(
    name: &QualifiedName,
    columns: &[ColumnSpec],
    location: &str,
    format: DataFormat,
    access: AccessPattern,
) -> String {
    let mut text = format!("CREATE TABLE {name} (\n");
    for (index, column) in columns.iter().enumerate() {
        text.push_str("   ");
        text.push_str(&quote_identifier(&column.name));
        text.push(' ');
        text.push_str(&sql_type_name(&column.data_type));
        if !column.nullable {
            text.push_str(" NOT NULL");
        }
        if index + 1 < columns.len() {
            text.push(',');
        }
        text.push('\n');
    }
    text.push_str(&format!(
        ")\nWITH (\n   location = '{}',\n   format = '{}',\n   access = '{}'\n)",
        location.replace('\'', "''"),
        format_name(format),
        access_name(access)
    ));
    text
}

pub fn format_name(format: DataFormat) -> &'static str {
    match format {
        DataFormat::Parquet => "parquet",
        DataFormat::Delta => "delta",
        DataFormat::Iceberg => "iceberg",
    }
}

pub fn access_name(access: AccessPattern) -> &'static str {
    match access {
        AccessPattern::Shortcut => "shortcut",
        AccessPattern::Optimized => "optimized",
    }
}

fn parse_error(error: ParserError) -> KaveonError {
    KaveonError::Sql(error.to_string())
}

fn sql_error(message: &str) -> KaveonError {
    KaveonError::Sql(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(sql: &str) -> CatalogStatement {
        parse_catalog_statement(sql)
            .unwrap_or_else(|error| panic!("{sql}: {error}"))
            .unwrap_or_else(|| panic!("{sql}: not a catalog statement"))
    }

    fn name(parts: &[&str]) -> QualifiedName {
        QualifiedName(parts.iter().map(|part| (*part).to_owned()).collect())
    }

    #[test]
    fn queries_and_other_statements_are_not_catalog_statements() {
        for sql in [
            "SELECT 1",
            "WITH t AS (SELECT 1) SELECT * FROM t",
            "INSERT INTO product.datasets (id, document_json) VALUES ('a', '{}')",
            "CREATE VIEW v AS SELECT 1",
            "ANALYZE lake.sales.orders",
            "EXPLAIN SELECT 1",
            "SHOW SESSION",
            "SET SESSION result_cache = false",
            "",
        ] {
            assert_eq!(parse_catalog_statement(sql).unwrap(), None, "{sql}");
        }
    }

    #[test]
    fn create_schema_accepts_if_not_exists_and_qualification() {
        assert_eq!(
            parse("CREATE SCHEMA lake.sales"),
            CatalogStatement::CreateSchema {
                name: name(&["lake", "sales"]),
                if_not_exists: false,
            }
        );
        assert_eq!(
            parse("create schema if not exists sales;"),
            CatalogStatement::CreateSchema {
                name: name(&["sales"]),
                if_not_exists: true,
            }
        );
        assert!(
            parse_catalog_statement("CREATE SCHEMA a.b.c")
                .unwrap_err()
                .to_string()
                .contains("[catalog.]schema")
        );
    }

    #[test]
    fn create_table_with_inferred_columns_reads_its_options() {
        assert_eq!(
            parse(
                "CREATE TABLE IF NOT EXISTS lake.sales.orders WITH (location = 'sales/orders', format = 'delta')"
            ),
            CatalogStatement::CreateTable {
                name: name(&["lake", "sales", "orders"]),
                if_not_exists: true,
                columns: None,
                location: "sales/orders".into(),
                format: DataFormat::Delta,
                access: AccessPattern::Shortcut,
            }
        );
        assert_eq!(
            parse(
                "CREATE TABLE orders WITH (format = 'Parquet', access = 'optimized', location = 'orders.parquet')"
            ),
            CatalogStatement::CreateTable {
                name: name(&["orders"]),
                if_not_exists: false,
                columns: None,
                location: "orders.parquet".into(),
                format: DataFormat::Parquet,
                access: AccessPattern::Optimized,
            }
        );
    }

    #[test]
    fn create_table_with_declared_columns_maps_sql_types_to_arrow() {
        let CatalogStatement::CreateTable { columns, .. } = parse(
            "CREATE TABLE \"Orders\" (id BIGINT NOT NULL, region VARCHAR, amount DECIMAL(12, 2), day DATE, flag BOOLEAN NULL, ratio DOUBLE, small SMALLINT, n INTEGER, ts TIMESTAMP, raw Int64, txt Utf8) WITH (location = 'orders', format = 'iceberg')",
        ) else {
            panic!("expected CREATE TABLE");
        };
        let columns = columns.unwrap();
        let expected = [
            ("id", DataType::Int64, false),
            ("region", DataType::Utf8, true),
            ("amount", DataType::Decimal128(12, 2), true),
            ("day", DataType::Date32, true),
            ("flag", DataType::Boolean, true),
            ("ratio", DataType::Float64, true),
            ("small", DataType::Int16, true),
            ("n", DataType::Int32, true),
            ("ts", DataType::Timestamp(TimeUnit::Microsecond, None), true),
            ("raw", DataType::Int64, true),
            ("txt", DataType::Utf8, true),
        ];
        assert_eq!(columns.len(), expected.len());
        for (column, (name, data_type, nullable)) in columns.iter().zip(expected) {
            assert_eq!(column.name, name);
            assert_eq!(column.data_type, data_type, "{name}");
            assert_eq!(column.nullable, nullable, "{name}");
        }
    }

    #[test]
    fn create_table_errors_name_what_is_missing() {
        let cases = [
            ("CREATE TABLE t WITH (format = 'delta')", "location"),
            ("CREATE TABLE t WITH (location = 'x')", "format"),
            (
                "CREATE TABLE t WITH (location = 'x', format = 'orc')",
                "format 'orc'",
            ),
            ("CREATE TABLE t (id BIGINT)", "requires WITH"),
            (
                "CREATE TABLE t () WITH (location = 'x', format = 'delta')",
                "identifier",
            ),
            (
                "CREATE TABLE t (id BIGINT, id VARCHAR) WITH (location = 'x', format = 'delta')",
                "twice",
            ),
            (
                "CREATE TABLE t WITH (location = '../x', format = 'delta')",
                "'..'",
            ),
            (
                "CREATE TABLE t WITH (location = 'abfss://c@a.dfs.core.windows.net/x', format = 'delta')",
                "not a URI",
            ),
            (
                "CREATE TABLE t WITH (location = 'x', format = 'delta', compression = 'zstd')",
                "compression",
            ),
            (
                "CREATE TABLE t (id GEOMETRY) WITH (location = 'x', format = 'delta')",
                "GEOMETRY",
            ),
            (
                "CREATE TABLE t WITH (location = 'x', format = 'delta') extra",
                "unexpected",
            ),
            (
                "CREATE TABLE a.b.c.d WITH (location = 'x', format = 'delta')",
                "[catalog.][schema.]table",
            ),
        ];
        for (sql, expected) in cases {
            let error = parse_catalog_statement(sql)
                .err()
                .unwrap_or_else(|| panic!("{sql} was accepted"))
                .to_string();
            assert!(error.contains(expected), "{sql}: {error}");
        }
    }

    #[test]
    fn drop_statements_carry_if_exists_and_cascade() {
        assert_eq!(
            parse("DROP TABLE IF EXISTS lake.sales.orders"),
            CatalogStatement::DropTable {
                name: name(&["lake", "sales", "orders"]),
                if_exists: true,
            }
        );
        assert_eq!(
            parse("DROP SCHEMA lake.sales CASCADE"),
            CatalogStatement::DropSchema {
                name: name(&["lake", "sales"]),
                if_exists: false,
                cascade: true,
            }
        );
        assert_eq!(
            parse("DROP SCHEMA IF EXISTS sales RESTRICT"),
            CatalogStatement::DropSchema {
                name: name(&["sales"]),
                if_exists: true,
                cascade: false,
            }
        );
        assert_eq!(
            parse("DROP CATALOG IF EXISTS lake CASCADE"),
            CatalogStatement::DropCatalog {
                name: "lake".into(),
                if_exists: true,
                cascade: true,
            }
        );
        assert_eq!(parse_catalog_statement("DROP VIEW v").unwrap(), None);
    }

    #[test]
    fn alter_table_set_location_is_the_only_alteration() {
        assert_eq!(
            parse("ALTER TABLE lake.sales.orders SET LOCATION 'sales/orders_v2'"),
            CatalogStatement::AlterTableSetLocation {
                name: name(&["lake", "sales", "orders"]),
                if_exists: false,
                location: "sales/orders_v2".into(),
            }
        );
        assert_eq!(
            parse("ALTER TABLE IF EXISTS orders SET LOCATION 'orders'"),
            CatalogStatement::AlterTableSetLocation {
                name: name(&["orders"]),
                if_exists: true,
                location: "orders".into(),
            }
        );
        assert!(
            parse_catalog_statement("ALTER TABLE orders ADD COLUMN x BIGINT")
                .unwrap_err()
                .to_string()
                .contains("SET LOCATION")
        );
        assert!(
            parse_catalog_statement("ALTER TABLE orders SET LOCATION orders")
                .unwrap_err()
                .to_string()
                .contains("quoted location")
        );
    }

    #[test]
    fn show_and_describe_statements_resolve_scope_and_like() {
        assert_eq!(
            parse("SHOW CATALOGS"),
            CatalogStatement::ShowCatalogs { like: None }
        );
        assert_eq!(
            parse("show catalogs like 'Open%'"),
            CatalogStatement::ShowCatalogs {
                like: Some("Open%".into())
            }
        );
        assert_eq!(
            parse("SHOW SCHEMAS"),
            CatalogStatement::ShowSchemas {
                catalog: None,
                like: None
            }
        );
        assert_eq!(
            parse("SHOW SCHEMAS FROM OpenSource LIKE 'nyc%'"),
            CatalogStatement::ShowSchemas {
                catalog: Some("OpenSource".into()),
                like: Some("nyc%".into())
            }
        );
        assert_eq!(
            parse("SHOW TABLES IN OpenSource.nyc_taxi"),
            CatalogStatement::ShowTables {
                schema: Some(name(&["OpenSource", "nyc_taxi"])),
                like: None
            }
        );
        assert_eq!(
            parse("SHOW TABLES LIKE '%trips'"),
            CatalogStatement::ShowTables {
                schema: None,
                like: Some("%trips".into())
            }
        );
        assert_eq!(
            parse("SHOW CREATE TABLE lake.sales.orders"),
            CatalogStatement::ShowCreateTable {
                name: name(&["lake", "sales", "orders"])
            }
        );
        for sql in [
            "DESCRIBE orders",
            "DESC TABLE orders",
            "SHOW COLUMNS FROM orders",
            "SHOW COLUMNS IN orders",
        ] {
            assert_eq!(
                parse(sql),
                CatalogStatement::Describe {
                    name: name(&["orders"])
                },
                "{sql}"
            );
        }
        assert!(
            parse_catalog_statement("SHOW CATALOGS LIKE Open")
                .unwrap_err()
                .to_string()
                .contains("single-quoted")
        );
        assert!(
            parse_catalog_statement("SHOW TABLES FROM a.b.c")
                .unwrap_err()
                .to_string()
                .contains("[catalog.]schema")
        );
    }

    #[test]
    fn call_register_table_is_create_table_with_inferred_columns() {
        assert_eq!(
            parse(
                "CALL lake.system.register_table(schema_name => 'sales', table_name => 'orders', table_location => 'sales/orders')"
            ),
            CatalogStatement::CreateTable {
                name: name(&["lake", "sales", "orders"]),
                if_not_exists: false,
                columns: None,
                location: "sales/orders".into(),
                format: DataFormat::Delta,
                access: AccessPattern::Shortcut,
            }
        );
        assert_eq!(
            parse(
                "CALL system.register_table(table_location => 'hits.parquet', format => 'parquet', schema_name => 'clickbench', table_name => 'hits')"
            ),
            CatalogStatement::CreateTable {
                name: name(&["clickbench", "hits"]),
                if_not_exists: false,
                columns: None,
                location: "hits.parquet".into(),
                format: DataFormat::Parquet,
                access: AccessPattern::Shortcut,
            }
        );
        assert_eq!(
            parse("CALL system.unregister_table(schema_name => 'sales', table_name => 'orders')"),
            CatalogStatement::DropTable {
                name: name(&["sales", "orders"]),
                if_exists: false,
            }
        );
        let cases = [
            (
                "CALL system.register_table(schema_name => 'a')",
                "table_name",
            ),
            (
                "CALL system.register_table(schema_name => 'a', table_name => 'b')",
                "table_location",
            ),
            (
                "CALL system.register_table(schema_name => 'a', table_name => 'b', table_location => 'c', owner => 'd')",
                "owner",
            ),
            (
                "CALL system.register_table(schema_name => 'a', schema_name => 'b')",
                "twice",
            ),
            ("CALL system.vacuum(schema_name => 'a')", "vacuum"),
            (
                "CALL other.register_table(schema_name => 'a')",
                "system.register_table",
            ),
        ];
        for (sql, expected) in cases {
            let error = parse_catalog_statement(sql).unwrap_err().to_string();
            assert!(error.contains(expected), "{sql}: {error}");
        }
    }

    #[test]
    fn create_catalog_maps_storage_and_credential_options() {
        assert_eq!(
            parse(
                "CREATE CATALOG IF NOT EXISTS Benchmarks WITH (storage = 'adls', account = 'kvtest', container = 'opensource', root = 'benchmarks', credential = 'workload-identity:kaveon-test-reader')"
            ),
            CatalogStatement::CreateCatalog {
                name: "Benchmarks".into(),
                if_not_exists: true,
                storage: CatalogStorageSpec::AdlsGen2 {
                    account: "kvtest".into(),
                    container: "opensource".into(),
                    root_path: "benchmarks".into(),
                },
                credential: Some(CredentialSpec {
                    kind: "workload-identity".into(),
                    reference: "kaveon-test-reader".into(),
                }),
            }
        );
        assert_eq!(
            parse("CREATE CATALOG local WITH (storage = 'local', base_path = '/data/warehouse')"),
            CatalogStatement::CreateCatalog {
                name: "local".into(),
                if_not_exists: false,
                storage: CatalogStorageSpec::Local {
                    base_path: "/data/warehouse".into(),
                },
                credential: None,
            }
        );
        let cases = [
            (
                "CREATE CATALOG c WITH (storage = 'adls', account = 'a')",
                "container",
            ),
            ("CREATE CATALOG c WITH (account = 'a')", "storage"),
            (
                "CREATE CATALOG c WITH (storage = 'gcs', bucket = 'b')",
                "'gcs'",
            ),
            (
                "CREATE CATALOG c WITH (storage = 'local', base_path = '/x', bucket = 'b')",
                "does not apply",
            ),
            (
                "CREATE CATALOG c WITH (storage = 'local', base_path = '/x', credential = 'secret')",
                "kind:reference",
            ),
            ("CREATE CATALOG c", "requires WITH"),
        ];
        for (sql, expected) in cases {
            let error = parse_catalog_statement(sql).unwrap_err().to_string();
            assert!(error.contains(expected), "{sql}: {error}");
        }
    }

    #[test]
    fn one_statement_per_request() {
        let error = parse_catalog_statement("DROP TABLE a; DROP TABLE b")
            .unwrap_err()
            .to_string();
        assert!(error.contains("one statement"), "{error}");
    }

    #[test]
    fn show_create_table_output_parses_back_to_the_same_definition() {
        let columns = vec![
            ColumnSpec {
                name: "id".into(),
                data_type: DataType::Int64,
                nullable: false,
            },
            ColumnSpec {
                name: "Region Name".into(),
                data_type: DataType::Utf8,
                nullable: true,
            },
            ColumnSpec {
                name: "amount".into(),
                data_type: DataType::Decimal128(12, 2),
                nullable: true,
            },
            ColumnSpec {
                name: "ts".into(),
                data_type: DataType::Timestamp(TimeUnit::Microsecond, None),
                nullable: true,
            },
            ColumnSpec {
                name: "wide".into(),
                data_type: DataType::UInt64,
                nullable: true,
            },
        ];
        let rendered = render_create_table(
            &name(&["OpenSource", "nyc_taxi", "yellow_trips"]),
            &columns,
            "nyc/taxi's/yellow",
            DataFormat::Delta,
            AccessPattern::Shortcut,
        );
        assert!(rendered.starts_with("CREATE TABLE \"OpenSource\".nyc_taxi.yellow_trips (\n"));
        assert_eq!(
            parse(&rendered),
            CatalogStatement::CreateTable {
                name: name(&["OpenSource", "nyc_taxi", "yellow_trips"]),
                if_not_exists: false,
                columns: Some(columns),
                location: "nyc/taxi's/yellow".into(),
                format: DataFormat::Delta,
                access: AccessPattern::Shortcut,
            }
        );
    }
}
