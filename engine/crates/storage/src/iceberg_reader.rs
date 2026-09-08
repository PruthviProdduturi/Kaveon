//! Read-only Iceberg v1/v2 snapshots from an immutable metadata JSON pointer.
//! Manifest status/content and field-ID projection follow https://iceberg.apache.org/spec/.
use crate::{
    ObjectLocation, ObjectParquetReader, ParquetReader, ScanMetrics, ScanPartition,
    delta_snapshot::blocking,
    object_reader::{error, storage_error},
};
use apache_avro::{Reader as AvroReader, types::Value as Avro};
use arrow::{
    array::{ArrayRef, new_null_array},
    datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit},
    record_batch::RecordBatch,
};
use kaveon_core::{BatchSource, Result};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

const FIELD_ID: &str = "PARQUET:field_id";
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct IcebergSnapshot {
    pub metadata_uri: String,
    pub snapshot_id: Option<i64>,
    pub schema: SchemaRef,
    pub files: Vec<String>,
    pub row_count: u64,
}

pub struct IcebergReader {
    metadata_uri: String,
    snapshot_id: Option<i64>,
    columns: Option<Vec<String>>,
    partition: Option<ScanPartition>,
    batch_size: usize,
    object_store: Option<Arc<dyn object_store::ObjectStore>>,
}

impl IcebergReader {
    /// The catalog supplies the committed metadata pointer; directory listing
    /// cannot reliably determine the current committed Iceberg table version.
    pub fn new(metadata_uri: impl Into<String>) -> Self {
        Self {
            metadata_uri: metadata_uri.into(),
            snapshot_id: None,
            columns: None,
            partition: None,
            batch_size: 8192,
            object_store: None,
        }
    }
    pub fn with_snapshot_id(mut self, id: i64) -> Self {
        self.snapshot_id = Some(id);
        self
    }
    pub fn with_columns(mut self, columns: Vec<String>) -> Self {
        self.columns = Some(columns);
        self
    }
    pub fn with_partition(mut self, partition: ScanPartition) -> Self {
        self.partition = Some(partition);
        self
    }
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }
    /// Inject a catalog-owned object-store client; all referenced object URIs
    /// must retain the registered metadata pointer's authority.
    pub fn with_object_store(mut self, store: Arc<dyn object_store::ObjectStore>) -> Self {
        self.object_store = Some(store);
        self
    }
    fn io(&self) -> IcebergIo {
        IcebergIo {
            store: self.object_store.clone(),
            authority: self
                .metadata_uri
                .split_once("://")
                .and_then(|(_, rest)| rest.split_once('/'))
                .map(|(a, _)| a.to_owned()),
        }
    }
    pub fn snapshot(&self) -> Result<IcebergSnapshot> {
        let uri = self.metadata_uri.clone();
        let id = self.snapshot_id;
        let io = self.io();
        blocking(async move { resolve_snapshot(&uri, id, &io).await })
    }
    pub fn read_blocking(self) -> Result<IcebergSource> {
        if self.batch_size == 0 {
            return Err(error("Iceberg batch size must be positive"));
        }
        let start = std::time::Instant::now();
        let snapshot = self.snapshot()?;
        let io = self.io();
        let metrics = ScanMetrics::default();
        metrics.snapshot_time(start.elapsed());
        let schema = if let Some(columns) = self.columns {
            let mut seen = BTreeSet::new();
            let fields = columns
                .iter()
                .map(|name| {
                    if !seen.insert(name) {
                        return Err(error("duplicate Iceberg projection column"));
                    }
                    snapshot
                        .schema
                        .field_with_name(name)
                        .cloned()
                        .map_err(storage_error)
                })
                .collect::<Result<Vec<_>>>()?;
            if fields.is_empty() {
                return Err(error("Iceberg projection cannot be empty"));
            }
            Arc::new(Schema::new(fields))
        } else {
            snapshot.schema
        };
        let files = snapshot
            .files
            .into_iter()
            .enumerate()
            .filter_map(|(i, f)| self.partition.is_none_or(|p| p.contains(i)).then_some(f))
            .collect::<Vec<_>>()
            .into_iter();
        Ok(IcebergSource {
            files,
            schema,
            current: None,
            batch_size: self.batch_size,
            metrics,
            io,
        })
    }
}

