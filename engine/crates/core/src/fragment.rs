use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    DataFormat, ExchangeId, Expr, KaveonError, PartitionColumn, Partitioning, Result, StageId,
    StoragePredicate,
};

/// The wire format a coordinator sends. Version 6 added the directory
/// listing to a scan (`ScanSpec::listing`); version 5 carried the location
/// alone. A worker accepts both: a version-5 fragment from an older
/// coordinator reads as version 6 with no listing, and its tasks list the
/// location themselves as they always did.
pub const EXECUTABLE_FRAGMENT_VERSION: u16 = 6;
/// The oldest wire format a worker still executes.
pub const OLDEST_EXECUTABLE_FRAGMENT_VERSION: u16 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FragmentNodeId(pub u32);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutableFragment {
    pub version: u16,
    pub stage_id: StageId,
    pub root: FragmentNodeId,
    pub nodes: Vec<FragmentNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FragmentNode {
    pub id: FragmentNodeId,
    pub inputs: Vec<FragmentNodeId>,
    pub operator: FragmentOperator,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FragmentOperator {
    Scan(ScanSpec),
    ExchangeInput(ExchangeInput),
    Filter {
        predicate: Expr,
    },
    Project {
        expressions: Vec<NamedExpr>,
    },
    Aggregate {
        mode: AggregateMode,
        group_by: Vec<NamedExpr>,
        aggregates: Vec<AggregateSpec>,
    },
    Sort {
        keys: Vec<SortSpec>,
    },
    TopN {
        keys: Vec<SortSpec>,
        limit: usize,
    },
    Limit {
        limit: usize,
    },
    Offset {
        offset: usize,
    },
    Distinct,
    Union,
    Window {
        window_exprs: Vec<Expr>,
    },
    Intersect,
    Except,
    ExchangeOutput(ExchangeOutput),
    HashJoin(JoinSpec),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanSpec {
    pub source_uri: String,
    pub format: DataFormat,
    /// Coordinator-selected Delta version shared by every split and retry.
    pub delta_version: Option<u64>,
    pub iceberg_snapshot_id: Option<i64>,
    pub table: ScanTable,
    pub projection: Vec<String>,
    pub predicate: Option<StoragePredicate>,
    /// The coordinator's listing of a directory Parquet table, so every
    /// task reads the files planning read — pruned by partition values and
    /// skipped by the table's statistics — and lists nothing. `None` for a
    /// single object, a Delta or Iceberg table, and every fragment of an
    /// older coordinator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<ScanListing>,
}

/// The listing of a directory table as the coordinator took it for the
/// query, with the files each scan partition reads already decided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanListing {
    /// The whole directory as the coordinator listed it — before pruning
    /// and skipping — for a task that must list the location itself to
    /// check it lists the same files.
    pub source: ListingDigest,
    /// The partition columns the files' paths carry, typed as the
    /// coordinator typed them (inferred, or declared by the table).
    pub partition_columns: Vec<PartitionColumn>,
    /// Files the coordinator's pruning left out by their partition values.
    pub files_pruned_by_partition: u64,
    /// Files the coordinator's statistics proved empty of matches.
    pub files_skipped: u64,
    /// What each scan partition reads; `None` when the listing exceeded
    /// the fragment listing limit (or kept no file) and the tasks list the
    /// location themselves under the digest in `source`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment: Option<ScanAssignment>,
}

/// The coordinator's assignment of a listing's files to the scan
/// partitions of a stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanAssignment {
    /// The first kept file of the listing: its footer is the schema every
    /// task checks its files against, and what a task with nothing to read
    /// presents.
    pub first: ScanFile,
    /// `partitions[p]` is what scan partition `p` reads, in listing order.
    pub partitions: Vec<Vec<ScanFile>>,
}

/// A listing's identity: a digest over every file's path, size and store
/// identity, and how many files it holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListingDigest {
    pub sha256: String,
    pub files: u64,
}

