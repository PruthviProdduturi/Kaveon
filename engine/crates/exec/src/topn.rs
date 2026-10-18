use std::collections::{HashMap, VecDeque};

use arrow::array::{
    Array, Decimal128Array, Float32Array, Int8Array, Int16Array, Int32Array, Int64Array,
    UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow::compute::{SortColumn, SortOptions, concat_batches, lexsort_to_indices, take};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, KaveonError, MemoryReservation, OperatorMemoryAccount, Result};

use crate::expr_eval::evaluate;
use crate::sort::{ExternalMerge, SortExpr, compact_runs};
use crate::spill::SpillManager;

pub struct TopNOperator {
    source: Box<dyn BatchOperator>,
    sort_exprs: Vec<SortExpr>,
    limit: usize,
    schema: SchemaRef,
    output: VecDeque<RecordBatch>,
    initialized: bool,
    spill: Option<(OperatorMemoryAccount, SpillManager)>,
    external_merge: Option<ExternalMerge>,
    merge_fan_in: usize,
    memory: Option<OperatorMemoryAccount>,
    output_memory: Vec<MemoryReservation>,
}

const DEFAULT_MERGE_FAN_IN: usize = 16;
const MINIMUM_MERGE_FAN_IN: usize = 2;

impl TopNOperator {
    pub fn new(
        source: Box<dyn BatchOperator>,
        sort_exprs: Vec<SortExpr>,
        limit: usize,
    ) -> Result<Self> {
        if sort_exprs.is_empty() {
            return Err(KaveonError::Execution(
                "TopN requires at least one ordering expression".into(),
            ));
        }
        let schema = source.schema().clone();
        Ok(Self {
            source,
            sort_exprs,
            limit,
            schema,
            output: VecDeque::new(),
            initialized: false,
            spill: None,
            external_merge: None,
            merge_fan_in: DEFAULT_MERGE_FAN_IN,
            memory: None,
            output_memory: Vec::new(),
        })
    }

    pub fn new_with_spill(
        source: Box<dyn BatchOperator>,
        sort_exprs: Vec<SortExpr>,
        limit: usize,
        memory: OperatorMemoryAccount,
        spill: SpillManager,
    ) -> Result<Self> {
        let mut operator = Self::new(source, sort_exprs, limit)?;
        operator.memory = Some(memory.clone());
        operator.spill = Some((memory, spill));
        Ok(operator)
    }

    pub fn with_memory(mut self, memory: OperatorMemoryAccount) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn with_merge_fan_in(mut self, merge_fan_in: usize) -> Result<Self> {
        if merge_fan_in < MINIMUM_MERGE_FAN_IN {
            return Err(KaveonError::Execution(format!(
                "TopN spill merge fan-in must be at least {MINIMUM_MERGE_FAN_IN}"
            )));
        }
        self.merge_fan_in = merge_fan_in;
        Ok(self)
    }

    fn initialize(&mut self) -> Result<()> {
        if self.limit == 0 {
            return Ok(());
        }
        let mut batches = Vec::new();
        let mut reservations = Vec::new();
        let mut retained_bytes = 0_u64;
        let mut runs = Vec::new();
        while let Some(batch) = self.source.next_batch()? {
            if batch.num_rows() == 0 {
                continue;
            }
            let bytes = crate::sort::sort_workspace_bytes(&batch);
            if let Some((memory, spill)) = &self.spill {
                let target = memory.query().snapshot().limit_bytes / (self.merge_fan_in as u64 + 2);
                if !batches.is_empty() && retained_bytes.saturating_add(bytes) > target {
                    let candidate =
                        merge_top_n(&self.schema, &batches, &self.sort_exprs, self.limit)?
                            .expect("nonempty candidates");
                    runs.push(spill.write_run(&self.schema, &[candidate])?);
                    batches.clear();
                    reservations.clear();
                    retained_bytes = 0;
                    if runs.len() >= self.merge_fan_in.saturating_mul(2) {
                        runs = compact_runs(
                            runs,
                            spill,
                            self.schema.clone(),
                            &self.sort_exprs,
                            self.limit,
                            self.merge_fan_in,
                            Some(self.limit),
                            self.memory.as_ref(),
                        )?;
                    }
                }
            }
            let workspace = self
                .memory
                .as_ref()
                .map(|memory| memory.reserve(bytes))
                .transpose()?;
            let candidate = merge_top_n(&self.schema, &[batch], &self.sort_exprs, self.limit)?
                .expect("nonempty input");
            drop(workspace);
            let candidate_bytes = crate::sort::sort_workspace_bytes(&candidate);
            if let Some(memory) = &self.memory {
                reservations.push(memory.reserve(candidate_bytes)?);
            }
            retained_bytes = retained_bytes.saturating_add(candidate_bytes);
            batches.push(candidate);
            if self.spill.is_none() && batches.len() >= self.merge_fan_in {
                // Keep at most fan-in candidate batches. Without compaction,
                // even LIMIT 1 accumulates state proportional to input batches.
                let candidate = merge_top_n(&self.schema, &batches, &self.sort_exprs, self.limit)?
                    .expect("nonempty candidates");
                let bytes = crate::sort::sort_workspace_bytes(&candidate);
                let retained = self
                    .memory
                    .as_ref()
                    .map(|memory| memory.reserve(bytes))
                    .transpose()?;
                batches.clear();
                reservations.clear();
                retained_bytes = bytes;
                batches.push(candidate);
                if let Some(retained) = retained {
                    reservations.push(retained);
                }
            }
        }
        if batches.is_empty() && runs.is_empty() {
            return Ok(());
        }
        if !runs.is_empty() {
            let spill = &self.spill.as_ref().expect("spill mode active").1;
            if !batches.is_empty() {
                let candidate = merge_top_n(&self.schema, &batches, &self.sort_exprs, self.limit)?
                    .expect("nonempty candidates");
                runs.push(spill.write_run(&self.schema, &[candidate])?);
            }
            batches.clear();
            reservations.clear();
            let runs = compact_runs(
                runs,
                spill,
                self.schema.clone(),
                &self.sort_exprs,
                self.limit,
                self.merge_fan_in,
                Some(self.limit),
                self.memory.as_ref(),
            )?;
            self.external_merge = Some(ExternalMerge::new(
                runs,
                self.schema.clone(),
                &self.sort_exprs,
                self.limit,
                Some(self.limit),
                self.memory.clone(),
            )?);
            return Ok(());
        }
        if let Some(batch) = merge_top_n(&self.schema, &batches, &self.sort_exprs, self.limit)? {
            self.output.push_back(batch);
        }
        self.output_memory = reservations;
        Ok(())
    }
}

