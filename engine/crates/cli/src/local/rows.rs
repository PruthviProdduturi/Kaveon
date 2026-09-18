//! Arrow batches to the JSON row shape the coordinator serves.
//!
//! Mirrors the server's `batches_to_json` so an embedded result and a remote
//! result look identical to the renderers: integers and floats as JSON
//! numbers, strings, booleans, nulls; dictionary columns presented as their
//! values; anything else via its Arrow display string.

use arrow::array::{Array, ArrayRef, AsArray};
use arrow::datatypes::{
    DataType, Float32Type, Float64Type, Int8Type, Int16Type, Int32Type, Int64Type, Schema,
    UInt8Type, UInt16Type, UInt32Type, UInt64Type,
};
use arrow::record_batch::RecordBatch;
use serde_json::Value;

/// `(name, type)` per field, with the type spelled the way the coordinator
/// presents it: a dictionary column is its value type.
pub fn columns_of(schema: &Schema) -> Vec<(String, String)> {
    schema
        .fields()
        .iter()
        .map(|field| (field.name().clone(), presented_type(field.data_type())))
        .collect()
}

pub fn presented_type(data_type: &DataType) -> String {
    match data_type {
        DataType::Dictionary(_, values) => values.to_string(),
        other => other.to_string(),
    }
}

pub fn batches_to_rows(batches: &[RecordBatch]) -> Vec<Vec<Value>> {
    let mut rows = Vec::with_capacity(batches.iter().map(RecordBatch::num_rows).sum());
    for batch in batches {
        let columns: Vec<ArrayRef> = batch
            .columns()
            .iter()
            .map(|column| match column.data_type() {
                DataType::Dictionary(_, values) => {
                    arrow::compute::cast(column, values).unwrap_or_else(|_| column.clone())
                }
                _ => column.clone(),
            })
            .collect();
        for row in 0..batch.num_rows() {
            rows.push(columns.iter().map(|column| cell(column, row)).collect());
        }
    }
    rows
}

fn cell(array: &ArrayRef, row: usize) -> Value {
    if array.is_null(row) {
        return Value::Null;
    }
    match array.data_type() {
        DataType::Boolean => Value::Bool(array.as_boolean().value(row)),
        DataType::Int8 => Value::from(array.as_primitive::<Int8Type>().value(row)),
        DataType::Int16 => Value::from(array.as_primitive::<Int16Type>().value(row)),
        DataType::Int32 => Value::from(array.as_primitive::<Int32Type>().value(row)),
        DataType::Int64 => Value::from(array.as_primitive::<Int64Type>().value(row)),
        DataType::UInt8 => Value::from(array.as_primitive::<UInt8Type>().value(row)),
        DataType::UInt16 => Value::from(array.as_primitive::<UInt16Type>().value(row)),
        DataType::UInt32 => Value::from(array.as_primitive::<UInt32Type>().value(row)),
        DataType::UInt64 => Value::from(array.as_primitive::<UInt64Type>().value(row)),
        DataType::Float32 => Value::from(array.as_primitive::<Float32Type>().value(row)),
        DataType::Float64 => Value::from(array.as_primitive::<Float64Type>().value(row)),
        DataType::Utf8 => Value::String(array.as_string::<i32>().value(row).to_owned()),
        DataType::LargeUtf8 => Value::String(array.as_string::<i64>().value(row).to_owned()),
        _ => Value::String(format!("{:?}", array.slice(row, 1))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{BooleanArray, Float64Array, Int64Array, StringArray};
    use arrow::datatypes::Field;
    use serde_json::json;
    use std::sync::Arc;

    fn batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("score", DataType::Float64, true),
            Field::new("name", DataType::Utf8, true),
            Field::new("active", DataType::Boolean, true),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![Some(1), Some(-7), None])),
                Arc::new(Float64Array::from(vec![Some(1.5), None, Some(0.0)])),
                Arc::new(StringArray::from(vec![Some("a"), Some(""), None])),
                Arc::new(BooleanArray::from(vec![Some(true), None, Some(false)])),
            ],
        )
        .expect("test batch should build")
    }

    #[test]
    fn columns_carry_arrow_type_names() {
        let batch = batch();
        assert_eq!(
            columns_of(&batch.schema()),
            vec![
                ("id".to_owned(), "Int64".to_owned()),
                ("score".to_owned(), "Float64".to_owned()),
                ("name".to_owned(), "Utf8".to_owned()),
                ("active".to_owned(), "Boolean".to_owned()),
            ]
        );
    }

    #[test]
    fn rows_follow_the_coordinator_shape() {
        let rows = batches_to_rows(&[batch()]);
        assert_eq!(
            rows,
            vec![
                vec![json!(1), json!(1.5), json!("a"), json!(true)],
                vec![json!(-7), Value::Null, json!(""), Value::Null],
                vec![Value::Null, json!(0.0), Value::Null, json!(false)],
            ]
        );
        assert!(rows[0][0].is_i64());
        assert!(rows[0][1].is_f64());
    }

    #[test]
    fn rows_span_batches_and_tolerate_none() {
        assert!(batches_to_rows(&[]).is_empty());
        assert_eq!(batches_to_rows(&[batch(), batch()]).len(), 6);
    }

    #[test]
    fn dictionary_columns_present_their_values() {
        use arrow::array::DictionaryArray;
        use arrow::datatypes::Int32Type;
        let dictionary: DictionaryArray<Int32Type> =
            vec![Some("x"), None, Some("y")].into_iter().collect();
        let schema = Arc::new(Schema::new(vec![Field::new(
            "tag",
            dictionary.data_type().clone(),
            true,
        )]));
        let batch = RecordBatch::try_new(schema, vec![Arc::new(dictionary)])
            .expect("dictionary batch should build");
        assert_eq!(
            columns_of(&batch.schema()),
            vec![("tag".to_owned(), "Utf8".to_owned())]
        );
        assert_eq!(
            batches_to_rows(&[batch]),
            vec![vec![json!("x")], vec![Value::Null], vec![json!("y")]]
        );
    }
}
