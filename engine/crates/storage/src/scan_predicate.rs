//! The part of a scan's predicate the storage layer evaluates itself, and
//! the two places it runs: inside the Parquet decoder as a row filter
//! (late materialisation — the predicate's columns are decoded first and
//! the rest only for the rows that survive), and on decoded batches before
//! they leave a decoder lane. Both run the same compiled predicate over the
//! same Arrow kernels the executor uses (`cmp`, `like`, `is_null`), so a
//! row admitted here and a row admitted by the executor's filter agree on
//! every shape this module accepts; the executor still evaluates the whole
//! predicate on what arrives, which keeps it the single source of truth.
//!
//! Soundness rule: a compiled predicate is implied by the scan's predicate.
//! A conjunct the storage layer cannot evaluate is left out (weaker is
//! fine); a disjunction or negation is compiled whole or not at all.
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, AsArray, BooleanArray, Datum, Scalar};
use arrow::compute::kernels::cmp;
use arrow::compute::kernels::comparison::{ilike, like, nilike, nlike};
use arrow::datatypes::{DataType, Int32Type, SchemaRef};
use arrow::error::ArrowError;
use arrow::record_batch::RecordBatch;
use kaveon_core::{CompareOp, ScalarValue, StoragePredicate};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ArrowPredicate, RowFilter};
use parquet::file::metadata::ParquetMetaData;
use parquet::schema::types::SchemaDescriptor;

use crate::ScanMetrics;

/// Over object storage a row filter fetches the predicate's columns and
/// the rest of the projection in two rounds per row group. That pays when
/// the rest is large: the rows the filter rejects are never decoded for
/// those columns, and a row group it empties is never fetched for them.
/// For a narrow projection the second round costs more than the decode it
/// saves (measured on the cluster: 1.7x slower on aggregate shapes whose
/// predicate column was one of two projected; on the wide benchmark in
/// `scan_bench`, a two-column projection over an in-memory store 42 → 54 ms
/// with the filter, 100 columns of plain text 467 → 331 ms), so the filter
/// is applied only when the remaining projection carries at least this many
/// times the predicate columns' compressed bytes.
pub(crate) const LATE_MATERIALISATION_RATIO: u64 = 4;

/// One comparison, match, or null test over a column of a batch, or a
/// boolean composition of them. Column indices address the batch the
/// predicate is compiled against.
#[derive(Clone)]
pub(crate) enum CompiledPredicate {
    Compare {
        column: usize,
        op: CompareOp,
        literal: Scalar<ArrayRef>,
    },
    Like {
        column: usize,
        pattern: Scalar<ArrayRef>,
        negated: bool,
        case_insensitive: bool,
    },
    IsNull {
        column: usize,
    },
    IsNotNull {
        column: usize,
    },
    In {
        column: usize,
        literals: Vec<Scalar<ArrayRef>>,
    },
    And(Vec<CompiledPredicate>),
    Or(Vec<CompiledPredicate>),
    Not(Box<CompiledPredicate>),
}

impl CompiledPredicate {
    /// The evaluable part of `predicate` over batches of `schema`, or None
    /// when nothing in it can be evaluated on those columns.
    pub(crate) fn compile(predicate: &StoragePredicate, schema: &SchemaRef) -> Option<Self> {
        compile(predicate, schema, false)
    }

