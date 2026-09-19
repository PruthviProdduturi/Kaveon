//! Rewriting a Parquet table's files in place: the storage side of
//! `OPTIMIZE`.
//!
//! A [`RewriteTarget`] is a Parquet location — one file, or a directory of
//! data files under the Hive/Spark visibility rule of `parquet_directory` —
//! on the local filesystem or an object store. It lists the table's files,
//! selects the ones a predicate may touch by their footer statistics,
//! reads the selected files as one [`BatchSource`] (every row, no
//! predicate: a selected file is rewritten whole), stages the new files
//! through a [`ClusteredParquetWriter`], and publishes them in a
//! crash-safe order. A Hive-partitioned directory is rewritten one
//! partition directory at a time ([`RewriteTarget::groups`]): the new files
//! of a partition land under its own `key=value` path, so the layout rule
//! every listing applies still holds, and a predicate on a partition
//! column selects by the path values before any footer is read.
//!
//! 1. the new files are written to a hidden staging area (`_kaveon_optimize/
//!    <id>/` under a local location, the process temp directory for an
//!    object store), invisible to every listing;
//! 2. for a directory, a recovery manifest `_kaveon_optimize/<id>.json`
//!    naming the files to replace and the files written is stored;
//! 3. the new files become visible — renamed into place on a local
//!    filesystem, uploaded to an object store; a single-file table's one
//!    file replaces the old one in one rename or one PUT;
//! 4. the replaced files are deleted;
//! 5. the manifest is deleted.
//!
//! A crash before 3 leaves the table untouched. A crash after 3 leaves the
//! old and new files both visible; [`RewriteTarget::open`] reads any
//! manifest left behind and either finishes (every written file present:
//! delete the replaced ones) or rolls back (some written file missing:
//! delete the ones that landed), so no row is ever lost and the next
//! `OPTIMIZE` starts from a consistent listing. A query planned while step
//! 3 or 4 runs may list both sets: a plain Parquet directory has no
//! snapshot to isolate it, which is documented as the cost of the format.
//! Delta and Iceberg tables are not rewritten here — their files are owned
//! by a log this crate does not write.

use std::collections::VecDeque;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use futures::TryStreamExt;
use kaveon_core::{BatchSource, KaveonError, Result, StoragePredicate};
use object_store::{ObjectStore, local::LocalFileSystem, path::Path};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::async_reader::{ParquetObjectReader, ParquetRecordBatchStreamBuilder};
use sha2::{Digest, Sha256};

use crate::clustered_writer::{
    ClusteredParquetWriter, ClusteringLayout, FileNaming, LocalDirectorySink, WrittenFile,
};
use crate::delta_snapshot::blocking;
use crate::object_reader::ObjectParquetReader;
use crate::parquet_directory::{
    DirectoryListing, KeptFile, ObjectDirectoryReader, ParquetLocation, PartitionLayout,
    list_parquet_directory, prune_files,
};
use crate::parquet_reader::{ParquetReader, footer_may_match};

/// The hidden directory under a table location that holds staged files
/// and recovery manifests. Hidden by the listing rule: a leading `_`.
pub const REWRITE_AREA: &str = "_kaveon_optimize";
const MANIFEST_VERSION: u64 = 1;
static NEXT_REWRITE: AtomicU64 = AtomicU64::new(0);

/// One data file of the table: its path relative to the store the target
/// opened, its size as listed, and the directory segments between the
/// table root and the file (the `key=value` path of a partitioned
/// directory; empty for a flat one or a single file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableFile {
    pub path: Path,
    pub size: u64,
    pub directory: Vec<String>,
}

/// The files of one directory, rewritten together: the new files are
/// placed under `directory`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewriteGroup {
    pub directory: Vec<String>,
    /// Indices into [`RewriteTarget::files`].
    pub files: Vec<usize>,
}

/// What the location holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RewriteKind {
    /// One Parquet file; its one replacement keeps the name.
    SingleFile,
    /// A directory of data files; replacements are `part-<id>-<n>.parquet`.
    Directory,
}

/// What [`RewriteTarget::open`] did about manifests a previous rewrite
/// left behind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recovery {
    pub finished: u64,
    pub rolled_back: u64,
}

/// The outcome of [`RewriteTarget::publish`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewriteReport {
    pub files_replaced: u64,
    pub files_written: u64,
    pub rows: u64,
    pub row_groups: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

enum Backend {
    /// The store is a `LocalFileSystem` rooted at `prefix`; files are
    /// renamed into place.
    Local { prefix: PathBuf },
    /// Files are uploaded.
    Object,
}

/// A Parquet location opened for rewriting.
pub struct RewriteTarget {
    store: Arc<dyn ObjectStore>,
    backend: Backend,
    /// The location within the store: the file, or the directory root
    /// (empty for a local directory, which is the store's prefix).
    root: Path,
    kind: RewriteKind,
    files: Vec<TableFile>,
    /// The directory listing, for a directory target: its partition
    /// columns and every file's path values.
    listing: Option<DirectoryListing>,
    recovery: Recovery,
    id: String,
}

/// The staged files of one rewrite, before publication.
pub struct Staging {
    directory: PathBuf,
    /// Where under the table root the files are placed.
    placement: Vec<String>,
    id: String,
}

