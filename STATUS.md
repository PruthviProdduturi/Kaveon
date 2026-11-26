# Kaveon product status

This page describes what a user can install and run from the default branch. See the [installation guide](DEPLOYMENT.md) for deployment choices and the [Engine capability matrix](engine/DISTRIBUTED_EXECUTION_STATUS.md) for operator-level coverage.

| Component | Available today | Qualification boundary |
| --- | --- | --- |
| Studio | SQL Lab, semantic datasets, charts, dashboards and sharing | Visibility and permitted actions depend on the signed-in identity. |
| Data Language Model | Deterministic answers from compiled dataset context and SQL generation for supported questions | Coverage depends on the dataset’s metrics, dimensions and freshness. |
| Engine | Rust coordinator and workers execute columnar scans, joins and aggregates. The CLI and Studio submit statements to the Engine. | The local Docker release is qualified. Distributed SQL coverage varies by operator and data type. |
| KaveonDB | Versioned product records for datasets, charts, dashboards, saved queries and history | The coordinator owns the transaction head. Multi-coordinator write availability remains open. |
| Lake storage | Local Parquet and Delta, local Iceberg, and qualified ADLS Gen2 Parquet and Delta reads | Live S3 and Iceberg on ADLS require further qualification. |

## Deployment

Docker Compose runs Studio, the API, the coordinator and workers locally or on a Linux host. The Azure installation preview provisions infrastructure and Microsoft Entra sign-in. Helm deploys the application on AKS. AWS installation is planned. [Deployment guide](DEPLOYMENT.md)

## Release boundaries

Broader cloud qualification includes identity rotation, node and disk failure, and upgrade and recovery under sustained workload. For SQL compatibility, consult the [reference](docs/reference/engine-sql-compatibility.md) before depending on a dialect-specific feature. Reproducible test inputs ship with the source. Deployment receipts, account costs and individual test runs stay with the operator.
