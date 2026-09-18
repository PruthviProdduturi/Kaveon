//! Semi and anti joins: rows of the left input with (semi) or without
//! (anti) a match in the right input on one key. The right input is built
//! first — as a set of keys, or, under a residual predicate, as its rows
//! by key — and the left input is probed batch by batch.
//!
//! A residual is the rest of a correlated `EXISTS`: `EXISTS (SELECT * FROM
//! lineitem l2 WHERE l2.l_orderkey = l1.l_orderkey AND l2.l_suppkey <>
//! l1.l_suppkey)` is a semi join on `l_orderkey` whose residual
//! `l2.l_suppkey <> l1.l_suppkey` is evaluated over every (left row, right
//! row) pair sharing the key; a left row matches when any pair holds. The
//! build side keeps only the columns the residual reads, and the pairs of
//! a probe batch are evaluated in bounded chunks, so a key shared by many
//! rows on both sides never materialises its whole product at once.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch, StringArray, UInt32Array};
use arrow::compute::{interleave, take};
use arrow::datatypes::{Float64Type, Int32Type, Int64Type, Schema, SchemaRef, UInt64Type};
use kaveon_core::{
    BatchOperator, Expr, KaveonError, OperatorMemoryAccount, ReservationSlab, Result,
};

#[derive(Debug, PartialEq, Eq, Hash)]
enum Key {
    Bool(bool),
    Number(i128, i8),
    Float(u64),
    Text(String),
}

/// The pairs a residual evaluates at once: bounds the pair batch whatever
/// the key's fan-out on either side.
const RESIDUAL_PAIR_CHUNK: usize = 8192;

/// Bytes charged per retained build row reference and per key entry.
const BUILD_ROW_REFERENCE_BYTES: u64 = 8;
const BUILD_KEY_BYTES: u64 = 128;

/// A predicate over (left row, right row) pairs sharing a key, resolved
/// against the two inputs' schemas once: which columns of each side it
/// reads, and the schema of the pair batch it is evaluated over (the left
/// columns, then the right).
struct Residual {
    predicate: Expr,
    left_columns: Vec<usize>,
    right_columns: Vec<usize>,
    schema: SchemaRef,
}

/// The built right input.
enum Build {
    /// The keys, for a join without a residual.
    Keys(HashSet<Key>),
    /// The rows by key, for a join with a residual: `(batch, row)` into
    /// `batches`, which hold only the residual's right columns.
    Rows {
        index: HashMap<Key, Vec<(u32, u32)>>,
        batches: Vec<RecordBatch>,
        /// Bytes of the retained batches per row, for sizing pair batches.
        bytes_per_row: u64,
    },
}

pub struct SemiJoinOperator {
    left: Box<dyn BatchOperator>,
    right: Box<dyn BatchOperator>,
    left_key: Expr,
    right_key: Expr,
    anti: bool,
    residual: Option<Residual>,
    build: Option<Build>,
    right_has_null: bool,
    memory: Option<OperatorMemoryAccount>,
    reservations: ReservationSlab,
}

impl SemiJoinOperator {
    pub fn new(
        left: Box<dyn BatchOperator>,
        right: Box<dyn BatchOperator>,
        mut left_key: Expr,
        right_key: Expr,
        anti: bool,
    ) -> Result<Self> {
        // The right key is a literal (an uncorrelated EXISTS: existence
        // alone), the subquery's sole column by position (`*`, an IN
        // subquery), or a column by name (a correlated EXISTS, whose
        // subquery projects its key beside the columns a residual reads).
        let mut right_key = match right_key {
            Expr::Column(name) if name == "*" => {
                if right.schema().fields().len() != 1 {
                    return Err(KaveonError::Execution(
                        "IN subquery must return exactly one column".into(),
                    ));
                }
                Expr::Column(right.schema().field(0).name().clone())
            }
            other => other,
        };
        // Keys compare by value; a dictionary-encoded side is its value type.
        let left_type = logical_type(
            crate::expr_eval::evaluate(&left_key, &RecordBatch::new_empty(left.schema().clone()))?
                .data_type(),
        );
        let right_type = logical_type(
            crate::expr_eval::evaluate(
                &right_key,
                &RecordBatch::new_empty(right.schema().clone()),
            )?
            .data_type(),
        );
        if left_type.is_numeric()
            && right_type.is_numeric()
            && (left_type == arrow::datatypes::DataType::Float64
                || right_type == arrow::datatypes::DataType::Float64)
        {
            left_key = Expr::Cast {
                expr: Box::new(left_key),
                data_type: kaveon_core::CastTarget::Float64,
            };
            right_key = Expr::Cast {
                expr: Box::new(right_key),
                data_type: kaveon_core::CastTarget::Float64,
            };
        } else if left_type != right_type
            && !(left_type.is_numeric() && right_type.is_numeric())
            && left_type != arrow::datatypes::DataType::Null
            && right_type != arrow::datatypes::DataType::Null
        {
            return Err(KaveonError::Execution(format!(
                "incompatible IN key types: {left_type} and {right_type}"
            )));
        }
        Ok(Self {
            left,
            right,
            left_key,
            right_key,
            anti,
            residual: None,
            build: None,
            right_has_null: false,
            memory: None,
            reservations: ReservationSlab::default(),
        })
    }