struct Manifest {
    version: u64,
    replaced: Vec<String>,
    written: Vec<String>,
}

impl Manifest {
    fn to_json(&self) -> Vec<u8> {
        serde_json::json!({
            "version": self.version,
            "replaced": self.replaced,
            "written": self.written,
        })
        .to_string()
        .into_bytes()
    }

    fn from_json(bytes: &[u8]) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let names = |key: &str| {
            value
                .get(key)?
                .as_array()?
                .iter()
                .map(|name| name.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        };
        Some(Self {
            version: value.get("version")?.as_u64()?,
            replaced: names("replaced")?,
            written: names("written")?,
        })
    }
}

impl RewriteTarget {
    /// Open `location`: a local path, or an `abfss://` / `s3://` URI, of one
    /// Parquet file or a directory of them. Manifests left by an
    /// interrupted rewrite are finished or rolled back first, and the
    /// listing taken afterwards.
    pub fn open(location: &str) -> Result<Self> {
        let (store, backend, root): (Arc<dyn ObjectStore>, Backend, Path) =
            if location.starts_with("abfss://") || location.starts_with("s3://") {
                let reader = ObjectDirectoryReader::from_uri(location)?;
                (reader.store(), Backend::Object, reader.root().clone())
            } else {
                let path = FsPath::new(location);
                let (prefix, root) = if path.is_dir() {
                    (path.to_path_buf(), Path::default())
                } else {
                    let parent = path
                        .parent()
                        .filter(|parent| parent.is_dir())
                        .ok_or_else(|| error(format!("location '{location}' does not exist")))?;
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| error(format!("location '{location}' has no file name")))?;
                    (parent.to_path_buf(), Path::from(name))
                };
                let store = LocalFileSystem::new_with_prefix(&prefix).map_err(storage_error)?;
                (Arc::new(store), Backend::Local { prefix }, root)
            };
        Self::build(store, backend, root)
    }

    /// A location on a store the caller holds, published by upload.
    pub fn over_store(store: Arc<dyn ObjectStore>, root: Path) -> Result<Self> {
        Self::build(store, Backend::Object, root)
    }

    fn build(store: Arc<dyn ObjectStore>, backend: Backend, root: Path) -> Result<Self> {
        let id = rewrite_id();
        let mut target = Self {
            store,
            backend,
            root,
            kind: RewriteKind::Directory,
            files: Vec::new(),
            listing: None,
            recovery: Recovery::default(),
            id,
        };
        target.kind = match target.probe()? {
            ParquetLocation::Object(_) => RewriteKind::SingleFile,
            ParquetLocation::Directory(_) => RewriteKind::Directory,
        };
        if target.kind == RewriteKind::Directory {
            target.recovery = target.recover()?;
        }
        target.list()?;
        Ok(target)
    }

    fn probe(&self) -> Result<ParquetLocation> {
        let store = Arc::clone(&self.store);
        let root = self.root.clone();
        blocking(async move {
            // An empty root is the store's own prefix: a local directory,
            // which no `HEAD` describes.
            let head = if root.as_ref().is_empty() {
                Err(object_store::Error::NotFound {
                    path: String::new(),
                    source: "the location is a directory".into(),
                })
            } else {
                store.head(&root).await
            };
            match head {
                Ok(object) => Ok(ParquetLocation::Object(object)),
                Err(object_store::Error::NotFound { .. }) => {
                    let listing = list_parquet_directory(store.as_ref(), &root).await?;
                    if listing.files.is_empty() {
                        return Err(error(format!(
                            "location '{root}' is neither a Parquet file nor a directory of Parquet files"
                        )));
                    }
                    Ok(ParquetLocation::Directory(listing))
                }
                Err(failure) => Err(storage_error(failure)),
            }
        })
    }

    fn list(&mut self) -> Result<()> {
        match self.probe()? {
            ParquetLocation::Object(object) => {
                self.files = vec![TableFile {
                    path: object.location,
                    size: u64::try_from(object.size).map_err(storage_error)?,
                    directory: Vec::new(),
                }];
                self.listing = None;
            }
            ParquetLocation::Directory(listing) => {
                self.files = listing
                    .files
                    .iter()
                    .map(|file| {
                        // The segments between the root and the file name.
                        let mut directory: Vec<String> = file
                            .path
                            .prefix_match(&self.root)
                            .map(|parts| parts.map(|part| part.as_ref().to_owned()).collect())
                            .unwrap_or_default();
                        directory.pop();
                        TableFile {
                            path: file.path.clone(),
                            size: file.size,
                            directory,
                        }
                    })
                    .collect();
                self.listing = Some(listing);
            }
        }
        Ok(())
    }

    pub fn kind(&self) -> &RewriteKind {
        &self.kind
    }

    /// The table's data files, sorted by path.
    pub fn files(&self) -> &[TableFile] {
        &self.files
    }

    /// The partition columns of a Hive-partitioned directory, in path
    /// order; empty otherwise.
    pub fn partition_columns(&self) -> &[kaveon_core::PartitionColumn] {
        self.listing
            .as_ref()
            .map_or(&[], |listing| listing.partitions.as_slice())
    }

    /// `selected` grouped by directory, in listing order: one group per
    /// partition directory, one group for a flat directory or a file.
    pub fn groups(&self, selected: &[usize]) -> Result<Vec<RewriteGroup>> {
        let mut groups: Vec<RewriteGroup> = Vec::new();
        for index in selected {
            let file = self
                .files
                .get(*index)
                .ok_or_else(|| error(format!("file index {index} is out of range")))?;
            match groups
                .iter_mut()
                .find(|group| group.directory == file.directory)
            {
                Some(group) => group.files.push(*index),
                None => groups.push(RewriteGroup {
                    directory: file.directory.clone(),
                    files: vec![*index],
                }),
            }
        }
        Ok(groups)
    }

    /// What was done about interrupted rewrites when the target opened.
    pub fn recovery(&self) -> &Recovery {
        &self.recovery
    }

    /// This rewrite's identifier: the staging directory's and the
    /// manifest's name, and part of every written file's name.
    pub fn id(&self) -> &str {
        &self.id
    }

    fn area(&self) -> Path {
        self.root.child(REWRITE_AREA)
    }

    /// Finish or roll back every manifest under the rewrite area, then
    /// clear the area of anything else (staged files of an interrupted
    /// rewrite).
    fn recover(&self) -> Result<Recovery> {
        let store = Arc::clone(&self.store);
        let area = self.area();
        blocking(async move {
            let mut recovery = Recovery::default();
            let objects = store
                .list(Some(&area))
                .try_collect::<Vec<_>>()
                .await
                .map_err(storage_error)?;
            for object in &objects {
                if !object.location.as_ref().ends_with(".json")
                    || object
                        .location
                        .prefix_match(&area)
                        .is_none_or(|mut parts| parts.next().is_some() && parts.next().is_some())
                {
                    continue;
                }
                let bytes = store
                    .get(&object.location)
                    .await
                    .map_err(storage_error)?
                    .bytes()
                    .await
                    .map_err(storage_error)?;
                let manifest = Manifest::from_json(&bytes).ok_or_else(|| {
                    error(format!(
                        "rewrite manifest '{}' is not readable",
                        object.location
                    ))
                })?;
                if manifest.version != MANIFEST_VERSION {
                    return Err(error(format!(
                        "rewrite manifest '{}' is version {}, this Engine writes {MANIFEST_VERSION}",
                        object.location, manifest.version
                    )));
                }
                let mut landed = Vec::new();
                let mut complete = true;
                for name in &manifest.written {
                    let path = Path::parse(name).map_err(storage_error)?;
                    match store.head(&path).await {
                        Ok(_) => landed.push(path),
                        Err(object_store::Error::NotFound { .. }) => complete = false,
                        Err(failure) => return Err(storage_error(failure)),
                    }
                }
                if complete {
                    for name in &manifest.replaced {
                        delete_if_present(
                            store.as_ref(),
                            &Path::parse(name).map_err(storage_error)?,
                        )
                        .await?;
                    }
                    recovery.finished += 1;
                } else {
                    for path in &landed {
                        delete_if_present(store.as_ref(), path).await?;
                    }
                    recovery.rolled_back += 1;
                }
                delete_if_present(store.as_ref(), &object.location).await?;
            }
            // Whatever else is under the area is a leftover of a rewrite
            // that never reached its manifest: staged files, deletable.
            for object in objects {
                delete_if_present(store.as_ref(), &object.location).await?;
            }
            Ok(recovery)
        })
    }

    /// The indices of the files that may hold rows satisfying `predicate`:
    /// a partitioned directory's path values are folded first (the same
    /// rule a scan prunes files by, the partition columns typed as
    /// `catalog_schema` declares them), then the footer statistics answer
    /// what the path left open. Every file when `predicate` is `None`.
    pub fn select(
        &self,
        predicate: Option<&StoragePredicate>,
        catalog_schema: Option<&SchemaRef>,
    ) -> Result<Vec<usize>> {
        let Some(predicate) = predicate else {
            return Ok((0..self.files.len()).collect());
        };
        let kept = match &self.listing {
            Some(listing) if !listing.partitions.is_empty() => {
                let layout = PartitionLayout::of(listing, catalog_schema)?;
                prune_files(&layout, Some(predicate)).kept
            }
            _ => (0..self.files.len())
                .map(|index| KeptFile {
                    index,
                    residual: Some(predicate.clone()),
                })
                .collect(),
        };
        let mut selected = Vec::new();
        for kept in kept {
            let file = &self.files[kept.index];
            let admitted = match &kept.residual {
                None => true,
                Some(residual) => {
                    let (metadata, schema) = self.footer(file)?;
                    footer_may_match(&metadata, &schema, residual)?
                }
            };
            if admitted {
                selected.push(kept.index);
            }
        }
        Ok(selected)
    }

    fn footer(
        &self,
        file: &TableFile,
    ) -> Result<(Arc<parquet::file::metadata::ParquetMetaData>, SchemaRef)> {
        match &self.backend {
            Backend::Local { prefix } => {
                let path = prefix.join(file.path.as_ref());
                let opened = std::fs::File::open(&path).map_err(|failure| {
                    error(format!("cannot open '{}': {failure}", path.display()))
                })?;
                let builder =
                    ParquetRecordBatchReaderBuilder::try_new(opened).map_err(storage_error)?;
                Ok((Arc::clone(builder.metadata()), Arc::clone(builder.schema())))
            }
            Backend::Object => {
                let store = Arc::clone(&self.store);
                let path = file.path.clone();
                blocking(async move {
                    let meta = store.head(&path).await.map_err(storage_error)?;
                    let builder =
                        ParquetRecordBatchStreamBuilder::new(ParquetObjectReader::new(store, meta))
                            .await
                            .map_err(storage_error)?;
                    Ok((Arc::clone(builder.metadata()), Arc::clone(builder.schema())))
                })
            }
        }
    }

    /// Every row of the selected files, file after file in listing order,
    /// as one source with the first file's schema; a file of another
    /// schema is an error naming both files.
    pub fn source(&self, selected: &[usize]) -> Result<Box<dyn BatchSource>> {
        let mut openers: VecDeque<(String, FileOpener)> = VecDeque::new();
        for index in selected {
            let file = self
                .files
                .get(*index)
                .ok_or_else(|| error(format!("file index {index} is out of range")))?;
            let name = file.path.to_string();
            let opener: FileOpener = match &self.backend {
                Backend::Local { prefix } => {
                    let path = prefix.join(file.path.as_ref());
                    Box::new(move || {
                        Ok(Box::new(ParquetReader::new(path).read()?) as Box<dyn BatchSource>)
                    })
                }
                Backend::Object => {
                    let store = Arc::clone(&self.store);
                    let path = file.path.clone();
                    Box::new(move || {
                        Ok(
                            Box::new(ObjectParquetReader::new(store, path).read_blocking()?)
                                as Box<dyn BatchSource>,
                        )
                    })
                }
            };
            openers.push_back((name, opener));
        }
        let Some((first_name, first)) = openers.pop_front() else {
            return Err(error("no files selected"));
        };
        let current = first()?;
        Ok(Box::new(ChainedFiles {
            schema: current.schema().clone(),
            first_name,
            current: Some(current),
            pending: openers,
        }))
    }

    /// Bytes of the selected files as listed.
    pub fn selected_bytes(&self, selected: &[usize]) -> u64 {
        selected
            .iter()
            .filter_map(|index| self.files.get(*index))
            .fold(0_u64, |total, file| total.saturating_add(file.size))
    }

    /// A writer whose files are staged for [`Self::publish`], to be placed
    /// under `placement` (a group's directory) below the table root.
    pub fn stage(
        &self,
        layout: ClusteringLayout,
        schema: SchemaRef,
        placement: &[String],
    ) -> Result<(ClusteredParquetWriter, Staging)> {
        let mut directory = match &self.backend {
            Backend::Local { prefix } => prefix.join(REWRITE_AREA).join(&self.id),
            Backend::Object => std::env::temp_dir().join("kaveon-optimize").join(&self.id),
        };
        for segment in placement {
            directory.push(segment);
        }
        let naming = match self.kind {
            RewriteKind::SingleFile => {
                let name = self
                    .root
                    .filename()
                    .ok_or_else(|| error("the location has no file name"))?
                    .to_owned();
                FileNaming::Single(name)
            }
            RewriteKind::Directory => FileNaming::Parts {
                prefix: format!("part-{}", self.id),
            },
        };
        let sink = LocalDirectorySink::new(&directory)?;
        let writer = ClusteredParquetWriter::new(layout, schema, naming, Box::new(sink))?;
        Ok((
            writer,
            Staging {
                directory,
                placement: placement.to_vec(),
                id: self.id.clone(),
            },
        ))
    }

    /// The object path of a written file: the root, the placement, the
    /// name.
    fn placed(&self, placement: &[String], name: &str) -> Path {
        self.root
            .parts()
            .map(|part| part.as_ref().to_owned())
            .chain(placement.iter().cloned())
            .chain(std::iter::once(name.to_owned()))
            .collect()
    }

    /// Make the staged files the table's files in place of `replaced`
    /// (indices into [`Self::files`]), in the order the module
    /// documentation gives, and remove the staging area.
    pub fn publish(
        &self,
        staging: Staging,
        written: &[WrittenFile],
        replaced: &[usize],
    ) -> Result<RewriteReport> {
        if staging.id != self.id {
            return Err(error("staging does not belong to this rewrite"));
        }
        let replaced_files = replaced
            .iter()
            .map(|index| {
                self.files
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| error(format!("file index {index} is out of range")))
            })
            .collect::<Result<Vec<_>>>()?;
        let result = self.publish_inner(&staging, written, &replaced_files);
        let _ = std::fs::remove_dir_all(&staging.directory);
        if let Backend::Local { prefix } = &self.backend {
            // The rewrite's staging directory and the area itself, when
            // nothing else is staged in them.
            let mut staged = staging.directory.clone();
            for _ in &staging.placement {
                staged.pop();
                let _ = std::fs::remove_dir(&staged);
            }
            let _ = std::fs::remove_dir(prefix.join(REWRITE_AREA));
        }
        result
    }

    fn publish_inner(
        &self,
        staging: &Staging,
        written: &[WrittenFile],
        replaced: &[TableFile],
    ) -> Result<RewriteReport> {
        let bytes_before = replaced
            .iter()
            .fold(0_u64, |total, file| total.saturating_add(file.size));
        let bytes_after = written
            .iter()
            .fold(0_u64, |total, file| total.saturating_add(file.bytes));
        let report = RewriteReport {
            files_replaced: replaced.len() as u64,
            files_written: written.len() as u64,
            rows: written.iter().map(|file| file.rows).sum(),
            row_groups: written.iter().map(|file| file.row_groups as u64).sum(),
            bytes_before,
            bytes_after,
        };
        match self.kind {
            RewriteKind::SingleFile => {
                let [file] = written else {
                    return Err(error(format!(
                        "a single-file table is rewritten as one file, not {}",
                        written.len()
                    )));
                };
                self.place(&staging.directory.join(&file.name), &self.root)?;
            }
            RewriteKind::Directory => {
                // One manifest per group: a partitioned table's groups are
                // published one after another, each complete on its own.
                let manifest = self.area().child(
                    format!(
                        "{}-{}.json",
                        self.id,
                        staging.placement.join("-").replace('/', "-")
                    )
                    .as_str(),
                );
                let placed = written
                    .iter()
                    .map(|file| self.placed(&staging.placement, &file.name))
                    .collect::<Vec<_>>();
                let document = Manifest {
                    version: MANIFEST_VERSION,
                    replaced: replaced.iter().map(|file| file.path.to_string()).collect(),
                    written: placed.iter().map(Path::to_string).collect(),
                }
                .to_json();
                self.put(&manifest, document)?;
                for (file, target) in written.iter().zip(&placed) {
                    self.place(&staging.directory.join(&file.name), target)?;
                }
                for file in replaced {
                    self.delete(&file.path)?;
                }
                self.delete(&manifest)?;
            }
        }
        Ok(report)
    }

    /// A staged local file becomes the object at `target`: one rename on a
    /// local filesystem, one upload otherwise.
    fn place(&self, staged: &FsPath, target: &Path) -> Result<()> {
        match &self.backend {
            Backend::Local { prefix } => {
                let destination = prefix.join(target.as_ref());
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent).map_err(|failure| {
                        error(format!("cannot create '{}': {failure}", parent.display()))
                    })?;
                }
                std::fs::rename(staged, &destination).map_err(|failure| {
                    error(format!(
                        "cannot move '{}' to '{}': {failure}",
                        staged.display(),
                        destination.display()
                    ))
                })
            }
            Backend::Object => {
                let store = Arc::clone(&self.store);
                let staged = staged.to_path_buf();
                let target = target.clone();
                blocking(async move {
                    let mut source = tokio::fs::File::open(&staged).await.map_err(|failure| {
                        error(format!("cannot open '{}': {failure}", staged.display()))
                    })?;
                    let mut upload = object_store::buffered::BufWriter::new(store, target.clone());
                    tokio::io::copy(&mut source, &mut upload)
                        .await
                        .map_err(|failure| {
                            error(format!("upload of '{target}' failed: {failure}"))
                        })?;
                    tokio::io::AsyncWriteExt::shutdown(&mut upload)
                        .await
                        .map_err(|failure| error(format!("upload of '{target}' failed: {failure}")))
                })
            }
        }
    }

    fn put(&self, path: &Path, bytes: Vec<u8>) -> Result<()> {
        let store = Arc::clone(&self.store);
        let path = path.clone();
        blocking(async move {
            store
                .put(&path, bytes.into())
                .await
                .map(|_| ())
                .map_err(storage_error)
        })
    }

    fn delete(&self, path: &Path) -> Result<()> {
        let store = Arc::clone(&self.store);
        let path = path.clone();
        blocking(async move { delete_if_present(store.as_ref(), &path).await })
    }
}