    /// The verdict per row, three-valued: a null is a comparison with a
    /// null, which SQL never selects.
    pub(crate) fn evaluate(&self, batch: &RecordBatch) -> Result<BooleanArray, ArrowError> {
        match self {
            CompiledPredicate::Compare {
                column,
                op,
                literal,
            } => compare_column(batch.column(*column), *op, literal),
            CompiledPredicate::Like {
                column,
                pattern,
                negated,
                case_insensitive,
            } => {
                let values = batch.column(*column);
                match (negated, case_insensitive) {
                    (false, false) => like(values, pattern),
                    (true, false) => nlike(values, pattern),
                    (false, true) => ilike(values, pattern),
                    (true, true) => nilike(values, pattern),
                }
            }
            CompiledPredicate::IsNull { column } => arrow::compute::is_null(batch.column(*column)),
            CompiledPredicate::IsNotNull { column } => {
                arrow::compute::is_not_null(batch.column(*column))
            }
            CompiledPredicate::In { column, literals } => {
                let values = batch.column(*column);
                let mut verdict: Option<BooleanArray> = None;
                for literal in literals {
                    let this = compare_column(values, CompareOp::Eq, literal)?;
                    verdict = Some(match verdict {
                        None => this,
                        Some(previous) => arrow::compute::or_kleene(&previous, &this)?,
                    });
                }
                Ok(verdict.unwrap_or_else(|| BooleanArray::from(vec![false; batch.num_rows()])))
            }
            CompiledPredicate::And(children) => {
                let mut verdict: Option<BooleanArray> = None;
                for child in children {
                    let this = child.evaluate(batch)?;
                    verdict = Some(match verdict {
                        None => this,
                        Some(previous) => arrow::compute::and_kleene(&previous, &this)?,
                    });
                }
                Ok(verdict.unwrap_or_else(|| BooleanArray::from(vec![true; batch.num_rows()])))
            }
            CompiledPredicate::Or(children) => {
                let mut verdict: Option<BooleanArray> = None;
                for child in children {
                    let this = child.evaluate(batch)?;
                    verdict = Some(match verdict {
                        None => this,
                        Some(previous) => arrow::compute::or_kleene(&previous, &this)?,
                    });
                }
                Ok(verdict.unwrap_or_else(|| BooleanArray::from(vec![false; batch.num_rows()])))
            }
            CompiledPredicate::Not(inner) => arrow::compute::not(&inner.evaluate(batch)?),
        }
    }

    /// The rows to keep: the verdict with nulls as false.
    pub(crate) fn selection(&self, batch: &RecordBatch) -> Result<BooleanArray, ArrowError> {
        let verdict = self.evaluate(batch)?;
        Ok(if verdict.null_count() > 0 {
            arrow::compute::prep_null_mask_filter(&verdict)
        } else {
            verdict
        })
    }
}

fn compile(
    predicate: &StoragePredicate,
    schema: &SchemaRef,
    exact: bool,
) -> Option<CompiledPredicate> {
    match predicate {
        StoragePredicate::Compare { column, op, value } => {
            let index = schema.index_of(column).ok()?;
            let literal = comparison_literal(value, schema.field(index).data_type())?;
            Some(CompiledPredicate::Compare {
                column: index,
                op: *op,
                literal: Scalar::new(literal),
            })
        }
        StoragePredicate::Like {
            column,
            pattern,
            negated,
            case_insensitive,
        } => {
            let index = schema.index_of(column).ok()?;
            let pattern = text_literal(pattern, schema.field(index).data_type())?;
            Some(CompiledPredicate::Like {
                column: index,
                pattern: Scalar::new(pattern),
                negated: *negated,
                case_insensitive: *case_insensitive,
            })
        }
        StoragePredicate::IsNull { column } => Some(CompiledPredicate::IsNull {
            column: schema.index_of(column).ok()?,
        }),
        StoragePredicate::IsNotNull { column } => Some(CompiledPredicate::IsNotNull {
            column: schema.index_of(column).ok()?,
        }),
        StoragePredicate::In { column, values } => {
            let index = schema.index_of(column).ok()?;
            let data_type = schema.field(index).data_type();
            let literals = values
                .iter()
                .map(|value| comparison_literal(value, data_type).map(Scalar::new))
                .collect::<Option<Vec<_>>>()?;
            Some(CompiledPredicate::In {
                column: index,
                literals,
            })
        }
        StoragePredicate::And(children) => {
            let compiled = children
                .iter()
                .map(|child| compile(child, schema, exact))
                .collect::<Vec<_>>();
            if exact && compiled.iter().any(Option::is_none) {
                return None;
            }
            let compiled = compiled.into_iter().flatten().collect::<Vec<_>>();
            match compiled.len() {
                0 => None,
                1 => compiled.into_iter().next(),
                _ => Some(CompiledPredicate::And(compiled)),
            }
        }
        StoragePredicate::Or(children) => {
            let compiled = children
                .iter()
                .map(|child| compile(child, schema, exact))
                .collect::<Option<Vec<_>>>()?;
            match compiled.len() {
                0 => None,
                1 => compiled.into_iter().next(),
                _ => Some(CompiledPredicate::Or(compiled)),
            }
        }
        StoragePredicate::Not(inner) => {
            compile(inner, schema, true).map(|inner| CompiledPredicate::Not(Box::new(inner)))
        }
    }
}