    pub fn with_memory(mut self, memory: OperatorMemoryAccount) -> Self {
        self.memory = Some(memory);
        self
    }

    /// Evaluate `predicate` over each (left row, right row) pair sharing a
    /// key; a left row matches when any pair holds. Every column the
    /// predicate reads must resolve on exactly one side.
    pub fn with_residual(mut self, predicate: Expr) -> Result<Self> {
        let mut references = Vec::new();
        crate::expr_eval::column_references(&predicate, &mut references);
        references.sort_unstable();
        references.dedup();
        let (left_schema, right_schema) = (self.left.schema(), self.right.schema());
        let mut left_columns = Vec::new();
        let mut right_columns = Vec::new();
        for reference in &references {
            let on_left = crate::expr_eval::resolve_column_index(left_schema, reference).ok();
            let on_right = crate::expr_eval::resolve_column_index(right_schema, reference).ok();
            match (on_left, on_right) {
                (Some(index), None) => left_columns.push(index),
                (None, Some(index)) => right_columns.push(index),
                (Some(_), Some(_)) => {
                    return Err(KaveonError::Execution(format!(
                        "semi-join residual column '{reference}' resolves on both sides"
                    )));
                }
                (None, None) => {
                    return Err(KaveonError::Execution(format!(
                        "semi-join residual column '{reference}' resolves on neither side"
                    )));
                }
            }
        }
        left_columns.sort_unstable();
        left_columns.dedup();
        right_columns.sort_unstable();
        right_columns.dedup();
        let fields = left_columns
            .iter()
            .map(|index| left_schema.field(*index).clone())
            .chain(
                right_columns
                    .iter()
                    .map(|index| right_schema.field(*index).clone()),
            )
            .collect::<Vec<_>>();
        let mut names = fields.iter().map(|field| field.name()).collect::<Vec<_>>();
        names.sort_unstable();
        if names.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(KaveonError::Execution(
                "semi-join residual reads a column named alike on both sides".into(),
            ));
        }
        self.residual = Some(Residual {
            predicate,
            left_columns,
            right_columns,
            schema: Arc::new(Schema::new(fields)),
        });
        Ok(self)
    }

    fn build_right(&mut self) -> Result<()> {
        let build = match &self.residual {
            None => Build::Keys(self.build_keys()?),
            Some(_) => self.build_rows()?,
        };
        self.build = Some(build);
        Ok(())
    }

    fn build_keys(&mut self) -> Result<HashSet<Key>> {
        let mut keys = HashSet::new();
        while let Some(batch) = self.right.next_batch()? {
            let _workspace = self.workspace(&batch, 3)?;
            let col = crate::expr_eval::evaluate(&self.right_key, &batch)?;
            for row in 0..batch.num_rows() {
                if row % 1024 == 0 {
                    crate::expr_eval::check_expression_cancelled()?;
                }
                if is_null_at(col.as_ref(), row) {
                    self.right_has_null = true;
                    continue;
                }
                let key = extract_value(col.as_ref(), row)?;
                if !keys.contains(&key) {
                    if let Some(memory) = &self.memory {
                        self.reservations.reserve(memory, key_bytes(&key))?;
                    }
                    keys.insert(key);
                }
            }
        }
        Ok(keys)
    }

    /// The right rows by key, holding only the residual's columns; a row
    /// whose key is NULL matches nothing and is dropped.
    fn build_rows(&mut self) -> Result<Build> {
        let residual = self.residual.as_ref().expect("a residual");
        let mut index: HashMap<Key, Vec<(u32, u32)>> = HashMap::new();
        let mut batches: Vec<RecordBatch> = Vec::new();
        let mut retained_bytes = 0u64;
        let mut retained_rows = 0u64;
        while let Some(batch) = self.right.next_batch()? {
            let _workspace = self.workspace(&batch, 3)?;
            let col = crate::expr_eval::evaluate(&self.right_key, &batch)?;
            let kept = batch.project(&residual.right_columns).map_err(|error| {
                KaveonError::Execution(format!("semi-join build projection: {error}"))
            })?;
            let batch_index = u32::try_from(batches.len()).map_err(|_| {
                KaveonError::Execution("semi-join build side exceeds the batch limit".into())
            })?;
            let bytes = kept.get_array_memory_size() as u64;
            if let Some(memory) = &self.memory {
                self.reservations
                    .reserve(memory, bytes.saturating_add(64))?;
            }
            retained_bytes = retained_bytes.saturating_add(bytes);
            retained_rows = retained_rows.saturating_add(kept.num_rows() as u64);
            for row in 0..batch.num_rows() {
                if row % 1024 == 0 {
                    crate::expr_eval::check_expression_cancelled()?;
                }
                if is_null_at(col.as_ref(), row) {
                    self.right_has_null = true;
                    continue;
                }
                let key = extract_value(col.as_ref(), row)?;
                let reference = (batch_index, row as u32);
                match index.get_mut(&key) {
                    Some(rows) => {
                        if let Some(memory) = &self.memory {
                            self.reservations
                                .reserve(memory, BUILD_ROW_REFERENCE_BYTES)?;
                        }
                        rows.push(reference);
                    }
                    None => {
                        if let Some(memory) = &self.memory {
                            self.reservations.reserve(
                                memory,
                                key_bytes(&key).saturating_add(BUILD_ROW_REFERENCE_BYTES),
                            )?;
                        }
                        index.insert(key, vec![reference]);
                    }
                }
            }
            batches.push(kept);
        }
        Ok(Build::Rows {
            index,
            batches,
            bytes_per_row: retained_bytes
                .checked_div(retained_rows)
                .unwrap_or(0)
                .max(1),
        })
    }

    fn workspace(
        &self,
        batch: &RecordBatch,
        factor: u64,
    ) -> Result<Option<kaveon_core::MemoryReservation>> {
        self.memory
            .as_ref()
            .map(|memory| {
                memory.reserve(
                    (batch.get_array_memory_size() as u64)
                        .saturating_mul(factor)
                        .saturating_add((batch.num_rows() as u64).saturating_mul(32)),
                )
            })
            .transpose()
    }
}

