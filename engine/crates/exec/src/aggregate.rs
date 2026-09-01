use arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, Float64Array, Int32Array, Int64Array,
    StringArray, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Float64Type, Int32Type, Int64Type, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, KaveonError, Result};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub enum AggFunc {
    Sum,
    Count,
    Min,
    Max,
    Avg,
}

#[derive(Debug, Clone)]
pub struct AggExpr {
    pub func: AggFunc,
    pub column: String,
    pub alias: Option<String>,
}

impl AggExpr {
    pub fn new(func: AggFunc, column: impl Into<String>) -> Self {
        Self {
            func,
            column: column.into(),
            alias: None,
        }
    }

    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.alias = Some(alias.into());
        self
    }

    fn output_name(&self) -> String {
        if let Some(ref alias) = self.alias {
            return alias.clone();
        }
        let func_name = match self.func {
            AggFunc::Sum => "sum",
            AggFunc::Count => "count",
            AggFunc::Min => "min",
            AggFunc::Max => "max",
            AggFunc::Avg => "avg",
        };
        format!("{func_name}_{}", self.column)
    }

    fn output_type(&self) -> DataType {
        match self.func {
            AggFunc::Count => DataType::UInt64,
            _ => DataType::Float64,
        }
    }
}

struct Accumulator {
    sum: f64,
    count: u64,
    min: f64,
    max: f64,
}

