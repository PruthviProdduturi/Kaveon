# Supported Azure infrastructure entry points

Kaveon has one supported Azure infrastructure path:

1. `aks-production.bicep` provisions the AKS, network, managed identities and
   narrowly scoped ADLS/ACR role assignments.
2. `infra/helm/kaveon-engine` installs the distributed Engine.
3. `infra/helm/kaveon-platform` installs the API and Studio workloads in
   PostgreSQL-free retirement mode.
4. The checked-in GitHub Actions workflows build immutable images, render the
   charts, enforce the cloud budget gate and perform health checks.

The old Container Apps/PostgreSQL template is retired and intentionally absent.
KaveonDB is the product metadata and transaction authority; a new deployment
must not introduce a PostgreSQL server, `DATABASE_URL`, or a mutable image tag.

## Marketplace packaging boundary

The infrastructure templates are the deployment core, not yet a complete
Azure Marketplace offer. Marketplace packaging still needs a customer-facing
ARM/Bicep wrapper, `createUiDefinition.json`, managed-identity consent flow,
post-deployment Helm orchestration, upgrade/rollback actions, and a clean
subscription installation test. Until those artifacts exist, describe this as
an operator-led private-cloud deployment rather than one-click Marketplace
installation.