impl BatchOperator for SemiJoinOperator {
    fn schema(&self) -> &SchemaRef {
        self.left.schema()
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let expression_memory = self.memory.clone();
        crate::expr_eval::with_expression_memory(expression_memory.as_ref(), || {
            if self.build.is_none() {
                self.build_right()?;
            }

            loop {
                let batch = match self.left.next_batch()? {
                    Some(b) => b,
                    None => {
                        self.build = Some(Build::Keys(HashSet::new()));
                        self.reservations.clear();
                        return Ok(None);
                    }
                };

                let _workspace = self.workspace(&batch, 4)?;

                let col = crate::expr_eval::evaluate(&self.left_key, &batch)?;
                let build = self.build.as_ref().expect("built before probing");
                let indices = match (build, &self.residual) {
                    (Build::Keys(keys), _) => {
                        probe_keys(keys, self.right_has_null, self.anti, col)?
                    }
                    (
                        Build::Rows {
                            index,
                            batches,
                            bytes_per_row,
                        },
                        Some(residual),
                    ) => probe_rows(
                        residual,
                        index,
                        batches,
                        *bytes_per_row,
                        self.anti,
                        &batch,
                        col,
                        self.memory.as_ref(),
                    )?,
                    (Build::Rows { .. }, None) => {
                        return Err(KaveonError::Execution(
                            "semi-join build rows without a residual".into(),
                        ));
                    }
                };

                if indices.is_empty() {
                    continue;
                }

                let idx_array = UInt32Array::from(indices);
                let columns: Vec<ArrayRef> = batch
                    .columns()
                    .iter()
                    .map(|c| take(c.as_ref(), &idx_array, None))
                    .collect::<std::result::Result<_, _>>()
                    .map_err(|e| KaveonError::Execution(format!("semi-join take: {e}")))?;

                let result = RecordBatch::try_new(batch.schema(), columns)
                    .map_err(|e| KaveonError::Execution(format!("semi-join batch: {e}")))?;
                return Ok(Some(result));
            }
        })
    }
}