impl Accumulator {
    fn new() -> Self {
        Self {
            sum: 0.0,
            count: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    fn update(&mut self, value: f64) {
        self.sum += value;
        self.count += 1;
        if value < self.min {
            self.min = value;
        }
        if value > self.max {
            self.max = value;
        }
    }

    fn result(&self, func: AggFunc) -> f64 {
        match func {
            AggFunc::Sum => self.sum,
            AggFunc::Count => self.count as f64,
            AggFunc::Min => {
                if self.count == 0 {
                    0.0
                } else {
                    self.min
                }
            }
            AggFunc::Max => {
                if self.count == 0 {
                    0.0
                } else {
                    self.max
                }
            }
            AggFunc::Avg => {
                if self.count == 0 {
                    0.0
                } else {
                    self.sum / self.count as f64
                }
            }
        }
    }
}

pub struct HashAggregate {
    source: Box<dyn BatchOperator>,
    group_by: Vec<String>,
    aggregates: Vec<AggExpr>,
    output_schema: SchemaRef,
    emitted: bool,
}

impl HashAggregate {
    pub fn new(
        source: Box<dyn BatchOperator>,
        group_by: Vec<String>,
        aggregates: Vec<AggExpr>,
    ) -> Result<Self> {
        if aggregates.is_empty() {
            return Err(KaveonError::Execution(
                "at least one aggregate expression required".into(),
            ));
        }
        let source_schema = source.schema().clone();
        for col in &group_by {
            source_schema
                .index_of(col)
                .map_err(|_| exec_err(format!("group-by column '{col}' not in input")))?;
        }
        for agg in &aggregates {
            if !matches!(agg.func, AggFunc::Count) {
                source_schema.index_of(&agg.column).map_err(|_| {
                    exec_err(format!("aggregate column '{}' not in input", agg.column))
                })?;
            }
        }

        let mut fields: Vec<Field> = group_by
            .iter()
            .map(|col| {
                let f = source_schema.field_with_name(col).unwrap();
                f.clone()
            })
            .collect();
        for agg in &aggregates {
            fields.push(Field::new(agg.output_name(), agg.output_type(), true));
        }
        let output_schema = Arc::new(Schema::new(fields));

        Ok(Self {
            source,
            group_by,
            aggregates,
            output_schema,
            emitted: false,
        })
    }
}

impl BatchOperator for HashAggregate {
    fn schema(&self) -> &SchemaRef {
        &self.output_schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.emitted {
            return Ok(None);
        }
        self.emitted = true;

        let mut groups: HashMap<Vec<GroupKey>, Vec<Accumulator>> = HashMap::new();
        let num_aggs = self.aggregates.len();

        while let Some(batch) = self.source.next_batch()? {
            let schema = batch.schema();

            let group_arrays: Vec<&ArrayRef> = self
                .group_by
                .iter()
                .map(|col| {
                    let idx = schema.index_of(col).unwrap();
                    batch.column(idx)
                })
                .collect();

            let agg_arrays: Vec<Option<&ArrayRef>> = self
                .aggregates
                .iter()
                .map(|agg| {
                    if matches!(agg.func, AggFunc::Count) && agg.column == "*" {
                        None
                    } else {
                        Some(batch.column(schema.index_of(&agg.column).unwrap()))
                    }
                })
                .collect();

            for row in 0..batch.num_rows() {
                let key: Vec<GroupKey> =
                    group_arrays.iter().map(|arr| extract_key(arr, row)).collect();

                let accums = groups
                    .entry(key)
                    .or_insert_with(|| (0..num_aggs).map(|_| Accumulator::new()).collect());

                for (i, agg) in self.aggregates.iter().enumerate() {
                    if matches!(agg.func, AggFunc::Count) && agg.column == "*" {
                        accums[i].count += 1;
                    } else if let Some(arr) = agg_arrays[i] {
                        if !arr.is_null(row) {
                            let val = extract_f64(arr, row);
                            accums[i].update(val);
                        }
                    }
                }
            }
        }

        if groups.is_empty() && !self.group_by.is_empty() {
            return Ok(Some(RecordBatch::new_empty(self.output_schema.clone())));
        }

        let num_rows = if groups.is_empty() { 1 } else { groups.len() };
        let entries: Vec<(Vec<GroupKey>, Vec<Accumulator>)> = if groups.is_empty() {
            vec![(vec![], (0..num_aggs).map(|_| Accumulator::new()).collect())]
        } else {
            groups.into_iter().collect()
        };

        let mut columns: Vec<ArrayRef> = Vec::with_capacity(self.group_by.len() + num_aggs);

        for (gi, col_name) in self.group_by.iter().enumerate() {
            let field = self.output_schema.field_with_name(col_name).unwrap();
            let arr = build_group_column(&entries, gi, num_rows, field.data_type());
            columns.push(arr);
        }

        for (ai, agg) in self.aggregates.iter().enumerate() {
            match agg.output_type() {
                DataType::UInt64 => {
                    let values: Vec<u64> = entries
                        .iter()
                        .map(|(_, accums)| accums[ai].count)
                        .collect();
                    columns.push(Arc::new(UInt64Array::from(values)));
                }
                _ => {
                    let values: Vec<f64> = entries
                        .iter()
                        .map(|(_, accums)| accums[ai].result(agg.func))
                        .collect();
                    columns.push(Arc::new(Float64Array::from(values)));
                }
            }
        }

        let batch = RecordBatch::try_new(self.output_schema.clone(), columns)?;
        Ok(Some(batch))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum GroupKey {
    Null,
    Bool(bool),
    Int32(i32),
    Int64(i64),
    Utf8(String),
    Float64Bits(u64),
}

fn extract_key(arr: &ArrayRef, row: usize) -> GroupKey {
    if arr.is_null(row) {
        return GroupKey::Null;
    }
    match arr.data_type() {
        DataType::Boolean => {
            GroupKey::Bool(arr.as_any().downcast_ref::<BooleanArray>().unwrap().value(row))
        }
        DataType::Int32 => GroupKey::Int32(arr.as_primitive::<Int32Type>().value(row)),
        DataType::Int64 => GroupKey::Int64(arr.as_primitive::<Int64Type>().value(row)),
        DataType::Float64 => {
            GroupKey::Float64Bits(arr.as_primitive::<Float64Type>().value(row).to_bits())
        }
        DataType::Utf8 => GroupKey::Utf8(arr.as_string::<i32>().value(row).to_owned()),
        DataType::LargeUtf8 => GroupKey::Utf8(arr.as_string::<i64>().value(row).to_owned()),
        _ => GroupKey::Utf8(format!("{:?}", arr.slice(row, 1))),
    }
}

fn extract_f64(arr: &ArrayRef, row: usize) -> f64 {
    match arr.data_type() {
        DataType::Float64 => arr.as_primitive::<Float64Type>().value(row),
        DataType::Float32 => {
            arr.as_primitive::<arrow::datatypes::Float32Type>().value(row) as f64
        }
        DataType::Int64 => arr.as_primitive::<Int64Type>().value(row) as f64,
        DataType::Int32 => arr.as_primitive::<Int32Type>().value(row) as f64,
        DataType::UInt64 => {
            arr.as_primitive::<arrow::datatypes::UInt64Type>().value(row) as f64
        }
        _ => 0.0,
    }
}

fn build_group_column(
    entries: &[(Vec<GroupKey>, Vec<Accumulator>)],
    group_index: usize,
    num_rows: usize,
    data_type: &DataType,
) -> ArrayRef {
    match data_type {
        DataType::Int32 => {
            let values: Vec<Option<i32>> = entries
                .iter()
                .map(|(keys, _)| match &keys[group_index] {
                    GroupKey::Int32(v) => Some(*v),
                    GroupKey::Null => None,
                    _ => None,
                })
                .collect();
            Arc::new(Int32Array::from(values))
        }
        DataType::Int64 => {
            let values: Vec<Option<i64>> = entries
                .iter()
                .map(|(keys, _)| match &keys[group_index] {
                    GroupKey::Int64(v) => Some(*v),
                    GroupKey::Null => None,
                    _ => None,
                })
                .collect();
            Arc::new(Int64Array::from(values))
        }
        DataType::Float64 => {
            let values: Vec<Option<f64>> = entries
                .iter()
                .map(|(keys, _)| match &keys[group_index] {
                    GroupKey::Float64Bits(bits) => Some(f64::from_bits(*bits)),
                    GroupKey::Null => None,
                    _ => None,
                })
                .collect();
            Arc::new(Float64Array::from(values))
        }
        DataType::Boolean => {
            let values: Vec<Option<bool>> = entries
                .iter()
                .map(|(keys, _)| match &keys[group_index] {
                    GroupKey::Bool(v) => Some(*v),
                    GroupKey::Null => None,
                    _ => None,
                })
                .collect();
            Arc::new(BooleanArray::from(values))
        }
        _ => {
            let values: Vec<Option<&str>> = entries
                .iter()
                .map(|(keys, _)| match &keys[group_index] {
                    GroupKey::Utf8(v) => Some(v.as_str()),
                    GroupKey::Null => None,
                    _ => None,
                })
                .collect();
            Arc::new(StringArray::from(values))
        }
    }
}

fn exec_err(msg: impl Into<String>) -> KaveonError {
    KaveonError::Execution(msg.into())
}
