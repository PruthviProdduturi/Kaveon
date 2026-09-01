pub mod error;
pub mod operator;
pub mod predicate;
pub mod types;

pub use error::{KaveonError, Result};
pub use operator::{BatchOperator, BatchSource, collect_batches};
pub use predicate::{CompareOp, ScalarValue, StoragePredicate};