pub struct IcebergSource {
    files: std::vec::IntoIter<String>,
    schema: SchemaRef,
    current: Option<Box<dyn BatchSource>>,
    batch_size: usize,
    metrics: ScanMetrics,
    io: IcebergIo,
}
impl IcebergSource {
    pub fn metrics(&self) -> ScanMetrics {
        self.metrics.clone()
    }
}
impl BatchSource for IcebergSource {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            if let Some(source) = &mut self.current {
                if let Some(batch) = source.next_batch()? {
                    return Ok(Some(project_by_id(batch, &self.schema)?));
                }
                self.current = None;
            }
            let Some(file) = self.files.next() else {
                return Ok(None);
            };
            let source: Box<dyn BatchSource> = if is_object(&file) {
                let location = self.io.location(&file)?;
                Box::new(
                    ObjectParquetReader::new(location.store, location.path)
                        .with_batch_size(self.batch_size)
                        .with_metrics(self.metrics.clone())
                        .read_blocking()?,
                )
            } else {
                Box::new(
                    ParquetReader::new(local_path(&file)?)
                        .with_batch_size(self.batch_size)
                        .with_metrics(self.metrics.clone())
                        .read()?,
                )
            };
            // Bind before reading any rows so empty files enforce the same schema rules.
            project_by_id(
                RecordBatch::new_empty(source.schema().clone()),
                &self.schema,
            )?;
            self.current = Some(source);
        }
    }
}

fn project_by_id(batch: RecordBatch, target: &SchemaRef) -> Result<RecordBatch> {
    let schema = batch.schema();
    let mut fields = HashMap::new();
    for (i, field) in schema.fields().iter().enumerate() {
        let id = field.metadata().get(FIELD_ID).ok_or_else(|| {
            error("Iceberg Parquet fields require field IDs; name mapping is unsupported")
        })?;
        if fields.insert(id.as_str(), i).is_some() {
            return Err(error("duplicate Parquet field ID"));
        }
    }
    let columns = target.fields().iter().map(|field| -> Result<ArrayRef> {
        let id = &field.metadata()[FIELD_ID];
        let Some(&index) = fields.get(id.as_str()) else {
            if !field.is_nullable() { return Err(error(format!("required Iceberg field {} is missing from data", field.name()))); }
            return Ok(new_null_array(field.data_type(), batch.num_rows()));
        };
        let source = batch.column(index);
        if !field.is_nullable() && source.null_count() > 0 { return Err(error("required Iceberg field contains NULL")); }
        if source.data_type() == field.data_type() { return Ok(source.clone()); }
        let promotion = matches!((source.data_type(), field.data_type()), (DataType::Int32, DataType::Int64) | (DataType::Float32, DataType::Float64))
            || matches!((source.data_type(), field.data_type()), (DataType::Decimal128(p, s), DataType::Decimal128(q, t)) if p <= q && s == t);
        if !promotion { return Err(error(format!("unsupported Iceberg type evolution: {} to {}", source.data_type(), field.data_type()))); }
        arrow::compute::cast(source, field.data_type()).map_err(storage_error)
    }).collect::<Result<Vec<_>>>()?;
    RecordBatch::try_new(target.clone(), columns).map_err(storage_error)
}

