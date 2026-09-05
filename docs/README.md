# Kaveon Documentation

This index separates runtime documentation from design and research material. Use
the **Current**, **Alpha**, and **Target** labels consistently: current features run
in Studio/API, alpha features are implemented but incomplete, and target features
are planned rather than executable.

## Start here

- [Project overview](../README.md)
- [Current release status](../STATUS.md)
- [Architecture and maturity boundaries](../ARCHITECTURE.md)
- [Deployment](../DEPLOYMENT.md)
- [Security model and open gaps](../SECURITY.md)
- [Contributing](../CONTRIBUTING.md)

## Runtime guides

- [Engine technical manual](engine/README.md)
- [Use the Engine CLI](guides/engine-cli.md)
- [Connect data sources](guides/data-sources.md)
- [Use SQL Lab](guides/sql-lab.md)
- [Build charts](guides/charts.md)
- [Build dashboards](guides/dashboards.md)
- [Natural-language query flow](guides/nl-to-sql.md)
- [Deploy Studio to Vercel](guides/vercel-deployment.md)
- [Deploy Vercel, Container Apps, and PostgreSQL](guides/deploy-vercel-azure-postgres.md)

## References

- [HTTP API](reference/api.md)
- [Configuration](reference/configuration.md)
- [Engine SQL compatibility](reference/engine-sql-compatibility.md)
- [Engine memory management](reference/engine-memory-management.md)
- [Connector capability matrix](reference/connector-capabilities.md)
- [Operations and troubleshooting](operations-troubleshooting.md)
- [Upgrade and version policy](upgrade-version-policy.md)
- [Release notes and changelog guidance](release-notes.md)
- [Engine execution pipeline](reference/kaveon-engine-pipeline.svg)
- [Platform architecture](reference/kaveon-platform-architecture.svg)
- [Deployment topology](reference/kaveon-deployment-topology.svg)
- [DLM flow](reference/kaveon-dlm-flow.svg)

The FastAPI service also exposes generated OpenAPI documentation at `/docs` while
it is running. The Engine HTTP API does not currently publish an OpenAPI document.

## Product and research documents

- [Product strategy](product-strategy.md) — roadmap and positioning; target claims
  are not runtime guarantees.
- [Data Language Model](whitepaper-dlm.md)
- [DLM curation](whitepaper-dlm-curation.md)
- [Adaptive context routing](whitepaper-adaptive-context-routing.md)
- [Template-based NL-to-SQL](whitepaper-nl-to-sql.md)
- [Adaptive-context patent draft](patent-adaptive-context-routing.md)
- [Kaveon and Trino: architectural comparison](research/kaveon-vs-trino.md)
- [Kaveon Engine and Fabric SQL analytics endpoint](research/kaveon-engine-vs-fabric-sql-analytics-endpoint.md)

White papers and the patent draft contain research descriptions and historical
measurements. They are not substitutes for current capability status. Performance
numbers require their original dataset, hardware, cache state, version, and date
before they can be treated as reproducible benchmarks.

## Historical implementation plans

Files under `superpowers/` record prior designs and execution plans. They may
describe superseded UI structure or future work and are not current product docs.
