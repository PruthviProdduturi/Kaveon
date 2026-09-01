pub mod error;
pub mod expr;
pub mod operator;
pub mod predicate;
pub mod types;

pub use error::{KaveonError, Result};
pub use expr::{BinaryOp, Expr};
pub use operator::{BatchOperator, BatchSource, collect_batches};
pub use predicate::{CompareOp, ScalarValue, StoragePredicate};
