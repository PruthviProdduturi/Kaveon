use ahash::{AHashMap, AHashSet};
use arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, DictionaryArray, Int32DictionaryArray, UInt64Array,
};
#[cfg(test)]
use arrow::array::{Float32Array, Float64Array, UInt32Array};
use arrow::compute::{concat_batches, take};
use arrow::datatypes::{DataType, Field, Int32Type, Schema, SchemaRef, TimeUnit};
use arrow::record_batch::RecordBatch;
use kaveon_core::{
    BatchOperator, Expr, KaveonError, MemoryReservation, OperatorMemoryAccount, Result,
    ScalarValue, StoragePredicate,
};
use std::sync::Arc;

const HASH_ROW_OVERHEAD_BYTES: u64 = 64;
const OUTPUT_INDEX_BYTES_PER_ROW: u64 = 32;
const OUTPUT_INDEX_GROWTH_ROWS: usize = 1_024;
const OUTPUT_BATCH_ROWS: usize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
    Semi,
    Anti,
}

pub struct HashJoin {
    left: Box<dyn BatchOperator>,
    right: Box<dyn BatchOperator>,
    join_type: JoinType,
    keys: Vec<(usize, usize)>,
    residual: Option<Expr>,
    schema: SchemaRef,
    memory: Option<OperatorMemoryAccount>,
    emitted: bool,
    state: Option<JoinState>,
    output_memory: Option<MemoryReservation>,
    /// A right side built by the caller, taken at the first probe.
    prebuilt: Option<BuiltSide>,
}

/// The right side of a hash join, built: the rows as one batch, the
/// index over the keys, the match bitmap an outer join keeps, and the
/// reservations that hold them.
pub(crate) struct BuiltSide {
    right: RecordBatch,
    index: Option<JoinIndex>,
    matched_right: Vec<bool>,
    reservations: Vec<MemoryReservation>,
}

impl BuiltSide {
    pub(crate) fn is_empty(&self) -> bool {
        self.right.num_rows() == 0
    }
}

/// A bounded runtime key filter derived from an in-memory build side. It is
/// intentionally separate from the join index so callers can apply it to
/// probe batches before hash probing without changing outer-join semantics.
pub(crate) struct BuildKeyFilter {
    index: JoinIndex,
    keys: Vec<(usize, usize)>,
    /// Per-column supersets for a composite build key. These let the probe
    /// side reject rows that cannot possibly match before constructing the
    /// complete tuple. The exact tuple index remains the final authority.
    composite_components: Option<Vec<AHashSet<Key>>>,
}

impl BuildKeyFilter {
    pub(crate) fn from_built(built: &BuiltSide, keys: &[(usize, usize)]) -> Option<Self> {
        // Cloning a very large index would defeat the purpose of filtering;
        // the bounded path is for the common small-build/large-probe case.
        (built.right.num_rows() <= 100_000)
            .then(|| {
                built.index.as_ref().map(|index| {
                    let composite_components = match index {
                        JoinIndex::Composite(entries) => {
                            let mut components =
                                (0..keys.len()).map(|_| AHashSet::new()).collect::<Vec<_>>();
                            for tuple in entries.keys() {
                                for (component, value) in tuple.iter().enumerate() {
                                    if let Some(values) = components.get_mut(component) {
                                        values.insert(value.clone());
                                    }
                                }
                            }
                            Some(components)
                        }
                        _ => None,
                    };
                    Self {
                        index: index.clone(),
                        keys: keys.to_vec(),
                        composite_components,
                    }
                })
            })
            .flatten()
    }

    pub(crate) fn matches(&self, batch: &RecordBatch, row: usize) -> Result<bool> {
        Ok(self.index.matches(batch, row, &self.keys)?.is_some())
    }

    /// Build one probe mask for a batch. Single INT64 and UTF-8 keys are
    /// common in fact/dimension joins and can use the already-built hash maps
    /// directly; composite and other keys retain the exact row-key path.
    pub(crate) fn filter_batch(&self, batch: &RecordBatch) -> Result<BooleanArray> {
        if let (
            JoinIndex::Int32 {
                first_rows,
                duplicate_rows,
            },
            [(left, _)],
        ) = (&self.index, self.keys.as_slice())
        {
            let values = batch
                .column(*left)
                .as_any()
                .downcast_ref::<arrow::array::Int32Array>()
                .ok_or_else(|| exec_err("runtime filter key type changed after planning"))?;
            return Ok((0..batch.num_rows())
                .map(|row| {
                    values.is_valid(row)
                        && (first_rows.contains_key(&values.value(row))
                            || duplicate_rows.contains_key(&values.value(row)))
                })
                .collect::<Vec<_>>()
                .into());
        }
        if let (
            JoinIndex::Bool {
                first_rows,
                duplicate_rows,
            },
            [(left, _)],
        ) = (&self.index, self.keys.as_slice())
        {
            let values = batch
                .column(*left)
                .as_any()
                .downcast_ref::<BooleanArray>()
                .ok_or_else(|| exec_err("runtime filter key type changed after planning"))?;
            return Ok((0..batch.num_rows())
                .map(|row| {
                    values.is_valid(row)
                        && (first_rows.contains_key(&values.value(row))
                            || duplicate_rows.contains_key(&values.value(row)))
                })
                .collect::<Vec<_>>()
                .into());
        }
        if let (
            JoinIndex::UInt64 {
                first_rows,
                duplicate_rows,
            },
            [(left, _)],
        ) = (&self.index, self.keys.as_slice())
        {
            let values = batch
                .column(*left)
                .as_any()
                .downcast_ref::<arrow::array::UInt64Array>()
                .ok_or_else(|| exec_err("runtime filter key type changed after planning"))?;
            return Ok((0..batch.num_rows())
                .map(|row| {
                    values.is_valid(row)
                        && (first_rows.contains_key(&values.value(row))
                            || duplicate_rows.contains_key(&values.value(row)))
                })
                .collect::<Vec<_>>()
                .into());
        }
        if let (
            JoinIndex::Decimal256 {
                first_rows,
                duplicate_rows,
                ..
            },
            [(left, _)],
        ) = (&self.index, self.keys.as_slice())
        {
            let values = batch
                .column(*left)
                .as_primitive::<arrow::datatypes::Decimal256Type>();
            return Ok((0..batch.num_rows())
                .map(|row| {
                    values.is_valid(row)
                        && (first_rows.contains_key(&values.value(row))
                            || duplicate_rows.contains_key(&values.value(row)))
                })
                .collect::<Vec<_>>()
                .into());
        }
        if let (
            JoinIndex::Date32 {
                first_rows,
                duplicate_rows,
            },
            [(left, _)],
        ) = (&self.index, self.keys.as_slice())
        {
            let values = batch
                .column(*left)
                .as_any()
                .downcast_ref::<arrow::array::Date32Array>()
                .ok_or_else(|| exec_err("runtime filter key type changed after planning"))?;
            return Ok((0..batch.num_rows())
                .map(|row| {
                    values.is_valid(row)
                        && (first_rows.contains_key(&values.value(row))
                            || duplicate_rows.contains_key(&values.value(row)))
                })
                .collect::<Vec<_>>()
                .into());
        }
        if let (
            JoinIndex::Int64 {
                first_rows,
                duplicate_rows,
            },
            [(left, _)],
        ) = (&self.index, self.keys.as_slice())
        {
            let values = batch
                .column(*left)
                .as_any()
                .downcast_ref::<arrow::array::Int64Array>()
                .ok_or_else(|| exec_err("runtime filter key type changed after planning"))?;
            return Ok((0..batch.num_rows())
                .map(|row| {
                    values.is_valid(row)
                        && (first_rows.contains_key(&values.value(row))
                            || duplicate_rows.contains_key(&values.value(row)))
                })
                .collect::<Vec<_>>()
                .into());
        }
        if let (
            JoinIndex::Utf8 {
                first_rows,
                duplicate_rows,
            },
            [(left, _)],
        ) = (&self.index, self.keys.as_slice())
        {
            let column = batch.column(*left);
            return match column.data_type() {
                arrow::datatypes::DataType::Utf8 => {
                    let values = column.as_string::<i32>();
                    Ok((0..batch.num_rows())
                        .map(|row| {
                            values.is_valid(row)
                                && (first_rows.contains_key(values.value(row))
                                    || duplicate_rows.contains_key(values.value(row)))
                        })
                        .collect::<Vec<_>>()
                        .into())
                }
                arrow::datatypes::DataType::LargeUtf8 => {
                    let values = column.as_string::<i64>();
                    Ok((0..batch.num_rows())
                        .map(|row| {
                            values.is_valid(row)
                                && (first_rows.contains_key(values.value(row))
                                    || duplicate_rows.contains_key(values.value(row)))
                        })
                        .collect::<Vec<_>>()
                        .into())
                }
                _ => Err(exec_err("runtime filter key type changed after planning")),
            };
        }
        if let (JoinIndex::Composite(_), Some(component_sets)) =
            (&self.index, &self.composite_components)
        {
            return (0..batch.num_rows())
                .map(|row| {
                    for (component, key_pair) in self.keys.iter().enumerate() {
                        let Some(key) = row_key(batch, row, std::slice::from_ref(key_pair), true)?
                        else {
                            return Ok(false);
                        };
                        if !component_sets[component].contains(&key[0]) {
                            return Ok(false);
                        }
                    }
                    // Component sets are only a safe superset filter. Keep
                    // the tuple lookup to reject cross-product combinations.
                    self.matches(batch, row)
                })
                .collect::<Result<Vec<_>>>()
                .map(Into::into);
        }
        (0..batch.num_rows())
            .map(|row| self.matches(batch, row))
            .collect::<Result<Vec<_>>>()
            .map(Into::into)
    }
}

impl BuildKeyFilter {
    /// Convert a bounded single-key build index into an exact storage `IN`
    /// predicate. Composite keys and unsigned values that cannot be represented
    /// by the storage literal type stay on the batch-level path.
    pub(crate) fn storage_predicate(&self, schema: &SchemaRef) -> Option<StoragePredicate> {
        let column = self
            .keys
            .first()
            .map(|(left, _)| schema.field(*left).name().to_owned());
        let values = match &self.index {
            JoinIndex::Bool { first_rows, .. } => Some(
                first_rows
                    .keys()
                    .map(|value| ScalarValue::Bool(*value))
                    .collect(),
            ),
            JoinIndex::Int32 { first_rows, .. } => Some(
                first_rows
                    .keys()
                    .map(|value| ScalarValue::Int64(i64::from(*value)))
                    .collect(),
            ),
            JoinIndex::Date32 { first_rows, .. } => Some(
                first_rows
                    .keys()
                    .map(|value| ScalarValue::Int64(i64::from(*value)))
                    .collect(),
            ),
            JoinIndex::Int64 { first_rows, .. } => Some(
                first_rows
                    .keys()
                    .map(|value| ScalarValue::Int64(*value))
                    .collect(),
            ),
            JoinIndex::Utf8 { first_rows, .. } => Some(
                first_rows
                    .keys()
                    .map(|value| ScalarValue::Utf8(value.clone()))
                    .collect(),
            ),
            JoinIndex::UInt64 { first_rows, .. } => first_rows
                .keys()
                .map(|value| i64::try_from(*value).ok().map(ScalarValue::Int64))
                .collect::<Option<Vec<_>>>(),
            JoinIndex::Decimal256 {
                first_rows, scale, ..
            } => Some(
                first_rows
                    .keys()
                    .map(|value| ScalarValue::Decimal256 {
                        value: value.to_string(),
                        precision: 76,
                        scale: *scale,
                    })
                    .collect(),
            ),
            JoinIndex::Composite(entries) => {
                let mut predicates = Vec::new();
                for (key_index, (left, _)) in self.keys.iter().enumerate() {
                    let mut seen = AHashSet::new();
                    let mut values = Vec::new();
                    for tuple in entries.keys() {
                        let Some(key) = tuple.get(key_index) else {
                            continue;
                        };
                        if !seen.insert(key.clone()) {
                            continue;
                        }
                        let value = match key {
                            Key::Bool(value) => ScalarValue::Bool(*value),
                            Key::Int32(value) => ScalarValue::Int64(i64::from(*value)),
                            Key::Int64(value) => ScalarValue::Int64(*value),
                            Key::UInt64(value) => ScalarValue::Int64(i64::try_from(*value).ok()?),
                            Key::Decimal(value, scale) => ScalarValue::Decimal128 {
                                value: *value,
                                precision: 38,
                                scale: *scale,
                            },
                            Key::Float32(bits) => {
                                let value = f32::from_bits(*bits);
                                if !value.is_finite() {
                                    continue;
                                }
                                ScalarValue::Float64(f64::from(value))
                            }
                            Key::Float64(bits) => {
                                let value = f64::from_bits(*bits);
                                if !value.is_finite() {
                                    continue;
                                }
                                ScalarValue::Float64(value)
                            }
                            Key::Utf8(value) => ScalarValue::Utf8(value.clone()),
                            Key::Decimal256(value, scale) => ScalarValue::Decimal256 {
                                value: value.to_string(),
                                precision: 76,
                                scale: *scale,
                            },
                        };
                        values.push(value);
                    }
                    if !values.is_empty() {
                        predicates.push(StoragePredicate::In {
                            column: schema.field(*left).name().to_owned(),
                            values,
                        });
                    }
                }
                return match predicates.len() {
                    0 => None,
                    1 => predicates.into_iter().next(),
                    _ => Some(StoragePredicate::And(predicates)),
                };
            }
        }?;
        let column = column?;
        (!values.is_empty()).then_some(StoragePredicate::In { column, values })
    }
}

