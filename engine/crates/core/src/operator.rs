use crate::{Result, ScalarValue, StoragePredicate};
use arrow::record_batch::RecordBatch;

pub trait BatchSource {
    fn schema(&self) -> &arrow::datatypes::SchemaRef;
    fn next_batch(&mut self) -> Result<Option<RecordBatch>>;

    /// Give a source an exact build-side key filter before it starts reading.
    /// Storage implementations may use this to prune files or row groups;
    /// the default keeps non-storage sources correct by declining the push.
    fn apply_runtime_filter(&mut self, _column: &str, _values: &[ScalarValue]) -> Result<bool> {
        Ok(false)
    }

    /// Apply a safe conjunction of exact single-column `IN` predicates. A
    /// source may accept every child and prune before decoding; unsupported
    /// predicate shapes decline so the executor keeps its exact row filter.
    fn apply_runtime_predicate(&mut self, predicate: &StoragePredicate) -> Result<bool> {
        match predicate {
            StoragePredicate::In { column, values } => self.apply_runtime_filter(column, values),
            StoragePredicate::And(children) if !children.is_empty() => {
                let mut accepted = true;
                for child in children {
                    accepted &= self.apply_runtime_predicate(child)?;
                }
                Ok(accepted)
            }
            _ => Ok(false),
        }
    }
}

pub trait BatchOperator {
    fn schema(&self) -> &arrow::datatypes::SchemaRef;
    fn next_batch(&mut self) -> Result<Option<RecordBatch>>;

    /// Forward an exact build-side filter to a not-yet-started scan when the
    /// operator owns one. Operators that cannot push it keep the existing
    /// batch-level runtime-filter path.
    fn apply_runtime_filter(&mut self, _column: &str, _values: &[ScalarValue]) -> Result<bool> {
        Ok(false)
    }

    fn apply_runtime_predicate(&mut self, predicate: &StoragePredicate) -> Result<bool> {
        match predicate {
            StoragePredicate::In { column, values } => self.apply_runtime_filter(column, values),
            StoragePredicate::And(children) if !children.is_empty() => {
                let mut accepted = true;
                for child in children {
                    accepted &= self.apply_runtime_predicate(child)?;
                }
                Ok(accepted)
            }
            _ => Ok(false),
        }
    }
}

pub fn collect_batches(source: &mut dyn BatchOperator) -> Result<Vec<RecordBatch>> {
    let mut out = Vec::new();
    while let Some(batch) = source.next_batch()? {
        out.push(batch);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RecordingSource {
        accepted: usize,
    }

    impl BatchSource for RecordingSource {
        fn schema(&self) -> &arrow::datatypes::SchemaRef {
            unreachable!("schema is not used by this contract test")
        }

        fn next_batch(&mut self) -> Result<Option<arrow::record_batch::RecordBatch>> {
            Ok(None)
        }

        fn apply_runtime_filter(&mut self, _column: &str, _values: &[ScalarValue]) -> Result<bool> {
            self.accepted += 1;
            Ok(true)
        }
    }

    #[test]
    fn applies_every_child_of_an_and_runtime_predicate() {
        let mut source = RecordingSource { accepted: 0 };
        let predicate = StoragePredicate::And(vec![
            StoragePredicate::In {
                column: "a".into(),
                values: vec![ScalarValue::Int64(1)],
            },
            StoragePredicate::In {
                column: "b".into(),
                values: vec![ScalarValue::Utf8("x".into())],
            },
        ]);
        assert!(source.apply_runtime_predicate(&predicate).unwrap());
        assert_eq!(source.accepted, 2);
    }
}
