use pyo3::prelude::*;

#[pyfunction]
fn execute(_sql: &str, _data_path: &str) -> PyResult<String> {
    // TODO: wire up kaveon-sql → kaveon-exec pipeline
    Ok(r#"{"columns": [], "rows": []}"#.to_string())
}

#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[pymodule]
fn kaveon_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(execute, m)?)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    Ok(())
}
