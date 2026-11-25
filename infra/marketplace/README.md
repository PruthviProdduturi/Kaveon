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

## Open the infrastructure form in Azure Portal

Compile the resource-group template from the repository root.

    az bicep build --file infra/marketplace/main.bicep --outfile mainTemplate.json

In Azure Portal, choose **Create a resource**, search for **Template deployment (deploy using custom templates)**, then select **Create → Build your own template in the editor → Load file**. Load the compiled JSON and select **Save**. Azure generates a parameter form. Choose the subscription, resource group, environment, AKS administrator object ID and capacity. Review the resource changes before creating them. The AKS administrator object ID controls cluster administration; the directory bootstrap below assigns Kaveon product roles.

For the designed Marketplace form, preview `createUiDefinition.json` in the Azure Create UI Definition sandbox. The custom deployment editor accepts the compiled ARM template. Completing that form provisions infrastructure; publishing images, installing the Helm releases and configuring sign-in are subsequent steps.

## First installation in a Microsoft Entra tenant

Run the directory bootstrap from the repository root using an Azure CLI login in the customer's tenant. Use the final HTTPS Studio origin. A directory administrator supplies the initial Admin group or an enabled user's object ID. The command previews the changes. Repeat with `--apply` to provision them. Add `--grant-consent` when the directory administrator wants to grant delegated consent for Kaveon's own API.

```sh
python scripts/install-entra.py --tenant "$TENANT_ID" --origin "https://studio.example.org" --group "Admin=$ADMIN_GROUP_ID" --catalog-group "OpenSource=$ANALYTICS_GROUP_ID" --output kaveon-directory.json
```

For a tenant starting without groups, use `--admin-user "$ADMIN_USER_OBJECT_ID"` in place of the Admin group. The installer creates a dedicated single-tenant application and enterprise application, registers the Studio SPA callback, exposes the delegated `access_as_user` scope and enables the four product roles. It requires assignment to sign in. The bootstrap uses a public client and produces a non-secret Helm values file. Reuse the same origin when retrying a partial installation. The installer adds the assignments explicitly requested in that invocation. Ongoing membership and assignment changes belong in Entra.

| Assignment | What it allows |
|---|---|
| Viewer | Read published content and browse assigned lake catalogs. |
| Analyst | Create own content and query assigned catalogs. |
| Editor | Edit and publish content and query assigned catalogs. |
| Admin | Administer the deployment and manage its catalogs. |
| `--catalog-group OpenSource=<group-id>` | Assign the group the scoped `Kaveon.Query.OpenSource` role and Analyst by default. Supply a different base role with `--group` for the same group. |
| `--catalog OpenSource` | Make this a shared catalog for **all** assigned base roles, with browse access for Viewer and query access for Analyst and Editor. |

Use the receipt as the last values file on the Engine Helm install alongside the installation's image, TLS and storage values. Supply that same file to the platform installer through `KAVEON_ENTRA_VALUES`. For GitHub Actions, save the receipt JSON as the KAVEON_ENTRA_RECEIPT Actions variable in the deployment environment. Deploy the Engine before the platform. This connects Studio, API and CLI to the same tenant and client. Direct API clients request a Kaveon access token with the delegated scope. Azure Storage continues to authorize the Engine's workload identity independently.

Verify with an Admin and a member of the analytics group. Open **Settings → Access control**, query the assigned catalog, and confirm a different private catalog stays inaccessible. Remove the group assignment in Entra, obtain a fresh token, and repeat in Studio and CLI. Previously issued tokens can retain access until expiry. If the installer returns HTTP 403, a directory administrator must perform the relevant app registration, assignment or consent step. Subscription Owner alone does not grant those directory permissions. Group-based assignment requires appropriate Entra licensing.

AWS IAM integration is coming soon. Standalone installations can continue using Kaveon-managed catalog grants.

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