/// The rows of a probe batch to emit, by key membership. NOT IN
/// semantics on NULLs: a NULL probe key is unknown against a non-empty
/// set, and a NULL in the set empties an anti join.
fn probe_keys(
    keys: &HashSet<Key>,
    right_has_null: bool,
    anti: bool,
    col: ArrayRef,
) -> Result<Vec<u32>> {
    // A dictionary column is probed once per distinct value, then every
    // row reads its value's verdict.
    let (col, verdicts, dictionary_keys) = match col.data_type() {
        arrow::datatypes::DataType::Dictionary(key_type, _)
            if key_type.as_ref() == &arrow::datatypes::DataType::Int32 =>
        {
            let dictionary = col.as_dictionary::<Int32Type>();
            let values = dictionary.values().clone();
            let verdicts = (0..values.len())
                .map(|index| {
                    if values.is_null(index) {
                        Ok(false)
                    } else {
                        Ok(keys.contains(&extract_value(values.as_ref(), index)?))
                    }
                })
                .collect::<Result<Vec<bool>>>()?;
            (values, Some(verdicts), Some(dictionary.keys().clone()))
        }
        _ => (col, None, None),
    };
    let rows = match &dictionary_keys {
        Some(dictionary_keys) => dictionary_keys.len(),
        None => col.len(),
    };
    let mut indices = Vec::new();
    for row in 0..rows {
        let is_null = match &dictionary_keys {
            Some(dictionary_keys) => {
                dictionary_keys.is_null(row) || col.is_null(dictionary_keys.value(row) as usize)
            }
            None => col.is_null(row),
        };
        if is_null {
            if anti && keys.is_empty() && !right_has_null {
                indices.push(row as u32);
            }
            continue;
        }
        let found = match (&verdicts, &dictionary_keys) {
            (Some(verdicts), Some(dictionary_keys)) => {
                verdicts[dictionary_keys.value(row) as usize]
            }
            _ => keys.contains(&extract_value(col.as_ref(), row)?),
        };
        if (found && !anti) || (!found && anti && !right_has_null) {
            indices.push(row as u32);
        }
    }
    Ok(indices)
}

/// The rows of a probe batch to emit under a residual: a row matches
/// when any build row sharing its key satisfies the residual with it.
/// EXISTS semantics on NULLs: a NULL probe key shares no key, so it
/// matches nothing (kept by an anti join, dropped by a semi join).
#[allow(clippy::too_many_arguments)]
fn probe_rows(
    residual: &Residual,
    index: &HashMap<Key, Vec<(u32, u32)>>,
    batches: &[RecordBatch],
    build_bytes_per_row: u64,
    anti: bool,
    batch: &RecordBatch,
    col: ArrayRef,
    memory: Option<&OperatorMemoryAccount>,
) -> Result<Vec<u32>> {
    let rows = batch.num_rows();
    let mut matched = vec![false; rows];
    let mut pairs: Vec<(u32, (u32, u32))> = Vec::new();
    let left_bytes_per_row = residual
        .left_columns
        .iter()
        .map(|column| batch.column(*column).get_array_memory_size() as u64)
        .sum::<u64>()
        .checked_div(rows as u64)
        .unwrap_or(0)
        .max(1);
    let pair_bytes = left_bytes_per_row.saturating_add(build_bytes_per_row);
    if !index.is_empty() {
        for row in 0..rows {
            if row % 1024 == 0 {
                crate::expr_eval::check_expression_cancelled()?;
            }
            if is_null_at(col.as_ref(), row) {
                continue;
            }
            let Some(references) = index.get(&extract_value(col.as_ref(), row)?) else {
                continue;
            };
            for reference in references {
                pairs.push((row as u32, *reference));
                if pairs.len() == RESIDUAL_PAIR_CHUNK {
                    evaluate_pairs(
                        residual,
                        batches,
                        batch,
                        &mut pairs,
                        &mut matched,
                        pair_bytes,
                        memory,
                    )?;
                }
            }
        }
        if !pairs.is_empty() {
            evaluate_pairs(
                residual,
                batches,
                batch,
                &mut pairs,
                &mut matched,
                pair_bytes,
                memory,
            )?;
        }
    }
    Ok((0..rows)
        .filter(|row| matched[*row] != anti)
        .map(|row| row as u32)
        .collect())
}