/// Compare a column with a literal. A dictionary column is compared through
/// its dictionary — once per distinct value — and the verdicts are taken
/// through the keys, so a 3 M-row batch of 26 countries costs 26 comparisons
/// and one gather rather than 3 M string comparisons.
fn compare_column(
    column: &ArrayRef,
    op: CompareOp,
    literal: &Scalar<ArrayRef>,
) -> Result<BooleanArray, ArrowError> {
    let compare = |values: &dyn Datum| match op {
        CompareOp::Eq => cmp::eq(values, literal),
        CompareOp::Ne => cmp::neq(values, literal),
        CompareOp::Lt => cmp::lt(values, literal),
        CompareOp::Le => cmp::lt_eq(values, literal),
        CompareOp::Gt => cmp::gt(values, literal),
        CompareOp::Ge => cmp::gt_eq(values, literal),
    };
    if let DataType::Dictionary(key_type, _) = column.data_type()
        && key_type.as_ref() == &DataType::Int32
    {
        let dictionary = column.as_dictionary::<Int32Type>();
        let verdicts = compare(dictionary.values())?;
        let gathered = arrow::compute::take(&verdicts, dictionary.keys(), None)?;
        return Ok(gathered.as_boolean().clone());
    }
    compare(column)
}

/// The literal as a one-element array of the column's own type, or None when
/// the kernels cannot compare the two. A dictionary column compares through
/// its dictionary: once per distinct value, then an index lookup per row.
fn comparison_literal(value: &ScalarValue, data_type: &DataType) -> Option<ArrayRef> {
    use arrow::array::{BooleanArray, Float64Array, Int64Array};
    Some(match (value, data_type) {
        (ScalarValue::Int64(value), DataType::Int64) => Arc::new(Int64Array::from(vec![*value])),
        // Narrower and unsigned integer columns, and day-number dates,
        // compare against the literal cast to the column's own type; a
        // literal outside that type's range is no pushdown at all.
        (
            ScalarValue::Int64(value),
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Date32,
        ) => {
            let literal = Int64Array::from(vec![*value]);
            arrow::compute::cast_with_options(
                &literal,
                data_type,
                &arrow::compute::CastOptions {
                    safe: false,
                    ..Default::default()
                },
            )
            .ok()?
        }
        (ScalarValue::Float64(value), DataType::Float64) => {
            Arc::new(Float64Array::from(vec![*value]))
        }
        (ScalarValue::Bool(value), DataType::Boolean) => Arc::new(BooleanArray::from(vec![*value])),
        (ScalarValue::Utf8(value), data_type) => return text_literal(value, data_type),
        _ => return None,
    })
}

/// A text literal as a one-element array of the column's text type, through
/// a dictionary column's value type.
fn text_literal(value: &str, data_type: &DataType) -> Option<ArrayRef> {
    use arrow::array::{LargeStringArray, StringArray};
    let data_type = match data_type {
        DataType::Dictionary(_, values) => values.as_ref(),
        other => other,
    };
    Some(match data_type {
        DataType::Utf8 => Arc::new(StringArray::from(vec![value])),
        DataType::LargeUtf8 => Arc::new(LargeStringArray::from(vec![value])),
        _ => return None,
    })
}

/// A predicate over decoded batches of one schema: the projected batch a
/// lane emits. Rows it rejects never leave the lane.
pub(crate) struct BatchPredicate {
    compiled: CompiledPredicate,
}

impl BatchPredicate {
    pub(crate) fn new(schema: &SchemaRef, predicate: &StoragePredicate) -> Option<Self> {
        CompiledPredicate::compile(predicate, schema).map(|compiled| Self { compiled })
    }

    pub(crate) fn apply(&self, batch: RecordBatch) -> parquet::errors::Result<RecordBatch> {
        let mask = self
            .compiled
            .selection(&batch)
            .map_err(|error| parquet::errors::ParquetError::External(Box::new(error)))?;
        if mask.true_count() == batch.num_rows() {
            return Ok(batch);
        }
        arrow::compute::filter_record_batch(&batch, &mask)
            .map_err(|error| parquet::errors::ParquetError::External(Box::new(error)))
    }
}