async fn delete_if_present(store: &dyn ObjectStore, path: &Path) -> Result<()> {
    match store.delete(path).await {
        Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
        Err(failure) => Err(storage_error(failure)),
    }
}

/// Opens one selected file as a source when the chain reaches it.
type FileOpener = Box<dyn FnOnce() -> Result<Box<dyn BatchSource>> + Send>;

/// The selected files read one after another.
struct ChainedFiles {
    schema: SchemaRef,
    first_name: String,
    current: Option<Box<dyn BatchSource>>,
    pending: VecDeque<(String, FileOpener)>,
}

impl BatchSource for ChainedFiles {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            if let Some(current) = &mut self.current
                && let Some(batch) = current.next_batch()?
            {
                return Ok(Some(batch));
            }
            let Some((name, open)) = self.pending.pop_front() else {
                self.current = None;
                return Ok(None);
            };
            let next = open()?;
            if next.schema().fields() != self.schema.fields() {
                return Err(error(format!(
                    "file '{name}' has a different schema from '{}': {:?} versus {:?}",
                    self.first_name,
                    next.schema().fields(),
                    self.schema.fields()
                )));
            }
            self.current = Some(next);
        }
    }
}

/// A short identifier unique to this process, time and call.
fn rewrite_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let digest = Sha256::digest(
        format!(
            "{nanos}:{}:{}",
            std::process::id(),
            NEXT_REWRITE.fetch_add(1, Ordering::Relaxed)
        )
        .as_bytes(),
    );
    format!("{digest:x}")[..16].to_owned()
}

