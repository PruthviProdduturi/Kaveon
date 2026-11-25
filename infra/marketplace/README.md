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

## Current package contents

- `main.bicep` creates secure ADLS Gen2 containers and ACR, then invokes the
  supported AKS infrastructure template.
- `main.parameters.json` is a resource-group deployment example. Supply the administrator object ID.
- `subscription.bicep` creates the application resource group and runs the same foundation in it. Use a dedicated work subscription and an unused environment name for a fresh install. The AKS service creates its own node resource group.

### Preview a work installation

Run from the repository root using the work subscription ID and the work administrator's Entra user object ID. This command previews changes, including creation of the resource group.

```powershell
az deployment sub what-if --subscription $workSubscriptionId --location westus2 --template-file infra/marketplace/subscription.bicep --parameters location=westus2 environment=dev operatorObjectId=$workOperatorObjectId
```

In this example the application group is `rg-kaveon-dev` and the AKS-managed node group is `rg-kaveon-dev-nodes`. AKS, the VNet, and the two explicit managed identities use the prefix `kaveon-dev-`. The storage and registry names include a deterministic 13-character suffix derived from the application resource group ID. AKS assigns names to its child compute, disk, and load-balancer resources. The system pool requests two Standard_D4s_v5 nodes and the worker pool starts at one Standard_B2s node. Verify regional capacity and the cost for the planned uptime before applying.
- `createUiDefinition.json` is the Marketplace-style portal form with explicit
  worker and NAT cost choices.

This package intentionally stops before Helm because image digests, Engine TLS,
Key Vault references, Entra configuration, and customer-specific ingress must
be supplied through the deployment environment rather than embedded in a
public ARM parameter file. The next automation layer must consume the Bicep
outputs and invoke the existing digest-pinned Helm workflows; it must never
deploy `latest`, store secrets in parameters, or create a database authority
outside KaveonDB.