/// One conjunct of the scan predicate as a decoder stage: the columns it
/// reads, and the predicate compiled against a batch of exactly those
/// columns in file order.
pub(crate) struct RowFilterStage {
    columns: Vec<usize>,
    compiled: CompiledPredicate,
}

/// The decoder-side plan for a scan predicate: the stages in the order the
/// decoder runs them, cheapest column set first so the later stages see the
/// fewest rows.
pub(crate) struct RowFilterPlan {
    stages: Vec<RowFilterStage>,
}

impl RowFilterPlan {
    /// The stages of `predicate` over files of `schema`: one per top-level
    /// conjunct with an evaluable part, or None when there is none.
    pub(crate) fn new(predicate: &StoragePredicate, schema: &SchemaRef) -> Option<Self> {
        let conjuncts = match predicate {
            StoragePredicate::And(children) => children.iter().collect::<Vec<_>>(),
            other => vec![other],
        };
        let mut stages = Vec::new();
        for conjunct in conjuncts {
            // Compile first against the full schema to learn which columns
            // the evaluable part reads, then against those columns alone.
            let Some(over_file) = CompiledPredicate::compile(conjunct, schema) else {
                continue;
            };
            let mut columns = Vec::new();
            over_file.collect_columns(&mut columns);
            columns.sort_unstable();
            columns.dedup();
            let stage_schema = Arc::new(schema.project(&columns).ok()?);
            let compiled = CompiledPredicate::compile(conjunct, &stage_schema)?;
            stages.push(RowFilterStage { columns, compiled });
        }
        (!stages.is_empty()).then_some(Self { stages })
    }

    /// Every column a stage reads, each once, in file order.
    pub(crate) fn columns(&self) -> Vec<usize> {
        let mut columns = self
            .stages
            .iter()
            .flat_map(|stage| stage.columns.iter().copied())
            .collect::<Vec<_>>();
        columns.sort_unstable();
        columns.dedup();
        columns
    }

    /// Order the stages by the compressed bytes their columns hold in the
    /// selected row groups, smallest first.
    pub(crate) fn order_by_bytes(&mut self, metadata: &ParquetMetaData, groups: &[usize]) {
        self.stages.sort_by_key(|stage| {
            stage
                .columns
                .iter()
                .map(|column| column_bytes(metadata, groups, *column))
                .sum::<u64>()
        });
    }

    /// The parquet crate's row filter for one decoder: the stages in order,
    /// each over its own projection.
    pub(crate) fn row_filter(
        &self,
        parquet_schema: &SchemaDescriptor,
        metrics: &ScanMetrics,
    ) -> RowFilter {
        let last = self.stages.len().saturating_sub(1);
        RowFilter::new(
            self.stages
                .iter()
                .enumerate()
                .map(|(index, stage)| {
                    Box::new(StagePredicate {
                        projection: ProjectionMask::roots(parquet_schema, stage.columns.clone()),
                        compiled: stage.compiled.clone(),
                        metrics: metrics.clone(),
                        first: index == 0,
                        last: index == last,
                    }) as Box<dyn ArrowPredicate>
                })
                .collect(),
        )
    }
}

/// One stage inside the decoder: the exact verdict over its columns. The
/// selection is not coarsened: measured on the wide benchmark
/// (`scan_bench`), admitting skip runs under 32 rows decoded 7.5x the rows
/// for no wall-time gain, since every admitted row is decoded for the whole
/// projection and filtered again by the executor.
struct StagePredicate {
    projection: ProjectionMask,
    compiled: CompiledPredicate,
    metrics: ScanMetrics,
    first: bool,
    last: bool,
}

impl ArrowPredicate for StagePredicate {
    fn projection(&self) -> &ProjectionMask {
        &self.projection
    }

    fn evaluate(&mut self, batch: RecordBatch) -> Result<BooleanArray, ArrowError> {
        if self.first {
            self.metrics.row_filter_examined(batch.num_rows());
        }
        let selection = self.compiled.selection(&batch)?;
        if self.last {
            self.metrics.row_filter_admitted(selection.true_count());
        }
        Ok(selection)
    }
}

