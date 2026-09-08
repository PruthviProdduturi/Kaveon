//! Delta v1 snapshot reconciliation shared by local and cloud readers.
//! Unsupported reader features fail explicitly; they are never silently ignored.
use crate::object_reader::{error, relative_path, storage_error};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use arrow::json::LineDelimitedWriter;
use futures::TryStreamExt;
use kaveon_core::Result;
use object_store::{ObjectStore, path::Path};
use parquet::arrow::{ParquetRecordBatchStreamBuilder, async_reader::ParquetObjectReader};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const MAX_LOG_BYTES: usize = 64 * 1024 * 1024;
const MAX_METADATA_OBJECTS: usize = 100_000;
const MAX_ACTIVE_PATH_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct DeltaSnapshot {
    pub version: u64,
    pub files: Vec<Path>,
    pub schema: Option<SchemaRef>,
}

fn join_path(root: &Path, relative: &str) -> Result<Path> {
    Path::parse(if root.as_ref().is_empty() {
        relative.to_owned()
    } else {
        format!("{root}/{relative}")
    })
    .map_err(storage_error)
}

#[derive(Default)]
struct SnapshotState {
    files: BTreeSet<Path>,
    path_bytes: usize,
    schema: Option<SchemaRef>,
}

impl SnapshotState {
    fn restore_checkpoint_action(&mut self, action: &Value) -> Result<()> {
        // A checkpoint is a reconciled set, not an ordered transaction log.
        // Remove actions are retained tombstones, never operations to replay.
        let mut active = action.clone();
        if let Some(fields) = active.as_object_mut() {
            fields.remove("remove");
        }
        self.apply(&active)
    }
    fn apply(&mut self, action: &Value) -> Result<()> {
        if let Some(protocol) = action.get("protocol").filter(|v| !v.is_null()) {
            let minimum = protocol
                .get("minReaderVersion")
                .and_then(Value::as_u64)
                .ok_or_else(|| error("Delta protocol is missing minReaderVersion"))?;
            if minimum != 1
                || protocol
                    .get("readerFeatures")
                    .and_then(Value::as_array)
                    .is_some_and(|v| !v.is_empty())
            {
                return Err(error(
                    "Delta reader currently supports reader protocol v1 only; column mapping, deletion vectors, and table features require another reader",
                ));
            }
        }
        if action.get("sidecar").is_some_and(|v| !v.is_null())
            || action
                .get("checkpointMetadata")
                .is_some_and(|v| !v.is_null())
        {
            return Err(error("Delta v2 checkpoint sidecars are not supported"));
        }
        if let Some(metadata) = action.get("metaData").filter(|v| !v.is_null()) {
            if let Some(schema) = metadata.get("schemaString").and_then(Value::as_str) {
                self.schema = Some(parse_schema(schema)?);
            }
            if metadata
                .pointer("/format/provider")
                .and_then(Value::as_str)
                .is_some_and(|s| s != "parquet")
            {
                return Err(error("Delta data format must be Parquet"));
            }
            if metadata
                .pointer("/configuration/delta.columnMapping.mode")
                .and_then(Value::as_str)
                .is_some_and(|s| s != "none")
            {
                return Err(error(
                    "Delta column mapping requires reader protocol support",
                ));
            }
            if metadata
                .get("partitionColumns")
                .and_then(Value::as_array)
                .is_some_and(|v| !v.is_empty())
            {
                return Err(error(
                    "Delta partition-column reconstruction is not yet supported; refusing an incomplete schema",
                ));
            }
        }
        if let Some(add) = action.get("add").filter(|v| !v.is_null()) {
            if add.get("deletionVector").is_some_and(|v| !v.is_null()) {
                return Err(error("Delta deletion vectors are not supported"));
            }
            if add
                .get("partitionValues")
                .and_then(Value::as_object)
                .is_some_and(|v| !v.is_empty())
            {
                return Err(error(
                    "Delta partition-column reconstruction is not yet supported",
                ));
            }
            let path = action_path(add)?;
            if !self.files.contains(&path) {
                let bytes = path
                    .as_ref()
                    .len()
                    .checked_add(128)
                    .and_then(|n| self.path_bytes.checked_add(n))
                    .ok_or_else(|| error("Delta active-file metadata size overflow"))?;
                if bytes > MAX_ACTIVE_PATH_BYTES {
                    return Err(error(
                        "Delta active-file metadata exceeds its 64 MiB budget",
                    ));
                }
                self.path_bytes = bytes;
                self.files.insert(path);
            }
        }
        if let Some(remove) = action.get("remove").filter(|v| !v.is_null()) {
            let path = action_path(remove)?;
            if self.files.remove(&path) {
                self.path_bytes -= path.as_ref().len() + 128;
            }
        }
        Ok(())
    }
}