impl BuiltSide {
    /// Build from batches the caller holds (and has accounted for): the
    /// concatenated copy, then the index, then the bitmap, each reserved
    /// before it is made. On a refusal nothing is retained, and the caller
    /// still has its batches.
    pub(crate) fn build(
        schema: &SchemaRef,
        batches: &[RecordBatch],
        keys: &[(usize, usize)],
        join_type: JoinType,
        memory: Option<&OperatorMemoryAccount>,
    ) -> Result<Self> {
        let mut reservations = Vec::new();
        // The built side holds its rows on its own reservation, so the
        // caller's per-batch guards can go once it exists.
        let total = batches.iter().try_fold(0_u64, |total, batch| {
            total
                .checked_add(batch.get_array_memory_size() as u64)
                .ok_or_else(|| exec_err("join input memory estimate overflow"))
        })?;
        reservations.extend(reserve_bytes(memory, total)?);
        let right = match batches {
            [] => RecordBatch::new_empty(Arc::clone(schema)),
            [batch] => batch.clone(),
            batches => concat_batches(schema, batches)?,
        };
        reservations.extend(reserve_join_index(memory, &right, keys)?);
        let index = if join_type == JoinType::Cross || keys.is_empty() {
            None
        } else {
            Some(JoinIndex::build(&right, keys, memory)?)
        };
        let needs_matches = matches!(join_type, JoinType::Right | JoinType::Full);
        reservations.extend(reserve_bytes(
            memory,
            if needs_matches {
                right.num_rows() as u64
            } else {
                0
            },
        )?);
        let matched_right = if needs_matches {
            vec![false; right.num_rows()]
        } else {
            Vec::new()
        };
        Ok(Self {
            right,
            index,
            matched_right,
            reservations,
        })
    }
}

/// The key columns of a join, resolved on each side after the join's
/// ambiguity checks, with their types checked equal.
pub(crate) fn resolve_join_keys(
    left_schema: &SchemaRef,
    right_schema: &SchemaRef,
    keys: &[(String, String)],
) -> Result<Vec<(usize, usize)>> {
    keys.iter()
        .map(|(left_name, right_name)| {
            let left_index = resolve_column(left_schema, left_name, "left")?;
            let right_index = resolve_column(right_schema, right_name, "right")?;
            let left_type = left_schema.field(left_index).data_type();
            let right_type = right_schema.field(right_index).data_type();
            if left_type != right_type {
                return Err(exec_err(format!(
                    "join key types differ: {left_name} is {left_type}, {right_name} is {right_type}"
                )));
            }
            Ok((left_index, right_index))
        })
        .collect()
}

struct JoinState {
    right: RecordBatch,
    index: Option<JoinIndex>,
    matched_right: Vec<bool>,
    left: Option<RecordBatch>,
    left_row: usize,
    match_offset: usize,
    match_rows: Vec<usize>,
    unmatched_right_row: usize,
    probe_finished: bool,
    _build_memory: Vec<MemoryReservation>,
    left_memory: Option<MemoryReservation>,
}

impl HashJoin {
    pub fn try_new(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        join_type: JoinType,
        keys: Vec<(String, String)>,
    ) -> Result<Self> {
        Self::try_new_qualified(left, right, join_type, keys, None, None)
    }

    pub fn try_new_qualified(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        join_type: JoinType,
        keys: Vec<(String, String)>,
        left_qualifier: Option<&str>,
        right_qualifier: Option<&str>,
    ) -> Result<Self> {
        Self::try_new_qualified_with_residual(
            left,
            right,
            join_type,
            keys,
            left_qualifier,
            right_qualifier,
            None,
        )
    }

    pub fn try_new_qualified_with_residual(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        join_type: JoinType,
        keys: Vec<(String, String)>,
        left_qualifier: Option<&str>,
        right_qualifier: Option<&str>,
        residual: Option<Expr>,
    ) -> Result<Self> {
        if join_type != JoinType::Cross && keys.is_empty() && residual.is_none() {
            return Err(exec_err("hash joins require at least one equality key"));
        }
        let left_schema = Arc::clone(left.schema());
        let right_schema = Arc::clone(right.schema());
        let key_indices = resolve_join_keys(&left_schema, &right_schema, &keys)?;
        let fields = if matches!(join_type, JoinType::Semi | JoinType::Anti) {
            left_schema
                .fields()
                .iter()
                .map(|field| qualified_field(field, left_qualifier))
                .collect::<Vec<_>>()
        } else {
            left_schema
                .fields()
                .iter()
                .map(|field| qualified_field(field, left_qualifier))
                .chain(
                    right_schema
                        .fields()
                        .iter()
                        .map(|field| qualified_field(field, right_qualifier)),
                )
                .collect::<Vec<_>>()
        };
        Ok(Self {
            left,
            right,
            join_type,
            keys: key_indices,
            residual,
            schema: Arc::new(Schema::new(fields)),
            memory: None,
            emitted: false,
            state: None,
            output_memory: None,
            prebuilt: None,
        })
    }

    /// Attach a residual predicate to an equality hash join. The predicate is
    /// evaluated over candidate pairs after key matching and before outer-join
    /// null extension, so an outer row is retained only when no candidate
    /// satisfies the complete SQL `ON` condition.
    pub fn with_residual(mut self, residual: Option<Expr>) -> Self {
        self.residual = residual;
        self
    }

    /// A join whose right side the caller built (`BuiltSide::build`): the
    /// right source is never read.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_new_built_with_residual(
        left: Box<dyn BatchOperator>,
        right_schema: SchemaRef,
        join_type: JoinType,
        keys: Vec<(String, String)>,
        left_qualifier: Option<&str>,
        right_qualifier: Option<&str>,
        memory: Option<OperatorMemoryAccount>,
        residual: Option<Expr>,
        built: BuiltSide,
    ) -> Result<Self> {
        let mut operator = Self::try_new_qualified_with_residual(
            left,
            Box::new(crate::local_parallel::EmptyInput(right_schema)),
            join_type,
            keys,
            left_qualifier,
            right_qualifier,
            residual,
        )?;
        operator.memory = memory;
        operator.prebuilt = Some(built);
        Ok(operator)
    }

    pub fn try_new_qualified_with_memory(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        join_type: JoinType,
        keys: Vec<(String, String)>,
        left_qualifier: Option<&str>,
        right_qualifier: Option<&str>,
        memory: OperatorMemoryAccount,
    ) -> Result<Self> {
        let mut operator = Self::try_new_qualified(
            left,
            right,
            join_type,
            keys,
            left_qualifier,
            right_qualifier,
        )?;
        operator.memory = Some(memory);
        Ok(operator)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn try_new_qualified_with_memory_and_residual(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        join_type: JoinType,
        keys: Vec<(String, String)>,
        left_qualifier: Option<&str>,
        right_qualifier: Option<&str>,
        memory: OperatorMemoryAccount,
        residual: Option<Expr>,
    ) -> Result<Self> {
        let mut operator = Self::try_new_qualified_with_residual(
            left,
            right,
            join_type,
            keys,
            left_qualifier,
            right_qualifier,
            residual,
        )?;
        operator.memory = Some(memory);
        Ok(operator)
    }
}

fn qualified_field(field: &Field, qualifier: Option<&str>) -> Field {
    let name = qualifier
        .map(|qualifier| format!("{qualifier}.{}", field.name()))
        .unwrap_or_else(|| field.name().clone());
    Field::new(name, field.data_type().clone(), true)
}

fn resolve_column(schema: &SchemaRef, name: &str, side: &str) -> Result<usize> {
    if let Ok(index) = schema.index_of(name) {
        return Ok(index);
    }
    let unqualified = name.rsplit('.').next().unwrap_or(name);
    let matches = schema
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, field)| {
            field.name() == unqualified
                || field
                    .name()
                    .strip_suffix(unqualified)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(exec_err(format!("{side} join column '{name}' not found"))),
        _ => Err(exec_err(format!(
            "{side} join column '{name}' is ambiguous"
        ))),
    }
}

/// Evaluate a residual `ON` predicate over equality-key candidates. SQL join
/// predicates keep only TRUE; FALSE and UNKNOWN both mean that the pair did
/// not match. Candidates are evaluated in bounded chunks so a skewed key
/// cannot allocate an unbounded temporary joined batch.
fn residual_matches(
    residual: Option<&Expr>,
    left: &RecordBatch,
    right: &RecordBatch,
    left_row: usize,
    candidates: &[usize],
    output_schema: &SchemaRef,
) -> Result<Vec<usize>> {
    let Some(residual) = residual else {
        return Ok(candidates.to_vec());
    };
    let mut accepted = Vec::new();
    for chunk in candidates.chunks(OUTPUT_BATCH_ROWS) {
        let left_indices = UInt64Array::from(vec![left_row as u64; chunk.len()]);
        let right_indices =
            UInt64Array::from(chunk.iter().map(|row| *row as u64).collect::<Vec<_>>());
        let columns = left
            .columns()
            .iter()
            .map(|column| take(column, &left_indices, None))
            .chain(
                right
                    .columns()
                    .iter()
                    .map(|column| take(column, &right_indices, None)),
            )
            .collect::<std::result::Result<Vec<ArrayRef>, _>>()?;
        let candidate_batch = RecordBatch::try_new(output_schema.clone(), columns)?;
        let predicate = crate::expr_eval::evaluate(residual, &candidate_batch)?;
        let predicate = predicate
            .as_any()
            .downcast_ref::<BooleanArray>()
            .ok_or_else(|| exec_err("join residual predicate must evaluate to BOOLEAN"))?;
        accepted.extend(chunk.iter().enumerate().filter_map(|(index, right_row)| {
            (predicate.is_valid(index) && predicate.value(index)).then_some(*right_row)
        }));
    }
    Ok(accepted)
}