/// Evaluate the residual over `pairs` — (probe row, build row reference)
/// — and mark the probe rows with a holding pair; `pairs` is emptied.
fn evaluate_pairs(
    residual: &Residual,
    batches: &[RecordBatch],
    batch: &RecordBatch,
    pairs: &mut Vec<(u32, (u32, u32))>,
    matched: &mut [bool],
    pair_bytes: u64,
    memory: Option<&OperatorMemoryAccount>,
) -> Result<()> {
    let _workspace = memory
        .map(|memory| {
            memory.reserve(
                pair_bytes
                    .saturating_mul(pairs.len() as u64)
                    .saturating_mul(2)
                    .saturating_add((pairs.len() as u64).saturating_mul(16)),
            )
        })
        .transpose()?;
    let probe = UInt32Array::from(pairs.iter().map(|(row, _)| *row).collect::<Vec<_>>());
    let build = pairs
        .iter()
        .map(|(_, (batch, row))| (*batch as usize, *row as usize))
        .collect::<Vec<_>>();
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(residual.schema.fields().len());
    for column in &residual.left_columns {
        columns.push(
            take(batch.column(*column).as_ref(), &probe, None)
                .map_err(|e| KaveonError::Execution(format!("semi-join residual take: {e}")))?,
        );
    }
    for position in 0..residual.right_columns.len() {
        let arrays = batches
            .iter()
            .map(|batch| batch.column(position).as_ref())
            .collect::<Vec<_>>();
        columns.push(
            interleave(&arrays, &build).map_err(|e| {
                KaveonError::Execution(format!("semi-join residual interleave: {e}"))
            })?,
        );
    }
    let pair_batch = RecordBatch::try_new(residual.schema.clone(), columns)
        .map_err(|e| KaveonError::Execution(format!("semi-join residual batch: {e}")))?;
    let verdict = crate::expr_eval::evaluate_predicate(&residual.predicate, &pair_batch)?;
    for (position, (row, _)) in pairs.iter().enumerate() {
        if verdict.is_valid(position) && verdict.value(position) {
            matched[*row as usize] = true;
        }
    }
    pairs.clear();
    Ok(())
}

fn key_bytes(key: &Key) -> u64 {
    BUILD_KEY_BYTES.saturating_add(match key {
        Key::Text(value) => value.len() as u64,
        _ => 0,
    })
}

/// Whether the value at `row` is NULL, through a dictionary's keys and
/// values alike.
fn is_null_at(array: &dyn Array, row: usize) -> bool {
    match array.data_type() {
        arrow::datatypes::DataType::Dictionary(key_type, _)
            if key_type.as_ref() == &arrow::datatypes::DataType::Int32 =>
        {
            let dictionary = array.as_dictionary::<Int32Type>();
            dictionary.keys().is_null(row)
                || dictionary
                    .values()
                    .is_null(dictionary.keys().value(row) as usize)
        }
        _ => array.is_null(row),
    }
}
fn logical_type(data_type: &arrow::datatypes::DataType) -> arrow::datatypes::DataType {
    match data_type {
        arrow::datatypes::DataType::Dictionary(_, values) => values.as_ref().clone(),
        other => other.clone(),
    }
}