async fn resolve_snapshot(
    uri: &str,
    requested: Option<i64>,
    io: &IcebergIo,
) -> Result<IcebergSnapshot> {
    if !uri.ends_with(".json") {
        return Err(error(
            "Iceberg location must be a committed metadata JSON file",
        ));
    }
    let metadata: Value =
        serde_json::from_slice(&io.read_bytes(uri).await?).map_err(storage_error)?;
    let version = metadata["format-version"]
        .as_i64()
        .ok_or_else(|| error("missing Iceberg format-version"))?;
    if !(1..=2).contains(&version) {
        return Err(error("only Iceberg format v1/v2 is supported"));
    }
    if metadata.get("encryption-keys").is_some() {
        return Err(error("encrypted Iceberg tables are unsupported"));
    }
    let location = metadata["location"]
        .as_str()
        .ok_or_else(|| error("missing Iceberg table location"))?;
    let selected = requested
        .or_else(|| metadata["current-snapshot-id"].as_i64())
        .filter(|&id| id != -1);
    let snapshot = if let Some(id) = selected {
        Some(
            metadata["snapshots"]
                .as_array()
                .and_then(|list| list.iter().find(|s| s["snapshot-id"].as_i64() == Some(id)))
                .ok_or_else(|| error("Iceberg snapshot does not exist in committed metadata"))?,
        )
    } else {
        None
    };
    let schema_id = if requested.is_some() {
        snapshot
            .and_then(|s| s["schema-id"].as_i64())
            .or_else(|| metadata["current-schema-id"].as_i64())
    } else {
        metadata["current-schema-id"].as_i64()
    };
    let schema = match metadata.get("schemas").and_then(Value::as_array) {
        Some(schemas) => schemas
            .iter()
            .find(|s| s["schema-id"].as_i64() == schema_id)
            .ok_or_else(|| error("Iceberg schema ID not found"))?,
        None if version == 1 => metadata
            .get("schema")
            .ok_or_else(|| error("missing Iceberg v1 schema"))?,
        None => return Err(error("missing Iceberg schemas")),
    };
    let schema = parse_schema(schema)?;
    let mut files = BTreeSet::new();
    let mut row_count = 0u64;
    if let Some(snapshot) = snapshot {
        let manifests = if let Some(list) = snapshot["manifest-list"].as_str() {
            let list = resolve_path(location, list)?;
            let bytes = io.read_bytes(&list).await?;
            let reader = AvroReader::new(bytes.as_slice()).map_err(storage_error)?;
            let mut manifests = Vec::new();
            for entry in reader {
                let entry = entry.map_err(storage_error)?;
                let content = avro_field(&entry, "content")
                    .and_then(avro_i64)
                    .or((version == 1).then_some(0))
                    .ok_or_else(|| error("v2 manifest list entry is missing content"))?;
                if content != 0 {
                    return Err(error("Iceberg delete manifests are unsupported"));
                }
                if avro_field(&entry, "key_metadata").is_some_and(avro_has_value) {
                    return Err(error("encrypted Iceberg manifests are unsupported"));
                }
                manifests.push(
                    avro_str(
                        avro_field(&entry, "manifest_path")
                            .ok_or_else(|| error("manifest list entry has no path"))?,
                    )?
                    .to_owned(),
                );
            }
            manifests
        } else if version == 1 {
            snapshot["manifests"]
                .as_array()
                .ok_or_else(|| error("snapshot has no manifest list"))?
                .iter()
                .map(|p| {
                    p.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| error("invalid manifest path"))
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            return Err(error("v2 snapshot has no manifest-list"));
        };
        for manifest in manifests {
            let bytes = io.read_bytes(&resolve_path(location, &manifest)?).await?;
            let reader = AvroReader::new(bytes.as_slice()).map_err(storage_error)?;
            for entry in reader {
                let entry = entry.map_err(storage_error)?;
                match avro_field(&entry, "status").and_then(avro_i64) {
                    Some(2) => continue,
                    Some(0 | 1) => {}
                    _ => return Err(error("invalid Iceberg manifest entry status")),
                }
                let data = avro_field(&entry, "data_file")
                    .ok_or_else(|| error("manifest entry has no data_file"))?;
                if avro_field(data, "content")
                    .and_then(avro_i64)
                    .or((version == 1).then_some(0))
                    .ok_or_else(|| error("v2 manifest data file is missing content"))?
                    != 0
                {
                    return Err(error("Iceberg equality/position deletes are unsupported"));
                }
                if avro_field(data, "key_metadata").is_some_and(avro_has_value) {
                    return Err(error("encrypted Iceberg data files are unsupported"));
                }
                if !avro_str(
                    avro_field(data, "file_format")
                        .ok_or_else(|| error("missing Iceberg data format"))?,
                )?
                .eq_ignore_ascii_case("PARQUET")
                {
                    return Err(error("Iceberg reader supports Parquet data files only"));
                }
                let file = avro_str(
                    avro_field(data, "file_path").ok_or_else(|| error("missing data file path"))?,
                )?;
                let file = resolve_path(location, file)?;
                if !files.insert(file) {
                    return Err(error("duplicate live Iceberg data file"));
                }
                let count = avro_field(data, "record_count")
                    .and_then(avro_i64)
                    .filter(|n| *n >= 0)
                    .ok_or_else(|| error("invalid Iceberg file record count"))?
                    as u64;
                row_count = row_count
                    .checked_add(count)
                    .ok_or_else(|| error("Iceberg row count overflow"))?;
            }
        }
    }
    Ok(IcebergSnapshot {
        metadata_uri: uri.into(),
        snapshot_id: selected,
        schema,
        files: files.into_iter().collect(),
        row_count,
    })
}

fn parse_schema(schema: &Value) -> Result<SchemaRef> {
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    let fields = schema["fields"]
        .as_array()
        .ok_or_else(|| error("Iceberg schema requires fields"))?
        .iter()
        .map(|f| {
            let id = f["id"]
                .as_i64()
                .filter(|id| *id > 0)
                .ok_or_else(|| error("invalid Iceberg field ID"))?;
            let name = f["name"]
                .as_str()
                .ok_or_else(|| error("missing Iceberg field name"))?;
            if !ids.insert(id) || !names.insert(name) {
                return Err(error("duplicate Iceberg field ID/name"));
            }
            if f.get("initial-default").is_some() || f.get("write-default").is_some() {
                return Err(error("Iceberg field defaults are unsupported"));
            }
            let primitive = f["type"]
                .as_str()
                .ok_or_else(|| error("nested Iceberg types are unsupported"))?;
            let data_type = match primitive {
                "boolean" => DataType::Boolean,
                "int" => DataType::Int32,
                "long" => DataType::Int64,
                "float" => DataType::Float32,
                "double" => DataType::Float64,
                "date" => DataType::Date32,
                "time" => DataType::Time64(TimeUnit::Microsecond),
                "timestamp" => DataType::Timestamp(TimeUnit::Microsecond, None),
                "timestamptz" => DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                "string" => DataType::Utf8,
                "binary" => DataType::Binary,
                value if value.starts_with("decimal(") && value.ends_with(')') => {
                    let (precision, scale) = value[8..value.len() - 1]
                        .split_once(',')
                        .ok_or_else(|| error("invalid Iceberg decimal"))?;
                    let precision = precision.trim().parse::<u8>().map_err(storage_error)?;
                    let scale = scale.trim().parse::<i8>().map_err(storage_error)?;
                    if precision == 0 || precision > 38 || scale < 0 || scale as u8 > precision {
                        return Err(error("invalid Iceberg decimal precision/scale"));
                    }
                    DataType::Decimal128(precision, scale)
                }
                _ => {
                    return Err(error(format!(
                        "unsupported Iceberg primitive type {primitive}"
                    )));
                }
            };
            let required = f["required"]
                .as_bool()
                .ok_or_else(|| error("Iceberg field required flag missing"))?;
            Ok(Field::new(name, data_type, !required)
                .with_metadata(HashMap::from([(FIELD_ID.into(), id.to_string())])))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Arc::new(Schema::new(fields)))
}

fn avro_unwrap(value: &Avro) -> &Avro {
    match value {
        Avro::Union(_, value) => avro_unwrap(value),
        _ => value,
    }
}
fn avro_field<'a>(value: &'a Avro, name: &str) -> Option<&'a Avro> {
    if let Avro::Record(fields) = avro_unwrap(value) {
        fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| avro_unwrap(v))
    } else {
        None
    }
}
fn avro_i64(value: &Avro) -> Option<i64> {
    match avro_unwrap(value) {
        Avro::Int(n) => Some(*n as i64),
        Avro::Long(n) => Some(*n),
        _ => None,
    }
}
fn avro_str(value: &Avro) -> Result<&str> {
    match avro_unwrap(value) {
        Avro::String(s) => Ok(s),
        _ => Err(error("Iceberg manifest field must be a string")),
    }
}
fn avro_has_value(value: &Avro) -> bool {
    !matches!(avro_unwrap(value), Avro::Null)
}
fn is_object(uri: &str) -> bool {
    uri.starts_with("s3://") || uri.starts_with("abfss://")
}
fn local_path(uri: &str) -> Result<PathBuf> {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    if path.contains("://") || path.contains(['?', '#']) {
        return Err(error("unsupported Iceberg file URI"));
    }
    #[cfg(windows)]
    let path = if path.starts_with('/') && path.as_bytes().get(2) == Some(&b':') {
        &path[1..]
    } else {
        path
    };
    Ok(PathBuf::from(path))
}
fn resolve_path(location: &str, reference: &str) -> Result<String> {
    if reference.contains(['?', '#']) || reference.split(['/', '\\']).any(|s| s == "..") {
        return Err(error("invalid Iceberg metadata path"));
    }
    if reference.contains("://") {
        if !is_object(reference) && !reference.starts_with("file://") {
            return Err(error("unsupported Iceberg storage scheme"));
        }
        return Ok(reference.into());
    }
    if Path::new(reference).is_absolute() {
        return Ok(reference.into());
    }
    if is_object(location) {
        return Ok(format!("{}/{}", location.trim_end_matches('/'), reference));
    }
    Ok(local_path(location)?
        .join(reference)
        .to_string_lossy()
        .into_owned())
}
#[derive(Clone)]
struct IcebergIo {
    store: Option<Arc<dyn object_store::ObjectStore>>,
    authority: Option<String>,
}
impl IcebergIo {
    fn location(&self, uri: &str) -> Result<ObjectLocation> {
        if let Some(store) = &self.store {
            let (_, rest) = uri
                .split_once("://")
                .ok_or_else(|| error("invalid object URI"))?;
            let (authority, path) = rest
                .split_once('/')
                .ok_or_else(|| error("invalid object URI"))?;
            if self.authority.as_deref() != Some(authority) {
                return Err(error(
                    "Iceberg reference changes injected object-store authority",
                ));
            }
            return Ok(ObjectLocation {
                store: store.clone(),
                path: crate::object_reader::relative_path(path)?,
            });
        }
        ObjectLocation::from_uri(uri)
    }
    async fn read_bytes(&self, uri: &str) -> Result<Vec<u8>> {
        if is_object(uri) {
            let location = self.location(uri)?;
            let meta = location
                .store
                .head(&location.path)
                .await
                .map_err(storage_error)?;
            if meta.size as u64 > MAX_METADATA_BYTES {
                return Err(error("Iceberg metadata exceeds 64 MiB reader limit"));
            }
            let bytes = location
                .store
                .get(&location.path)
                .await
                .map_err(storage_error)?
                .bytes()
                .await
                .map_err(storage_error)?;
            if bytes.len() as u64 > MAX_METADATA_BYTES {
                return Err(error("Iceberg metadata exceeds reader limit"));
            }
            Ok(bytes.to_vec())
        } else {
            let path = local_path(uri)?;
            if std::fs::metadata(&path).map_err(storage_error)?.len() > MAX_METADATA_BYTES {
                return Err(error("Iceberg metadata exceeds 64 MiB reader limit"));
            }
            std::fs::read(path).map_err(storage_error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apache_avro::{Schema as AvroSchema, Writer};
    use arrow::array::{Array, AsArray, Int32Array, Int64Array};
    use serde_json::json;
    struct Fixture {
        root: PathBuf,
        metadata: String,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let temp = std::env::temp_dir();
            if self.root.starts_with(&temp)
                && self
                    .root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("kaveon-iceberg-")
            {
                let _ = std::fs::remove_dir_all(&self.root);
            }
        }
    }
    fn record(fields: Vec<(&str, Avro)>) -> Avro {
        Avro::Record(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    fn avro_file(path: &Path, schema: Value, records: Vec<Avro>) {
        let schema = AvroSchema::parse_str(&schema.to_string()).unwrap();
        let mut writer = Writer::new(&schema, Vec::new()).unwrap();
        for value in records {
            writer.append_value(value).unwrap();
        }
        std::fs::write(path, writer.into_inner().unwrap()).unwrap();
    }
    fn fixture(delete_content: i32) -> Fixture {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kaveon-iceberg-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let data_schema = Arc::new(Schema::new(vec![
            Field::new("old_name", DataType::Int32, false)
                .with_metadata(HashMap::from([(FIELD_ID.into(), "1".into())])),
        ]));
        for (name, values) in [("a.parquet", vec![1, 2]), ("c.parquet", vec![3, 4])] {
            let batch = RecordBatch::try_new(
                data_schema.clone(),
                vec![Arc::new(Int32Array::from(values))],
            )
            .unwrap();
            let mut writer = parquet::arrow::ArrowWriter::try_new(
                std::fs::File::create(root.join(name)).unwrap(),
                data_schema.clone(),
                None,
            )
            .unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        }
        let manifest_schema = json!({"type":"record","name":"manifest_entry","fields":[
            {"name":"status","type":"int"}, {"name":"snapshot_id","type":["null","long"]},
            {"name":"sequence_number","type":["null","long"]}, {"name":"file_sequence_number","type":["null","long"]},
            {"name":"data_file","type":{"type":"record","name":"r2","fields":[
                {"name":"content","type":"int"}, {"name":"file_path","type":"string"}, {"name":"file_format","type":"string"},
                {"name":"partition","type":{"type":"record","name":"r102","fields":[]}},
                {"name":"record_count","type":"long"}, {"name":"file_size_in_bytes","type":"long"}
            ]}}
        ]});
        let entry = |status, name: &str| {
            record(vec![
                ("status", Avro::Int(status)),
                ("snapshot_id", Avro::Union(1, Box::new(Avro::Long(1)))),
                ("sequence_number", Avro::Union(0, Box::new(Avro::Null))),
                ("file_sequence_number", Avro::Union(0, Box::new(Avro::Null))),
                (
                    "data_file",
                    record(vec![
                        ("content", Avro::Int(0)),
                        ("file_path", Avro::String(name.into())),
                        ("file_format", Avro::String("PARQUET".into())),
                        ("partition", record(vec![])),
                        ("record_count", Avro::Long(2)),
                        ("file_size_in_bytes", Avro::Long(100)),
                    ]),
                ),
            ])
        };
        avro_file(
            &root.join("m1.avro"),
            manifest_schema.clone(),
            vec![entry(1, "a.parquet")],
        );
        avro_file(
            &root.join("m2.avro"),
            manifest_schema,
            vec![
                entry(0, "a.parquet"),
                entry(1, "c.parquet"),
                entry(2, "deleted-does-not-exist.parquet"),
            ],
        );
        let list_schema = json!({"type":"record","name":"manifest_file","fields":[{"name":"manifest_path","type":"string"},{"name":"content","type":"int"}]});
        avro_file(
            &root.join("s1.avro"),
            list_schema.clone(),
            vec![record(vec![
                ("manifest_path", Avro::String("m1.avro".into())),
                ("content", Avro::Int(0)),
            ])],
        );
        avro_file(
            &root.join("s2.avro"),
            list_schema,
            vec![record(vec![
                ("manifest_path", Avro::String("m2.avro".into())),
                ("content", Avro::Int(delete_content)),
            ])],
        );
        let metadata = root.join("v2.metadata.json");
        std::fs::write(&metadata,json!({"format-version":2,"location":root.to_string_lossy(),"current-snapshot-id":2,"current-schema-id":1,
            "schemas":[{"type":"struct","schema-id":1,"fields":[{"id":1,"name":"renamed","required":true,"type":"long"},{"id":2,"name":"added","required":false,"type":"string"}]}],
            "snapshots":[{"snapshot-id":1,"schema-id":1,"manifest-list":"s1.avro"},{"snapshot-id":2,"schema-id":1,"manifest-list":"s2.avro"}]
        }).to_string()).unwrap();
        Fixture {
            root,
            metadata: metadata.to_string_lossy().into_owned(),
        }
    }
    fn values(mut source: IcebergSource) -> Vec<i64> {
        let mut values = Vec::new();
        while let Some(batch) = source.next_batch().unwrap() {
            values.extend(
                batch
                    .column(0)
                    .as_primitive::<arrow::datatypes::Int64Type>()
                    .values()
                    .iter()
                    .copied(),
            );
            if batch.num_columns() > 1 {
                assert_eq!(batch.column(1).null_count(), batch.num_rows());
            }
        }
        values
    }
    #[test]
    fn reads_live_snapshot_and_projects_renamed_fields_by_id() {
        let f = fixture(0);
        let snapshot = IcebergReader::new(&f.metadata).snapshot().unwrap();
        assert_eq!(snapshot.snapshot_id, Some(2));
        assert_eq!(snapshot.row_count, 4);
        assert_eq!(snapshot.files.len(), 2);
        assert_eq!(
            values(IcebergReader::new(&f.metadata).read_blocking().unwrap()),
            vec![1, 2, 3, 4]
        );
        assert_eq!(
            values(
                IcebergReader::new(&f.metadata)
                    .with_snapshot_id(1)
                    .with_columns(vec!["renamed".into()])
                    .read_blocking()
                    .unwrap()
            ),
            vec![1, 2]
        );
        assert!(
            IcebergReader::new(&f.metadata)
                .with_snapshot_id(999)
                .snapshot()
                .is_err()
        );
    }
    #[test]
    fn scan_partitions_cover_snapshot_once() {
        let f = fixture(0);
        let mut all = Vec::new();
        for index in 0..3 {
            all.extend(values(
                IcebergReader::new(&f.metadata)
                    .with_partition(ScanPartition::new(index, 3).unwrap())
                    .read_blocking()
                    .unwrap(),
            ));
        }
        all.sort();
        assert_eq!(all, vec![1, 2, 3, 4]);
    }
    #[test]
    fn delete_manifests_fail_before_any_data_is_returned() {
        let f = fixture(1);
        assert!(
            IcebergReader::new(&f.metadata)
                .read_blocking()
                .err()
                .unwrap()
                .to_string()
                .contains("delete manifests")
        );
    }
    #[test]
    fn empty_table_preserves_schema_and_missing_ids_fail() {
        let f = fixture(0);
        let mut metadata: Value =
            serde_json::from_slice(&std::fs::read(&f.metadata).unwrap()).unwrap();
        metadata["current-snapshot-id"] = Value::Null;
        std::fs::write(&f.metadata, metadata.to_string()).unwrap();
        let mut source = IcebergReader::new(&f.metadata).read_blocking().unwrap();
        assert_eq!(source.schema().field(0).data_type(), &DataType::Int64);
        assert!(source.next_batch().unwrap().is_none());
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "renamed",
                DataType::Int64,
                false,
            )])),
            vec![Arc::new(Int64Array::from(vec![1]))],
        )
        .unwrap();
        assert!(project_by_id(batch, source.schema()).is_err());
    }

    #[test]
    fn reads_object_store_snapshot_using_same_manifest_and_field_id_contract() {
        let fixture = fixture(0);
        let store: Arc<dyn object_store::ObjectStore> =
            Arc::new(object_store::memory::InMemory::new());
        let files = std::fs::read_dir(&fixture.root)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                let mut bytes = std::fs::read(&path).unwrap();
                if name.ends_with(".json") {
                    let mut metadata: Value = serde_json::from_slice(&bytes).unwrap();
                    metadata["location"] = Value::String("s3://bucket/table".into());
                    bytes = metadata.to_string().into_bytes();
                }
                (name, bytes)
            })
            .collect::<Vec<_>>();
        let upload = store.clone();
        blocking(async move {
            for (name, bytes) in files {
                upload
                    .put(
                        &object_store::path::Path::from(format!("table/{name}")),
                        bytes.into(),
                    )
                    .await
                    .map_err(storage_error)?;
            }
            Ok(())
        })
        .unwrap();
        let reader =
            IcebergReader::new("s3://bucket/table/v2.metadata.json").with_object_store(store);
        assert_eq!(reader.snapshot().unwrap().files.len(), 2);
        assert_eq!(values(reader.read_blocking().unwrap()), vec![1, 2, 3, 4]);
    }
}