impl CompiledPredicate {
    fn collect_columns(&self, out: &mut Vec<usize>) {
        match self {
            CompiledPredicate::Compare { column, .. }
            | CompiledPredicate::Like { column, .. }
            | CompiledPredicate::IsNull { column }
            | CompiledPredicate::IsNotNull { column }
            | CompiledPredicate::In { column, .. } => out.push(*column),
            CompiledPredicate::And(children) | CompiledPredicate::Or(children) => {
                for child in children {
                    child.collect_columns(out);
                }
            }
            CompiledPredicate::Not(inner) => inner.collect_columns(out),
        }
    }
}

/// Compressed bytes of one column over the selected row groups.
fn column_bytes(metadata: &ParquetMetaData, groups: &[usize], column: usize) -> u64 {
    groups
        .iter()
        .map(|group| {
            metadata
                .row_group(*group)
                .column(column)
                .compressed_size()
                .max(0) as u64
        })
        .sum()
}

/// Whether a row filter over `filter_columns` pays for a projection of
/// `projected` (None: every column) over the selected row groups, by
/// `LATE_MATERIALISATION_RATIO`.
pub(crate) fn late_materialisation_pays(
    metadata: &ParquetMetaData,
    groups: &[usize],
    projected: Option<&[usize]>,
    filter_columns: &[usize],
) -> bool {
    let width = metadata.file_metadata().schema_descr().num_columns();
    let filter_bytes = filter_columns
        .iter()
        .map(|column| column_bytes(metadata, groups, *column))
        .sum::<u64>();
    let rest_bytes = (0..width)
        .filter(|column| projected.is_none_or(|projected| projected.contains(column)))
        .filter(|column| !filter_columns.contains(column))
        .map(|column| column_bytes(metadata, groups, column))
        .sum::<u64>();
    rest_bytes >= filter_bytes.saturating_mul(LATE_MATERIALISATION_RATIO)
}

/// The operator's choice for decoder-side filtering, `KAVEON_LATE_MATERIALISATION`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LateMaterialisation {
    /// By the byte ratio of the projection (`late_materialisation_pays`),
    /// and always for an object held in memory, where the second round
    /// reads memory.
    Auto,
    /// Whenever the predicate has an evaluable part.
    Always,
    /// Never: the lanes filter decoded batches instead.
    Never,
}