fn extract_value(array: &dyn Array, row: usize) -> Result<Key> {
    match array.data_type() {
        arrow::datatypes::DataType::Dictionary(key_type, _)
            if key_type.as_ref() == &arrow::datatypes::DataType::Int32 =>
        {
            let dictionary = array.as_dictionary::<Int32Type>();
            extract_value(
                dictionary.values().as_ref(),
                dictionary.keys().value(row) as usize,
            )
        }
        arrow::datatypes::DataType::Boolean => Ok(Key::Bool(array.as_boolean().value(row))),
        arrow::datatypes::DataType::Int32 => Ok(Key::Number(
            array.as_primitive::<Int32Type>().value(row) as i128,
            0,
        )),
        arrow::datatypes::DataType::Int64 => Ok(Key::Number(
            array.as_primitive::<Int64Type>().value(row) as i128,
            0,
        )),
        arrow::datatypes::DataType::UInt64 => Ok(Key::Number(
            array.as_primitive::<UInt64Type>().value(row) as i128,
            0,
        )),
        arrow::datatypes::DataType::Decimal128(_, scale) => {
            let mut value = array
                .as_primitive::<arrow::datatypes::Decimal128Type>()
                .value(row);
            let mut scale = *scale;
            while scale > 0 && value % 10 == 0 {
                value /= 10;
                scale -= 1;
            }
            if scale < 0 {
                value = value
                    .checked_mul(
                        10i128
                            .checked_pow((-scale) as u32)
                            .ok_or_else(|| KaveonError::Execution("decimal key overflow".into()))?,
                    )
                    .ok_or_else(|| KaveonError::Execution("decimal key overflow".into()))?;
                scale = 0;
            }
            Ok(Key::Number(value, scale))
        }
        arrow::datatypes::DataType::Float64 => {
            let v = array.as_primitive::<Float64Type>().value(row);
            Ok(Key::Float(if v == 0.0 {
                0
            } else if v.is_nan() {
                f64::NAN.to_bits()
            } else {
                v.to_bits()
            }))
        }
        arrow::datatypes::DataType::Utf8 => Ok(Key::Text(
            array
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("Utf8")
                .value(row)
                .to_owned(),
        )),
        dt => Err(KaveonError::Execution(format!(
            "unsupported type for semi-join key: {dt}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::{
        array::Int64Array,
        datatypes::{DataType, Field, Schema},
    };
    use std::sync::Arc;
    struct Source {
        schema: SchemaRef,
        batches: std::vec::IntoIter<RecordBatch>,
    }
    impl BatchOperator for Source {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }
        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.next())
        }
    }
    fn source(values: &[Option<i64>]) -> Box<dyn BatchOperator> {
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, true)]));
        let batches = values
            .chunks(2)
            .map(|v| {
                RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(v.to_vec()))])
                    .unwrap()
            })
            .collect::<Vec<_>>()
            .into_iter();
        Box::new(Source { schema, batches })
    }
    fn run(
        left: &[Option<i64>],
        right: &[Option<i64>],
        anti: bool,
        exists: bool,
    ) -> Vec<Option<i64>> {
        let key = if exists {
            Expr::Literal(kaveon_core::predicate::ScalarValue::Int64(1))
        } else {
            Expr::Column("x".into())
        };
        let mut op =
            SemiJoinOperator::new(source(left), source(right), key.clone(), key, anti).unwrap();
        let mut values = Vec::new();
        while let Some(batch) = op.next_batch().unwrap() {
            values.extend(batch.column(0).as_primitive::<Int64Type>().iter());
        }
        values
    }
    #[test]
    fn dictionary_left_keys_probe_by_value() {
        use arrow::array::{DictionaryArray, Int32Array, StringArray};
        let dictionary_type =
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
        let left_schema = Arc::new(Schema::new(vec![Field::new(
            "country",
            dictionary_type,
            true,
        )]));
        let left = RecordBatch::try_new(
            left_schema.clone(),
            vec![Arc::new(DictionaryArray::<Int32Type>::new(
                Int32Array::from(vec![Some(0), Some(1), None, Some(2), Some(1)]),
                Arc::new(StringArray::from(vec![Some("Japan"), None, Some("Kenya")])),
            ))],
        )
        .unwrap();
        let right_schema = Arc::new(Schema::new(vec![Field::new(
            "country",
            DataType::Utf8,
            true,
        )]));
        let right = RecordBatch::try_new(
            right_schema.clone(),
            vec![Arc::new(StringArray::from(vec!["Kenya", "Brazil"]))],
        )
        .unwrap();
        let run = |anti: bool| {
            let mut op = SemiJoinOperator::new(
                Box::new(Source {
                    schema: left_schema.clone(),
                    batches: vec![left.clone()].into_iter(),
                }),
                Box::new(Source {
                    schema: right_schema.clone(),
                    batches: vec![right.clone()].into_iter(),
                }),
                Expr::Column("country".into()),
                Expr::Column("country".into()),
                anti,
            )
            .unwrap();
            let mut rows = Vec::new();
            while let Some(batch) = op.next_batch().unwrap() {
                let column = arrow::compute::cast(batch.column(0), &DataType::Utf8).unwrap();
                rows.extend(
                    column
                        .as_string::<i32>()
                        .iter()
                        .map(|value| value.map(str::to_owned)),
                );
            }
            rows
        };
        assert_eq!(run(false), vec![Some("Kenya".to_owned())]);
        assert_eq!(run(true), vec![Some("Japan".to_owned())]);
    }

    #[test]
    fn not_in_null_truth_table_across_batches() {
        let left = [Some(1), Some(2), None, Some(2)];
        assert_eq!(
            run(&left, &[Some(1), None], true, false),
            Vec::<Option<i64>>::new()
        );
        assert_eq!(run(&left, &[Some(1)], true, false), vec![Some(2), Some(2)]);
        assert_eq!(run(&left, &[], true, false), left);
        assert_eq!(
            run(&left, &[Some(2), Some(2), None], false, false),
            vec![Some(2), Some(2)]
        );
    }
    #[test]
    fn exists_checks_cardinality_and_preserves_outer_duplicates_nulls() {
        let left = [Some(1), Some(2), None, Some(2)];
        assert_eq!(run(&left, &[None], false, true), left);
        assert_eq!(
            run(&left, &[Some(99), None], true, true),
            Vec::<Option<i64>>::new()
        );
        assert_eq!(run(&left, &[], true, true), left);
        assert_eq!(run(&left, &[], false, true), Vec::<Option<i64>>::new());
    }
    #[test]
    fn numeric_keys_are_exact_across_widths_and_zero_signs() {
        assert_eq!(
            extract_value(&arrow::array::Int32Array::from(vec![42]), 0).unwrap(),
            extract_value(&Int64Array::from(vec![42]), 0).unwrap()
        );
        assert_ne!(
            extract_value(&arrow::array::UInt64Array::from(vec![u64::MAX]), 0).unwrap(),
            extract_value(&Int64Array::from(vec![-1]), 0).unwrap()
        );
        assert_eq!(
            extract_value(&arrow::array::Float64Array::from(vec![-0.0]), 0).unwrap(),
            extract_value(&arrow::array::Float64Array::from(vec![0.0]), 0).unwrap()
        );
    }

    /// Two-column integer rows, `rows_per_batch` at a time.
    fn pairs_source(
        names: [&str; 2],
        rows: &[(Option<i64>, i64)],
        rows_per_batch: usize,
    ) -> Box<dyn BatchOperator> {
        let schema = Arc::new(Schema::new(vec![
            Field::new(names[0], DataType::Int64, true),
            Field::new(names[1], DataType::Int64, false),
        ]));
        let batches = rows
            .chunks(rows_per_batch.max(1))
            .map(|chunk| {
                RecordBatch::try_new(
                    schema.clone(),
                    vec![
                        Arc::new(Int64Array::from(
                            chunk.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
                        )),
                        Arc::new(Int64Array::from(
                            chunk.iter().map(|(_, value)| *value).collect::<Vec<_>>(),
                        )),
                    ],
                )
                .unwrap()
            })
            .collect::<Vec<_>>()
            .into_iter();
        Box::new(Source { schema, batches })
    }

    fn not_equal(left: &str, right: &str) -> Expr {
        Expr::BinaryOp {
            left: Box::new(Expr::Column(left.into())),
            op: kaveon_core::BinaryOp::Ne,
            right: Box::new(Expr::Column(right.into())),
        }
    }

    fn drain(mut op: SemiJoinOperator) -> Vec<(Option<i64>, i64)> {
        let mut rows = Vec::new();
        while let Some(batch) = op.next_batch().unwrap() {
            let keys = batch.column(0).as_primitive::<Int64Type>();
            let values = batch.column(1).as_primitive::<Int64Type>();
            for row in 0..batch.num_rows() {
                rows.push((
                    (!keys.is_null(row)).then(|| keys.value(row)),
                    values.value(row),
                ));
            }
        }
        rows
    }

    /// Q21's shape: a line item qualifies when another supplier's line
    /// shares its order (semi) or when none does (anti). A NULL probe key
    /// shares no key, so the anti join keeps it and the semi join drops it.
    #[test]
    fn a_residual_semi_join_keeps_a_left_row_when_any_pair_holds() {
        let left = [
            (Some(1), 10),
            (Some(1), 11),
            (Some(2), 20),
            (Some(3), 30),
            (None, 40),
        ];
        let right = [
            (Some(1), 10),
            (Some(1), 11),
            (Some(2), 20),
            (Some(3), 30),
            (Some(3), 30),
            (None, 99),
        ];
        let run = |anti: bool| {
            let op = SemiJoinOperator::new(
                pairs_source(["l_orderkey", "l_suppkey"], &left, 2),
                pairs_source(["l2.l_orderkey", "__kaveon_corr_0"], &right, 2),
                Expr::Column("l_orderkey".into()),
                Expr::Column("l2.l_orderkey".into()),
                anti,
            )
            .unwrap()
            .with_residual(not_equal("__kaveon_corr_0", "l_suppkey"))
            .unwrap();
            drain(op)
        };
        assert_eq!(run(false), vec![(Some(1), 10), (Some(1), 11)]);
        assert_eq!(run(true), vec![(Some(2), 20), (Some(3), 30), (None, 40)]);
    }

    /// More pairs than one chunk holds, spread over several build batches:
    /// the row whose only holding pair is the last one still matches, and
    /// the row whose only holding pair is the first still matches.
    #[test]
    fn residual_pairs_are_evaluated_in_chunks_across_build_batches() {
        let build_rows = RESIDUAL_PAIR_CHUNK as i64 / 2 + 7;
        let left = [
            (Some(1), build_rows - 1),
            (Some(1), build_rows - 2),
            (Some(1), -1),
        ];
        let right = (0..build_rows)
            .map(|value| (Some(1), value))
            .collect::<Vec<_>>();
        let greater = Expr::BinaryOp {
            left: Box::new(Expr::Column("corr".into())),
            op: kaveon_core::BinaryOp::Gt,
            right: Box::new(Expr::Column("value".into())),
        };
        let pool = kaveon_core::QueryMemoryPool::new("residual", 64 * 1024 * 1024).unwrap();
        let op = SemiJoinOperator::new(
            pairs_source(["key", "value"], &left, 8),
            pairs_source(["key", "corr"], &right, 1000),
            Expr::Column("key".into()),
            Expr::Column("key".into()),
            false,
        )
        .unwrap()
        .with_residual(greater)
        .unwrap()
        .with_memory(pool.operator("semi-join").unwrap());
        assert_eq!(drain(op), vec![(Some(1), build_rows - 2), (Some(1), -1)]);
        assert_eq!(pool.snapshot().current_bytes, 0, "reservations released");
    }

    #[test]
    fn a_residual_build_side_that_exceeds_the_budget_fails_closed() {
        let right = (0..4_000)
            .map(|value| (Some(value), value))
            .collect::<Vec<_>>();
        let pool = kaveon_core::QueryMemoryPool::new("residual-budget", 16 * 1024).unwrap();
        let mut op = SemiJoinOperator::new(
            pairs_source(["key", "value"], &[(Some(1), 1)], 8),
            pairs_source(["key", "corr"], &right, 500),
            Expr::Column("key".into()),
            Expr::Column("key".into()),
            false,
        )
        .unwrap()
        .with_residual(not_equal("corr", "value"))
        .unwrap()
        .with_memory(pool.operator("semi-join").unwrap());
        assert!(matches!(op.next_batch(), Err(KaveonError::MemoryLimit(_))));
        drop(op);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn a_residual_column_must_resolve_on_exactly_one_side() {
        let build = || {
            SemiJoinOperator::new(
                pairs_source(["key", "value"], &[(Some(1), 1)], 8),
                pairs_source(["key", "corr"], &[(Some(1), 2)], 8),
                Expr::Column("key".into()),
                Expr::Column("key".into()),
                false,
            )
            .unwrap()
        };
        let error = |result: Result<SemiJoinOperator>| match result {
            Ok(_) => panic!("the residual was accepted"),
            Err(error) => error.to_string(),
        };
        let neither = error(build().with_residual(not_equal("corr", "missing")));
        assert!(neither.contains("neither side"), "{neither}");
        let both = error(build().with_residual(not_equal("corr", "key")));
        assert!(both.contains("both sides"), "{both}");
    }

    /// The build side's residual column is dictionary text with a
    /// dictionary per batch; the pairs gather across them by value.
    #[test]
    fn residual_text_columns_gather_across_build_dictionaries() {
        use arrow::array::{DictionaryArray, Int32Array};
        let left_schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Int64, true),
            Field::new("name", DataType::Utf8, false),
        ]));
        let left = RecordBatch::try_new(
            left_schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 1, 2])),
                Arc::new(StringArray::from(vec!["Japan", "Kenya", "Brazil"])),
            ],
        )
        .unwrap();
        let dictionary_type =
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
        let right_schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Int64, true),
            Field::new("corr", dictionary_type, true),
        ]));
        let right_batch = |keys: Vec<i64>, codes: Vec<Option<i32>>, values: Vec<&str>| {
            RecordBatch::try_new(
                right_schema.clone(),
                vec![
                    Arc::new(Int64Array::from(keys)),
                    Arc::new(DictionaryArray::<Int32Type>::new(
                        Int32Array::from(codes),
                        Arc::new(StringArray::from(values)),
                    )),
                ],
            )
            .unwrap()
        };
        let right = vec![
            right_batch(vec![1, 2], vec![Some(0), Some(1)], vec!["Japan", "Brazil"]),
            right_batch(vec![1, 2], vec![Some(0), None], vec!["Japan"]),
        ];
        let run = |anti: bool| {
            let op = SemiJoinOperator::new(
                Box::new(Source {
                    schema: left_schema.clone(),
                    batches: vec![left.clone()].into_iter(),
                }),
                Box::new(Source {
                    schema: right_schema.clone(),
                    batches: right.clone().into_iter(),
                }),
                Expr::Column("key".into()),
                Expr::Column("key".into()),
                anti,
            )
            .unwrap()
            .with_residual(not_equal("corr", "name"))
            .unwrap();
            let mut names = Vec::new();
            let mut op = op;
            while let Some(batch) = op.next_batch().unwrap() {
                names.extend(
                    batch
                        .column(1)
                        .as_string::<i32>()
                        .iter()
                        .map(|value| value.unwrap().to_owned()),
                );
            }
            names
        };
        // Kenya (key 1) differs from Japan; Japan (key 1) matches only
        // Japan rows; Brazil (key 2) meets Brazil and a NULL, neither of
        // which holds.
        assert_eq!(run(false), vec!["Kenya".to_owned()]);
        assert_eq!(run(true), vec!["Japan".to_owned(), "Brazil".to_owned()]);
    }
}
