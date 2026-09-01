use thiserror::Error;

#[derive(Debug, Error)]
pub enum KaveonError {
    #[error("storage: {0}")]
    Storage(String),

    #[error("execution: {0}")]
    Execution(String),

    #[error("sql: {0}")]
    Sql(String),

    #[error("arrow: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, KaveonError>;
