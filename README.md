<div align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/reference/kaveon-logo-dark.svg">
    <img src="docs/reference/kaveon-logo.svg" alt="Kaveon" width="300">
  </picture>
</div>

# Talk to your data

Kaveon brings a **deterministic Data Language Model**, a **distributed SQL engine**, and an **analytics studio** together over data you own. Define what a metric means once, ask about it in chat, and use the same definition in a chart or dashboard.

[Open Studio](https://kaveon.io) · [Read the docs](https://kaveon.io/docs) · [Run locally](#run-locally)

## Inside Kaveon

[![Kaveon Studio routes questions to the DLM, lake queries to distributed Engine workers, and product edits to KaveonDB.](docs/reference/kaveon-platform-architecture.svg)](docs/reference/kaveon-platform-architecture.svg)

Suppose your dataset defines **sessions** as a metric and **country** as a dimension. Here is what happens when you ask *“Which three countries had the most sessions?”*

- **Ask in Studio.** The DLM resolves the question against the dataset’s vocabulary. If fresh compiled context covers the breakdown, it can return the top three without scanning the lake. Otherwise, supported questions can take the live SQL path. The DLM uses no hosted language model.
- **Run SQL.** Generated SQL and statements you write in SQL Lab can use the same lake execution path. The Rust coordinator plans the work. Workers read Parquet or Delta, exchange Arrow batches when rows need repartitioning, and return the result.
- **Save the work.** Saving a chart or editing a dashboard commits a versioned product record in KaveonDB. The underlying lake table stays where it is.

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
