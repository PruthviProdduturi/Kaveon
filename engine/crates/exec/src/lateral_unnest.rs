use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, GenericListArray, Int64Array, UInt32Array, new_empty_array, new_null_array,
};
use arrow::compute::{concat, kernels::take::take};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use kaveon_core::{BatchOperator, Expr, KaveonError, Result};

use crate::expr_eval::{evaluate, resolve_column_index};

/// Expands array expressions once for every row from the input relation.
/// Multiple arrays use Trino's null-padding rule, and each input row is
/// repeated for the length of its longest array. Empty arrays produce no row
/// for that input row, matching CROSS JOIN LATERAL semantics.
pub struct LateralUnnestOperator {
    input: Box<dyn BatchOperator>,
    expressions: Vec<Expr>,
    ordinality: bool,
    ordinality_start: i64,
    schema: SchemaRef,
}

impl LateralUnnestOperator {
    pub fn new(
        input: Box<dyn BatchOperator>,
        expressions: Vec<Expr>,
        columns: Vec<String>,
        ordinality: bool,
        ordinality_column: impl Into<String>,
        ordinality_start: i64,
    ) -> Result<Self> {
        if expressions.is_empty() || expressions.len() != columns.len() {
            return Err(KaveonError::Execution(
                "lateral UNNEST requires one output column for each ARRAY expression".into(),
            ));
        }
        let input_schema = input.schema().clone();
        let empty = RecordBatch::new_empty(input_schema.clone());
        let mut fields = input_schema.fields().iter().cloned().collect::<Vec<_>>();
        for (expression, column) in expressions.iter().zip(&columns) {
            let element_type = array_element_type(expression, &input_schema, &empty)?;
            fields.push(Field::new(column, element_type, true).into());
        }
        if ordinality {
            fields.push(Field::new(ordinality_column, DataType::Int64, false).into());
        }
        Ok(Self {
            input,
            expressions,
            ordinality,
            ordinality_start,
            schema: Arc::new(Schema::new(fields)),
        })
    }
}

impl BatchOperator for LateralUnnestOperator {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        let Some(input) = self.input.next_batch()? else {
            return Ok(None);
        };
        let arrays = self
            .expressions
            .iter()
            .map(|expression| evaluate(expression, &input))
            .collect::<Result<Vec<_>>>()?;
        let mut lists = Vec::with_capacity(arrays.len());
        for array in arrays {
            lists.push(list_values(&array)?);
        }

        let mut row_indices = Vec::new();
        let mut ordinals = Vec::new();
        let mut element_chunks = (0..lists.len())
            .map(|_| Vec::<ArrayRef>::new())
            .collect::<Vec<_>>();
        let mut element_types = Vec::with_capacity(lists.len());
        for list in &lists {
            element_types.push(list.element_type.clone());
        }

        for row in 0..input.num_rows() {
            let lengths = lists
                .iter()
                .map(|list| list.row_len(row))
                .collect::<Vec<_>>();
            let width = lengths.iter().copied().max().unwrap_or(0);
            for (index, list) in lists.iter().enumerate() {
                let value = list.row_value(row, width)?;
                element_chunks[index].push(value);
            }
            for offset in 0..width {
                row_indices.push(row as u32);
                if self.ordinality {
                    ordinals.push(self.ordinality_start + offset as i64);
                }
            }
        }

        let indices = UInt32Array::from(row_indices);
        let mut output = input
            .columns()
            .iter()
            .map(|column| take(column.as_ref(), &indices, None))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (chunks, element_type) in element_chunks.into_iter().zip(element_types) {
            output.push(concat_arrays(&chunks, &element_type)?);
        }
        if self.ordinality {
            output.push(Arc::new(Int64Array::from(ordinals)) as ArrayRef);
        }
        Ok(Some(RecordBatch::try_new(self.schema.clone(), output)?))
    }
}

struct ListValues {
    values: ArrayRef,
    offsets: Vec<usize>,
    validity: Option<Vec<bool>>,
    element_type: DataType,
}

impl ListValues {
    fn row_len(&self, row: usize) -> usize {
        if self.validity.as_ref().is_some_and(|valid| !valid[row]) {
            return 0;
        }
        self.offsets[row + 1].saturating_sub(self.offsets[row])
    }

