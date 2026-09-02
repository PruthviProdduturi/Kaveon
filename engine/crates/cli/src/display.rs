use arrow::array::{Array, AsArray};
use arrow::datatypes::{
    DataType, Float32Type, Float64Type, Int8Type, Int16Type, Int32Type, Int64Type, UInt8Type,
    UInt16Type, UInt32Type, UInt64Type,
};
use arrow::record_batch::RecordBatch;
use std::cmp;

const MAX_COL_WIDTH: usize = 40;
const NULL_DISPLAY: &str = "NULL";

pub fn format_batches(batches: &[RecordBatch]) -> String {
    if batches.is_empty() {
        return "(0 rows)\n".to_owned();
    }

    let schema = batches[0].schema();
    let num_cols = schema.fields().len();
    if num_cols == 0 {
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        return format!("({total} rows)\n");
    }

    let headers: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();

    let mut all_rows: Vec<Vec<String>> = Vec::new();
    for batch in batches {
        for row in 0..batch.num_rows() {
            let mut cells = Vec::with_capacity(num_cols);
            for col in 0..num_cols {
                cells.push(format_cell(batch.column(col), row));
            }
            all_rows.push(cells);
        }
    }

    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in &all_rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = cmp::max(widths[i], cell.len());
        }
    }
    for w in &mut widths {
        *w = cmp::min(*w, MAX_COL_WIDTH);
    }

    let mut out = String::new();
    out.push_str(&separator(&widths));
    out.push_str(&format_row(&headers, &widths));
    out.push_str(&separator(&widths));
    for row in &all_rows {
        out.push_str(&format_row(row, &widths));
    }
    out.push_str(&separator(&widths));
    out.push_str(&format!("({} rows)\n", all_rows.len()));
    out
}

fn separator(widths: &[usize]) -> String {
    let mut s = String::from("+");
    for w in widths {
        s.push_str(&"-".repeat(w + 2));
        s.push('+');
    }
    s.push('\n');
    s
}

fn format_row(cells: &[String], widths: &[usize]) -> String {
    let mut s = String::from("|");
    for (i, cell) in cells.iter().enumerate() {
        let w = widths[i];
        let display = if cell.len() > w {
            format!("{}...", &cell[..w.saturating_sub(3)])
        } else {
            cell.clone()
        };
        s.push_str(&format!(" {display:<w$} |", w = w));
    }
    s.push('\n');
    s
}

fn format_cell(array: &dyn Array, row: usize) -> String {
    if array.is_null(row) {
        return NULL_DISPLAY.to_owned();
    }
    match array.data_type() {
        DataType::Boolean => array
            .as_any()
            .downcast_ref::<arrow::array::BooleanArray>()
            .unwrap()
            .value(row)
            .to_string(),
        DataType::Int8 => array.as_primitive::<Int8Type>().value(row).to_string(),
        DataType::Int16 => array.as_primitive::<Int16Type>().value(row).to_string(),
        DataType::Int32 => array.as_primitive::<Int32Type>().value(row).to_string(),
        DataType::Int64 => array.as_primitive::<Int64Type>().value(row).to_string(),
        DataType::UInt8 => array.as_primitive::<UInt8Type>().value(row).to_string(),
        DataType::UInt16 => array.as_primitive::<UInt16Type>().value(row).to_string(),
        DataType::UInt32 => array.as_primitive::<UInt32Type>().value(row).to_string(),
        DataType::UInt64 => array.as_primitive::<UInt64Type>().value(row).to_string(),
        DataType::Float32 => format!("{:.4}", array.as_primitive::<Float32Type>().value(row)),
        DataType::Float64 => format!("{:.4}", array.as_primitive::<Float64Type>().value(row)),
        DataType::Utf8 => array.as_string::<i32>().value(row).to_owned(),
        DataType::LargeUtf8 => array.as_string::<i64>().value(row).to_owned(),
        _ => format!("{:?}", array.slice(row, 1)),
    }
}
