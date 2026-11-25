<div align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/reference/kaveon-logo-dark.svg">
    <img src="docs/reference/kaveon-logo.svg" alt="Kaveon" width="300">
  </picture>
</div>

# Talk to your data

Kaveon is an open-source data intelligence platform. Connect your lake, define the metrics your team uses, and turn those definitions into answers, charts and interactive dashboards. Kaveon Studio brings that work together with a deterministic Data Language Model (DLM), a distributed Rust engine and KaveonDB for transactional product state.

[Open Studio](https://kaveon.io) · [Read the docs](https://kaveon.io/docs) · [Run locally](#run-locally)

## Inside Kaveon

[![Exploded architecture of Kaveon Studio, the Data Language Model, the distributed SQL engine and KaveonDB over your lake.](docs/reference/kaveon-platform-architecture.svg)](docs/reference/kaveon-platform-architecture.svg)

**Build a semantic dataset.** Start with a registered lake table. Define a metric such as *sessions*, choose dimensions such as *country* and *platform*, and compile its DLM context. That context captures vocabulary and precomputed breakdowns for the dataset. [Semantic datasets](https://kaveon.io/docs/datasets) · [Inside the DLM](https://kaveon.io/docs/dlm)

**Explore in chat or SQL.** Ask *“Which three countries had the most sessions?”* A fresh country breakdown can answer immediately from context. When supported questions need live data, the DLM can generate SQL. Analysts can also write SQL directly in SQL Lab and inspect how the Engine executed it. [SQL Lab](https://kaveon.io/docs/sql-lab)

**Build and publish dashboards.** Turn the country breakdown into a chart, add a platform comparison, and arrange both on a dashboard. Viewers can select a country in one chart to filter compatible charts on the same dashboard. KaveonDB persists the chart definitions and layout while the underlying tables stay in your lake. [Dashboards](https://kaveon.io/docs/dashboards)

[Architecture](ARCHITECTURE.md) · [Engine manual](https://kaveon.io/docs/engine) · [CLI guide](docs/guides/engine-cli.md) · [Project status](STATUS.md)

## Run locally

You need Git, Bash and Docker with Compose v2. On Windows, use WSL with Docker integration enabled.

```bash
git clone https://github.com/PruthviProdduturi/Kaveon.git
cd Kaveon
bash scripts/kaveon-up.sh
```

Open [Studio on localhost](http://localhost:3000), choose **SQL Lab**, and run:

```sql
SELECT 1 AS ready;
```

You should see one row with the value `1`. To query your own files, restart the launcher with `--data /absolute/path/to/warehouse`, then register a table under the mounted `/data` directory. [Local quickstart](https://kaveon.io/docs/quickstart)

## Deploy

| Environment | Start here |
| --- | --- |
| **Local machine or Linux VM** | Start with Docker Compose. Configure authentication and HTTPS before sharing a VM installation. [Self-hosting guide](docs/guides/self-hosting.md) |
| **Azure Kubernetes Service** | The guided preview provisions infrastructure and Microsoft Entra sign-in. Install the Engine and Studio/API Helm releases to bring the application online. [Azure installation package](https://github.com/PruthviProdduturi/Kaveon/tree/a2c4d3f99f4bf9e49f77726b856a59182e8d29ef/infra/marketplace) |
| **AWS** | Coming soon. |

## Contribute

The [contributing guide](CONTRIBUTING.md) covers local development and pull requests. For security issues, use the [security reporting policy](SECURITY.md). Kaveon is licensed under the [MIT License](LICENSE).