impl LateMaterialisation {
    /// The process-wide default from `KAVEON_LATE_MATERIALISATION`
    /// (`auto`, `always`, `never`; unset or unrecognised is `auto`).
    pub fn from_environment() -> Self {
        match std::env::var("KAVEON_LATE_MATERIALISATION")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("always" | "on") => Self::Always,
            Some("never" | "off") => Self::Never,
            _ => Self::Auto,
        }
    }

    /// Whether the row filter runs for this scan: `in_memory` says the
    /// object's bytes are held in the process, so a second fetch round
    /// costs nothing.
    pub(crate) fn applies(
        self,
        metadata: &ParquetMetaData,
        groups: &[usize],
        projected: Option<&[usize]>,
        filter_columns: &[usize],
        in_memory: bool,
    ) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => {
                in_memory || late_materialisation_pays(metadata, groups, projected, filter_columns)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray, StringDictionaryBuilder};
    use arrow::datatypes::{Field, Schema};

    fn batch() -> RecordBatch {
        let mut dictionary = StringDictionaryBuilder::<Int32Type>::new();
        for value in [
            Some("google.com"),
            Some("bing.com"),
            None,
            Some("Google Maps"),
        ] {
            dictionary.append_option(value);
        }
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("url", DataType::Utf8, true),
                Field::new(
                    "site",
                    DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                    true,
                ),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3, 4])),
                Arc::new(StringArray::from(vec![
                    Some("http://google.com/a"),
                    Some("http://bing.com"),
                    None,
                    Some("http://maps.Google.com"),
                ])),
                Arc::new(dictionary.finish()),
            ],
        )
        .unwrap()
    }

    fn like(
        column: &str,
        pattern: &str,
        negated: bool,
        case_insensitive: bool,
    ) -> StoragePredicate {
        StoragePredicate::Like {
            column: column.into(),
            pattern: pattern.into(),
            negated,
            case_insensitive,
        }
    }

    fn rows(predicate: &StoragePredicate) -> Vec<i64> {
        let batch = batch();
        let compiled = CompiledPredicate::compile(predicate, &batch.schema()).expect("compiles");
        let mask = compiled.selection(&batch).unwrap();
        let kept = arrow::compute::filter_record_batch(&batch, &mask).unwrap();
        kept.column(0)
            .as_primitive::<arrow::datatypes::Int64Type>()
            .values()
            .to_vec()
    }

    #[test]
    fn like_runs_on_plain_and_dictionary_text_with_sql_null_rules() {
        assert_eq!(rows(&like("url", "%google%", false, false)), vec![1]);
        assert_eq!(rows(&like("url", "%google%", false, true)), vec![1, 4]);
        // NOT LIKE never selects the null row.
        assert_eq!(rows(&like("url", "%google%", true, false)), vec![2, 4]);
        assert_eq!(rows(&like("site", "%oogle%", false, false)), vec![1, 4]);
        assert_eq!(rows(&like("site", "google%", true, true)), vec![2]);
        // LIKE over a non-text column has no storage form.
        assert!(
            CompiledPredicate::compile(&like("id", "1%", false, false), &batch().schema())
                .is_none()
        );
    }

    #[test]
    fn compositions_follow_three_valued_logic_and_soundness() {
        let is_null = StoragePredicate::IsNull {
            column: "url".into(),
        };
        let bing = StoragePredicate::Compare {
            column: "site".into(),
            op: CompareOp::Eq,
            value: ScalarValue::Utf8("bing.com".into()),
        };
        assert_eq!(rows(&is_null), vec![3]);
        assert_eq!(
            rows(&StoragePredicate::Or(vec![is_null.clone(), bing.clone()])),
            vec![2, 3]
        );
        // NOT (url LIKE '%google%') is null for the null row: not selected.
        assert_eq!(
            rows(&StoragePredicate::Not(Box::new(like(
                "url", "%google%", false, false
            )))),
            vec![2, 4]
        );
        // NOT over a conjunction with an unknown column is not compiled at
        // all; the same conjunction outside NOT keeps its known part.
        let with_unknown = StoragePredicate::And(vec![
            bing.clone(),
            StoragePredicate::IsNull {
                column: "missing".into(),
            },
        ]);
        assert!(
            CompiledPredicate::compile(
                &StoragePredicate::Not(Box::new(with_unknown.clone())),
                &batch().schema()
            )
            .is_none()
        );
        assert_eq!(rows(&with_unknown), vec![2]);
        assert_eq!(
            rows(&StoragePredicate::In {
                column: "site".into(),
                values: vec![
                    ScalarValue::Utf8("bing.com".into()),
                    ScalarValue::Utf8("Google Maps".into())
                ],
            }),
            vec![2, 4]
        );
        assert_eq!(
            rows(&StoragePredicate::Not(Box::new(StoragePredicate::In {
                column: "site".into(),
                values: vec![ScalarValue::Utf8("bing.com".into())],
            }))),
            vec![1, 4]
        );
    }

    #[test]
    fn stages_are_one_per_conjunct_over_their_own_columns() {
        let schema = batch().schema();
        let predicate = StoragePredicate::And(vec![
            like("url", "%google%", false, false),
            StoragePredicate::Or(vec![
                StoragePredicate::Compare {
                    column: "id".into(),
                    op: CompareOp::Gt,
                    value: ScalarValue::Int64(1),
                },
                StoragePredicate::IsNull {
                    column: "site".into(),
                },
            ]),
            StoragePredicate::IsNull {
                column: "missing".into(),
            },
        ]);
        let plan = RowFilterPlan::new(&predicate, &schema).expect("two stages");
        assert_eq!(plan.stages.len(), 2);
        assert_eq!(plan.stages[0].columns, vec![1]);
        assert_eq!(plan.stages[1].columns, vec![0, 2]);
        assert_eq!(plan.columns(), vec![0, 1, 2]);
        // Each stage evaluates against a batch of its own columns only.
        let batch = batch();
        let second = batch.project(&[0, 2]).unwrap();
        let mask = plan.stages[1].compiled.selection(&second).unwrap();
        assert_eq!(mask.true_count(), 3);
        assert!(
            RowFilterPlan::new(
                &StoragePredicate::IsNull {
                    column: "missing".into()
                },
                &schema
            )
            .is_none()
        );
    }
}
