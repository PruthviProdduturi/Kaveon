# Kaveon Azure Marketplace package

This directory is the boundary for the future one-click Azure deployment. The
Marketplace package must orchestrate the already-supported PostgreSQL-free
stack; it must not revive the retired Container Apps or PostgreSQL templates.

## Customer experience

The customer supplies only the Azure region, resource-group name, ADLS
account/container, administrator identity, worker sizing, and optional
hostname. The package then creates or validates the resource group, ACR, AKS,
managed identities, Key Vault, network policy, Engine, API, Studio, catalog
bootstrap, ingress/TLS configuration, and a deployment receipt containing the
Studio URL and health status.

## Required artifacts before publishing

- `mainTemplate.json` or an equivalent Marketplace ARM/Bicep wrapper;
- `createUiDefinition.json` with safe defaults and explicit cost-impacting
  worker sizing;
- deployment-script orchestration for Helm and catalog bootstrap;
- managed identity and least-privilege role assignments;
- generated-secret and Key Vault wiring with no credentials in parameters;
- upgrade, rollback, uninstall, and data-retention behavior;
- post-deployment health and smoke checks;
- clean-subscription installation evidence;
- Partner Center offer metadata, support, privacy, and billing documentation.

Until these artifacts and a clean-subscription rehearsal pass, this is not yet
a Marketplace-ready one-click offer. The supported operator path remains the
AKS Bicep template plus the two reviewed Helm deployment workflows.