fn parse_schema(text: &str) -> Result<SchemaRef> {
    let value: Value = serde_json::from_str(text).map_err(storage_error)?;
    if value.get("type").and_then(Value::as_str) != Some("struct") {
        return Err(error("Delta schema must be a struct"));
    }
    let fields = value
        .get("fields")
        .and_then(Value::as_array)
        .ok_or_else(|| error("Delta schema has no fields"))?;
    let mut names = BTreeSet::new();
    let fields = fields
        .iter()
        .map(|field| {
            let name = field
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| error("Delta schema field has no name"))?;
            if !names.insert(name) {
                return Err(error("Delta schema has duplicate fields"));
            }
            let kind = field
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| error("nested Delta logical schemas are not yet supported"))?;
            let data_type = match kind {
                "byte" => DataType::Int8,
                "short" => DataType::Int16,
                "integer" => DataType::Int32,
                "long" => DataType::Int64,
                "float" => DataType::Float32,
                "double" => DataType::Float64,
                "boolean" => DataType::Boolean,
                "string" => DataType::Utf8,
                "binary" => DataType::Binary,
                "date" => DataType::Date32,
                "timestamp" => DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                value if value.starts_with("decimal(") && value.ends_with(')') => {
                    let (precision, scale) = value[8..value.len() - 1]
                        .split_once(',')
                        .ok_or_else(|| error("invalid Delta decimal type"))?;
                    let precision: u8 = precision.trim().parse().map_err(storage_error)?;
                    let scale: i8 = scale.trim().parse().map_err(storage_error)?;
                    if precision == 0 || precision > 38 || scale < 0 || scale as u8 > precision {
                        return Err(error("invalid Delta decimal precision/scale"));
                    }
                    DataType::Decimal128(precision, scale)
                }
                _ => return Err(error(format!("unsupported Delta logical type {kind}"))),
            };
            Ok(Field::new(
                name,
                data_type,
                field
                    .get("nullable")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Arc::new(Schema::new(fields)))
}

/// Fail closed when a table's logical schema requires reconstruction we do not
/// yet implement; SELECT * must never silently return an older physical schema.
pub(crate) fn validate_physical_schema(logical: &SchemaRef, physical: &SchemaRef) -> Result<()> {
    if logical.fields().len() != physical.fields().len()
        || logical
            .fields()
            .iter()
            .zip(physical.fields())
            .any(|(logical, physical)| {
                logical.name() != physical.name() || logical.data_type() != physical.data_type()
            })
    {
        return Err(error(
            "Delta logical schema differs from its Parquet files; schema reconstruction is required",
        ));
    }
    Ok(())
}

fn action_path(action: &Value) -> Result<Path> {
    let value = action
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| error("Delta file action has no path"))?;
    // Delta paths are URI encoded. Decode once, then reject separators/traversal.
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let digits = bytes
                .get(i + 1..i + 3)
                .ok_or_else(|| error("invalid Delta path escape"))?;
            let text = std::str::from_utf8(digits).map_err(storage_error)?;
            decoded.push(u8::from_str_radix(text, 16).map_err(storage_error)?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(decoded).map_err(storage_error)?;
    if decoded.contains(':') || decoded.chars().any(char::is_control) {
        return Err(error("unsafe Delta file path"));
    }
    relative_path(&decoded)
}

fn version(name: &str) -> Option<u64> {
    (name.len() == 20 && name.bytes().all(|c| c.is_ascii_digit()))
        .then(|| name.parse().ok())
        .flatten()
}

pub async fn resolve_snapshot(
    store: Arc<dyn ObjectStore>,
    root: &Path,
    target: Option<u64>,
) -> Result<DeltaSnapshot> {
    let prefix = root.child("_delta_log");
    let mut listing = store.list(Some(&prefix));
    let mut commits = BTreeMap::new();
    let mut checkpoints: BTreeMap<u64, BTreeMap<(u32, u32), Path>> = BTreeMap::new();
    let mut count = 0;
    while let Some(meta) = listing.try_next().await.map_err(storage_error)? {
        count += 1;
        if count > MAX_METADATA_OBJECTS {
            return Err(error("Delta log listing exceeds metadata object limit"));
        }
        let Some(name) = meta.location.as_ref().strip_prefix(&format!("{prefix}/")) else {
            continue;
        };
        if name.contains('/') {
            continue;
        }
        if let Some(v) = name.strip_suffix(".json").and_then(version) {
            if target.is_none_or(|target| v <= target) {
                commits.insert(v, meta.location);
            }
            continue;
        }
        let parts = name.split('.').collect::<Vec<_>>();
        if parts.len() < 3 || parts[1] != "checkpoint" {
            continue;
        }
        let Some(v) = version(parts[0]) else {
            continue;
        };
        if target.is_some_and(|target| v > target) {
            continue;
        }
        let coordinates = match parts.as_slice() {
            [_, "checkpoint", "parquet"] => Some((1, 1)),
            [_, "checkpoint", part, total, "parquet"] => {
                part.parse::<u32>().ok().zip(total.parse::<u32>().ok())
            }
            _ => None,
        };
        if let Some((part, total)) = coordinates {
            if total == 0 || total > 10_000 || part == 0 || part > total {
                return Err(error("invalid Delta checkpoint part numbering"));
            }
            checkpoints
                .entry(v)
                .or_default()
                .insert((total, part), meta.location);
        }
    }
    let complete = checkpoints.iter().rev().find_map(|(&v, pieces)| {
        // A classic checkpoint takes precedence over multipart at the same version.
        for &(total, _) in pieces.keys() {
            if (1..=total).all(|part| pieces.contains_key(&(total, part))) {
                return Some((
                    v,
                    (1..=total)
                        .map(|part| pieces[&(total, part)].clone())
                        .collect::<Vec<_>>(),
                ));
            }
        }
        None
    });
    let latest = commits
        .keys()
        .next_back()
        .copied()
        .into_iter()
        .chain(complete.as_ref().map(|(v, _)| *v))
        .max()
        .ok_or_else(|| error("Delta transaction log contains no complete snapshot"))?;
    if target.is_some_and(|target| target != latest) {
        return Err(error("requested Delta version is not available"));
    }
    let mut state = SnapshotState::default();
    let start = if let Some((v, pieces)) = complete {
        for path in pieces {
            let meta = store.head(&path).await.map_err(storage_error)?;
            let reader = ParquetObjectReader::new(store.clone(), meta);
            let mut batches = ParquetRecordBatchStreamBuilder::new(reader)
                .await
                .map_err(storage_error)?
                .with_batch_size(1_024)
                .build()
                .map_err(storage_error)?;
            while let Some(batch) = batches.try_next().await.map_err(storage_error)? {
                let mut writer = LineDelimitedWriter::new(Vec::new());
                writer.write(&batch).map_err(storage_error)?;
                writer.finish().map_err(storage_error)?;
                for line in writer
                    .into_inner()
                    .split(|b| *b == b'\n')
                    .filter(|line| !line.is_empty())
                {
                    state.restore_checkpoint_action(
                        &serde_json::from_slice::<Value>(line).map_err(storage_error)?,
                    )?;
                }
            }
        }
        v.checked_add(1)
            .ok_or_else(|| error("Delta version overflow"))?
    } else {
        0
    };
    for v in start..=latest {
        let path = commits
            .get(&v)
            .ok_or_else(|| error(format!("Delta JSON history is incomplete at version {v}")))?;
        let result = store.get(path).await.map_err(storage_error)?;
        if result.meta.size > MAX_LOG_BYTES {
            return Err(error("Delta JSON commit exceeds 64 MiB limit"));
        }
        let bytes = result.bytes().await.map_err(storage_error)?;
        if bytes.len() > MAX_LOG_BYTES {
            return Err(error("Delta JSON commit exceeds 64 MiB limit"));
        }
        for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
            state.apply(&serde_json::from_slice::<Value>(line).map_err(storage_error)?)?;
        }
    }
    Ok(DeltaSnapshot {
        version: latest,
        schema: state.schema,
        files: state
            .files
            .into_iter()
            .map(|path| join_path(root, path.as_ref()))
            .collect::<Result<_>>()?,
    })
}