impl BatchOperator for HashJoin {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        self.output_memory = None;
        if self.emitted {
            return Ok(None);
        }
        let result = (|| {
            let residual = self.residual.clone();
            if self.state.is_none() {
                let built = match self.prebuilt.take() {
                    Some(built) => built,
                    None => {
                        let (batches, batch_memory) =
                            collect_input(&mut self.right, self.memory.as_ref())?;
                        let built = BuiltSide::build(
                            self.right.schema(),
                            &batches,
                            &self.keys,
                            self.join_type,
                            self.memory.as_ref(),
                        )?;
                        drop(batch_memory);
                        built
                    }
                };
                self.state = Some(JoinState {
                    right: built.right,
                    index: built.index,
                    matched_right: built.matched_right,
                    left: None,
                    left_row: 0,
                    match_offset: 0,
                    match_rows: Vec::new(),
                    unmatched_right_row: 0,
                    probe_finished: false,
                    _build_memory: built.reservations,
                    left_memory: None,
                });
            }
            let state = self.state.as_mut().expect("join initialized");
            loop {
                check_cancelled(self.memory.as_ref())?;
                if state
                    .left
                    .as_ref()
                    .is_some_and(|batch| state.left_row >= batch.num_rows())
                {
                    state.left = None;
                    state.left_memory = None;
                }
                if state.left.is_none() && !state.probe_finished {
                    match self.left.next_batch()? {
                        Some(batch) => {
                            if batch.schema() != *self.left.schema() {
                                return Err(exec_err("join probe schema changed"));
                            }
                            state.left_memory = reserve_bytes(
                                self.memory.as_ref(),
                                batch.get_array_memory_size() as u64,
                            )?;
                            state.left = Some(batch);
                            state.left_row = 0;
                            state.match_offset = 0;
                            state.match_rows.clear();
                        }
                        None => state.probe_finished = true,
                    }
                }
                let empty_left = RecordBatch::new_empty(self.left.schema().clone());
                let left = state.left.as_ref().unwrap_or(&empty_left);
                let mut left_rows = Vec::new();
                let mut right_rows = Vec::new();
                let mut reservations = Vec::new();
                while state.left_row < left.num_rows() && left_rows.len() < OUTPUT_BATCH_ROWS {
                    if state.left_row.is_multiple_of(1024) {
                        check_cancelled(self.memory.as_ref())?;
                    }
                    if state.match_rows.is_empty() && state.match_offset == 0 {
                        let candidates =
                            if self.join_type == JoinType::Cross || state.index.is_none() {
                                (0..state.right.num_rows()).collect::<Vec<_>>()
                            } else {
                                state
                                    .index
                                    .as_ref()
                                    .map(|index| index.matches(left, state.left_row, &self.keys))
                                    .transpose()?
                                    .flatten()
                                    .unwrap_or_default()
                                    .to_vec()
                            };
                        state.match_rows = residual_matches(
                            residual.as_ref(),
                            left,
                            &state.right,
                            state.left_row,
                            &candidates,
                            &self.schema,
                        )?;
                    }
                    let match_count = state.match_rows.len();
                    if matches!(self.join_type, JoinType::Semi | JoinType::Anti) {
                        let keep = (self.join_type == JoinType::Semi && match_count > 0)
                            || (self.join_type == JoinType::Anti && match_count == 0);
                        if keep {
                            left_rows.push(Some(state.left_row as u64));
                        }
                        state.left_row += 1;
                        state.match_offset = 0;
                        state.match_rows.clear();
                        continue;
                    }
                    if match_count == 0 {
                        if matches!(self.join_type, JoinType::Left | JoinType::Full) {
                            push_output_pair(
                                &mut left_rows,
                                &mut right_rows,
                                Some(state.left_row as u64),
                                None,
                                self.memory.as_ref(),
                                &mut reservations,
                            )?;
                        }
                        state.left_row += 1;
                        state.match_offset = 0;
                        state.match_rows.clear();
                        continue;
                    }
                    while state.match_offset < match_count && left_rows.len() < OUTPUT_BATCH_ROWS {
                        let right_row = state.match_rows[state.match_offset];
                        push_output_pair(
                            &mut left_rows,
                            &mut right_rows,
                            Some(state.left_row as u64),
                            Some(right_row as u64),
                            self.memory.as_ref(),
                            &mut reservations,
                        )?;
                        if !state.matched_right.is_empty() {
                            state.matched_right[right_row] = true;
                        }
                        state.match_offset += 1;
                    }
                    if state.match_offset == match_count {
                        state.left_row += 1;
                        state.match_offset = 0;
                        state.match_rows.clear();
                    }
                }
                if state.probe_finished
                    && matches!(self.join_type, JoinType::Right | JoinType::Full)
                {
                    while state.unmatched_right_row < state.right.num_rows()
                        && left_rows.len() < OUTPUT_BATCH_ROWS
                    {
                        let row = state.unmatched_right_row;
                        if row.is_multiple_of(1024) {
                            check_cancelled(self.memory.as_ref())?;
                        }
                        state.unmatched_right_row += 1;
                        if !state.matched_right[row] {
                            push_output_pair(
                                &mut left_rows,
                                &mut right_rows,
                                None,
                                Some(row as u64),
                                self.memory.as_ref(),
                                &mut reservations,
                            )?;
                        }
                    }
                }
                if left_rows.is_empty() {
                    if state.probe_finished {
                        return Ok(None);
                    }
                    continue;
                }
                let bytes = estimated_output_bytes(left, &left_rows)?;
                let bytes = if matches!(self.join_type, JoinType::Semi | JoinType::Anti) {
                    bytes
                } else {
                    bytes
                        .checked_add(estimated_output_bytes(&state.right, &right_rows)?)
                        .ok_or_else(|| exec_err("join output memory estimate overflow"))?
                };
                self.output_memory = reserve_bytes(self.memory.as_ref(), bytes)?;
                let left_indices = UInt64Array::from(left_rows);
                let columns = if matches!(self.join_type, JoinType::Semi | JoinType::Anti) {
                    left.columns()
                        .iter()
                        .map(|column| take(column, &left_indices, None))
                        .collect::<std::result::Result<Vec<ArrayRef>, _>>()?
                } else {
                    let right_indices = UInt64Array::from(right_rows);
                    left.columns()
                        .iter()
                        .map(|column| take(column, &left_indices, None))
                        .chain(
                            state
                                .right
                                .columns()
                                .iter()
                                .map(|column| take(column, &right_indices, None)),
                        )
                        .collect::<std::result::Result<Vec<ArrayRef>, _>>()?
                };
                return Ok(Some(RecordBatch::try_new(self.schema.clone(), columns)?));
            }
        })();
        if !matches!(result, Ok(Some(_))) {
            self.emitted = true;
            self.state = None;
            self.output_memory = None;
        }
        result
    }
}

/// Every batch of the build side, each reserved as it arrives.
fn collect_input(
    source: &mut Box<dyn BatchOperator>,
    memory: Option<&OperatorMemoryAccount>,
) -> Result<(Vec<RecordBatch>, Vec<MemoryReservation>)> {
    let mut batches = Vec::new();
    let mut batch_reservations = Vec::new();
    while let Some(batch) = source.next_batch()? {
        check_cancelled(memory)?;
        let bytes = u64::try_from(batch.get_array_memory_size())
            .map_err(|_| exec_err("join input batch memory size exceeds u64"))?;
        if let Some(reservation) = reserve_bytes(memory, bytes)? {
            batch_reservations.push(reservation);
        }
        batches.push(batch);
    }
    Ok((batches, batch_reservations))
}

pub(crate) fn estimated_output_bytes(batch: &RecordBatch, indices: &[Option<u64>]) -> Result<u64> {
    batch.columns().iter().try_fold(0_u64, |total, column| {
        // Fixed-width columns need an arithmetic bound, not a pass over every
        // output row. Keep the same conservative validity/alignment allowance.
        let width: Option<u64> = match column.data_type() {
            DataType::Boolean | DataType::Int8 | DataType::UInt8 => Some(2),
            DataType::Int16 | DataType::UInt16 | DataType::Float16 => Some(3),
            DataType::Int32
            | DataType::UInt32
            | DataType::Float32
            | DataType::Date32
            | DataType::Time32(_) => Some(5),
            DataType::Int64
            | DataType::UInt64
            | DataType::Float64
            | DataType::Date64
            | DataType::Time64(_)
            | DataType::Timestamp(_, _)
            | DataType::Duration(_) => Some(9),
            DataType::Decimal128(_, _) => Some(17),
            DataType::Decimal256(_, _) => Some(33),
            DataType::Null => Some(0),
            _ => None,
        };
        if let Some(width) = width {
            return width
                .checked_mul(indices.len() as u64)
                .and_then(|bytes| bytes.checked_add(256))
                .and_then(|bytes| total.checked_add(bytes))
                .ok_or_else(|| exec_err("join output memory estimate overflow"));
        }
        // `take` over a dictionary column takes its keys and shares its
        // values: the output holds a key per row and the values once.
        // (A one-row slice reports the whole values buffer, which
        // charged every row for the dictionary.)
        if let DataType::Dictionary(key_type, _) = column.data_type() {
            let key_width = key_type.primitive_width().unwrap_or(8) as u64 + 1;
            let values = column.as_any_dictionary().values().get_array_memory_size() as u64;
            return key_width
                .checked_mul(indices.len() as u64)
                .and_then(|bytes| bytes.checked_add(values))
                .and_then(|bytes| bytes.checked_add(256))
                .and_then(|bytes| total.checked_add(bytes))
                .ok_or_else(|| exec_err("join output memory estimate overflow"));
        }
        let mut bytes = 256_u64;
        for index in indices {
            let row_bytes = match column.data_type() {
                DataType::Boolean | DataType::Int8 | DataType::UInt8 => 2,
                DataType::Int16 | DataType::UInt16 | DataType::Float16 => 3,
                DataType::Int32
                | DataType::UInt32
                | DataType::Float32
                | DataType::Date32
                | DataType::Time32(_) => 5,
                DataType::Int64
                | DataType::UInt64
                | DataType::Float64
                | DataType::Date64
                | DataType::Time64(_)
                | DataType::Timestamp(_, _)
                | DataType::Duration(_) => 9,
                DataType::Decimal128(_, _) => 17,
                DataType::Decimal256(_, _) => 33,
                DataType::Null => 0,
                DataType::Utf8 => {
                    8 + index.map_or(0, |index| {
                        column.as_string::<i32>().value(index as usize).len() as u64
                    })
                }
                DataType::LargeUtf8 => {
                    16 + index.map_or(0, |index| {
                        column.as_string::<i64>().value(index as usize).len() as u64
                    })
                }
                // Taking one row gives Arrow's own estimate for nested/variable
                // types without allocating a duplicate output array.
                _ => index.map_or(8, |index| {
                    column.slice(index as usize, 1).get_buffer_memory_size() as u64 + 1
                }),
            };
            bytes = bytes
                .checked_add(row_bytes)
                .ok_or_else(|| exec_err("join output memory estimate overflow"))?;
        }
        total
            .checked_add(bytes)
            .ok_or_else(|| exec_err("join output memory estimate overflow"))
    })
}

fn reserve_join_index(
    memory: Option<&OperatorMemoryAccount>,
    right: &RecordBatch,
    keys: &[(usize, usize)],
) -> Result<Option<MemoryReservation>> {
    let key_bytes = keys.iter().try_fold(0_u64, |total, (_, right_index)| {
        let bytes = u64::try_from(right.column(*right_index).get_array_memory_size())
            .map_err(|_| exec_err("join key memory size exceeds u64"))?;
        total
            .checked_add(bytes)
            .ok_or_else(|| exec_err("join key memory estimate overflow"))
    })?;
    let row_overhead = u64::try_from(right.num_rows())
        .map_err(|_| exec_err("join build row count exceeds u64"))?
        .checked_mul(HASH_ROW_OVERHEAD_BYTES)
        .ok_or_else(|| exec_err("join index memory estimate overflow"))?;
    reserve_bytes(memory, key_bytes.saturating_add(row_overhead))
}

fn reserve_bytes(
    memory: Option<&OperatorMemoryAccount>,
    bytes: u64,
) -> Result<Option<MemoryReservation>> {
    memory.map(|account| account.reserve(bytes)).transpose()
}

fn push_output_pair(
    left_rows: &mut Vec<Option<u64>>,
    right_rows: &mut Vec<Option<u64>>,
    left: Option<u64>,
    right: Option<u64>,
    memory: Option<&OperatorMemoryAccount>,
    reservations: &mut Vec<MemoryReservation>,
) -> Result<()> {
    if left_rows.len().is_multiple_of(1024) {
        check_cancelled(memory)?;
    }
    if left_rows.len() == left_rows.capacity() {
        let growth = left_rows.capacity().max(OUTPUT_INDEX_GROWTH_ROWS);
        let bytes = u64::try_from(growth)
            .map_err(|_| exec_err("join output capacity exceeds u64"))?
            .checked_mul(OUTPUT_INDEX_BYTES_PER_ROW)
            .ok_or_else(|| exec_err("join output memory estimate overflow"))?;
        if let Some(reservation) = reserve_bytes(memory, bytes)? {
            reservations.push(reservation);
        }
        left_rows.reserve_exact(growth);
        right_rows.reserve_exact(growth);
    }
    left_rows.push(left);
    right_rows.push(right);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Bool(bool),
    Int32(i32),
    Int64(i64),
    UInt64(u64),
    Decimal(i128, i8),
    Decimal256(arrow::datatypes::i256, i8),
    Float32(u32),
    Float64(u64),
    Utf8(String),
}

