//! The interactive shell.
pub mod app;
#[allow(dead_code)]
pub mod commands;
#[allow(dead_code)]
pub mod complete;
pub mod editor;
#[allow(dead_code)]
pub mod highlight;
pub mod rowlimit;
pub mod status;

pub use app::run;
