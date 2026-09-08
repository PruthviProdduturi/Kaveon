# AKS medallion smoke-test fixture

Run from the repository root with Python and `pyarrow==25.0.1` installed:

```powershell
python scripts/generate-medallion-fixture.py --output tmp/aks-medallion
```

This generates synthetic data only: 100 customers and 10,000 orders, spanning seven days. Every 97th order has a NULL customer, every 89th has a NULL amount, and every 11th is cancelled. Monetary values use integer cents for exact comparisons. Dates are ISO strings. This small fixture validates connectivity and query correctness; it is not a performance benchmark or a production ingestion pipeline.

Upload each layer into its matching ADLS Gen2 filesystem/container, preserving the paths below the layer directory:

| Filesystem | Paths | Purpose |
|---|---|---|
| bronze | `customers/part-00000.csv`, `.jsonl`; `orders/part-00000.csv`, `.jsonl` | Raw source representations; CSV NULL values are empty cells |
| silver | `customers/part-00000.parquet`, `orders/part-00000.parquet` | Typed dimensions and facts |
| gold | `daily_sales/part-00000.parquet` | Completed orders grouped by date |

Register the silver tables as `customers` and `orders`, and the gold table as `daily_sales` in the engine catalog. `expected-results.json` contains six ordered, exact SQL result sets covering counts, NULL semantics, integer aggregates, joins and silver/gold reconciliation. Expectations are calculated independently in Python, and generation checks each Parquet file by reading it back. Run the statements against the deployed engine and compare all returned rows. No engine execution is implied by successful generation.

`manifest.json` records schema, row counts, PyArrow version and SHA-256 hashes of all generated data and the expectation file. It has no clock-dependent fields. Reproducible Parquet bytes require the same PyArrow version. The default fixture is intentionally small to reduce deployment cost. Regeneration replaces known fixture files; use a dedicated output directory. Keep manifest and expected results locally or upload them to a separate validation prefix.