/// Common integer joins avoid allocating a Vec<Key> for every probe row.
/// Other key layouts use the existing exact typed composite representation.
#[derive(Clone)]
enum JoinIndex {
    Bool {
        first_rows: AHashMap<bool, usize>,
        duplicate_rows: AHashMap<bool, Vec<usize>>,
    },
    Int32 {
        first_rows: AHashMap<i32, usize>,
        duplicate_rows: AHashMap<i32, Vec<usize>>,
    },
    UInt64 {
        first_rows: AHashMap<u64, usize>,
        duplicate_rows: AHashMap<u64, Vec<usize>>,
    },
    Decimal256 {
        first_rows: AHashMap<arrow::datatypes::i256, usize>,
        duplicate_rows: AHashMap<arrow::datatypes::i256, Vec<usize>>,
        scale: i8,
    },
    Date32 {
        first_rows: AHashMap<i32, usize>,
        duplicate_rows: AHashMap<i32, Vec<usize>>,
    },
    Int64 {
        first_rows: AHashMap<i64, usize>,
        duplicate_rows: AHashMap<i64, Vec<usize>>,
    },
    Utf8 {
        first_rows: AHashMap<String, usize>,
        duplicate_rows: AHashMap<String, Vec<usize>>,
    },
    Composite(AHashMap<Vec<Key>, Vec<usize>>),
}

impl JoinIndex {
    fn build(
        right: &RecordBatch,
        keys: &[(usize, usize)],
        memory: Option<&OperatorMemoryAccount>,
    ) -> Result<Self> {
        if let [(.., index)] = keys
            && right.column(*index).data_type() == &DataType::Boolean
        {
            let values = right.column(*index).as_boolean();
            let mut first_rows = AHashMap::new();
            let mut duplicate_rows: AHashMap<bool, Vec<usize>> = AHashMap::new();
            for row in 0..values.len() {
                if row.is_multiple_of(1024) {
                    check_cancelled(memory)?;
                }
                if values.is_valid(row) {
                    let key = values.value(row);
                    if let Some(first_row) = first_rows.get(&key).copied() {
                        duplicate_rows
                            .entry(key)
                            .or_insert_with(|| vec![first_row])
                            .push(row);
                    } else {
                        first_rows.insert(key, row);
                    }
                }
            }
            return Ok(Self::Bool {
                first_rows,
                duplicate_rows,
            });
        }
        if let [(.., index)] = keys
            && right.column(*index).data_type() == &DataType::Int32
        {
            let values = right
                .column(*index)
                .as_primitive::<arrow::datatypes::Int32Type>();
            let mut first_rows = AHashMap::new();
            let mut duplicate_rows: AHashMap<i32, Vec<usize>> = AHashMap::new();
            for row in 0..values.len() {
                if row.is_multiple_of(1024) {
                    check_cancelled(memory)?;
                }
                if values.is_valid(row) {
                    let key = values.value(row);
                    if let Some(first_row) = first_rows.get(&key).copied() {
                        duplicate_rows
                            .entry(key)
                            .or_insert_with(|| vec![first_row])
                            .push(row);
                    } else {
                        first_rows.insert(key, row);
                    }
                }
            }
            return Ok(Self::Int32 {
                first_rows,
                duplicate_rows,
            });
        }
        if let [(.., index)] = keys
            && right.column(*index).data_type() == &DataType::UInt64
        {
            let values = right
                .column(*index)
                .as_primitive::<arrow::datatypes::UInt64Type>();
            let mut first_rows = AHashMap::new();
            let mut duplicate_rows: AHashMap<u64, Vec<usize>> = AHashMap::new();
            for row in 0..values.len() {
                if row.is_multiple_of(1024) {
                    check_cancelled(memory)?;
                }
                if values.is_valid(row) {
                    let key = values.value(row);
                    if let Some(first_row) = first_rows.get(&key).copied() {
                        duplicate_rows
                            .entry(key)
                            .or_insert_with(|| vec![first_row])
                            .push(row);
                    } else {
                        first_rows.insert(key, row);
                    }
                }
            }
            return Ok(Self::UInt64 {
                first_rows,
                duplicate_rows,
            });
        }
        if let [(.., index)] = keys
            && matches!(right.column(*index).data_type(), DataType::Decimal256(_, _))
        {
            let values = right
                .column(*index)
                .as_primitive::<arrow::datatypes::Decimal256Type>();
            let mut first_rows = AHashMap::new();
            let mut duplicate_rows: AHashMap<arrow::datatypes::i256, Vec<usize>> = AHashMap::new();
            for row in 0..values.len() {
                if row.is_multiple_of(1024) {
                    check_cancelled(memory)?;
                }
                if values.is_valid(row) {
                    let key = values.value(row);
                    if let Some(first_row) = first_rows.get(&key).copied() {
                        duplicate_rows
                            .entry(key)
                            .or_insert_with(|| vec![first_row])
                            .push(row);
                    } else {
                        first_rows.insert(key, row);
                    }
                }
            }
            let scale = match right.column(*index).data_type() {
                DataType::Decimal256(_, scale) => *scale,
                _ => unreachable!("Decimal256 index branch has a non-decimal column"),
            };
            return Ok(Self::Decimal256 {
                first_rows,
                duplicate_rows,
                scale,
            });
        }
        if let [(.., index)] = keys
            && right.column(*index).data_type() == &DataType::Date32
        {
            let values = right
                .column(*index)
                .as_primitive::<arrow::datatypes::Date32Type>();
            let mut first_rows = AHashMap::new();
            let mut duplicate_rows: AHashMap<i32, Vec<usize>> = AHashMap::new();
            for row in 0..values.len() {
                if row.is_multiple_of(1024) {
                    check_cancelled(memory)?;
                }
                if values.is_valid(row) {
                    let key = values.value(row);
                    if let Some(first_row) = first_rows.get(&key).copied() {
                        duplicate_rows
                            .entry(key)
                            .or_insert_with(|| vec![first_row])
                            .push(row);
                    } else {
                        first_rows.insert(key, row);
                    }
                }
            }
            return Ok(Self::Date32 {
                first_rows,
                duplicate_rows,
            });
        }
        if let [(.., index)] = keys
            && right.column(*index).data_type() == &DataType::Int64
        {
            let values = right
                .column(*index)
                .as_primitive::<arrow::datatypes::Int64Type>();
            let mut first_rows = AHashMap::new();
            let mut duplicate_rows: AHashMap<i64, Vec<usize>> = AHashMap::new();
            for row in 0..values.len() {
                if row.is_multiple_of(1024) {
                    check_cancelled(memory)?;
                }
                if values.is_valid(row) {
                    let key = values.value(row);
                    if let Some(first_row) = first_rows.get(&key).copied() {
                        duplicate_rows
                            .entry(key)
                            .or_insert_with(|| vec![first_row])
                            .push(row);
                    } else {
                        first_rows.insert(key, row);
                    }
                }
            }
            return Ok(Self::Int64 {
                first_rows,
                duplicate_rows,
            });
        }
        if let [(.., index)] = keys
            && matches!(
                right.column(*index).data_type(),
                DataType::Utf8 | DataType::LargeUtf8
            )
        {
            let column = right.column(*index);
            let mut first_rows = AHashMap::new();
            let mut duplicate_rows: AHashMap<String, Vec<usize>> = AHashMap::new();
            for row in 0..column.len() {
                if row.is_multiple_of(1024) {
                    check_cancelled(memory)?;
                }
                let key = match column.data_type() {
                    DataType::Utf8 => column
                        .as_string::<i32>()
                        .is_valid(row)
                        .then(|| column.as_string::<i32>().value(row).to_owned()),
                    DataType::LargeUtf8 => column
                        .as_string::<i64>()
                        .is_valid(row)
                        .then(|| column.as_string::<i64>().value(row).to_owned()),
                    _ => unreachable!("the key type was checked above"),
                };
                if let Some(key) = key {
                    if let Some(first_row) = first_rows.get(&key).copied() {
                        duplicate_rows
                            .entry(key)
                            .or_insert_with(|| vec![first_row])
                            .push(row);
                    } else {
                        first_rows.insert(key, row);
                    }
                }
            }
            return Ok(Self::Utf8 {
                first_rows,
                duplicate_rows,
            });
        }
        let mut map: AHashMap<Vec<Key>, Vec<usize>> = AHashMap::new();
        for row in 0..right.num_rows() {
            if row.is_multiple_of(1024) {
                check_cancelled(memory)?;
            }
            if let Some(key) = row_key(right, row, keys, false)? {
                map.entry(key).or_default().push(row);
            }
        }
        Ok(Self::Composite(map))
    }

    fn matches(
        &self,
        left: &RecordBatch,
        row: usize,
        keys: &[(usize, usize)],
    ) -> Result<Option<&[usize]>> {
        Ok(match self {
            Self::Bool {
                first_rows,
                duplicate_rows,
            } => {
                let values = left.column(keys[0].0).as_boolean();
                if values.is_null(row) {
                    None
                } else {
                    let key = values.value(row);
                    if duplicate_rows.is_empty() {
                        first_rows.get(&key).map(std::slice::from_ref)
                    } else {
                        duplicate_rows
                            .get(&key)
                            .map(Vec::as_slice)
                            .or_else(|| first_rows.get(&key).map(std::slice::from_ref))
                    }
                }
            }
            Self::Int32 {
                first_rows,
                duplicate_rows,
            } => {
                let values = left
                    .column(keys[0].0)
                    .as_primitive::<arrow::datatypes::Int32Type>();
                if values.is_null(row) {
                    None
                } else {
                    let key = values.value(row);
                    if duplicate_rows.is_empty() {
                        first_rows.get(&key).map(std::slice::from_ref)
                    } else {
                        duplicate_rows
                            .get(&key)
                            .map(Vec::as_slice)
                            .or_else(|| first_rows.get(&key).map(std::slice::from_ref))
                    }
                }
            }
            Self::UInt64 {
                first_rows,
                duplicate_rows,
            } => {
                let values = left
                    .column(keys[0].0)
                    .as_primitive::<arrow::datatypes::UInt64Type>();
                if values.is_null(row) {
                    None
                } else {
                    let key = values.value(row);
                    if duplicate_rows.is_empty() {
                        first_rows.get(&key).map(std::slice::from_ref)
                    } else {
                        duplicate_rows
                            .get(&key)
                            .map(Vec::as_slice)
                            .or_else(|| first_rows.get(&key).map(std::slice::from_ref))
                    }
                }
            }
            Self::Decimal256 {
                first_rows,
                duplicate_rows,
                ..
            } => {
                let values = left
                    .column(keys[0].0)
                    .as_primitive::<arrow::datatypes::Decimal256Type>();
                if values.is_null(row) {
                    None
                } else {
                    let key = values.value(row);
                    if duplicate_rows.is_empty() {
                        first_rows.get(&key).map(std::slice::from_ref)
                    } else {
                        duplicate_rows
                            .get(&key)
                            .map(Vec::as_slice)
                            .or_else(|| first_rows.get(&key).map(std::slice::from_ref))
                    }
                }
            }
            Self::Date32 {
                first_rows,
                duplicate_rows,
            } => {
                let values = left
                    .column(keys[0].0)
                    .as_primitive::<arrow::datatypes::Date32Type>();
                if values.is_null(row) {
                    None
                } else {
                    let key = values.value(row);
                    if duplicate_rows.is_empty() {
                        first_rows.get(&key).map(std::slice::from_ref)
                    } else {
                        duplicate_rows
                            .get(&key)
                            .map(Vec::as_slice)
                            .or_else(|| first_rows.get(&key).map(std::slice::from_ref))
                    }
                }
            }
            Self::Int64 {
                first_rows,
                duplicate_rows,
            } => {
                let values = left
                    .column(keys[0].0)
                    .as_primitive::<arrow::datatypes::Int64Type>();
                if values.is_null(row) {
                    None
                } else {
                    let key = values.value(row);
                    if duplicate_rows.is_empty() {
                        first_rows.get(&key).map(std::slice::from_ref)
                    } else {
                        duplicate_rows
                            .get(&key)
                            .map(Vec::as_slice)
                            .or_else(|| first_rows.get(&key).map(std::slice::from_ref))
                    }
                }
            }
            Self::Utf8 {
                first_rows,
                duplicate_rows,
            } => {
                let column = left.column(keys[0].0);
                match column.data_type() {
                    DataType::Utf8 => {
                        let values = column.as_string::<i32>();
                        if values.is_null(row) {
                            None
                        } else {
                            let key = values.value(row);
                            if duplicate_rows.is_empty() {
                                first_rows.get(key).map(std::slice::from_ref)
                            } else {
                                duplicate_rows
                                    .get(key)
                                    .map(Vec::as_slice)
                                    .or_else(|| first_rows.get(key).map(std::slice::from_ref))
                            }
                        }
                    }
                    DataType::LargeUtf8 => {
                        let values = column.as_string::<i64>();
                        if values.is_null(row) {
                            None
                        } else {
                            let key = values.value(row);
                            if duplicate_rows.is_empty() {
                                first_rows.get(key).map(std::slice::from_ref)
                            } else {
                                duplicate_rows
                                    .get(key)
                                    .map(Vec::as_slice)
                                    .or_else(|| first_rows.get(key).map(std::slice::from_ref))
                            }
                        }
                    }
                    _ => return Err(exec_err("join key type changed after planning")),
                }
            }
            Self::Composite(map) => row_key(left, row, keys, true)?
                .and_then(|key| map.get(&key))
                .map(Vec::as_slice),
        })
    }
}