/// One file a scan partition reads of a directory table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanFile {
    /// The path as the listing carries it: relative to the directory for
    /// a local table, store-relative for an object store.
    pub path: String,
    pub size: u64,
    /// The `key=value` values of the path, one per partition column;
    /// `None` is the NULL partition.
    pub partition_values: Vec<Option<String>>,
    /// The row groups this partition reads of a file the coordinator split
    /// across the partitions; `None` reads the file whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_groups: Option<Vec<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanTable {
    pub catalog: String,
    pub schema: String,
    pub table: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedExpr {
    pub name: String,
    pub expression: Expr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateMode {
    Single,
    Partial,
    Final,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateFunction {
    Count,
    Sum,
    Min,
    Max,
    Avg,
    CountDistinct,
    /// `APPROX_COUNT_DISTINCT`: a HyperLogLog sketch of the argument, the
    /// estimate as the result.
    ApproxDistinct,
    /// `APPROX_PERCENTILE`: a KLL sketch of the argument, the values at
    /// the spec's `percentiles` as the result.
    ApproxPercentile,
}

impl AggregateFunction {
    /// Whether the result is an estimate from a sketch.
    pub const fn is_approximate(self) -> bool {
        matches!(self, Self::ApproxDistinct | Self::ApproxPercentile)
    }
}

/// The fractions an `APPROX_PERCENTILE` answers, and whether it answers
/// them as one list (`ARRAY[…]` was written) or as one value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Percentiles {
    pub fractions: Vec<f64>,
    pub list: bool,
}

impl Percentiles {
    /// Every fraction must lie in `[0, 1]`; a list must not be empty.
    pub fn validate(&self) -> crate::Result<()> {
        if self.fractions.is_empty() {
            return Err(crate::KaveonError::Sql(
                "APPROX_PERCENTILE requires at least one percentile".into(),
            ));
        }
        if let Some(fraction) = self
            .fractions
            .iter()
            .find(|fraction| !(0.0..=1.0).contains(*fraction))
        {
            return Err(crate::KaveonError::Sql(format!(
                "APPROX_PERCENTILE percentile {fraction} is not between 0 and 1"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AggregateSpec {
    pub function: AggregateFunction,
    pub argument: Option<Expr>,
    pub output: String,
    /// For `ApproxPercentile`: what it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentiles: Option<Percentiles>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SortSpec {
    pub expression: Expr,
    pub ascending: bool,
    pub nulls_first: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeInput {
    pub exchange_id: ExchangeId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeOutput {
    pub exchange_id: ExchangeId,
    pub partitioning: Partitioning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
    Semi,
    Anti,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JoinSpec {
    pub join_type: JoinType,
    pub left_qualifier: Option<String>,
    pub right_qualifier: Option<String>,
    pub left_keys: Vec<Expr>,
    pub right_keys: Vec<Expr>,
    pub residual: Option<Expr>,
    pub broadcast: bool,
}

impl ExecutableFragment {
    pub fn validate(&self) -> Result<()> {
        if !(OLDEST_EXECUTABLE_FRAGMENT_VERSION..=EXECUTABLE_FRAGMENT_VERSION)
            .contains(&self.version)
        {
            return invalid(format!(
                "unsupported executable fragment version {}",
                self.version
            ));
        }
        if self.nodes.is_empty() {
            return invalid("executable fragment must contain at least one node");
        }
        let mut nodes = BTreeMap::new();
        for node in &self.nodes {
            if nodes.insert(node.id, node).is_some() {
                return invalid(format!("duplicate fragment node ID {}", node.id.0));
            }
            node.validate_shape()?;
        }
        if !nodes.contains_key(&self.root) {
            return invalid(format!("fragment root node {} does not exist", self.root.0));
        }
        for node in &self.nodes {
            for input in &node.inputs {
                if !nodes.contains_key(input) {
                    return invalid(format!(
                        "node {} references unknown input {}",
                        node.id.0, input.0
                    ));
                }
            }
        }
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        visit(self.root, &nodes, &mut visiting, &mut visited)?;
        if visited.len() != nodes.len() {
            return invalid("fragment contains nodes unreachable from its root");
        }
        Ok(())
    }
}

impl FragmentNode {
    fn validate_shape(&self) -> Result<()> {
        let expected = match &self.operator {
            FragmentOperator::Scan(scan) => {
                scan.validate()?;
                0
            }
            FragmentOperator::ExchangeInput(input) => {
                validate_exchange_id(&input.exchange_id)?;
                0
            }
            FragmentOperator::HashJoin(join) => {
                join.validate()?;
                2
            }
            FragmentOperator::ExchangeOutput(output) => {
                validate_exchange_id(&output.exchange_id)?;
                output.partitioning.validate()?;
                1
            }
            FragmentOperator::Filter { .. } => 1,
            FragmentOperator::Project { expressions } => {
                validate_named_expressions(expressions, "project")?;
                1
            }
            FragmentOperator::Aggregate {
                group_by,
                aggregates,
                ..
            } => {
                validate_named_expressions(group_by, "group by")?;
                if aggregates.is_empty() {
                    return invalid("aggregate operator requires at least one aggregate");
                }
                let mut outputs = BTreeSet::new();
                for aggregate in aggregates {
                    validate_name(&aggregate.output, "aggregate output")?;
                    if !outputs.insert(aggregate.output.as_str()) {
                        return invalid(format!("duplicate aggregate output {}", aggregate.output));
                    }
                    if aggregate.function != AggregateFunction::Count
                        && aggregate.argument.is_none()
                    {
                        return invalid(format!("{:?} requires an argument", aggregate.function));
                    }
                }
                1
            }
            FragmentOperator::Sort { keys } => {
                if keys.is_empty() {
                    return invalid("sort requires at least one key");
                }
                1
            }
            FragmentOperator::TopN { keys, limit } => {
                if keys.is_empty() {
                    return invalid("TopN requires at least one key");
                }
                if *limit == 0 {
                    return invalid("TopN limit must be greater than zero");
                }
                1
            }
            FragmentOperator::Limit { limit } => {
                if *limit == 0 {
                    return invalid("limit must be greater than zero");
                }
                1
            }
            FragmentOperator::Offset { .. } => 1,
            FragmentOperator::Distinct => 1,
            FragmentOperator::Union => {
                // A union consumes every input it names; the executor
                // concatenates them in order.
                if self.inputs.len() < 2 {
                    return invalid(format!(
                        "node {} union requires at least two inputs, found {}",
                        self.id.0,
                        self.inputs.len()
                    ));
                }
                self.inputs.len()
            }
            FragmentOperator::Window { .. } => 1,
            FragmentOperator::Intersect => 2,
            FragmentOperator::Except => 2,
        };
        if self.inputs.len() != expected {
            return invalid(format!(
                "node {} requires {expected} inputs, found {}",
                self.id.0,
                self.inputs.len()
            ));
        }
        Ok(())
    }
}

impl ScanSpec {
    fn validate(&self) -> Result<()> {
        validate_name(&self.source_uri, "scan source URI")?;
        if (self.format == DataFormat::Delta) != self.delta_version.is_some() {
            return invalid(
                "Delta scans require a pinned version; other formats cannot set delta_version",
            );
        }
        if (self.format == DataFormat::Iceberg) != self.iceberg_snapshot_id.is_some()
            || self.iceberg_snapshot_id.is_some_and(|id| id < -1)
        {
            return invalid(
                "Iceberg scans require a pinned snapshot ID (-1 for empty); other formats cannot set it",
            );
        }
        validate_name(&self.table.catalog, "scan catalog")?;
        validate_name(&self.table.schema, "scan schema")?;
        validate_name(&self.table.table, "scan table")?;
        let mut columns = BTreeSet::new();
        for column in &self.projection {
            validate_name(column, "projected column")?;
            if !columns.insert(column) {
                return invalid(format!("duplicate projected column {column}"));
            }
        }
        if let Some(listing) = &self.listing {
            if self.format != DataFormat::Parquet {
                return invalid("only a Parquet scan carries a directory listing");
            }
            listing.validate()?;
        }
        Ok(())
    }
}

impl JoinSpec {
    fn validate(&self) -> Result<()> {
        if self.join_type == JoinType::Cross {
            if !self.left_keys.is_empty() || !self.right_keys.is_empty() {
                return invalid("cross join cannot define equality keys");
            }
        } else if self.left_keys.is_empty() || self.left_keys.len() != self.right_keys.len() {
            return invalid("hash join requires equal, non-empty left and right key lists");
        }
        if self.broadcast && matches!(self.join_type, JoinType::Right | JoinType::Full) {
            return invalid("broadcast build is not valid for right or full hash joins");
        }
        Ok(())
    }
}

fn visit(
    id: FragmentNodeId,
    nodes: &BTreeMap<FragmentNodeId, &FragmentNode>,
    visiting: &mut BTreeSet<FragmentNodeId>,
    visited: &mut BTreeSet<FragmentNodeId>,
) -> Result<()> {
    if visited.contains(&id) {
        return Ok(());
    }
    if !visiting.insert(id) {
        return invalid(format!("fragment contains a cycle at node {}", id.0));
    }
    for input in &nodes[&id].inputs {
        visit(*input, nodes, visiting, visited)?;
    }
    visiting.remove(&id);
    visited.insert(id);
    Ok(())
}

fn validate_named_expressions(expressions: &[NamedExpr], context: &str) -> Result<()> {
    let mut names = BTreeSet::new();
    for expression in expressions {
        validate_name(&expression.name, context)?;
        if !names.insert(expression.name.as_str()) {
            return invalid(format!("duplicate {context} output {}", expression.name));
        }
    }
    Ok(())
}

fn validate_exchange_id(id: &ExchangeId) -> Result<()> {
    validate_name(&id.0, "exchange ID")
}

fn validate_name(value: &str, context: &str) -> Result<()> {
    if value.trim().is_empty() {
        return invalid(format!("{context} cannot be empty"));
    }
    Ok(())
}

impl ScanListing {
    fn validate(&self) -> Result<()> {
        validate_name(&self.source.sha256, "listing digest")?;
        let Some(assignment) = &self.assignment else {
            return Ok(());
        };
        if assignment.partitions.is_empty() {
            return invalid("a scan assignment must cover at least one partition");
        }
        self.validate_file(&assignment.first, None)?;
        for (partition, files) in assignment.partitions.iter().enumerate() {
            for file in files {
                self.validate_file(file, Some(partition))?;
            }
        }
        Ok(())
    }

    fn validate_file(&self, file: &ScanFile, partition: Option<usize>) -> Result<()> {
        validate_name(&file.path, "listed file path")?;
        if file.partition_values.len() != self.partition_columns.len() {
            return invalid(format!(
                "listed file '{}' carries {} partition values for {} partition columns",
                file.path,
                file.partition_values.len(),
                self.partition_columns.len()
            ));
        }
        if file.row_groups.as_ref().is_some_and(Vec::is_empty) {
            return invalid(format!(
                "listed file '{}' is split for partition {} with no row groups",
                file.path,
                partition.unwrap_or_default()
            ));
        }
        Ok(())
    }
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(KaveonError::Execution(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(id: u32) -> FragmentNode {
        FragmentNode {
            id: FragmentNodeId(id),
            inputs: vec![],
            operator: FragmentOperator::Scan(ScanSpec {
                source_uri: "file:///events.parquet".into(),
                format: DataFormat::Parquet,
                delta_version: None,
                iceberg_snapshot_id: None,
                table: ScanTable {
                    catalog: "local".into(),
                    schema: "default".into(),
                    table: "events".into(),
                },
                projection: vec!["id".into()],
                predicate: None,
                listing: None,
            }),
        }
    }

    fn valid_fragment() -> ExecutableFragment {
        ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(1),
            root: FragmentNodeId(2),
            nodes: vec![
                scan(1),
                FragmentNode {
                    id: FragmentNodeId(2),
                    inputs: vec![FragmentNodeId(1)],
                    operator: FragmentOperator::Limit { limit: 10 },
                },
            ],
        }
    }

    #[test]
    fn validates_executable_fragment() {
        valid_fragment().validate().unwrap();
    }

    #[test]
    fn rejects_unknown_versions() {
        let mut fragment = valid_fragment();
        fragment.version += 1;
        assert!(
            fragment
                .validate()
                .unwrap_err()
                .to_string()
                .contains("version")
        );
    }

    /// A fragment from a coordinator of the previous version carries no
    /// listing and still executes: the field reads as absent.
    #[test]
    fn reads_the_previous_version_without_a_listing() {
        let mut fragment = valid_fragment();
        fragment.version = OLDEST_EXECUTABLE_FRAGMENT_VERSION;
        let mut json = serde_json::to_value(&fragment).unwrap();
        let scan = &mut json["nodes"][0]["operator"];
        assert!(scan.get("listing").is_none());
        scan.as_object_mut().unwrap().remove("listing");
        let decoded: ExecutableFragment = serde_json::from_value(json).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded, fragment);
        match &decoded.nodes[0].operator {
            FragmentOperator::Scan(scan) => assert!(scan.listing.is_none()),
            other => panic!("{other:?}"),
        }
        let mut older = fragment;
        older.version = OLDEST_EXECUTABLE_FRAGMENT_VERSION - 1;
        assert!(
            older
                .validate()
                .unwrap_err()
                .to_string()
                .contains("version")
        );
    }

    /// A listing travels as it was serialised, and one whose files do not
    /// fit its partition columns is refused.
    #[test]
    fn validates_the_listing() {
        let listing = ScanListing {
            source: ListingDigest {
                sha256: "ab".repeat(32),
                files: 3,
            },
            partition_columns: vec![
                PartitionColumn::new("dt", arrow::datatypes::DataType::Date32).unwrap(),
            ],
            files_pruned_by_partition: 1,
            files_skipped: 0,
            assignment: Some(ScanAssignment {
                first: ScanFile {
                    path: "dt=2026-01-01/a.parquet".into(),
                    size: 10,
                    partition_values: vec![Some("2026-01-01".into())],
                    row_groups: None,
                },
                partitions: vec![
                    vec![ScanFile {
                        path: "dt=2026-01-01/a.parquet".into(),
                        size: 10,
                        partition_values: vec![Some("2026-01-01".into())],
                        row_groups: None,
                    }],
                    vec![ScanFile {
                        path: "dt=2026-01-02/b.parquet".into(),
                        size: 10,
                        partition_values: vec![Some("2026-01-02".into())],
                        row_groups: Some(vec![1, 3]),
                    }],
                ],
            }),
        };
        let mut fragment = valid_fragment();
        let FragmentOperator::Scan(scan) = &mut fragment.nodes[0].operator else {
            unreachable!()
        };
        scan.listing = Some(listing.clone());
        fragment.validate().unwrap();
        let json = serde_json::to_string(&fragment).unwrap();
        let decoded: ExecutableFragment = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, fragment);

        let FragmentOperator::Scan(scan) = &mut fragment.nodes[0].operator else {
            unreachable!()
        };
        let mut wrong = listing.clone();
        wrong.assignment.as_mut().unwrap().partitions[0][0]
            .partition_values
            .clear();
        scan.listing = Some(wrong);
        assert!(
            fragment
                .validate()
                .unwrap_err()
                .to_string()
                .contains("partition values")
        );
        let FragmentOperator::Scan(scan) = &mut fragment.nodes[0].operator else {
            unreachable!()
        };
        let mut empty = listing;
        empty.assignment.as_mut().unwrap().partitions[1][0].row_groups = Some(Vec::new());
        scan.listing = Some(empty);
        assert!(
            fragment
                .validate()
                .unwrap_err()
                .to_string()
                .contains("no row groups")
        );
    }

    #[test]
    fn rejects_cycles() {
        let mut fragment = valid_fragment();
        fragment.nodes[0].inputs = vec![FragmentNodeId(2)];
        fragment.nodes[0].operator = FragmentOperator::Limit { limit: 1 };
        assert!(
            fragment
                .validate()
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
    }

    #[test]
    fn rejects_unreachable_nodes() {
        let mut fragment = valid_fragment();
        fragment.nodes.push(scan(3));
        assert!(
            fragment
                .validate()
                .unwrap_err()
                .to_string()
                .contains("unreachable")
        );
    }

    #[test]
    fn rejects_malformed_join() {
        let join = FragmentNode {
            id: FragmentNodeId(3),
            inputs: vec![FragmentNodeId(1), FragmentNodeId(2)],
            operator: FragmentOperator::HashJoin(JoinSpec {
                left_qualifier: None,
                right_qualifier: None,
                join_type: JoinType::Inner,
                left_keys: vec![],
                right_keys: vec![],
                residual: None,
                broadcast: false,
            }),
        };
        let error = join.validate_shape().unwrap_err();
        assert!(error.to_string().contains("hash join"));
    }
}