/// Merges partition-local TopN batches into the globally ordered TopN result.
///
/// Each input partition only needs to contribute its first `limit` rows. A row
/// ranked below that boundary cannot appear in the global TopN because at least
/// `limit` rows in its own partition already rank ahead of it.
pub fn merge_top_n(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    sort_exprs: &[SortExpr],
    limit: usize,
) -> Result<Option<RecordBatch>> {
    if sort_exprs.is_empty() {
        return Err(KaveonError::Execution(
            "TopN merge requires at least one ordering expression".into(),
        ));
    }
    if limit == 0 || batches.iter().all(|batch| batch.num_rows() == 0) {
        return Ok(None);
    }
    if batches.iter().any(|batch| batch.schema() != *schema) {
        return Err(KaveonError::Execution(
            "TopN merge received incompatible batch schemas".into(),
        ));
    }
    // Empty worker candidates are common after selective predicates. Remove
    // them once before the typed and sorted merge paths so key evaluation and
    // the comparator matrix scale with contributing workers only.
    let non_empty = batches
        .iter()
        .filter(|batch| batch.num_rows() > 0)
        .cloned()
        .collect::<Vec<_>>();

    // Distributed aggregate TopN commonly orders by one integer COUNT/SUM
    // column. Keep only row references while ranking those batches so the
    // final merge does not concatenate every payload column before selecting
    // the result. Unsupported expressions retain the general Arrow path.
    if sort_exprs.len() == 1
        && let Some(result) = try_integer_batch_top_n(schema, &non_empty, &sort_exprs[0], limit)?
    {
        return Ok(Some(result));
    }

    // Worker TopN operators also emit sorted batches for strings, dates,
    // decimals, and multi-key orderings.  Merge those heads without
    // concatenating the payload columns at the coordinator.  The integer
    // specialization above remains the cheaper hot path; this generic path
    // keeps Arrow's comparator semantics for every other key type.
    if let Some(result) = try_sorted_batch_top_n(schema, &non_empty, sort_exprs, limit)? {
        return Ok(Some(result));
    }

    let combined = if non_empty.len() == 1 {
        non_empty[0].clone()
    } else {
        concat_batches(schema, &non_empty)?
    };
    let columns = sort_exprs
        .iter()
        .map(|sort_expr| {
            Ok(SortColumn {
                values: evaluate(&sort_expr.expr, &combined)?,
                options: Some(SortOptions {
                    descending: !sort_expr.ascending,
                    nulls_first: sort_expr.nulls_first,
                }),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let indices = integer_top_n_indices(&columns, limit)
        .or_else(|| decimal_top_n_indices(&columns, limit))
        .or_else(|| decimal256_top_n_indices(&columns, limit))
        .map(Ok)
        .unwrap_or_else(|| lexsort_to_indices(&columns, Some(limit)))?;
    let output_columns = combined
        .columns()
        .iter()
        .map(|column| take(column.as_ref(), &indices, None))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(RecordBatch::try_new(schema.clone(), output_columns)?))
}

fn try_sorted_batch_top_n(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    sort_exprs: &[SortExpr],
    limit: usize,
) -> Result<Option<RecordBatch>> {
    let evaluated = batches
        .iter()
        .map(|batch| {
            sort_exprs
                .iter()
                .map(|sort_expr| evaluate(&sort_expr.expr, batch))
                .collect::<Result<Vec<_>>>()
        })
        .collect::<Result<Vec<_>>>()?;
    // Keep key arrays in their source batches. Concatenating every ordering
    // key at the coordinator adds a full copy before the bounded heap merge;
    // pairwise Arrow comparators preserve the same null/direction semantics
    // while comparing only the candidate rows that can enter the result.
    let comparators = evaluated
        .iter()
        .map(|left_keys| {
            evaluated
                .iter()
                .map(|right_keys| {
                    left_keys
                        .iter()
                        .zip(right_keys)
                        .zip(sort_exprs)
                        .map(|((left, right), sort_expr)| {
                            arrow::array::make_comparator(
                                left.as_ref(),
                                right.as_ref(),
                                SortOptions {
                                    descending: !sort_expr.ascending,
                                    nulls_first: sort_expr.nulls_first,
                                },
                            )
                            .map_err(|error| KaveonError::Execution(error.to_string()))
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .collect::<Result<Vec<_>>>()
        })
        .collect::<Result<Vec<_>>>()?;
    let compare = |left: (usize, usize), right: (usize, usize)| {
        for (key, _) in sort_exprs.iter().enumerate() {
            let ordering = comparators[left.0][right.0][key](left.1, right.1);
            if !ordering.is_eq() {
                return ordering;
            }
        }
        std::cmp::Ordering::Equal
    };
    let sorted = evaluated.iter().enumerate().all(|(batch, keys)| {
        (1..keys.first().map_or(0, Array::len))
            .all(|row| compare((batch, row - 1), (batch, row)) != std::cmp::Ordering::Greater)
    });
    if !sorted {
        return Ok(None);
    }

    // Keep only one head per worker in a binary heap.  The previous linear
    // scan was O(limit * workers), which made a large distributed TopN spend
    // most of its coordinator time comparing the same worker heads over and
    // over.  The heap keeps the exact comparator and tie behavior while
    // reducing the merge to O(limit * log(workers)).
    let mut heap = batches
        .iter()
        .enumerate()
        .filter_map(|(batch, input)| (input.num_rows() > 0).then_some((batch, 0)))
        .collect::<Vec<_>>();
    heap_build(&mut heap, |left, right| compare(*left, *right));
    let total_rows = batches.iter().map(RecordBatch::num_rows).sum::<usize>();
    let mut references = Vec::with_capacity(limit.min(total_rows));
    while references.len() < limit {
        let Some((batch, row)) = heap_pop(&mut heap, |left, right| compare(*left, *right)) else {
            break;
        };
        references.push((batch, row));
        let next_row = row + 1;
        if next_row < batches[batch].num_rows() {
            heap_push(&mut heap, (batch, next_row), |left, right| {
                compare(*left, *right)
            });
        }
    }
    if references.is_empty() {
        return Ok(None);
    }

    let mut selected = Vec::with_capacity(batches.len());
    let mut output_order = Vec::with_capacity(references.len());
    for (batch_index, batch) in batches.iter().enumerate() {
        let rows = references
            .iter()
            .filter_map(|(candidate_batch, row)| {
                (*candidate_batch == batch_index).then_some(*row as u32)
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            continue;
        }
        let indices = UInt32Array::from(rows);
        let columns = batch
            .columns()
            .iter()
            .map(|column| take(column.as_ref(), &indices, None))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        selected.push(RecordBatch::try_new(schema.clone(), columns)?);
    }
    let selected = concat_batches(schema, &selected)?;
    let mut selected_offsets = vec![0usize; batches.len()];
    let mut count = 0usize;
    for (batch_index, _batch) in batches.iter().enumerate() {
        selected_offsets[batch_index] = count;
        count += references
            .iter()
            .filter(|(candidate_batch, _)| *candidate_batch == batch_index)
            .count();
    }
    let mut seen = vec![0usize; batches.len()];
    for (batch, _) in &references {
        output_order.push((selected_offsets[*batch] + seen[*batch]) as u32);
        seen[*batch] += 1;
    }
    let order = UInt32Array::from(output_order);
    let columns = selected
        .columns()
        .iter()
        .map(|column| take(column.as_ref(), &order, None))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(RecordBatch::try_new(schema.clone(), columns)?))
}

/// Maintains a min-heap using a caller-provided ordering.  `std::collections`
/// cannot express the per-query Arrow comparator without allocating a wrapper
/// around it, so this small heap keeps the hot merge path allocation-free.
fn heap_build<T, F>(heap: &mut [T], compare: F)
where
    F: Fn(&T, &T) -> std::cmp::Ordering + Copy,
{
    if heap.len() < 2 {
        return;
    }
    for index in (0..heap.len() / 2).rev() {
        heap_sift_down(heap, index, &compare);
    }
}

fn heap_push<T, F>(heap: &mut Vec<T>, value: T, compare: F)
where
    F: Fn(&T, &T) -> std::cmp::Ordering,
{
    heap.push(value);
    let mut index = heap.len() - 1;
    while index > 0 {
        let parent = (index - 1) / 2;
        if compare(&heap[index], &heap[parent]) == std::cmp::Ordering::Less {
            heap.swap(index, parent);
            index = parent;
        } else {
            break;
        }
    }
}

fn heap_pop<T, F>(heap: &mut Vec<T>, compare: F) -> Option<T>
where
    F: Fn(&T, &T) -> std::cmp::Ordering,
{
    let last = heap.pop()?;
    if heap.is_empty() {
        return Some(last);
    }
    let result = std::mem::replace(&mut heap[0], last);
    heap_sift_down(heap, 0, &compare);
    Some(result)
}

fn heap_sift_down<T, F>(heap: &mut [T], mut index: usize, compare: &F)
where
    F: Fn(&T, &T) -> std::cmp::Ordering,
{
    loop {
        let left = index * 2 + 1;
        if left >= heap.len() {
            return;
        }
        let right = left + 1;
        let child = if right < heap.len()
            && compare(&heap[right], &heap[left]) == std::cmp::Ordering::Less
        {
            right
        } else {
            left
        };
        if compare(&heap[child], &heap[index]) == std::cmp::Ordering::Less {
            heap.swap(index, child);
            index = child;
        } else {
            return;
        }
    }
}

fn try_integer_batch_top_n(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    sort_expr: &SortExpr,
    limit: usize,
) -> Result<Option<RecordBatch>> {
    let mut keys = Vec::with_capacity(batches.len());
    for batch in batches {
        let values = evaluate(&sort_expr.expr, batch)?;
        let values = if let Some(values) = values.as_any().downcast_ref::<Int32Array>() {
            values
                .iter()
                .map(|value| value.map(i128::from))
                .collect::<Vec<_>>()
        } else if let Some(values) = values.as_any().downcast_ref::<Int64Array>() {
            values
                .iter()
                .map(|value| value.map(i128::from))
                .collect::<Vec<_>>()
        } else if let Some(values) = values.as_any().downcast_ref::<Int8Array>() {
            values
                .iter()
                .map(|value| value.map(i128::from))
                .collect::<Vec<_>>()
        } else if let Some(values) = values.as_any().downcast_ref::<Int16Array>() {
            values
                .iter()
                .map(|value| value.map(i128::from))
                .collect::<Vec<_>>()
        } else if let Some(values) = values.as_any().downcast_ref::<UInt32Array>() {
            values
                .iter()
                .map(|value| value.map(i128::from))
                .collect::<Vec<_>>()
        } else if let Some(values) = values.as_any().downcast_ref::<UInt8Array>() {
            values
                .iter()
                .map(|value| value.map(i128::from))
                .collect::<Vec<_>>()
        } else if let Some(values) = values.as_any().downcast_ref::<UInt16Array>() {
            values
                .iter()
                .map(|value| value.map(i128::from))
                .collect::<Vec<_>>()
        } else if let Some(values) = values.as_any().downcast_ref::<UInt64Array>() {
            values
                .iter()
                .map(|value| value.map(i128::from))
                .collect::<Vec<_>>()
        } else {
            return Ok(None);
        };
        keys.push(values);
    }

    let compare_keys = |left: Option<i128>, right: Option<i128>| match (left, right) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => {
            if sort_expr.nulls_first {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        }
        (Some(_), None) => {
            if sort_expr.nulls_first {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            }
        }
        (Some(left), Some(right)) => {
            let ordering = left.cmp(&right);
            if sort_expr.ascending {
                ordering
            } else {
                ordering.reverse()
            }
        }
    };
    let batches_are_sorted = keys.iter().all(|values| {
        values
            .windows(2)
            .all(|window| compare_keys(window[0], window[1]) != std::cmp::Ordering::Greater)
    });
    let references = if batches_are_sorted {
        // Worker TopN operators emit sorted batches. Merge their heads instead
        // of materializing and sorting every candidate row at the coordinator.
        // A heap avoids rescanning every worker head for each output row.
        // The fallback below remains available for raw/unsorted inputs.
        let mut heap = keys
            .iter()
            .enumerate()
            .filter_map(|(batch, values)| (!values.is_empty()).then_some((batch, 0)))
            .collect::<Vec<_>>();
        heap_build(&mut heap, |left, right| {
            compare_keys(keys[left.0][left.1], keys[right.0][right.1])
        });
        let mut merged = Vec::with_capacity(limit.min(keys.iter().map(Vec::len).sum()));
        while merged.len() < limit {
            let Some((batch, row)) = heap_pop(&mut heap, |left, right| {
                compare_keys(keys[left.0][left.1], keys[right.0][right.1])
            }) else {
                break;
            };
            merged.push((batch, row));
            let next_row = row + 1;
            if next_row < keys[batch].len() {
                heap_push(&mut heap, (batch, next_row), |left, right| {
                    compare_keys(keys[left.0][left.1], keys[right.0][right.1])
                });
            }
        }
        merged
    } else {
        let mut references = keys
            .iter()
            .enumerate()
            .flat_map(|(batch, values)| (0..values.len()).map(move |row| (batch, row)))
            .collect::<Vec<_>>();
        references
            .sort_by(|left, right| compare_keys(keys[left.0][left.1], keys[right.0][right.1]));
        references.truncate(limit);
        references
    };

    let mut selected = Vec::with_capacity(references.len());
    let mut offsets = vec![0u32; batches.len()];
    let mut positions = HashMap::with_capacity(references.len());
    let mut selected_rows = 0u32;
    for (batch_index, batch) in batches.iter().enumerate() {
        let rows = references
            .iter()
            .filter_map(|(candidate_batch, row)| {
                (*candidate_batch == batch_index).then_some(*row as u32)
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            continue;
        }
        offsets[batch_index] = selected_rows;
        for (local_index, row) in rows.iter().enumerate() {
            positions.insert((batch_index, *row as usize), local_index as u32);
        }
        selected_rows += rows.len() as u32;
        let indices = UInt32Array::from(rows);
        let columns = batch
            .columns()
            .iter()
            .map(|column| take(column.as_ref(), &indices, None))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        selected.push(RecordBatch::try_new(schema.clone(), columns)?);
    }
    let selected = concat_batches(schema, &selected)?;
    let order = references
        .iter()
        .map(|(batch, row)| offsets[*batch] + positions[&(*batch, *row)])
        .collect::<Vec<_>>();
    let order = UInt32Array::from(order);
    let columns = selected
        .columns()
        .iter()
        .map(|column| take(column.as_ref(), &order, None))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(RecordBatch::try_new(schema.clone(), columns)?))
}

// Common integer ORDER BY keys can compare typed values directly. Other types
// and nullable arrays retain Arrow's complete ordering semantics.
#[derive(Clone, Copy)]
enum IntegerSortKey<'a> {
    Float32(&'a Float32Array),
    Signed8(&'a Int8Array),
    Signed16(&'a Int16Array),
    Signed32(&'a Int32Array),
    Signed(&'a Int64Array),
    Unsigned8(&'a UInt8Array),
    Unsigned16(&'a UInt16Array),
    Unsigned32(&'a UInt32Array),
    Unsigned(&'a UInt64Array),
}

impl IntegerSortKey<'_> {
    fn len(self) -> usize {
        match self {
            Self::Float32(values) => values.len(),
            Self::Signed8(values) => values.len(),
            Self::Signed16(values) => values.len(),
            Self::Signed32(values) => values.len(),
            Self::Signed(values) => values.len(),
            Self::Unsigned8(values) => values.len(),
            Self::Unsigned16(values) => values.len(),
            Self::Unsigned32(values) => values.len(),
            Self::Unsigned(values) => values.len(),
        }
    }
}

fn integer_top_n_indices(columns: &[SortColumn], limit: usize) -> Option<UInt32Array> {
    // COUNT/SUM results are commonly a single non-null UInt64 ordering key.
    // Keep that hot path on the selection algorithm too; falling back to
    // lexsort here makes every 4K-row final aggregate batch pay a full sort.
    if columns.is_empty() {
        return None;
    }
    let keys = columns
        .iter()
        .map(|column| {
            let values = column.values.as_any();
            let key = if let Some(values) = values.downcast_ref::<Int8Array>() {
                IntegerSortKey::Signed8(values)
            } else if let Some(values) = values.downcast_ref::<Int16Array>() {
                IntegerSortKey::Signed16(values)
            } else if let Some(values) = values.downcast_ref::<Int32Array>() {
                IntegerSortKey::Signed32(values)
            } else if let Some(values) = values.downcast_ref::<Int64Array>() {
                IntegerSortKey::Signed(values)
            } else if let Some(values) = values.downcast_ref::<UInt8Array>() {
                IntegerSortKey::Unsigned8(values)
            } else if let Some(values) = values.downcast_ref::<UInt16Array>() {
                IntegerSortKey::Unsigned16(values)
            } else if let Some(values) = values.downcast_ref::<UInt32Array>() {
                IntegerSortKey::Unsigned32(values)
            } else if let Some(values) = values.downcast_ref::<Float32Array>() {
                // Arrow's ordering gives NaN and NULL special placement. Keep
                // those cases on lexsort; finite non-null values are safe for
                // the direct comparator below.
                if values
                    .iter()
                    .any(|value| value.is_some_and(|value| !value.is_finite()))
                {
                    return None;
                }
                IntegerSortKey::Float32(values)
            } else {
                IntegerSortKey::Unsigned(values.downcast_ref::<UInt64Array>()?)
            };
            (column.values.null_count() == 0)
                .then_some((key, column.options.unwrap_or_default().descending))
        })
        .collect::<Option<Vec<_>>>()?;
    let rows = keys.first()?.0.len();
    if rows > u32::MAX as usize || keys.iter().any(|(key, _)| key.len() != rows) {
        return None;
    }
    let mut indices: Vec<u32> = (0..rows as u32).collect();
    let compare = |a: &u32, b: &u32| {
        for (key, descending) in &keys {
            let ordering = match key {
                IntegerSortKey::Float32(values) => values
                    .value(*a as usize)
                    .partial_cmp(&values.value(*b as usize))
                    .unwrap_or(std::cmp::Ordering::Equal),
                IntegerSortKey::Signed8(values) => {
                    values.value(*a as usize).cmp(&values.value(*b as usize))
                }
                IntegerSortKey::Signed16(values) => {
                    values.value(*a as usize).cmp(&values.value(*b as usize))
                }
                IntegerSortKey::Signed32(values) => {
                    values.value(*a as usize).cmp(&values.value(*b as usize))
                }
                IntegerSortKey::Signed(values) => {
                    values.value(*a as usize).cmp(&values.value(*b as usize))
                }
                IntegerSortKey::Unsigned8(values) => {
                    values.value(*a as usize).cmp(&values.value(*b as usize))
                }
                IntegerSortKey::Unsigned16(values) => {
                    values.value(*a as usize).cmp(&values.value(*b as usize))
                }
                IntegerSortKey::Unsigned32(values) => {
                    values.value(*a as usize).cmp(&values.value(*b as usize))
                }
                IntegerSortKey::Unsigned(values) => {
                    values.value(*a as usize).cmp(&values.value(*b as usize))
                }
            };
            if !ordering.is_eq() {
                return if *descending {
                    ordering.reverse()
                } else {
                    ordering
                };
            }
        }
        std::cmp::Ordering::Equal
    };
    let retained = limit.min(rows);
    if retained < rows {
        indices.select_nth_unstable_by(retained, compare);
        indices.truncate(retained);
    }
    indices.sort_unstable_by(compare);
    Some(UInt32Array::from(indices))
}

/// Decimal128 ORDER BY keys are represented as scaled `i128` values.  When a
/// final TopN has one decimal key, rank row references directly instead of
/// concatenating every payload column before Arrow's general lexsort.  The
/// comparison is exact because every value in one Arrow decimal array shares
/// its scale; null placement follows the query's SortOptions.
fn decimal_top_n_indices(columns: &[SortColumn], limit: usize) -> Option<UInt32Array> {
    if columns.len() != 1 {
        return None;
    }
    let column = &columns[0];
    let values = column.values.as_any().downcast_ref::<Decimal128Array>()?;
    if values.len() > u32::MAX as usize {
        return None;
    }
    let options = column.options.unwrap_or_default();
    let mut indices: Vec<u32> = (0..values.len() as u32).collect();
    indices.sort_by(|left, right| {
        let left_null = values.is_null(*left as usize);
        let right_null = values.is_null(*right as usize);
        let ordering = match (left_null, right_null) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => {
                if options.nulls_first {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            }
            (false, true) => {
                if options.nulls_first {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                }
            }
            (false, false) => values
                .value(*left as usize)
                .cmp(&values.value(*right as usize)),
        };
        // Arrow's null placement is independent of direction: NULLS LAST
        // stays last for both ascending and descending order.
        if options.descending && !left_null && !right_null {
            ordering.reverse()
        } else {
            ordering
        }
    });
    indices.truncate(limit);
    Some(UInt32Array::from(indices))
}

/// Decimal256 counterpart to `decimal_top_n_indices`. Keep the comparison in
/// Arrow's native `i256` domain so large exact values never pass through a
/// floating point or i128 narrowing conversion.
fn decimal256_top_n_indices(columns: &[SortColumn], limit: usize) -> Option<UInt32Array> {
    if columns.len() != 1 {
        return None;
    }
    let column = &columns[0];
    let values = column
        .values
        .as_any()
        .downcast_ref::<arrow::array::Decimal256Array>()?;
    if values.len() > u32::MAX as usize {
        return None;
    }
    let options = column.options.unwrap_or_default();
    let mut indices: Vec<u32> = (0..values.len() as u32).collect();
    indices.sort_by(|left, right| {
        let left_null = values.is_null(*left as usize);
        let right_null = values.is_null(*right as usize);
        let ordering = match (left_null, right_null) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => {
                if options.nulls_first {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            }
            (false, true) => {
                if options.nulls_first {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                }
            }
            (false, false) => values
                .value(*left as usize)
                .cmp(&values.value(*right as usize)),
        };
        if options.descending && !left_null && !right_null {
            ordering.reverse()
        } else {
            ordering
        }
    });
    indices.truncate(limit);
    Some(UInt32Array::from(indices))
}

impl BatchOperator for TopNOperator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let expression_memory = self.memory.clone();
        crate::expr_eval::with_expression_memory(expression_memory.as_ref(), || {
            if !self.initialized {
                self.initialized = true;
                self.initialize()?;
            }
            if let Some(merge) = &mut self.external_merge {
                return merge.next_batch();
            }
            let batch = self.output.pop_front();
            if batch.is_none() {
                self.output_memory.clear();
            }
            Ok(batch)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::array::{
        Array, ArrayRef, Decimal128Array, Decimal256Array, Int8Array, Int16Array, Int32Array,
        Int64Array, StringArray, UInt8Array, UInt16Array, UInt32Array,
    };
    use arrow::datatypes::{DataType, Field, Schema};

    use super::*;
    use kaveon_core::Expr;

    #[test]
    fn typed_integer_topn_matches_arrow_for_mixed_orders_ties_and_extremes() {
        let first = Arc::new(Int64Array::from(
            (0..1024)
                .map(|row| match row % 17 {
                    0 => i64::MIN,
                    1 => i64::MAX,
                    _ => (row % 13) - 6,
                })
                .collect::<Vec<_>>(),
        ));
        let second = Arc::new(Int64Array::from(
            (0..1024).map(|row| (row * 31) % 43).collect::<Vec<_>>(),
        ));
        for descending in [false, true] {
            let columns = vec![
                SortColumn {
                    values: first.clone(),
                    options: Some(SortOptions {
                        descending,
                        nulls_first: true,
                    }),
                },
                SortColumn {
                    values: second.clone(),
                    options: Some(SortOptions {
                        descending: !descending,
                        nulls_first: false,
                    }),
                },
            ];
            for limit in [0, 1, 20, 1024, 2048] {
                let actual = integer_top_n_indices(&columns, limit).unwrap();
                let expected = lexsort_to_indices(&columns, Some(limit)).unwrap();
                // Equal keys may select different tied row indices; their values must agree.
                for column in &columns {
                    assert_eq!(
                        take(column.values.as_ref(), &actual, None)
                            .unwrap()
                            .to_data(),
                        take(column.values.as_ref(), &expected, None)
                            .unwrap()
                            .to_data()
                    );
                }
            }
        }
        let nullable = vec![
            SortColumn {
                values: Arc::new(Int64Array::from(vec![Some(1), None])),
                options: None,
            },
            SortColumn {
                values: Arc::new(Int64Array::from(vec![2, 3])),
                options: None,
            },
        ];
        assert!(integer_top_n_indices(&nullable, 1).is_none());
    }

    #[test]
    fn typed_unsigned_single_key_topn_matches_arrow() {
        let values = Arc::new(UInt64Array::from(vec![0, u64::MAX, 4, 4, 17, 2]));
        let columns = vec![SortColumn {
            values: values.clone(),
            options: Some(SortOptions {
                descending: true,
                nulls_first: false,
            }),
        }];
        let actual = integer_top_n_indices(&columns, 3).unwrap();
        let expected = lexsort_to_indices(&columns, Some(3)).unwrap();
        assert_eq!(
            take(values.as_ref(), &actual, None).unwrap().to_data(),
            take(values.as_ref(), &expected, None).unwrap().to_data()
        );
    }

    #[test]
    fn typed_float32_topn_matches_arrow_for_finite_values() {
        let values = Arc::new(Float32Array::from(vec![-3.5, 0.0, 4.25, 4.25, 1.0, -0.25]));
        for descending in [false, true] {
            let columns = vec![SortColumn {
                values: values.clone(),
                options: Some(SortOptions {
                    descending,
                    nulls_first: false,
                }),
            }];
            for limit in [0, 1, 3, 10] {
                let actual = integer_top_n_indices(&columns, limit).unwrap();
                let expected = lexsort_to_indices(&columns, Some(limit)).unwrap();
                assert_eq!(
                    take(values.as_ref(), &actual, None).unwrap().to_data(),
                    take(values.as_ref(), &expected, None).unwrap().to_data()
                );
            }
        }
        let nan = Arc::new(Float32Array::from(vec![1.0, f32::NAN, 2.0]));
        assert!(
            integer_top_n_indices(
                &[SortColumn {
                    values: nan,
                    options: None,
                }],
                2,
            )
            .is_none()
        );
    }

    #[test]
    fn typed_narrow_integer_topn_matches_arrow() {
        let signed8 = Arc::new(Int8Array::from(vec![i8::MIN, 7, -3, 7, i8::MAX]));
        let signed16 = Arc::new(Int16Array::from(vec![i16::MIN, 7, -3, 7, i16::MAX]));
        let signed = Arc::new(Int32Array::from(vec![i32::MIN, 7, -3, 7, i32::MAX]));
        let unsigned8 = Arc::new(UInt8Array::from(vec![0, u8::MAX, 4, 4, 17]));
        let unsigned16 = Arc::new(UInt16Array::from(vec![0, u16::MAX, 4, 4, 17]));
        let unsigned = Arc::new(UInt32Array::from(vec![0, u32::MAX, 4, 4, 17]));
        for values in [
            Arc::clone(&signed8) as arrow::array::ArrayRef,
            Arc::clone(&signed16) as arrow::array::ArrayRef,
            Arc::clone(&signed) as arrow::array::ArrayRef,
            Arc::clone(&unsigned8) as arrow::array::ArrayRef,
            Arc::clone(&unsigned16) as arrow::array::ArrayRef,
            Arc::clone(&unsigned) as arrow::array::ArrayRef,
        ] {
            for descending in [false, true] {
                let columns = vec![SortColumn {
                    values: values.clone(),
                    options: Some(SortOptions {
                        descending,
                        nulls_first: false,
                    }),
                }];
                for limit in [0, 1, 3, 10] {
                    let actual = integer_top_n_indices(&columns, limit).unwrap();
                    let expected = lexsort_to_indices(&columns, Some(limit)).unwrap();
                    assert_eq!(
                        take(values.as_ref(), &actual, None).unwrap().to_data(),
                        take(values.as_ref(), &expected, None).unwrap().to_data()
                    );
                }
            }
        }
    }

    #[test]
    fn typed_decimal_topn_matches_arrow_with_nulls_and_ordering() {
        let values = Arc::new(
            Decimal128Array::from(vec![
                Some(120),
                None,
                Some(-350),
                Some(120),
                Some(999),
                None,
                Some(-1),
            ])
            .with_precision_and_scale(10, 2)
            .unwrap(),
        );
        for (descending, nulls_first) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let columns = vec![SortColumn {
                values: values.clone(),
                options: Some(SortOptions {
                    descending,
                    nulls_first,
                }),
            }];
            for limit in [0, 1, 3, 7, 20] {
                let actual = decimal_top_n_indices(&columns, limit).unwrap();
                let expected = lexsort_to_indices(&columns, Some(limit)).unwrap();
                assert_eq!(
                    take(values.as_ref(), &actual, None).unwrap().to_data(),
                    take(values.as_ref(), &expected, None).unwrap().to_data()
                );
            }
        }
    }

    #[test]
    fn typed_decimal256_topn_matches_arrow_with_nulls_and_ordering() {
        let values = Arc::new(
            Decimal256Array::from(vec![
                Some(
                    arrow::datatypes::i256::from_string("12000000000000000000000000000120")
                        .unwrap(),
                ),
                None,
                Some(
                    arrow::datatypes::i256::from_string("-35000000000000000000000000000350")
                        .unwrap(),
                ),
                Some(
                    arrow::datatypes::i256::from_string("12000000000000000000000000000120")
                        .unwrap(),
                ),
                Some(
                    arrow::datatypes::i256::from_string("99900000000000000000000000000999")
                        .unwrap(),
                ),
                None,
                Some(arrow::datatypes::i256::from_string("-1").unwrap()),
            ])
            .with_precision_and_scale(40, 2)
            .unwrap(),
        );
        for (descending, nulls_first) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let columns = vec![SortColumn {
                values: values.clone(),
                options: Some(SortOptions {
                    descending,
                    nulls_first,
                }),
            }];
            for limit in [0, 1, 3, 7, 20] {
                let actual = decimal256_top_n_indices(&columns, limit).unwrap();
                let expected = lexsort_to_indices(&columns, Some(limit)).unwrap();
                assert_eq!(
                    take(values.as_ref(), &actual, None).unwrap().to_data(),
                    take(values.as_ref(), &expected, None).unwrap().to_data()
                );
            }
        }
    }

    struct MockOperator {
        schema: SchemaRef,
        batches: VecDeque<RecordBatch>,
    }

    impl MockOperator {
        fn new(values: Vec<Vec<Option<i64>>>) -> Self {
            let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
            let batches = values
                .into_iter()
                .map(|values| {
                    RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(values))])
                        .unwrap()
                })
                .collect();
            Self { schema, batches }
        }
    }

    impl BatchOperator for MockOperator {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }

        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batches.pop_front())
        }
    }

    fn values(batch: &RecordBatch) -> Vec<Option<i64>> {
        let array = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        (0..array.len())
            .map(|index| {
                if array.is_null(index) {
                    None
                } else {
                    Some(array.value(index))
                }
            })
            .collect()
    }

    #[test]
    fn selects_top_n_across_batches() {
        let source = MockOperator::new(vec![vec![Some(2), Some(5)], vec![Some(4), Some(1)]]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), false)],
            3,
        )
        .unwrap();
        let output = operator.next_batch().unwrap().unwrap();
        assert_eq!(values(&output), vec![Some(5), Some(4), Some(2)]);
        assert!(operator.next_batch().unwrap().is_none());
    }

    #[test]
    fn candidate_memory_is_bounded_across_many_input_batches() {
        let input = MockOperator::new((0..1000).map(|value| vec![Some(value)]).collect());
        let pool = kaveon_core::QueryMemoryPool::new("topn-many-batches", 64 * 1024).unwrap();
        let mut operator = TopNOperator::new(
            Box::new(input),
            vec![SortExpr::new(Expr::Column("id".into()), false)],
            3,
        )
        .unwrap()
        .with_memory(pool.operator("topn").unwrap());
        let batch = operator.next_batch().unwrap().unwrap();
        assert_eq!(
            batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values()
                .as_ref(),
            &[999, 998, 997]
        );
        assert!(operator.next_batch().unwrap().is_none());
        assert!(pool.snapshot().peak_bytes <= 64 * 1024);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn limit_larger_than_input_returns_all_rows() {
        let source = MockOperator::new(vec![vec![Some(2), Some(1)]]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), true)],
            10,
        )
        .unwrap();
        assert_eq!(
            values(&operator.next_batch().unwrap().unwrap()),
            vec![Some(1), Some(2)]
        );
    }

    #[test]
    fn zero_limit_does_not_read_input() {
        let source = MockOperator::new(vec![vec![Some(1)]]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), true)],
            0,
        )
        .unwrap();
        assert!(operator.next_batch().unwrap().is_none());
    }

    #[test]
    fn empty_input_produces_no_batch() {
        let source = MockOperator::new(vec![]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), true)],
            2,
        )
        .unwrap();
        assert!(operator.next_batch().unwrap().is_none());
    }

    #[test]
    fn respects_explicit_null_ordering() {
        let source = MockOperator::new(vec![vec![Some(2), None, Some(1)]]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), true).with_nulls_first(true)],
            2,
        )
        .unwrap();
        assert_eq!(
            values(&operator.next_batch().unwrap().unwrap()),
            vec![None, Some(1)]
        );
    }

    #[test]
    fn integer_fast_path_keeps_nulls_first_when_descending() {
        let source = MockOperator::new(vec![vec![Some(2), None, Some(1)]]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("id".into()), false).with_nulls_first(true)],
            2,
        )
        .unwrap();
        assert_eq!(
            values(&operator.next_batch().unwrap().unwrap()),
            vec![None, Some(2)]
        );
    }

    #[test]
    fn reports_invalid_configuration_and_expression() {
        let source = MockOperator::new(vec![vec![Some(1)]]);
        assert!(TopNOperator::new(Box::new(source), vec![], 1).is_err());

        let source = MockOperator::new(vec![vec![Some(1)]]);
        let mut operator = TopNOperator::new(
            Box::new(source),
            vec![SortExpr::new(Expr::Column("missing".into()), true)],
            1,
        )
        .unwrap();
        assert!(
            operator
                .next_batch()
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
    }

    #[test]
    fn merges_partition_top_n_with_multi_column_and_null_ordering() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("score", DataType::Int64, true),
            Field::new("name", DataType::Utf8, false),
        ]));
        let partition_a = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![None, Some(9), Some(8)])),
                Arc::new(StringArray::from(vec!["null-a", "amy", "zed"])),
            ],
        )
        .unwrap();
        let partition_b = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![None, Some(9), Some(7)])),
                Arc::new(StringArray::from(vec!["null-b", "zoe", "bob"])),
            ],
        )
        .unwrap();
        let ordering = vec![
            SortExpr::new(Expr::Column("score".into()), false).with_nulls_first(false),
            SortExpr::new(Expr::Column("name".into()), true),
        ];

        let result = merge_top_n(&schema, &[partition_a, partition_b], &ordering, 4)
            .unwrap()
            .unwrap();
        let scores = result
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        let names = result
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(
            (0..scores.len())
                .map(|index| (!scores.is_null(index)).then(|| scores.value(index)))
                .collect::<Vec<_>>(),
            vec![Some(9), Some(9), Some(8), Some(7)]
        );
        assert_eq!(
            names.iter().collect::<Vec<_>>(),
            vec![Some("amy"), Some("zoe"), Some("zed"), Some("bob")]
        );
    }

    #[test]
    fn three_worker_global_top_n_matches_single_partition_reference() {
        // Each worker keeps only its local candidates. The final merge must
        // still agree with sorting the complete input, including NULLs and
        // equal primary keys. This is the correctness contract for the
        // current centralized global ORDER BY/TopN merge.
        let schema = Arc::new(Schema::new(vec![
            Field::new("score", DataType::Int64, true),
            Field::new("name", DataType::Utf8, false),
        ]));
        let workers = [
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int64Array::from(vec![
                        Some(10),
                        None,
                        Some(8),
                        Some(8),
                        Some(2),
                    ])),
                    Arc::new(StringArray::from(vec!["a", "null-a", "z", "b", "low-a"])),
                ],
            )
            .unwrap(),
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int64Array::from(vec![
                        Some(10),
                        Some(9),
                        None,
                        Some(8),
                        Some(1),
                    ])),
                    Arc::new(StringArray::from(vec!["c", "n", "null-b", "c", "low-b"])),
                ],
            )
            .unwrap(),
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int64Array::from(vec![
                        Some(9),
                        Some(8),
                        Some(8),
                        None,
                        Some(0),
                    ])),
                    Arc::new(StringArray::from(vec!["m", "a", "d", "null-c", "low-c"])),
                ],
            )
            .unwrap(),
        ];
        let ordering = vec![
            SortExpr::new(Expr::Column("score".into()), false).with_nulls_first(false),
            SortExpr::new(Expr::Column("name".into()), true),
        ];
        let local_candidates = workers
            .iter()
            .map(|worker| merge_top_n(&schema, std::slice::from_ref(worker), &ordering, 4))
            .collect::<Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .map(Option::unwrap)
            .collect::<Vec<_>>();
        let distributed = merge_top_n(&schema, &local_candidates, &ordering, 4)
            .unwrap()
            .unwrap();

        let complete = concat_batches(&schema, &workers).unwrap();
        let columns = ordering
            .iter()
            .map(|sort_expr| {
                Ok(SortColumn {
                    values: evaluate(&sort_expr.expr, &complete).unwrap(),
                    options: Some(SortOptions {
                        descending: !sort_expr.ascending,
                        nulls_first: sort_expr.nulls_first,
                    }),
                })
            })
            .collect::<std::result::Result<Vec<_>, arrow::error::ArrowError>>()
            .unwrap();
        let indices = lexsort_to_indices(&columns, Some(4)).unwrap();
        let expected = RecordBatch::try_new(
            schema.clone(),
            complete
                .columns()
                .iter()
                .map(|column| take(column.as_ref(), &indices, None).unwrap())
                .collect(),
        )
        .unwrap();
        assert_eq!(distributed, expected);
    }

    #[test]
    fn global_top_n_candidate_merge_is_incremental_and_exact() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("score", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let workers = [
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int64Array::from(vec![11, 4, 8])),
                    Arc::new(StringArray::from(vec!["a", "b", "c"])),
                ],
            )
            .unwrap(),
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int64Array::from(vec![10, 9, 1])),
                    Arc::new(StringArray::from(vec!["d", "e", "f"])),
                ],
            )
            .unwrap(),
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int64Array::from(vec![12, 7, 6])),
                    Arc::new(StringArray::from(vec!["g", "h", "i"])),
                ],
            )
            .unwrap(),
        ];
        let ordering = vec![SortExpr::new(Expr::Column("score".into()), false)];
        let mut incremental = None;
        for worker in &workers {
            let mut inputs = Vec::new();
            if let Some(candidate) = incremental.take() {
                inputs.push(candidate);
            }
            inputs.push(worker.clone());
            incremental = merge_top_n(&schema, &inputs, &ordering, 4).unwrap();
        }
        let incremental = incremental.unwrap();
        let all = merge_top_n(&schema, &workers, &ordering, 4)
            .unwrap()
            .unwrap();
        assert_eq!(incremental, all);
    }

    #[test]
    fn global_top_n_ignores_empty_worker_candidates_without_changing_order() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("score", DataType::Int64, true),
            Field::new("name", DataType::Utf8, false),
        ]));
        let empty = RecordBatch::new_empty(schema.clone());
        let worker = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some(9), Some(3), Some(7)])),
                Arc::new(StringArray::from(vec!["a", "b", "c"])),
            ],
        )
        .unwrap();
        let ordering = vec![
            SortExpr::new(Expr::Column("score".into()), false),
            SortExpr::new(Expr::Column("name".into()), true),
        ];

        let with_empty = merge_top_n(
            &schema,
            &[empty.clone(), worker.clone(), empty.clone()],
            &ordering,
            2,
        )
        .unwrap()
        .unwrap();
        let without_empty = merge_top_n(&schema, &[worker], &ordering, 2)
            .unwrap()
            .unwrap();

        assert_eq!(with_empty, without_empty);
    }

    #[test]
    fn three_worker_global_decimal256_top_n_matches_single_partition_reference() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "score",
            DataType::Decimal256(40, 2),
            true,
        )]));
        let decimal = |values: Vec<Option<&str>>| {
            Arc::new(
                Decimal256Array::from(
                    values
                        .into_iter()
                        .map(|value| {
                            value.map(|value| arrow::datatypes::i256::from_string(value).unwrap())
                        })
                        .collect::<Vec<_>>(),
                )
                .with_precision_and_scale(40, 2)
                .unwrap(),
            ) as ArrayRef
        };
        let workers = [
            RecordBatch::try_new(
                schema.clone(),
                vec![decimal(vec![
                    Some("10000000000000000000000000000100"),
                    Some("80000000000000000000000000000800"),
                    None,
                ])],
            )
            .unwrap(),
            RecordBatch::try_new(
                schema.clone(),
                vec![decimal(vec![
                    Some("10000000000000000000000000000200"),
                    Some("90000000000000000000000000000900"),
                    Some("-1"),
                ])],
            )
            .unwrap(),
            RecordBatch::try_new(
                schema.clone(),
                vec![decimal(vec![
                    Some("10000000000000000000000000000300"),
                    Some("70000000000000000000000000000700"),
                    Some("10000000000000000000000000000100"),
                ])],
            )
            .unwrap(),
        ];
        let ordering =
            vec![SortExpr::new(Expr::Column("score".into()), false).with_nulls_first(false)];
        let local_candidates = workers
            .iter()
            .map(|worker| merge_top_n(&schema, std::slice::from_ref(worker), &ordering, 4))
            .collect::<Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .map(Option::unwrap)
            .collect::<Vec<_>>();
        let distributed = merge_top_n(&schema, &local_candidates, &ordering, 4)
            .unwrap()
            .unwrap();
        let complete = concat_batches(&schema, &workers).unwrap();
        let columns = vec![SortColumn {
            values: complete.column(0).clone(),
            options: Some(SortOptions {
                descending: true,
                nulls_first: false,
            }),
        }];
        let indices = lexsort_to_indices(&columns, Some(4)).unwrap();
        let expected = RecordBatch::try_new(
            schema.clone(),
            vec![take(complete.column(0).as_ref(), &indices, None).unwrap()],
        )
        .unwrap();
        assert_eq!(distributed, expected);
    }

    #[test]
    fn three_worker_global_uint64_top_n_preserves_extrema() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("score", DataType::UInt64, true),
            Field::new("id", DataType::Utf8, false),
        ]));
        let workers = [
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(UInt64Array::from(vec![Some(u64::MAX), Some(1), None])),
                    Arc::new(StringArray::from(vec!["max", "one", "null-a"])),
                ],
            )
            .unwrap(),
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(UInt64Array::from(vec![Some(u64::MAX - 1), Some(0), None])),
                    Arc::new(StringArray::from(vec!["near-max", "zero", "null-b"])),
                ],
            )
            .unwrap(),
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(UInt64Array::from(vec![Some(42), Some(2), Some(3)])),
                    Arc::new(StringArray::from(vec!["forty-two", "two", "three"])),
                ],
            )
            .unwrap(),
        ];
        let ordering =
            vec![SortExpr::new(Expr::Column("score".into()), false).with_nulls_first(false)];
        let local_candidates = workers
            .iter()
            .map(|worker| merge_top_n(&schema, std::slice::from_ref(worker), &ordering, 5))
            .collect::<Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .map(Option::unwrap)
            .collect::<Vec<_>>();
        let distributed = merge_top_n(&schema, &local_candidates, &ordering, 5)
            .unwrap()
            .unwrap();

        let complete = concat_batches(&schema, &workers).unwrap();
        let indices = lexsort_to_indices(
            &[SortColumn {
                values: complete.column(0).clone(),
                options: Some(SortOptions {
                    descending: true,
                    nulls_first: false,
                }),
            }],
            Some(5),
        )
        .unwrap();
        let expected = RecordBatch::try_new(
            schema,
            complete
                .columns()
                .iter()
                .map(|column| take(column.as_ref(), &indices, None).unwrap())
                .collect(),
        )
        .unwrap();
        assert_eq!(distributed, expected);
    }

    #[test]
    fn merges_sorted_integer_batches_with_k_way_heads() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("score", DataType::Int64, false),
            Field::new("id", DataType::Utf8, false),
        ]));
        let worker_a = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![9, 7, 1])),
                Arc::new(StringArray::from(vec!["a9", "a7", "a1"])),
            ],
        )
        .unwrap();
        let worker_b = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![8, 7, 2])),
                Arc::new(StringArray::from(vec!["b8", "b7", "b2"])),
            ],
        )
        .unwrap();
        let ordering = vec![SortExpr::new(Expr::Column("score".into()), false)];

        let result = merge_top_n(&schema, &[worker_a, worker_b], &ordering, 4)
            .unwrap()
            .unwrap();
        assert_eq!(
            result
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values(),
            &[9, 8, 7, 7]
        );
        assert_eq!(
            result
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some("a9"), Some("b8"), Some("a7"), Some("b7")]
        );
    }

    #[test]
    fn merges_sorted_string_batches_without_payload_concatenation() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, true),
            Field::new("payload", DataType::Utf8, false),
        ]));
        let worker_a = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![Some("alpha"), Some("charlie")])),
                Arc::new(StringArray::from(vec!["a", "c"])),
            ],
        )
        .unwrap();
        let worker_b = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![Some("bravo"), Some("delta")])),
                Arc::new(StringArray::from(vec!["b", "d"])),
            ],
        )
        .unwrap();

        let result = merge_top_n(
            &schema,
            &[worker_a, worker_b],
            &[SortExpr::new(Expr::Column("name".into()), true)],
            3,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some("alpha"), Some("bravo"), Some("charlie")]
        );
        assert_eq!(
            result
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some("a"), Some("b"), Some("c")]
        );
    }

    #[test]
    fn merge_rejects_incompatible_partition_schema() {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let incompatible_schema =
            Arc::new(Schema::new(vec![Field::new("id", DataType::Utf8, false)]));
        let batch = RecordBatch::try_new(
            incompatible_schema,
            vec![Arc::new(StringArray::from(vec!["1"]))],
        )
        .unwrap();
        let result = merge_top_n(
            &schema,
            &[batch],
            &[SortExpr::new(Expr::Column("id".into()), true)],
            1,
        );
        assert!(result.unwrap_err().to_string().contains("incompatible"));
    }

    #[test]
    fn spill_aware_top_n_matches_in_memory_result_under_a_tiny_limit() {
        let input_values = vec![
            vec![Some(2), Some(5)],
            vec![Some(4), Some(1)],
            vec![Some(8), Some(3)],
            vec![Some(7), Some(6)],
            vec![Some(10), Some(9)],
        ];
        let memory_limit = 16 * 1024;
        let memory_pool = kaveon_core::QueryMemoryPool::new("topn-spill", memory_limit).unwrap();
        let memory = memory_pool.operator("topn").unwrap();
        let spill = SpillManager::new(std::env::temp_dir(), 64 * 1_024).unwrap();
        let spill_metrics = spill.clone();
        let mut operator = TopNOperator::new_with_spill(
            Box::new(MockOperator::new(input_values)),
            vec![SortExpr::new(Expr::Column("id".into()), false)],
            3,
            memory,
            spill,
        )
        .unwrap()
        .with_merge_fan_in(2)
        .unwrap();

        assert_eq!(
            values(&operator.next_batch().unwrap().unwrap()),
            vec![Some(10), Some(9), Some(8)]
        );
        assert!(spill_metrics.snapshot().peak_bytes > 0);
        assert_eq!(
            spill_metrics.snapshot().current_bytes,
            0,
            "the final bounded batch releases its spill runs"
        );
        assert!(memory_pool.snapshot().peak_bytes <= memory_limit);
    }
}