fn check_cancelled(memory: Option<&OperatorMemoryAccount>) -> Result<()> {
    if let Some(memory) = memory {
        memory.check_cancelled()?;
    }
    Ok(())
}

fn row_key(
    batch: &RecordBatch,
    row: usize,
    keys: &[(usize, usize)],
    left: bool,
) -> Result<Option<Vec<Key>>> {
    keys.iter()
        .map(|(left_index, right_index)| {
            let column = batch.column(if left { *left_index } else { *right_index });
            if column.is_null(row) {
                return Ok(None);
            }
            Ok(Some(match column.data_type() {
                DataType::Boolean => Key::Bool(
                    column
                        .as_any()
                        .downcast_ref::<BooleanArray>()
                        .expect("type checked")
                        .value(row),
                ),
                // Narrow signed keys widen losslessly to the existing Int32
                // hash representation. The planner keeps the original
                // schema type, so this branch cannot alias an unrelated
                // signed width without the normal key-type check.
                DataType::Int8 => Key::Int32(
                    column
                        .as_primitive::<arrow::datatypes::Int8Type>()
                        .value(row) as i32,
                ),
                DataType::Int16 => Key::Int32(
                    column
                        .as_primitive::<arrow::datatypes::Int16Type>()
                        .value(row) as i32,
                ),
                DataType::Int32 => Key::Int32(
                    column
                        .as_primitive::<arrow::datatypes::Int32Type>()
                        .value(row),
                ),
                // Keep UInt32 distinct from signed keys while reusing the
                // existing lossless UInt64 hash representation.
                DataType::UInt32 => Key::UInt64(
                    column
                        .as_primitive::<arrow::datatypes::UInt32Type>()
                        .value(row) as u64,
                ),
                DataType::UInt8 => Key::UInt64(
                    column
                        .as_primitive::<arrow::datatypes::UInt8Type>()
                        .value(row) as u64,
                ),
                DataType::UInt16 => Key::UInt64(
                    column
                        .as_primitive::<arrow::datatypes::UInt16Type>()
                        .value(row) as u64,
                ),
                // Arrow Date32 is an i32 day number.  It has the same SQL
                // equality/hash semantics as an integer key, while the
                // schema check above still prevents Date32 from joining an
                // actual Int32 column accidentally.
                DataType::Date32 => Key::Int32(
                    column
                        .as_primitive::<arrow::datatypes::Date32Type>()
                        .value(row),
                ),
                DataType::Date64 => Key::Int64(
                    column
                        .as_primitive::<arrow::datatypes::Date64Type>()
                        .value(row),
                ),
                DataType::Time32(unit) => Key::Int32(match unit {
                    TimeUnit::Second => column
                        .as_primitive::<arrow::datatypes::Time32SecondType>()
                        .value(row),
                    TimeUnit::Millisecond => column
                        .as_primitive::<arrow::datatypes::Time32MillisecondType>()
                        .value(row),
                    _ => return Err(exec_err("unsupported TIME32 unit")),
                }),
                DataType::Time64(unit) => Key::Int64(match unit {
                    TimeUnit::Microsecond => column
                        .as_primitive::<arrow::datatypes::Time64MicrosecondType>()
                        .value(row),
                    TimeUnit::Nanosecond => column
                        .as_primitive::<arrow::datatypes::Time64NanosecondType>()
                        .value(row),
                    _ => return Err(exec_err("unsupported TIME64 unit")),
                }),
                DataType::Timestamp(unit, _) => Key::Int64(match unit {
                    TimeUnit::Second => column
                        .as_primitive::<arrow::datatypes::TimestampSecondType>()
                        .value(row),
                    TimeUnit::Millisecond => column
                        .as_primitive::<arrow::datatypes::TimestampMillisecondType>()
                        .value(row),
                    TimeUnit::Microsecond => column
                        .as_primitive::<arrow::datatypes::TimestampMicrosecondType>()
                        .value(row),
                    TimeUnit::Nanosecond => column
                        .as_primitive::<arrow::datatypes::TimestampNanosecondType>()
                        .value(row),
                }),
                DataType::Int64 => Key::Int64(
                    column
                        .as_primitive::<arrow::datatypes::Int64Type>()
                        .value(row),
                ),
                DataType::UInt64 => Key::UInt64(
                    column
                        .as_primitive::<arrow::datatypes::UInt64Type>()
                        .value(row),
                ),
                DataType::Decimal128(_, scale) => Key::Decimal(
                    column
                        .as_primitive::<arrow::datatypes::Decimal128Type>()
                        .value(row),
                    *scale,
                ),
                DataType::Decimal256(_, scale) => Key::Decimal256(
                    column
                        .as_primitive::<arrow::datatypes::Decimal256Type>()
                        .value(row),
                    *scale,
                ),
                DataType::Float32 => {
                    let value = column
                        .as_primitive::<arrow::datatypes::Float32Type>()
                        .value(row);
                    Key::Float32(if value == 0.0 {
                        0
                    } else if value.is_nan() {
                        f32::NAN.to_bits()
                    } else {
                        value.to_bits()
                    })
                }
                DataType::Float64 => {
                    let value = column
                        .as_primitive::<arrow::datatypes::Float64Type>()
                        .value(row);
                    Key::Float64(if value == 0.0 {
                        0
                    } else if value.is_nan() {
                        f64::NAN.to_bits()
                    } else {
                        value.to_bits()
                    })
                }
                DataType::Utf8 => Key::Utf8(column.as_string::<i32>().value(row).to_owned()),
                DataType::LargeUtf8 => Key::Utf8(column.as_string::<i64>().value(row).to_owned()),
                DataType::Dictionary(key_type, value_type)
                    if key_type.as_ref() == &DataType::Int32
                        && matches!(value_type.as_ref(), DataType::Utf8 | DataType::LargeUtf8) =>
                {
                    let dictionary = column
                        .as_any()
                        .downcast_ref::<Int32DictionaryArray>()
                        .expect("dictionary key type must match schema");
                    let dictionary_index = dictionary.keys().value(row) as usize;
                    let values = dictionary.values();
                    let value = match values.data_type() {
                        DataType::Utf8 => values.as_string::<i32>().value(dictionary_index),
                        DataType::LargeUtf8 => values.as_string::<i64>().value(dictionary_index),
                        _ => unreachable!("dictionary value type was checked above"),
                    };
                    Key::Utf8(value.to_owned())
                }
                DataType::Dictionary(key_type, value_type)
                    if key_type.as_ref() == &DataType::Int32
                        && matches!(
                            value_type.as_ref(),
                            DataType::Boolean
                                | DataType::Int32
                                | DataType::UInt32
                                | DataType::Float32
                                | DataType::Float64
                        ) =>
                {
                    let dictionary = column
                        .as_any()
                        .downcast_ref::<DictionaryArray<Int32Type>>()
                        .expect("dictionary key type must match schema");
                    let dictionary_index = dictionary.keys().value(row) as usize;
                    let values = dictionary.values();
                    if values.is_null(dictionary_index) {
                        return Ok(None);
                    }
                    match value_type.as_ref() {
                        DataType::Boolean => Key::Bool(
                            values
                                .as_any()
                                .downcast_ref::<BooleanArray>()
                                .expect("dictionary value type must match schema")
                                .value(dictionary_index),
                        ),
                        DataType::Int32 => Key::Int32(
                            values
                                .as_primitive::<arrow::datatypes::Int32Type>()
                                .value(dictionary_index),
                        ),
                        DataType::UInt32 => Key::UInt64(
                            values
                                .as_primitive::<arrow::datatypes::UInt32Type>()
                                .value(dictionary_index) as u64,
                        ),
                        DataType::Float32 => {
                            let value = values
                                .as_primitive::<arrow::datatypes::Float32Type>()
                                .value(dictionary_index);
                            Key::Float32(if value == 0.0 {
                                0
                            } else if value.is_nan() {
                                f32::NAN.to_bits()
                            } else {
                                value.to_bits()
                            })
                        }
                        DataType::Float64 => {
                            let value = values
                                .as_primitive::<arrow::datatypes::Float64Type>()
                                .value(dictionary_index);
                            Key::Float64(if value == 0.0 {
                                0
                            } else if value.is_nan() {
                                f64::NAN.to_bits()
                            } else {
                                value.to_bits()
                            })
                        }
                        _ => unreachable!("dictionary value type was checked above"),
                    }
                }
                DataType::Dictionary(key_type, value_type)
                    if key_type.as_ref() == &DataType::Int32
                        && matches!(
                            value_type.as_ref(),
                            DataType::Date32
                                | DataType::Date64
                                | DataType::Time32(_)
                                | DataType::Time64(_)
                                | DataType::Timestamp(_, _)
                        ) =>
                {
                    let dictionary = column
                        .as_any()
                        .downcast_ref::<DictionaryArray<Int32Type>>()
                        .expect("dictionary key type must match schema");
                    let dictionary_index = dictionary.keys().value(row) as usize;
                    let values = dictionary.values();
                    if values.is_null(dictionary_index) {
                        return Ok(None);
                    }
                    match value_type.as_ref() {
                        DataType::Date32 => Key::Int32(
                            values
                                .as_primitive::<arrow::datatypes::Date32Type>()
                                .value(dictionary_index),
                        ),
                        DataType::Date64 => Key::Int64(
                            values
                                .as_primitive::<arrow::datatypes::Date64Type>()
                                .value(dictionary_index),
                        ),
                        DataType::Time32(unit) => Key::Int32(match unit {
                            TimeUnit::Second => values
                                .as_primitive::<arrow::datatypes::Time32SecondType>()
                                .value(dictionary_index),
                            TimeUnit::Millisecond => values
                                .as_primitive::<arrow::datatypes::Time32MillisecondType>()
                                .value(dictionary_index),
                            _ => return Err(exec_err("unsupported TIME32 unit")),
                        }),
                        DataType::Time64(unit) => Key::Int64(match unit {
                            TimeUnit::Microsecond => values
                                .as_primitive::<arrow::datatypes::Time64MicrosecondType>()
                                .value(dictionary_index),
                            TimeUnit::Nanosecond => values
                                .as_primitive::<arrow::datatypes::Time64NanosecondType>()
                                .value(dictionary_index),
                            _ => return Err(exec_err("unsupported TIME64 unit")),
                        }),
                        DataType::Timestamp(unit, _) => Key::Int64(match unit {
                            TimeUnit::Second => values
                                .as_primitive::<arrow::datatypes::TimestampSecondType>()
                                .value(dictionary_index),
                            TimeUnit::Millisecond => values
                                .as_primitive::<arrow::datatypes::TimestampMillisecondType>()
                                .value(dictionary_index),
                            TimeUnit::Microsecond => values
                                .as_primitive::<arrow::datatypes::TimestampMicrosecondType>()
                                .value(dictionary_index),
                            TimeUnit::Nanosecond => values
                                .as_primitive::<arrow::datatypes::TimestampNanosecondType>()
                                .value(dictionary_index),
                        }),
                        _ => unreachable!("dictionary value type was checked above"),
                    }
                }
                data_type => {
                    return Err(exec_err(format!("unsupported join key type {data_type}")));
                }
            }))
        })
        .collect::<Result<Option<Vec<_>>>>()
}