    fn row_value(&self, row: usize, width: usize) -> Result<ArrayRef> {
        let length = self.row_len(row);
        if length > 0 {
            let start = self.offsets[row];
            let value = self.values.slice(start, length);
            if length == width {
                return Ok(value);
            }
            let nulls = new_null_array(&self.element_type, width - length);
            return concat_arrays(&[value, nulls], &self.element_type);
        }
        Ok(new_null_array(&self.element_type, width))
    }
}

fn list_values(array: &ArrayRef) -> Result<ListValues> {
    match array.data_type() {
        DataType::List(field) => {
            let list = array
                .as_any()
                .downcast_ref::<GenericListArray<i32>>()
                .ok_or_else(|| KaveonError::Execution("invalid LIST array".into()))?;
            Ok(ListValues {
                values: list.values().clone(),
                offsets: list
                    .value_offsets()
                    .iter()
                    .map(|offset| *offset as usize)
                    .collect(),
                validity: Some((0..list.len()).map(|row| list.is_valid(row)).collect()),
                element_type: field.data_type().clone(),
            })
        }
        DataType::LargeList(field) => {
            let list = array
                .as_any()
                .downcast_ref::<GenericListArray<i64>>()
                .ok_or_else(|| KaveonError::Execution("invalid LARGE_LIST array".into()))?;
            Ok(ListValues {
                values: list.values().clone(),
                offsets: list
                    .value_offsets()
                    .iter()
                    .map(|offset| *offset as usize)
                    .collect(),
                validity: Some((0..list.len()).map(|row| list.is_valid(row)).collect()),
                element_type: field.data_type().clone(),
            })
        }
        other => Err(KaveonError::Execution(format!(
            "lateral UNNEST requires an ARRAY value, found {other}"
        ))),
    }
}

fn array_element_type(
    expression: &Expr,
    schema: &SchemaRef,
    empty: &RecordBatch,
) -> Result<DataType> {
    let array_type = match expression {
        Expr::Column(name) => {
            let index = resolve_column_index(schema, name)?;
            schema.field(index).data_type().clone()
        }
        Expr::Alias { expr, .. } => return array_element_type(expr, schema, empty),
        _ => evaluate(expression, empty)?.data_type().clone(),
    };
    match array_type {
        DataType::List(field) | DataType::LargeList(field) => Ok(field.data_type().clone()),
        other => Err(KaveonError::Execution(format!(
            "lateral UNNEST requires an ARRAY value, found {other}"
        ))),
    }
}

fn concat_arrays(chunks: &[ArrayRef], data_type: &DataType) -> Result<ArrayRef> {
    if chunks.is_empty() {
        return Ok(new_empty_array(data_type));
    }
    let refs = chunks
        .iter()
        .map(|chunk| chunk.as_ref())
        .collect::<Vec<_>>();
    Ok(concat(&refs)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Builder, ListBuilder};

    struct OneBatch {
        schema: SchemaRef,
        batch: Option<RecordBatch>,
    }

    impl BatchOperator for OneBatch {
        fn schema(&self) -> &SchemaRef {
            &self.schema
        }

        fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
            Ok(self.batch.take())
        }
    }

    #[test]
    fn expands_each_input_array_and_repeats_left_rows() {
        let mut values = ListBuilder::new(Int64Builder::new());
        values.values().append_value(1);
        values.values().append_value(2);
        values.append(true);
        values.values().append_value(3);
        values.append(true);
        let array = Arc::new(values.finish()) as ArrayRef;
        let schema = Arc::new(Schema::new(vec![Field::new(
            "arr",
            array.data_type().clone(),
            true,
        )]));
        let batch = RecordBatch::try_new(schema.clone(), vec![array]).unwrap();
        let mut operator = LateralUnnestOperator::new(
            Box::new(OneBatch {
                schema,
                batch: Some(batch),
            }),
            vec![Expr::Column("arr".into())],
            vec!["value".into()],
            true,
            "ord",
            1,
        )
        .unwrap();
        let output = operator.next_batch().unwrap().unwrap();
        assert_eq!(output.num_rows(), 3);
        assert_eq!(output.schema().field(0).name(), "arr");
        assert_eq!(output.schema().field(1).name(), "value");
        assert_eq!(output.schema().field(2).name(), "ord");
        assert_eq!(
            output
                .column(2)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values(),
            &[1, 2, 1]
        );
        assert!(operator.next_batch().unwrap().is_none());
    }
}