pub(crate) fn blocking<T: Send + 'static>(
    future: impl std::future::Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(storage_error)?
            .block_on(future)
    })
    .join()
    .map_err(|_| error("Delta metadata worker panicked"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;
    #[test]
    fn checkpoint_tombstones_do_not_replay_as_ordered_removals() {
        let mut state = SnapshotState::default();
        state
            .restore_checkpoint_action(&serde_json::json!({"add":{"path":"one.parquet"}}))
            .unwrap();
        state
            .restore_checkpoint_action(&serde_json::json!({"remove":{"path":"one.parquet"}}))
            .unwrap();
        assert_eq!(state.files.len(), 1);
        state
            .apply(&serde_json::json!({"remove":{"path":"one.parquet"}}))
            .unwrap();
        assert!(state.files.is_empty());
    }

    #[test]
    fn rejects_features_and_decoded_traversal() {
        for action in [
            serde_json::json!({"protocol":{"minReaderVersion":3}}),
            serde_json::json!({"add":{"path":"%2e%2e/escape"}}),
            serde_json::json!({"add":{"path":"file","deletionVector":{}}}),
        ] {
            assert!(SnapshotState::default().apply(&action).is_err());
        }
        let mut state = SnapshotState::default();
        state
            .apply(&serde_json::json!({"add":{"path":"data%20file.parquet"}}))
            .unwrap();
        assert!(state.files.contains(&Path::from("data file.parquet")));
    }
    #[tokio::test]
    async fn reconciles_versions_and_rejects_gaps() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let root = Path::from("table");
        store
            .put(
                &join_path(&root, "_delta_log/00000000000000000000.json").unwrap(),
                b"{\"add\":{\"path\":\"one.parquet\"}}".to_vec().into(),
            )
            .await
            .unwrap();
        store
            .put(
                &join_path(&root, "_delta_log/00000000000000000001.json").unwrap(),
                b"{\"remove\":{\"path\":\"one.parquet\"}}\n{\"add\":{\"path\":\"two.parquet\"}}"
                    .to_vec()
                    .into(),
            )
            .await
            .unwrap();
        assert_eq!(
            resolve_snapshot(store.clone(), &root, Some(0))
                .await
                .unwrap()
                .files,
            vec![root.child("one.parquet")]
        );
        let snapshot = resolve_snapshot(store.clone(), &root, None).await.unwrap();
        assert_eq!(snapshot.version, 1);
        assert_eq!(snapshot.files, vec![root.child("two.parquet")]);
        store
            .delete(&join_path(&root, "_delta_log/00000000000000000000.json").unwrap())
            .await
            .unwrap();
        assert!(resolve_snapshot(store, &root, None).await.is_err());
    }

    fn checkpoint(paths: Vec<&str>) -> Vec<u8> {
        use arrow::{
            array::{ArrayRef, StringArray, StructArray},
            datatypes::{DataType, Field, Schema},
            record_batch::RecordBatch,
        };
        let fields = vec![Arc::new(Field::new("path", DataType::Utf8, true))].into();
        let add: ArrayRef = Arc::new(StructArray::new(
            fields,
            vec![Arc::new(StringArray::from(paths))],
            None,
        ));
        let schema = Arc::new(Schema::new(vec![Field::new(
            "add",
            add.data_type().clone(),
            true,
        )]));
        let batch = RecordBatch::try_new(schema.clone(), vec![add]).unwrap();
        let mut writer = parquet::arrow::ArrowWriter::try_new(Vec::new(), schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.into_inner().unwrap()
    }

    #[tokio::test]
    async fn checkpoint_restores_pruned_history_and_replays_tail() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let root = Path::from("table");
        store
            .put(
                &join_path(&root, "_delta_log/00000000000000000010.checkpoint.parquet").unwrap(),
                checkpoint(vec!["one.parquet", "sub/two.parquet"]).into(),
            )
            .await
            .unwrap();
        store
            .put(
                &join_path(&root, "_delta_log/00000000000000000011.json").unwrap(),
                b"{\"remove\":{\"path\":\"one.parquet\"}}".to_vec().into(),
            )
            .await
            .unwrap();
        let snapshot = resolve_snapshot(store.clone(), &root, None).await.unwrap();
        assert_eq!(snapshot.version, 11);
        assert_eq!(snapshot.files, vec![Path::from("table/sub/two.parquet")]);
        assert_eq!(
            resolve_snapshot(store, &root, Some(10))
                .await
                .unwrap()
                .files
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn multipart_checkpoint_requires_every_part() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let root = Path::from("table");
        store
            .put(
                &join_path(
                    &root,
                    "_delta_log/00000000000000000010.checkpoint.0000000001.0000000002.parquet",
                )
                .unwrap(),
                checkpoint(vec!["one.parquet"]).into(),
            )
            .await
            .unwrap();
        assert!(resolve_snapshot(store.clone(), &root, None).await.is_err());
        store
            .put(
                &join_path(
                    &root,
                    "_delta_log/00000000000000000010.checkpoint.0000000002.0000000002.parquet",
                )
                .unwrap(),
                checkpoint(vec!["two.parquet"]).into(),
            )
            .await
            .unwrap();
        let snapshot = resolve_snapshot(store.clone(), &root, None).await.unwrap();
        assert_eq!(snapshot.files.len(), 2);
        store
            .put(
                &join_path(&root, "_delta_log/00000000000000000012.json").unwrap(),
                b"{}".to_vec().into(),
            )
            .await
            .unwrap();
        assert!(
            resolve_snapshot(store, &root, None)
                .await
                .unwrap_err()
                .to_string()
                .contains("version 11")
        );
    }
}
