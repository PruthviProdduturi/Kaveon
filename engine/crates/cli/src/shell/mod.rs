//! The interactive shell.
pub mod app;
pub mod commands;
pub mod complete;
pub mod editor;
pub mod highlight;
pub mod progress;
pub mod rowlimit;
pub mod status;

pub use app::{run, run_local};