fn exec_err(message: impl Into<String>) -> KaveonError {
    KaveonError::Execution(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{
        Date32Array, Date64Array, Decimal128Array, Decimal256Array, Int8Array, Int16Array,
        Int32Array, Int64Array, LargeStringArray, StringArray, Time32MillisecondArray,
        Time64NanosecondArray, TimestampMillisecondArray, TimestampSecondArray, UInt8Array,
        UInt16Array,
    };
    use arrow::datatypes::i256;
    use std::collections::VecDeque;

    struct Input {
        schema: SchemaRef,
        batches: VecDeque<RecordBatch>,
    }
    impl Input {
        fn new(batch: RecordBatch) -> Self {
            Self {
                schema: batch.schema(),
                batches: VecDeque::from([batch]),
            }
        }
    }
    impl BatchOperator for Input {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.pop_front())
        }
    }
    fn input(ids: Vec<Option<i64>>, names: Vec<&str>, name: &str) -> Input {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new(name, DataType::Utf8, false),
        ]));
        Input::new(
            RecordBatch::try_new(
                schema,
                vec![
                    Arc::new(Int64Array::from(ids)),
                    Arc::new(StringArray::from(names)),
                ],
            )
            .unwrap(),
        )
    }

    #[test]
    fn semi_and_anti_hash_joins_emit_only_matching_left_rows() {
        for (join_type, expected) in [
            (JoinType::Semi, vec![Some(2), Some(3)]),
            (JoinType::Anti, vec![Some(1), None]),
        ] {
            let left = Box::new(input(
                vec![Some(1), Some(2), Some(3), None],
                vec!["a", "b", "c", "n"],
                "left_name",
            ));
            let right = Box::new(input(
                vec![Some(2), Some(3), None],
                vec!["b", "c", "n"],
                "right_name",
            ));
            let mut join =
                HashJoin::try_new(left, right, join_type, vec![("id".into(), "id".into())])
                    .unwrap();
            let batches = kaveon_core::collect_batches(&mut join).unwrap();
            let result = concat_batches(join.schema(), &batches).unwrap();
            assert_eq!(result.num_columns(), 2);
            assert_eq!(
                result
                    .column(0)
                    .as_primitive::<arrow::datatypes::Int64Type>()
                    .iter()
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn large_utf8_join_keys_match_and_skip_nulls() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "key",
            DataType::LargeUtf8,
            true,
        )]));
        let left = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(LargeStringArray::from(vec![
                Some("alpha"),
                Some("missing"),
                None,
            ]))],
        )
        .unwrap();
        let right = RecordBatch::try_new(
            schema,
            vec![Arc::new(LargeStringArray::from(vec![Some("alpha"), None]))],
        )
        .unwrap();
        let mut join = HashJoin::try_new(
            Box::new(Input::new(left)),
            Box::new(Input::new(right)),
            JoinType::Inner,
            vec![("key".into(), "key".into())],
        )
        .unwrap();
        let batches = kaveon_core::collect_batches(&mut join).unwrap();
        assert_eq!(
            concat_batches(join.schema(), &batches).unwrap().num_rows(),
            1
        );
    }

    #[test]
    fn integer_fast_path_matches_composite_path_with_nulls_duplicates_and_outer_rows() {
        let left_ids = (0..79)
            .map(|i| {
                if i % 11 == 0 {
                    None
                } else {
                    Some((i * 7 % 23) - 10)
                }
            })
            .collect::<Vec<_>>();
        let right_ids = (0..61)
            .map(|i| {
                if i % 9 == 0 {
                    None
                } else {
                    Some((i * 5 % 19) - 8)
                }
            })
            .collect::<Vec<_>>();
        for mode in [
            JoinType::Inner,
            JoinType::Left,
            JoinType::Right,
            JoinType::Full,
        ] {
            let run = |keys| {
                let left = Box::new(input(
                    left_ids.clone(),
                    vec!["l"; left_ids.len()],
                    "left_name",
                ));
                let right = Box::new(input(
                    right_ids.clone(),
                    vec!["r"; right_ids.len()],
                    "right_name",
                ));
                let mut join = HashJoin::try_new(left, right, mode, keys).unwrap();
                let batches = kaveon_core::collect_batches(&mut join).unwrap();
                concat_batches(join.schema(), &batches).unwrap()
            };
            assert_eq!(
                run(vec![("id".into(), "id".into())]),
                run(vec![("id".into(), "id".into()), ("id".into(), "id".into())])
            );
        }
    }

    #[test]
    fn timestamp_join_keys_match_across_duplicate_and_null_rows() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("ts", DataType::Timestamp(TimeUnit::Millisecond, None), true),
            Field::new("side", DataType::Utf8, false),
        ]));
        let left = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![
                    Some(1_000),
                    Some(2_000),
                    None,
                ])),
                Arc::new(StringArray::from(vec!["a", "b", "null"])),
            ],
        )
        .unwrap();
        let right = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![
                    Some(1_000),
                    Some(1_000),
                    None,
                ])),
                Arc::new(StringArray::from(vec!["x", "y", "null"])),
            ],
        )
        .unwrap();
        let mut join = HashJoin::try_new(
            Box::new(Input::new(left)),
            Box::new(Input::new(right)),
            JoinType::Inner,
            vec![("ts".into(), "ts".into())],
        )
        .unwrap();
        let batches = kaveon_core::collect_batches(&mut join).unwrap();
        let result = concat_batches(join.schema(), &batches).unwrap();
        assert_eq!(result.num_rows(), 2);
        assert_eq!(result.column(0).null_count(), 0);
    }

    #[test]
    fn uint32_join_keys_match_without_signed_narrowing() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::UInt32, true),
            Field::new("side", DataType::Utf8, false),
        ]));
        let left = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(UInt32Array::from(vec![Some(1), Some(u32::MAX), None])),
                Arc::new(StringArray::from(vec!["left", "max", "null"])),
            ],
        )
        .unwrap();
        let right = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(UInt32Array::from(vec![Some(u32::MAX), Some(2), None])),
                Arc::new(StringArray::from(vec!["max", "other", "null"])),
            ],
        )
        .unwrap();
        let mut join = HashJoin::try_new(
            Box::new(Input::new(left)),
            Box::new(Input::new(right)),
            JoinType::Inner,
            vec![("key".into(), "key".into())],
        )
        .unwrap();
        let batches = kaveon_core::collect_batches(&mut join).unwrap();
        let result = concat_batches(join.schema(), &batches).unwrap();
        assert_eq!(result.num_rows(), 1);
        assert_eq!(
            result
                .column(0)
                .as_primitive::<arrow::datatypes::UInt32Type>()
                .value(0),
            u32::MAX
        );
    }

    #[test]
    fn narrow_integer_join_keys_widen_losslessly_and_skip_nulls() {
        macro_rules! check {
            ($array:ty, $type:expr, [$($left:expr),*], [$($right:expr),*], $expected:expr) => {{
                let schema = Arc::new(Schema::new(vec![
                    Field::new("key", $type, true),
                    Field::new("side", DataType::Utf8, false),
                ]));
                let left = RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![
                        Arc::new(<$array>::from(vec![$($left),*])),
                        Arc::new(StringArray::from(vec!["left"; 3])),
                    ],
                ).unwrap();
                let right = RecordBatch::try_new(
                    schema,
                    vec![
                        Arc::new(<$array>::from(vec![$($right),*])),
                        Arc::new(StringArray::from(vec!["right"; 3])),
                    ],
                ).unwrap();
                let mut join = HashJoin::try_new(
                    Box::new(Input::new(left)), Box::new(Input::new(right)),
                    JoinType::Inner, vec![("key".into(), "key".into())],
                ).unwrap();
                let batches = kaveon_core::collect_batches(&mut join).unwrap();
                assert_eq!(concat_batches(join.schema(), &batches).unwrap().num_rows(), $expected);
            }};
        }
        check!(
            Int8Array,
            DataType::Int8,
            [Some(i8::MIN), Some(7), None],
            [Some(7), Some(i8::MIN), None],
            2
        );
        check!(
            Int16Array,
            DataType::Int16,
            [Some(i16::MIN), Some(7), None],
            [Some(7), Some(i16::MIN), None],
            2
        );
        check!(
            UInt8Array,
            DataType::UInt8,
            [Some(0), Some(u8::MAX), None],
            [Some(u8::MAX), Some(0), None],
            2
        );
        check!(
            UInt16Array,
            DataType::UInt16,
            [Some(0), Some(u16::MAX), None],
            [Some(u16::MAX), Some(0), None],
            2
        );
    }

    #[test]
    fn dictionary_numeric_and_boolean_join_keys_unbox_losslessly() {
        fn check(left_values: ArrayRef, right_values: ArrayRef, value_type: DataType) {
            let dictionary_type =
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(value_type));
            let schema = Arc::new(Schema::new(vec![Field::new("key", dictionary_type, true)]));
            let left_key = DictionaryArray::<Int32Type>::try_new(
                Int32Array::from(vec![Some(0), Some(1), Some(2)]),
                left_values,
            )
            .unwrap();
            let right_key = DictionaryArray::<Int32Type>::try_new(
                Int32Array::from(vec![Some(1), Some(0), Some(2)]),
                right_values,
            )
            .unwrap();
            let left = RecordBatch::try_new(Arc::clone(&schema), vec![Arc::new(left_key)]).unwrap();
            let right = RecordBatch::try_new(schema, vec![Arc::new(right_key)]).unwrap();
            let mut join = HashJoin::try_new(
                Box::new(Input::new(left)),
                Box::new(Input::new(right)),
                JoinType::Inner,
                vec![("key".into(), "key".into())],
            )
            .unwrap();
            let batches = kaveon_core::collect_batches(&mut join).unwrap();
            assert_eq!(
                concat_batches(join.schema(), &batches).unwrap().num_rows(),
                2
            );
        }
        check(
            Arc::new(Int32Array::from(vec![Some(i32::MIN), Some(i32::MAX), None])),
            Arc::new(Int32Array::from(vec![Some(i32::MIN), Some(i32::MAX), None])),
            DataType::Int32,
        );
        check(
            Arc::new(UInt32Array::from(vec![Some(0), Some(u32::MAX), None])),
            Arc::new(UInt32Array::from(vec![Some(0), Some(u32::MAX), None])),
            DataType::UInt32,
        );
        check(
            Arc::new(BooleanArray::from(vec![Some(false), Some(true), None])),
            Arc::new(BooleanArray::from(vec![Some(false), Some(true), None])),
            DataType::Boolean,
        );
    }

    #[test]
    fn dictionary_float_join_keys_preserve_zero_nan_and_null_semantics() {
        fn check(left_values: ArrayRef, right_values: ArrayRef, value_type: DataType) {
            let dictionary_type =
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(value_type));
            let schema = Arc::new(Schema::new(vec![Field::new("key", dictionary_type, true)]));
            let left_key = DictionaryArray::<Int32Type>::try_new(
                Int32Array::from(vec![Some(0), Some(1), Some(2), Some(3)]),
                left_values,
            )
            .unwrap();
            let right_key = DictionaryArray::<Int32Type>::try_new(
                Int32Array::from(vec![Some(0), Some(1), Some(2), Some(3)]),
                right_values,
            )
            .unwrap();
            let left = RecordBatch::try_new(Arc::clone(&schema), vec![Arc::new(left_key)]).unwrap();
            let right = RecordBatch::try_new(schema, vec![Arc::new(right_key)]).unwrap();
            let mut join = HashJoin::try_new(
                Box::new(Input::new(left)),
                Box::new(Input::new(right)),
                JoinType::Inner,
                vec![("key".into(), "key".into())],
            )
            .unwrap();
            let batches = kaveon_core::collect_batches(&mut join).unwrap();
            assert_eq!(
                concat_batches(join.schema(), &batches).unwrap().num_rows(),
                2
            );
        }
        check(
            Arc::new(Float32Array::from(vec![
                Some(-0.0),
                Some(f32::NAN),
                Some(1.5),
                None,
            ])),
            Arc::new(Float32Array::from(vec![
                Some(0.0),
                Some(f32::NAN),
                Some(2.5),
                None,
            ])),
            DataType::Float32,
        );
        check(
            Arc::new(Float64Array::from(vec![
                Some(-0.0),
                Some(f64::NAN),
                Some(1.5),
                None,
            ])),
            Arc::new(Float64Array::from(vec![
                Some(0.0),
                Some(f64::NAN),
                Some(2.5),
                None,
            ])),
            DataType::Float64,
        );
    }

    #[test]
    fn dictionary_temporal_join_keys_preserve_units_extrema_and_nulls() {
        fn check(left_values: ArrayRef, right_values: ArrayRef, value_type: DataType) {
            let dictionary_type =
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(value_type));
            let schema = Arc::new(Schema::new(vec![Field::new("key", dictionary_type, true)]));
            let left_key = DictionaryArray::<Int32Type>::try_new(
                Int32Array::from(vec![Some(0), Some(1), Some(2)]),
                left_values,
            )
            .unwrap();
            let right_key = DictionaryArray::<Int32Type>::try_new(
                Int32Array::from(vec![Some(1), Some(0), Some(2)]),
                right_values,
            )
            .unwrap();
            let left = RecordBatch::try_new(Arc::clone(&schema), vec![Arc::new(left_key)]).unwrap();
            let right = RecordBatch::try_new(schema, vec![Arc::new(right_key)]).unwrap();
            let mut join = HashJoin::try_new(
                Box::new(Input::new(left)),
                Box::new(Input::new(right)),
                JoinType::Inner,
                vec![("key".into(), "key".into())],
            )
            .unwrap();
            let batches = kaveon_core::collect_batches(&mut join).unwrap();
            assert_eq!(
                concat_batches(join.schema(), &batches).unwrap().num_rows(),
                2
            );
        }
        check(
            Arc::new(Date32Array::from(vec![
                Some(i32::MIN),
                Some(i32::MAX),
                None,
            ])),
            Arc::new(Date32Array::from(vec![
                Some(i32::MIN),
                Some(i32::MAX),
                None,
            ])),
            DataType::Date32,
        );
        check(
            Arc::new(Date64Array::from(vec![
                Some(i64::MIN),
                Some(i64::MAX),
                None,
            ])),
            Arc::new(Date64Array::from(vec![
                Some(i64::MIN),
                Some(i64::MAX),
                None,
            ])),
            DataType::Date64,
        );
        check(
            Arc::new(Time32MillisecondArray::from(vec![
                Some(i32::MIN),
                Some(i32::MAX),
                None,
            ])),
            Arc::new(Time32MillisecondArray::from(vec![
                Some(i32::MIN),
                Some(i32::MAX),
                None,
            ])),
            DataType::Time32(TimeUnit::Millisecond),
        );
        check(
            Arc::new(Time64NanosecondArray::from(vec![
                Some(i64::MIN),
                Some(i64::MAX),
                None,
            ])),
            Arc::new(Time64NanosecondArray::from(vec![
                Some(i64::MIN),
                Some(i64::MAX),
                None,
            ])),
            DataType::Time64(TimeUnit::Nanosecond),
        );
        check(
            Arc::new(
                TimestampSecondArray::from(vec![Some(i64::MIN), Some(i64::MAX), None])
                    .with_timezone("UTC"),
            ),
            Arc::new(
                TimestampSecondArray::from(vec![Some(i64::MIN), Some(i64::MAX), None])
                    .with_timezone("UTC"),
            ),
            DataType::Timestamp(TimeUnit::Second, Some("UTC".into())),
        );
    }

    #[test]
    fn date64_and_time32_join_keys_match_and_skip_nulls() {
        let date_schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Date64, true),
            Field::new("side", DataType::Utf8, false),
        ]));
        let left = RecordBatch::try_new(
            Arc::clone(&date_schema),
            vec![
                Arc::new(Date64Array::from(vec![Some(86_400_000), None])),
                Arc::new(StringArray::from(vec!["left", "null"])),
            ],
        )
        .unwrap();
        let right = RecordBatch::try_new(
            date_schema,
            vec![
                Arc::new(Date64Array::from(vec![Some(86_400_000), Some(172_800_000)])),
                Arc::new(StringArray::from(vec!["match", "other"])),
            ],
        )
        .unwrap();
        let mut join = HashJoin::try_new(
            Box::new(Input::new(left)),
            Box::new(Input::new(right)),
            JoinType::Inner,
            vec![("key".into(), "key".into())],
        )
        .unwrap();
        let batches = kaveon_core::collect_batches(&mut join).unwrap();
        assert_eq!(
            concat_batches(join.schema(), &batches).unwrap().num_rows(),
            1
        );

        let time_schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Time32(TimeUnit::Millisecond), true),
            Field::new("side", DataType::Utf8, false),
        ]));
        let left = RecordBatch::try_new(
            Arc::clone(&time_schema),
            vec![
                Arc::new(Time32MillisecondArray::from(vec![Some(1_000), None])),
                Arc::new(StringArray::from(vec!["left", "null"])),
            ],
        )
        .unwrap();
        let right = RecordBatch::try_new(
            time_schema,
            vec![
                Arc::new(Time32MillisecondArray::from(vec![Some(1_000), Some(2_000)])),
                Arc::new(StringArray::from(vec!["match", "other"])),
            ],
        )
        .unwrap();
        let mut join = HashJoin::try_new(
            Box::new(Input::new(left)),
            Box::new(Input::new(right)),
            JoinType::Inner,
            vec![("key".into(), "key".into())],
        )
        .unwrap();
        let batches = kaveon_core::collect_batches(&mut join).unwrap();
        assert_eq!(
            concat_batches(join.schema(), &batches).unwrap().num_rows(),
            1
        );
    }

    #[test]
    fn decimal_join_keys_match_exact_scaled_values_and_skip_nulls() {
        let data_type = DataType::Decimal128(10, 2);
        let schema = Arc::new(Schema::new(vec![
            Field::new("amount", data_type.clone(), true),
            Field::new("side", DataType::Utf8, false),
        ]));
        let decimal = |values: Vec<Option<i128>>| {
            Arc::new(
                Decimal128Array::from(values)
                    .with_precision_and_scale(10, 2)
                    .unwrap(),
            ) as Arc<dyn arrow::array::Array>
        };
        let left = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                decimal(vec![Some(125), Some(200), None]),
                Arc::new(StringArray::from(vec!["a", "b", "null"])),
            ],
        )
        .unwrap();
        let right = RecordBatch::try_new(
            schema,
            vec![
                decimal(vec![Some(125), Some(125), None]),
                Arc::new(StringArray::from(vec!["x", "y", "null"])),
            ],
        )
        .unwrap();
        let mut join = HashJoin::try_new(
            Box::new(Input::new(left)),
            Box::new(Input::new(right)),
            JoinType::Inner,
            vec![("amount".into(), "amount".into())],
        )
        .unwrap();
        let batches = kaveon_core::collect_batches(&mut join).unwrap();
        let result = concat_batches(join.schema(), &batches).unwrap();
        assert_eq!(result.num_rows(), 2);
        assert_eq!(result.column(0).null_count(), 0);
    }

    #[test]
    fn decimal256_join_keys_match_exact_values_and_skip_nulls() {
        let data_type = DataType::Decimal256(40, 2);
        let schema = Arc::new(Schema::new(vec![
            Field::new("amount", data_type.clone(), true),
            Field::new("side", DataType::Utf8, false),
        ]));
        let decimal = |values: Vec<Option<i256>>| {
            Arc::new(
                Decimal256Array::from(values)
                    .with_precision_and_scale(40, 2)
                    .unwrap(),
            ) as Arc<dyn arrow::array::Array>
        };
        let left = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                decimal(vec![
                    Some(i256::from_i128(125)),
                    Some(i256::from_i128(200)),
                    None,
                ]),
                Arc::new(StringArray::from(vec!["a", "b", "null"])),
            ],
        )
        .unwrap();
        let right = RecordBatch::try_new(
            schema,
            vec![
                decimal(vec![
                    Some(i256::from_i128(125)),
                    Some(i256::from_i128(125)),
                    None,
                ]),
                Arc::new(StringArray::from(vec!["x", "y", "null"])),
            ],
        )
        .unwrap();
        let mut join = HashJoin::try_new(
            Box::new(Input::new(left)),
            Box::new(Input::new(right)),
            JoinType::Inner,
            vec![("amount".into(), "amount".into())],
        )
        .unwrap();
        let batches = kaveon_core::collect_batches(&mut join).unwrap();
        let result = concat_batches(join.schema(), &batches).unwrap();
        assert_eq!(result.num_rows(), 2);
        assert_eq!(result.column(0).null_count(), 0);
    }

    #[test]
    fn decimal256_runtime_filter_uses_exact_typed_index_without_narrowing() {
        let data_type = DataType::Decimal256(40, 2);
        let schema = Arc::new(Schema::new(vec![Field::new("amount", data_type, true)]));
        let decimal = |values: Vec<Option<i256>>| {
            Arc::new(
                Decimal256Array::from(values)
                    .with_precision_and_scale(40, 2)
                    .unwrap(),
            ) as Arc<dyn arrow::array::Array>
        };
        let build = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![decimal(vec![
                Some(i256::from_i128(125)),
                Some(i256::from_i128(400)),
                Some(i256::from_i128(400)),
            ])],
        )
        .unwrap();
        let probe = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![decimal(vec![
                Some(i256::from_i128(100)),
                Some(i256::from_i128(125)),
                Some(i256::from_i128(400)),
                None,
            ])],
        )
        .unwrap();
        let built = BuiltSide::build(&schema, &[build], &[(0, 0)], JoinType::Inner, None).unwrap();
        let filter = BuildKeyFilter::from_built(&built, &[(0, 0)]).unwrap();
        let predicate = filter
            .storage_predicate(&schema)
            .expect("Decimal256 keys should reach storage pushdown losslessly");
        assert!(matches!(predicate, StoragePredicate::In { values, .. } if values.len() >= 2));
        let mask = filter.filter_batch(&probe).unwrap();
        assert_eq!(
            mask.iter().collect::<Vec<_>>(),
            vec![Some(false), Some(true), Some(true), Some(false)]
        );
    }

    #[test]
    fn boolean_runtime_filter_uses_typed_index_and_storage_predicate() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "enabled",
            DataType::Boolean,
            true,
        )]));
        let build = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(BooleanArray::from(vec![
                Some(true),
                Some(false),
                Some(true),
                None,
            ]))],
        )
        .unwrap();
        let probe = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(BooleanArray::from(vec![
                Some(false),
                Some(true),
                None,
            ]))],
        )
        .unwrap();
        let built = BuiltSide::build(&schema, &[build], &[(0, 0)], JoinType::Inner, None).unwrap();
        let filter = BuildKeyFilter::from_built(&built, &[(0, 0)]).unwrap();
        let StoragePredicate::In { values, .. } = filter.storage_predicate(&schema).unwrap() else {
            panic!("expected boolean single-key IN predicate");
        };
        assert_eq!(values.len(), 2);
        assert!(values.contains(&ScalarValue::Bool(true)));
        assert!(values.contains(&ScalarValue::Bool(false)));
        let mask = filter.filter_batch(&probe).unwrap();
        assert_eq!(
            mask.iter().collect::<Vec<_>>(),
            vec![Some(true), Some(true), Some(false)]
        );
    }

    #[test]
    fn inner_join_preserves_duplicates_and_excludes_null_keys() {
        let mut join = HashJoin::try_new(
            Box::new(input(
                vec![Some(1), Some(2), None],
                vec!["a", "b", "n"],
                "left_name",
            )),
            Box::new(input(
                vec![Some(1), Some(1), None],
                vec!["x", "y", "n"],
                "right_name",
            )),
            JoinType::Inner,
            vec![("id".into(), "id".into())],
        )
        .unwrap();
        assert_eq!(join.next_batch().unwrap().unwrap().num_rows(), 2);
        assert!(join.next_batch().unwrap().is_none());
    }

    #[test]
    fn full_join_emits_both_unmatched_sides() {
        let mut join = HashJoin::try_new(
            Box::new(input(vec![Some(1), Some(2)], vec!["a", "b"], "left_name")),
            Box::new(input(vec![Some(1), Some(3)], vec!["x", "z"], "right_name")),
            JoinType::Full,
            vec![("id".into(), "id".into())],
        )
        .unwrap();
        let batches = kaveon_core::collect_batches(&mut join).unwrap();
        let result = concat_batches(join.schema(), &batches).unwrap();
        assert_eq!(result.num_rows(), 3);
        assert_eq!(result.column(0).null_count(), 1);
        assert_eq!(result.column(2).null_count(), 1);
    }

    #[test]
    fn outer_join_residual_filters_pairs_before_null_extension() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, true),
            Field::new("value", DataType::Int32, false),
        ]));
        let left = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int32Array::from(vec![Some(1), Some(1), Some(2)])),
                Arc::new(Int32Array::from(vec![5, 1, 9])),
            ],
        )
        .unwrap();
        let right = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![Some(1), Some(1), Some(2)])),
                Arc::new(Int32Array::from(vec![3, 7, 2])),
            ],
        )
        .unwrap();
        let residual = Expr::BinaryOp {
            left: Box::new(Expr::Column("left.value".into())),
            op: kaveon_core::BinaryOp::Gt,
            right: Box::new(Expr::Column("right.value".into())),
        };
        let mut join = HashJoin::try_new_qualified(
            Box::new(Input::new(left)),
            Box::new(Input::new(right)),
            JoinType::Left,
            vec![("id".into(), "id".into())],
            Some("left"),
            Some("right"),
        )
        .unwrap()
        .with_residual(Some(residual));
        let batches = kaveon_core::collect_batches(&mut join).unwrap();
        let result = concat_batches(join.schema(), &batches).unwrap();
        assert_eq!(result.num_rows(), 3);
        assert_eq!(result.column(2).null_count(), 1);
        let right_values = result
            .column(3)
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap();
        assert_eq!(
            right_values.iter().collect::<Vec<_>>(),
            vec![Some(3), None, Some(2)]
        );
    }

    #[test]
    fn residual_only_outer_join_uses_broadcast_candidate_pairs() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int32,
            false,
        )]));
        let left = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int32Array::from(vec![5, 1]))],
        )
        .unwrap();
        let right =
            RecordBatch::try_new(schema, vec![Arc::new(Int32Array::from(vec![3, 7]))]).unwrap();
        let residual = Expr::BinaryOp {
            left: Box::new(Expr::Column("left.value".into())),
            op: kaveon_core::BinaryOp::Gt,
            right: Box::new(Expr::Column("right.value".into())),
        };
        let mut join = HashJoin::try_new_qualified_with_residual(
            Box::new(Input::new(left)),
            Box::new(Input::new(right)),
            JoinType::Left,
            Vec::new(),
            Some("left"),
            Some("right"),
            Some(residual),
        )
        .unwrap();
        let batches = kaveon_core::collect_batches(&mut join).unwrap();
        let result = concat_batches(join.schema(), &batches).unwrap();
        assert_eq!(result.num_rows(), 2);
        let right_values = result
            .column(1)
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap();
        assert_eq!(right_values.iter().collect::<Vec<_>>(), vec![Some(3), None]);
    }

    #[test]
    fn cross_join_returns_cartesian_product() {
        let mut join = HashJoin::try_new(
            Box::new(input(vec![Some(1), Some(2)], vec!["a", "b"], "left_name")),
            Box::new(input(
                vec![Some(3), Some(4), Some(5)],
                vec!["x", "y", "z"],
                "right_name",
            )),
            JoinType::Cross,
            Vec::new(),
        )
        .unwrap();
        assert_eq!(join.next_batch().unwrap().unwrap().num_rows(), 6);
    }

    #[test]
    fn streaming_join_preserves_fanout_and_outer_tail_across_output_boundaries() {
        for mode in [JoinType::Full, JoinType::Cross] {
            let mut right_ids = vec![Some(1); 9_000];
            right_ids.extend([None, Some(3)]);
            let mut join = HashJoin::try_new(
                Box::new(input(
                    vec![Some(1), Some(1), None, Some(2)],
                    vec!["l"; 4],
                    "l",
                )),
                Box::new(input(right_ids, vec!["r"; 9_002], "r")),
                mode,
                if mode == JoinType::Cross {
                    vec![]
                } else {
                    vec![("id".into(), "id".into())]
                },
            )
            .unwrap();
            let batches = kaveon_core::collect_batches(&mut join).unwrap();
            assert!(batches.len() >= 3);
            assert!(
                batches
                    .iter()
                    .all(|batch| batch.num_rows() <= OUTPUT_BATCH_ROWS)
            );
            let rows = batches.iter().map(RecordBatch::num_rows).sum::<usize>();
            assert_eq!(
                rows,
                if mode == JoinType::Full {
                    18_004
                } else {
                    36_008
                }
            );
            if mode == JoinType::Full {
                assert_eq!(
                    batches
                        .iter()
                        .map(|batch| batch.column(1).null_count())
                        .sum::<usize>(),
                    2
                );
                assert_eq!(
                    batches
                        .iter()
                        .map(|batch| batch.column(3).null_count())
                        .sum::<usize>(),
                    2
                );
            }
        }
    }

    #[test]
    fn streaming_join_does_not_collect_probe_and_releases_memory_on_eof() {
        struct Generated {
            batch: RecordBatch,
            remaining: usize,
            reads: std::rc::Rc<std::cell::Cell<usize>>,
        }
        impl BatchOperator for Generated {
            fn schema(&self) -> &SchemaRef {
                self.batch.schema_ref()
            }
            fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
                if self.remaining == 0 {
                    return Ok(None);
                }
                self.remaining -= 1;
                self.reads.set(self.reads.get() + 1);
                Ok(Some(self.batch.clone()))
            }
        }
        let batch = input(vec![Some(1); 512], vec!["l"; 512], "l")
            .batches
            .pop_front()
            .unwrap();
        let reads = std::rc::Rc::new(std::cell::Cell::new(0));
        let pool = kaveon_core::QueryMemoryPool::new("streaming-join", 256 * 1024).unwrap();
        let mut join = HashJoin::try_new_qualified_with_memory(
            Box::new(Generated {
                batch,
                remaining: 300,
                reads: reads.clone(),
            }),
            Box::new(input(vec![Some(1)], vec!["r"], "r")),
            JoinType::Inner,
            vec![("id".into(), "id".into())],
            None,
            None,
            pool.operator("join").unwrap(),
        )
        .unwrap();
        let first = join.next_batch().unwrap().unwrap();
        assert_eq!(reads.get(), 1);
        let mut count = first.num_rows();
        drop(first);
        while let Some(batch) = join.next_batch().unwrap() {
            count += batch.num_rows();
        }
        assert_eq!(count, 300 * 512);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.snapshot().peak_bytes <= 256 * 1024);
    }

    #[test]
    fn streaming_join_cancellation_releases_retained_build_and_output() {
        let pool =
            kaveon_core::QueryMemoryPool::new("cancel-streaming-join", 4 * 1024 * 1024).unwrap();
        let canceled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = canceled.clone();
        pool.set_cancellation_probe(move || signal.load(std::sync::atomic::Ordering::Acquire))
            .unwrap();
        let mut join = HashJoin::try_new_qualified_with_memory(
            Box::new(input(vec![Some(1); 2], vec!["l"; 2], "l")),
            Box::new(input(vec![Some(1); 9_000], vec!["r"; 9_000], "r")),
            JoinType::Inner,
            vec![("id".into(), "id".into())],
            None,
            None,
            pool.operator("join").unwrap(),
        )
        .unwrap();
        assert_eq!(
            join.next_batch().unwrap().unwrap().num_rows(),
            OUTPUT_BATCH_ROWS
        );
        assert!(pool.snapshot().current_bytes > 0);
        canceled.store(true, std::sync::atomic::Ordering::Release);
        assert!(join.next_batch().is_err());
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(join.next_batch().unwrap().is_none());
    }

    #[test]
    fn memory_aware_join_fails_before_unbounded_output_growth() {
        let pool = kaveon_core::QueryMemoryPool::new("bounded-join", 512).unwrap();
        let account = pool.operator("hash-join").unwrap();
        let mut join = HashJoin::try_new_qualified_with_memory(
            Box::new(input(vec![Some(1), Some(1)], vec!["a", "b"], "left_name")),
            Box::new(input(vec![Some(1), Some(1)], vec!["x", "y"], "right_name")),
            JoinType::Inner,
            vec![("id".into(), "id".into())],
            None,
            None,
            account,
        )
        .unwrap();

        let error = join.next_batch().unwrap_err().to_string();
        assert!(error.contains("query 'bounded-join' operator 'hash-join'"));
        drop(join);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.snapshot().peak_bytes <= pool.snapshot().limit_bytes);
    }

    #[test]
    fn composite_runtime_filter_exposes_safe_independent_storage_ins() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("left_a", DataType::Int32, true),
            Field::new("left_b", DataType::Utf8, true),
        ]));
        let build_schema = Arc::new(Schema::new(vec![
            Field::new("right_a", DataType::Int32, true),
            Field::new("right_b", DataType::Utf8, true),
        ]));
        let build = RecordBatch::try_new(
            build_schema.clone(),
            vec![
                Arc::new(arrow::array::Int32Array::from(vec![Some(1), Some(2)])),
                Arc::new(arrow::array::StringArray::from(vec![Some("x"), Some("y")])),
            ],
        )
        .unwrap();
        let built = BuiltSide::build(
            &build_schema,
            &[build],
            &[(0, 0), (1, 1)],
            JoinType::Inner,
            None,
        )
        .unwrap();
        let filter = BuildKeyFilter::from_built(&built, &[(0, 0), (1, 1)]).unwrap();
        let predicate = filter.storage_predicate(&schema).unwrap();
        let StoragePredicate::And(predicates) = predicate else {
            panic!("expected independent composite-key predicates");
        };
        assert_eq!(predicates.len(), 2);
        assert!(
            predicates
                .iter()
                .all(|predicate| matches!(predicate, StoragePredicate::In { .. }))
        );
    }

    #[test]
    fn single_key_runtime_filter_storage_in_excludes_duplicate_build_values() {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
        let build = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int64Array::from(vec![
                Some(1),
                Some(1),
                Some(2),
                None,
            ]))],
        )
        .unwrap();
        let built = BuiltSide::build(&schema, &[build], &[(0, 0)], JoinType::Inner, None).unwrap();
        let filter = BuildKeyFilter::from_built(&built, &[(0, 0)]).unwrap();
        let StoragePredicate::In { values, .. } = filter.storage_predicate(&schema).unwrap() else {
            panic!("expected single-key IN predicate");
        };
        assert_eq!(values.len(), 2);
        assert!(values.contains(&ScalarValue::Int64(1)));
        assert!(values.contains(&ScalarValue::Int64(2)));
    }

    #[test]
    fn composite_runtime_filter_excludes_null_build_keys() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("left_a", DataType::Int32, true),
            Field::new("left_b", DataType::Utf8, true),
        ]));
        let build_schema = Arc::new(Schema::new(vec![
            Field::new("right_a", DataType::Int32, true),
            Field::new("right_b", DataType::Utf8, true),
        ]));
        let build = RecordBatch::try_new(
            build_schema.clone(),
            vec![
                Arc::new(arrow::array::Int32Array::from(vec![Some(1), None, Some(2)])),
                Arc::new(arrow::array::StringArray::from(vec![
                    Some("x"),
                    Some("z"),
                    Some("y"),
                ])),
            ],
        )
        .unwrap();
        let built = BuiltSide::build(
            &build_schema,
            &[build],
            &[(0, 0), (1, 1)],
            JoinType::Inner,
            None,
        )
        .unwrap();
        let filter = BuildKeyFilter::from_built(&built, &[(0, 0), (1, 1)]).unwrap();
        let StoragePredicate::And(predicates) = filter.storage_predicate(&schema).unwrap() else {
            panic!("expected independent composite-key predicates");
        };
        assert_eq!(predicates.len(), 2);
        let StoragePredicate::In { values: ints, .. } = &predicates[0] else {
            panic!("int predicate")
        };
        assert_eq!(ints.len(), 2);
        assert!(ints.contains(&ScalarValue::Int64(1)));
        assert!(ints.contains(&ScalarValue::Int64(2)));
        let StoragePredicate::In { values: texts, .. } = &predicates[1] else {
            panic!("text predicate")
        };
        assert_eq!(texts.len(), 2);
        assert!(texts.contains(&ScalarValue::Utf8("x".into())));
        assert!(texts.contains(&ScalarValue::Utf8("y".into())));
    }

    #[test]
    fn composite_runtime_filter_prefilters_components_but_keeps_tuple_exactness() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("left_a", DataType::Int32, true),
            Field::new("left_b", DataType::Utf8, true),
        ]));
        let build_schema = Arc::new(Schema::new(vec![
            Field::new("right_a", DataType::Int32, true),
            Field::new("right_b", DataType::Utf8, true),
        ]));
        let build = RecordBatch::try_new(
            build_schema.clone(),
            vec![
                Arc::new(arrow::array::Int32Array::from(vec![Some(1), Some(2)])),
                Arc::new(arrow::array::StringArray::from(vec![Some("x"), Some("y")])),
            ],
        )
        .unwrap();
        let probe = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(arrow::array::Int32Array::from(vec![
                    Some(1),
                    Some(1),
                    Some(3),
                    None,
                ])),
                Arc::new(arrow::array::StringArray::from(vec![
                    Some("x"),
                    Some("y"),
                    Some("z"),
                    Some("x"),
                ])),
            ],
        )
        .unwrap();
        let built = BuiltSide::build(
            &build_schema,
            &[build],
            &[(0, 0), (1, 1)],
            JoinType::Inner,
            None,
        )
        .unwrap();
        let filter = BuildKeyFilter::from_built(&built, &[(0, 0), (1, 1)]).unwrap();
        assert_eq!(
            filter
                .filter_batch(&probe)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(true), Some(false), Some(false), Some(false)]
        );
    }
}
