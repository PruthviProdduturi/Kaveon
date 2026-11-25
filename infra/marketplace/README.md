# Kaveon Azure Marketplace package

The installation should end with an **Open Kaveon** link, a successful first administrator sign-in and a query in SQL Lab against the customer's own lake.

## Customer experience

[Open the guided installation preview in Azure](https://portal.azure.com/#create/Microsoft.Template/uri/https%3A%2F%2Fraw.githubusercontent.com%2FPruthviProdduturi%2FKaveon%2Fcodex%2Fazure-install-preview%2Finfra%2Fmarketplace%2FmainTemplate.json/createUIDefinitionUri/https%3A%2F%2Fraw.githubusercontent.com%2FPruthviProdduturi%2FKaveon%2Fcodex%2Fazure-install-preview%2Finfra%2Fmarketplace%2FcreateUiDefinition.json). On **Basics**, choose the subscription, resource group and region, then enter an installation name such as `kaveon` and choose an environment. On **Compute**, review the worker size and autoscaling limit. The default installation starts two system nodes and one worker, and reserves a Standard public IP. Review charges before selecting **Create**.

Production is selected by default. For a production installation, `rg-kaveon-prod` is an example resource group name. The guided form deploys into the group you select. To update an installation, select its original environment, installation name and resource group.

The installer generates the storage and registry names and assigns the signed-in user as the first administrator. Azure checks the generated names during deployment validation. If a global name is taken on a fresh installation, change the installation name and validate again.

If your tenant requires a service management reference on app registrations, set the resource group tag `ServiceTreeId` before deploying or pass the `serviceManagementReference` template parameter. Both registration steps use that value.

With directory provisioning enabled, the template reserves a DNS name in the customer's resource group, registers the Studio callback for that name in the customer's Entra tenant, and assigns the initial Admin. Outputs include the reserved IP name and non-secret Helm identity values. Directory permissions are required to register the app and assign roles. A tenant that restricts delegated consent may also require approval of Kaveon's own API scope.

**Preview boundary.** The template provisions the foundation. An operator still deploys the images and Helm releases, attaches ingress to the reserved IP, obtains the HTTPS certificate and verifies first sign-in and SQL execution. The planned Studio origin in deployment outputs only identifies the address. A searchable Marketplace listing is a separate publication step.

## Open the infrastructure form in Azure Portal

To use the manual portal editor, load the checked-in mainTemplate.json. To rebuild it from Bicep, run the following from the repository root.

    az bicep build --file infra/marketplace/main.bicep --outfile mainTemplate.json

In Azure Portal, choose **Create a resource**, search for **Template deployment (deploy using custom templates)**, then select **Create → Build your own template in the editor → Load file**. Load the compiled JSON and select **Save**. The portal generates a parameter form. Select the subscription, resource group, environment and capacity.

The guided link loads both the template and its UI definition. Loading the template alone opens the advanced parameter editor, where operators may override resource names, identities and networking defaults.

## First installation in a Microsoft Entra tenant

For installations provisioned without the portal's directory module, run the directory bootstrap from the repository root using an Azure CLI login in the customer's tenant. Use the final HTTPS Studio origin. A directory administrator supplies the initial Admin group or an enabled user's object ID. The command previews the changes. Repeat with `--apply` to provision them. Add `--grant-consent` when the directory administrator wants to grant delegated consent for Kaveon's own API. Installations created with the directory module already have their tenant and client ID in the entraHelmValues output.

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

The remaining installer work is to deploy a versioned Engine, API and Studio release using the template outputs, provision its runtime credentials and TLS, and bootstrap the first lake catalog. A release rehearsal must start in an empty resource group and exercise sign-in and SQL Lab through the public Studio URL.