fn error(message: impl Into<String>) -> KaveonError {
    KaveonError::Storage(message.into())
}

fn storage_error(failure: impl std::fmt::Display) -> KaveonError {
    KaveonError::Storage(failure.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use arrow::compute::{concat_batches, sort_to_indices, take};
    use arrow::datatypes::{DataType, Field, Schema};
    use kaveon_core::{CompareOp, ScalarValue};
    use object_store::memory::InMemory;
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;

    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("key", DataType::Int64, false),
            Field::new("region", DataType::Utf8, false),
        ]))
    }

    /// `rows` rows with keys `start..start + rows` in reverse order.
    fn rows(start: i64, rows: i64) -> RecordBatch {
        let keys = (start..start + rows).rev().collect::<Vec<_>>();
        RecordBatch::try_new(
            schema(),
            vec![
                Arc::new(Int64Array::from(keys.clone())),
                Arc::new(StringArray::from_iter_values(
                    keys.iter().map(|key| format!("r{}", key % 3)),
                )),
            ],
        )
        .unwrap()
    }

    fn parquet(batch: &RecordBatch) -> Vec<u8> {
        let mut bytes = Vec::new();
        let properties = WriterProperties::builder()
            .set_max_row_group_size(500)
            .build();
        let mut writer =
            ArrowWriter::try_new(&mut bytes, batch.schema(), Some(properties)).unwrap();
        writer.write(batch).unwrap();
        writer.close().unwrap();
        bytes
    }

    fn temporary(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "kaveon-rewrite-{label}-{}-{}",
            std::process::id(),
            rewrite_id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn sorted(batches: &[RecordBatch]) -> RecordBatch {
        let all = concat_batches(&schema(), batches).unwrap();
        let indices = sort_to_indices(all.column(0), None, None).unwrap();
        RecordBatch::try_new(
            schema(),
            all.columns()
                .iter()
                .map(|column| take(column, &indices, None).unwrap())
                .collect(),
        )
        .unwrap()
    }

    fn drain(source: &mut dyn BatchSource) -> Vec<RecordBatch> {
        let mut batches = Vec::new();
        while let Some(batch) = source.next_batch().unwrap() {
            batches.push(batch);
        }
        batches
    }

    fn keys(batches: &[RecordBatch]) -> Vec<i64> {
        batches
            .iter()
            .flat_map(|batch| {
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap()
                    .values()
                    .to_vec()
            })
            .collect()
    }

    /// Rewrite the selected files of `target` clustered by key, the way
    /// `OPTIMIZE` does with the executor's sort in between: one group per
    /// directory, each published on its own.
    fn rewrite(target: &RewriteTarget, selected: &[usize]) -> RewriteReport {
        let mut report = RewriteReport {
            files_replaced: 0,
            files_written: 0,
            rows: 0,
            row_groups: 0,
            bytes_before: 0,
            bytes_after: 0,
        };
        for group in target.groups(selected).unwrap() {
            let mut source = target.source(&group.files).unwrap();
            let batches = drain(source.as_mut());
            let sorted = sorted(&batches);
            let layout = ClusteringLayout::new(vec!["key".into()], vec![])
                .with_max_row_group_rows(1_000)
                .with_target_file_bytes(Some(8 * 1024));
            let (mut writer, staging) = target.stage(layout, schema(), &group.directory).unwrap();
            writer.write(&sorted).unwrap();
            let written = writer.finish().unwrap();
            let published = target.publish(staging, &written, &group.files).unwrap();
            report.files_replaced += published.files_replaced;
            report.files_written += published.files_written;
            report.rows += published.rows;
            report.row_groups += published.row_groups;
            report.bytes_before += published.bytes_before;
            report.bytes_after += published.bytes_after;
        }
        report
    }

    fn local_directory_table() -> (PathBuf, PathBuf) {
        let base = temporary("directory");
        let table = base.join("events");
        std::fs::create_dir_all(&table).unwrap();
        for (index, start) in [0_i64, 3_000, 6_000].into_iter().enumerate() {
            std::fs::write(
                table.join(format!("old-{index}.parquet")),
                parquet(&rows(start, 3_000)),
            )
            .unwrap();
        }
        std::fs::write(table.join("_SUCCESS"), b"").unwrap();
        (base, table)
    }

    #[test]
    fn a_local_directory_table_is_rewritten_in_place_with_a_clean_area() {
        let (base, table) = local_directory_table();
        let target = RewriteTarget::open(table.to_str().unwrap()).unwrap();
        assert_eq!(*target.kind(), RewriteKind::Directory);
        assert_eq!(target.files().len(), 3);
        assert_eq!(*target.recovery(), Recovery::default());

        let report = rewrite(&target, &[0, 1, 2]);
        assert_eq!(report.files_replaced, 3);
        assert!(report.files_written > 1, "{report:?}");
        assert_eq!(report.rows, 9_000);
        assert!(report.bytes_before > 0 && report.bytes_after > 0);

        let listed = RewriteTarget::open(table.to_str().unwrap()).unwrap();
        assert_eq!(listed.files().len() as u64, report.files_written);
        assert!(listed.files().iter().all(|file| {
            file.path
                .as_ref()
                .starts_with(&format!("part-{}-", target.id()))
        }));
        assert!(!table.join(REWRITE_AREA).exists());
        assert!(table.join("_SUCCESS").exists());
        let mut source = listed.source(&[0, 1]).unwrap();
        let first_two = keys(&drain(source.as_mut()));
        assert!(first_two.windows(2).all(|pair| pair[0] <= pair[1]));
        let mut all = listed
            .source(&(0..listed.files().len()).collect::<Vec<_>>())
            .unwrap();
        let mut every_key = keys(&drain(all.as_mut()));
        every_key.sort_unstable();
        assert_eq!(every_key, (0..9_000).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_predicate_selects_files_by_their_statistics_and_leaves_the_rest() {
        let (base, table) = local_directory_table();
        let target = RewriteTarget::open(table.to_str().unwrap()).unwrap();
        let predicate = StoragePredicate::Compare {
            column: "key".into(),
            op: CompareOp::Ge,
            value: ScalarValue::Int64(6_500),
        };
        let selected = target.select(Some(&predicate), None).unwrap();
        assert_eq!(selected, vec![2]);
        let unknown = StoragePredicate::Compare {
            column: "nope".into(),
            op: CompareOp::Eq,
            value: ScalarValue::Int64(1),
        };
        assert!(target.select(Some(&unknown), None).is_err());
        assert_eq!(target.select(None, None).unwrap(), vec![0, 1, 2]);

        let report = rewrite(&target, &selected);
        assert_eq!(report.files_replaced, 1);
        assert_eq!(report.rows, 3_000);
        let listed = RewriteTarget::open(table.to_str().unwrap()).unwrap();
        let names = listed
            .files()
            .iter()
            .map(|file| file.path.to_string())
            .collect::<Vec<_>>();
        assert!(names.contains(&"old-0.parquet".to_owned()), "{names:?}");
        assert!(names.contains(&"old-1.parquet".to_owned()), "{names:?}");
        assert!(!names.contains(&"old-2.parquet".to_owned()), "{names:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_single_file_table_is_replaced_by_one_clustered_file() {
        let base = temporary("single");
        let file = base.join("hits.parquet");
        std::fs::write(&file, parquet(&rows(0, 5_000))).unwrap();
        let target = RewriteTarget::open(file.to_str().unwrap()).unwrap();
        assert_eq!(*target.kind(), RewriteKind::SingleFile);
        assert_eq!(target.files().len(), 1);
        let report = rewrite(&target, &[0]);
        assert_eq!(report.files_written, 1);
        assert_eq!(report.rows, 5_000);
        assert!(file.exists());
        assert!(!base.join(REWRITE_AREA).exists());
        let reader = ParquetReader::new(&file);
        assert_eq!(reader.metadata().unwrap().row_group_count, 5);
        let read = keys(&reader.read_batches().unwrap());
        assert_eq!(read, (0..5_000).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn interrupted_rewrites_are_finished_or_rolled_back_on_open() {
        let (base, table) = local_directory_table();
        let area = table.join(REWRITE_AREA);
        std::fs::create_dir_all(&area).unwrap();
        // A rewrite that crashed after every new file landed: the two
        // replaced files are still there, the manifest names them.
        std::fs::write(
            table.join("part-done-00000.parquet"),
            parquet(&rows(0, 6_000)),
        )
        .unwrap();
        std::fs::write(
            area.join("done.json"),
            Manifest {
                version: MANIFEST_VERSION,
                replaced: vec!["old-0.parquet".into(), "old-1.parquet".into()],
                written: vec!["part-done-00000.parquet".into()],
                // (a flat directory: the store path is the name)
            }
            .to_json(),
        )
        .unwrap();
        // A rewrite that crashed with one of two new files landed: the
        // landed one goes, the replaced file stays.
        std::fs::write(
            table.join("part-half-00000.parquet"),
            parquet(&rows(6_000, 1_000)),
        )
        .unwrap();
        std::fs::write(
            area.join("half.json"),
            Manifest {
                version: MANIFEST_VERSION,
                replaced: vec!["old-2.parquet".into()],
                written: vec![
                    "part-half-00000.parquet".into(),
                    "part-half-00001.parquet".into(),
                ],
            }
            .to_json(),
        )
        .unwrap();
        // Staged files of a rewrite that never wrote its manifest.
        std::fs::create_dir_all(area.join("stale")).unwrap();
        std::fs::write(
            area.join("stale").join("part-stale-00000.parquet"),
            b"partial",
        )
        .unwrap();

        let target = RewriteTarget::open(table.to_str().unwrap()).unwrap();
        assert_eq!(
            *target.recovery(),
            Recovery {
                finished: 1,
                rolled_back: 1,
            }
        );
        let names = target
            .files()
            .iter()
            .map(|file| file.path.to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, ["old-2.parquet", "part-done-00000.parquet"]);
        assert!(!area.join("done.json").exists());
        assert!(!area.join("half.json").exists());
        assert!(!area.join("stale").join("part-stale-00000.parquet").exists());
        let mut all = target.source(&[0, 1]).unwrap();
        let mut every_key = keys(&drain(all.as_mut()));
        every_key.sort_unstable();
        assert_eq!(every_key, (0..9_000).collect::<Vec<_>>());

        // A manifest this Engine cannot read is an error, not a guess.
        std::fs::create_dir_all(&area).unwrap();
        std::fs::write(
            area.join("future.json"),
            b"{\"version\": 99, \"replaced\": [], \"written\": []}",
        )
        .unwrap();
        let error = RewriteTarget::open(table.to_str().unwrap())
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("version 99"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A Hive-partitioned directory on an object store: a predicate on the
    /// partition column selects by the paths, the groups are the
    /// partition directories, and every new file lands under its own.
    #[test]
    fn a_partitioned_directory_is_rewritten_under_its_partition_paths() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for (index, start) in [0_i64, 3_000, 6_000].into_iter().enumerate() {
            runtime
                .block_on(store.put(
                    &Path::from(format!("lake/sales/dt=2026-0{}/old.parquet", index + 1)),
                    parquet(&rows(start, 3_000)).into(),
                ))
                .unwrap();
        }
        let target =
            RewriteTarget::over_store(Arc::clone(&store), Path::from("lake/sales")).unwrap();
        assert_eq!(
            target
                .partition_columns()
                .iter()
                .map(|column| column.name())
                .collect::<Vec<_>>(),
            ["dt"]
        );
        assert_eq!(target.files()[1].directory, ["dt=2026-02"]);
        // The partition column is folded over the paths; a file column
        // meets the footers of the files the paths kept.
        let by_path = StoragePredicate::And(vec![
            StoragePredicate::Compare {
                column: "dt".into(),
                op: CompareOp::Ne,
                value: ScalarValue::Utf8("2026-01".into()),
            },
            StoragePredicate::Compare {
                column: "key".into(),
                op: CompareOp::Lt,
                value: ScalarValue::Int64(6_000),
            },
        ]);
        assert_eq!(target.select(Some(&by_path), None).unwrap(), vec![1]);
        let groups = target.groups(&[0, 2]).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].directory, ["dt=2026-03"]);

        let report = rewrite(&target, &[0, 2]);
        assert_eq!(report.files_replaced, 2);
        assert_eq!(report.rows, 6_000);
        let listed =
            RewriteTarget::over_store(Arc::clone(&store), Path::from("lake/sales")).unwrap();
        let paths = listed
            .files()
            .iter()
            .map(|file| file.path.to_string())
            .collect::<Vec<_>>();
        assert!(
            paths
                .iter()
                .all(|path| path.starts_with("lake/sales/dt=2026-0")),
            "{paths:?}"
        );
        assert!(paths.contains(&"lake/sales/dt=2026-02/old.parquet".to_owned()));
        assert!(!paths.contains(&"lake/sales/dt=2026-01/old.parquet".to_owned()));
        assert!(
            paths
                .iter()
                .filter(|path| path.starts_with("lake/sales/dt=2026-01/part-"))
                .count()
                > 0,
            "{paths:?}"
        );
        let mut all = listed
            .source(&(0..listed.files().len()).collect::<Vec<_>>())
            .unwrap();
        let mut every_key = keys(&drain(all.as_mut()));
        every_key.sort_unstable();
        assert_eq!(every_key, (0..9_000).collect::<Vec<_>>());
    }

    #[test]
    fn an_object_store_directory_is_rewritten_by_upload() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for (index, start) in [0_i64, 3_000].into_iter().enumerate() {
            runtime
                .block_on(store.put(
                    &Path::from(format!("lake/events/old-{index}.parquet")),
                    parquet(&rows(start, 3_000)).into(),
                ))
                .unwrap();
        }
        let target =
            RewriteTarget::over_store(Arc::clone(&store), Path::from("lake/events")).unwrap();
        assert_eq!(*target.kind(), RewriteKind::Directory);
        let report = rewrite(&target, &[0, 1]);
        assert_eq!(report.files_replaced, 2);
        assert_eq!(report.rows, 6_000);
        let listed =
            RewriteTarget::over_store(Arc::clone(&store), Path::from("lake/events")).unwrap();
        assert_eq!(listed.files().len() as u64, report.files_written);
        let mut all = listed
            .source(&(0..listed.files().len()).collect::<Vec<_>>())
            .unwrap();
        let read = keys(&drain(all.as_mut()));
        assert_eq!(read, (0..6_000).collect::<Vec<_>>());
        let objects = runtime
            .block_on(store.list(None).try_collect::<Vec<_>>())
            .unwrap();
        assert!(
            objects
                .iter()
                .all(|object| !object.location.as_ref().contains(REWRITE_AREA)),
            "{objects:?}"
        );

        // A single object: replaced by one PUT under the same name.
        runtime
            .block_on(store.put(
                &Path::from("lake/hits.parquet"),
                parquet(&rows(0, 2_000)).into(),
            ))
            .unwrap();
        let target =
            RewriteTarget::over_store(Arc::clone(&store), Path::from("lake/hits.parquet")).unwrap();
        assert_eq!(*target.kind(), RewriteKind::SingleFile);
        let report = rewrite(&target, &[0]);
        assert_eq!(report.files_written, 1);
        let listed =
            RewriteTarget::over_store(Arc::clone(&store), Path::from("lake/hits.parquet")).unwrap();
        assert_eq!(listed.files()[0].path.as_ref(), "lake/hits.parquet");
        let mut all = listed.source(&[0]).unwrap();
        assert_eq!(keys(&drain(all.as_mut())), (0..2_000).collect::<Vec<_>>());
    }
}
