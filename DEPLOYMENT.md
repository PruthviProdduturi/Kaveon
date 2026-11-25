# Deploy Kaveon

A self-hosted installation runs Studio, the platform API, the Engine coordinator and its workers in your environment. Studio exposes the browser interface. Its authenticated server-side proxy calls the API over the private network.

| Environment | How to install |
| --- | --- |
| Local Docker | Run `bash scripts/kaveon-up.sh` from a checkout. Open Studio on port 3000 and run `SELECT 1 AS ready;` in SQL Lab. [Quickstart](https://kaveon.io/docs/quickstart) |
| Linux VM | Use Docker Compose, a persistent disk, configured sign-in and an HTTPS reverse proxy for Studio. [Self-hosting guide](docs/guides/self-hosting.md) |
| Azure Kubernetes Service | Provision the cluster, storage and Microsoft Entra registration with the [guided Azure preview](https://kaveon.io/docs/installation#azure). Install the Engine Helm release, then the platform release containing the API and Studio. |
| AWS | Coming soon. |

## Verify an Azure installation

1. Configure Studio ingress for the reserved hostname, attach a valid HTTPS certificate and open Studio.
2. Sign in with the assigned Microsoft Entra administrator. Run `SELECT 1 AS ready;` in SQL Lab to verify the coordinator can execute a statement.
3. Register a lake table and sample its rows to verify the storage identity and data path. Save a chart and reopen it to verify product persistence.

Keep the API and coordinator private. Back up the coordinator catalog volume as well as the product store. [Authentication](https://kaveon.io/docs/auth) · [Operations and recovery](https://kaveon.io/docs/operations) · [Troubleshooting](https://kaveon.io/docs/troubleshooting)
