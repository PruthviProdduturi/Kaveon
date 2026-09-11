# Public demo host

The public demo runs on one Oracle Cloud **Always Free** instance —
`VM.Standard.A1.Flex`, 4 OCPU / 24 GB, arm64 — hosting the Compose stack:
KaveonDB coordinator and two workers, the API, PostgreSQL metadata, and Caddy as
the only public door. Studio stays on Vercel and reaches the API over HTTPS through
its same-origin proxy. The work-tenant AKS cluster remains validation only.

Decision record (September 10, 2026). Spot on Azure was the first choice; the
Visual Studio subscription is credit-limited and Azure does not offer Spot on
credit-limited offers, and no on-demand shape fits the budget always-on. Oracle's
Always Free tier is the only genuinely free machine that runs this stack. Phase 1
keeps the lake on the host's disk so the demo is Azure-free from the first day;
object storage returns with the Engine's S3 work.

| Phase | Where the lake lives | Azure bill |
|---|---|---|
| 1 — now | the host's boot volume (`KAVEON_DATA_PATH`) | $0 after retirement below |
| 2 — after S3-compatible storage lands in the Engine | OCI Object Storage (20 GB free) or any S3 | $0 |

`infra/bicep/environments/demo-vm.bicep` remains as the Azure on-demand fallback
(`Standard_D2s_v5` fits the credit always-on; `D4s_v5` with a shutdown schedule).

## What Terraform creates (`infra/oci`)

- VCN `10.70.0.0/24`, internet gateway, public subnet.
- Security list: SSH from the operator CIDR only; 443 from anywhere; 80 for the
  certificate challenge only.
- `kaveon-demo` instance from the newest Canonical Ubuntu 24.04 aarch64 image,
  150 GB boot volume (Always Free covers 200 GB), cloud-init from
  `infra/bicep/cloud-init/demo-vm.yaml` — Docker, unattended upgrades, fail2ban,
  the `kaveon` service account, `/opt/kaveon`.

Capacity for A1 varies by region and availability domain; if `apply` reports
out-of-capacity, change `availability_domain_index` or the home region and retry.

```powershell
terraform -chdir=infra/oci init
terraform -chdir=infra/oci apply -var-file=demo.tfvars
```

`demo.tfvars` holds `tenancy_ocid`, `compartment_ocid`, `region`, `ssh_public_key`
and `operator_cidr`; it is ignored by Git.

## Images

`.github/workflows/demo-images.yml` publishes `ghcr.io/pruthviprodduturi/kaveon-engine`
and `kaveon-api` as multi-architecture manifests tagged `:demo` and `:sha-<commit>`
after every green CI run on `dev`. Each architecture builds natively on its own
GitHub runner; a Rust release build under emulation would take an hour. The same
tag therefore serves the arm64 demo host and any amd64 host.

## Bring up the stack

```bash
ssh -i ~/.ssh/kaveon-demo ubuntu@<public_ip>
sudo -iu kaveon
cd /opt/kaveon
git clone --branch dev https://github.com/PruthviProdduturi/Kaveon.git src && cd src
cp ../.env .env                     # written by the operator; see below
export KAVEON_DATA_PATH=/opt/kaveon/data
docker compose -f docker-compose.yml -f docker-compose.demo.yml pull
docker compose -f docker-compose.yml -f docker-compose.demo.yml up -d --no-build
```

`/opt/kaveon/.env` (mode 0600, owner `kaveon`) carries only these values, each
generated fresh:

```
KAVEON_API_HOST=<public DNS name for the API>
KAVEON_WEB_URL=https://kaveon.vercel.app
KAVEON_PROXY_SECRET=
KAVEON_POSTGRES_PASSWORD=
KAVEON_ENGINE_BRIDGE_TOKEN=
KAVEON_CATALOG_ADMIN_TOKEN=
KAVEON_EXCHANGE_TOKEN=
KAVEON_CREDENTIAL_KEYS=
KAVEON_CREDENTIAL_ACTIVE_KEY=
KAVEON_SECURITY_JSON={"principals":[{"token":"<admin token>","principal":"demo-admin","role":"admin"}],"bridge_token":"<same as KAVEON_ENGINE_BRIDGE_TOKEN>"}
```

`KAVEON_API_HOST` needs a DNS name pointing at the instance's public IP before
Caddy can obtain a certificate; a free `sslip.io` or `nip.io` name works until a
custom domain exists (`<ip-with-dashes>.sslip.io`). Confirm with
`curl -sI https://<host>/api/health`.

Security posture in Phase 1, stated plainly: the Engine plane (coordinator,
workers, exchange) runs with `KAVEON_INSECURE_DEVELOPMENT` inherited from the
root Compose file — plaintext on the private Docker network, unreachable from
outside, exactly the posture of the qualified local stack. The API is the only
public listener and admits requests only with the proxy secret. Enabling the
Engine's TLS and PKI on the host (the AKS secrets generator produces a compatible
bundle) is the first hardening step after the demo is up.

## Load the lake and the metadata

Snapshot: download `snapshots/2026-09-09-v1` from the work-tenant ADLS account
with `azcopy` (user-delegation SAS) to a workstation, then `rsync` it to
`/opt/kaveon/data/opensource/`. Register the catalog in Studio → Settings →
Storage as a *local* source at `/data/opensource`, synchronize, and register the
schemas and tables from the snapshot manifest as on AKS.

Metadata: `pg_dump` the Azure PostgreSQL `kaveonmeta` database and restore it into
the stack's `postgres` container; verify dashboards, charts and DLM artifacts open
in Studio before anything in Azure is removed.

## Point Studio at it

On Vercel set `API_URL=https://<KAVEON_API_HOST>` and `KAVEON_PROXY_SECRET` to the
same value as the host. Redeploy. Sign-in providers stay GitHub and Google;
Microsoft Entra is optional and can be removed, which also closes the exposed
Entra client-secret item.

## Azure retirement (after a verified restore on the host)

| Resource (`kaveon-rg`) | Action |
|---|---|
| `kaveon-db` (PostgreSQL Flexible Server) | delete after `pg_dump` is restored and verified on the host |
| `kaveon-api` (Container App) and `kaveon-env` | delete |
| `kaveon-logs` (Log Analytics) | delete |
| `kaveonacr` | delete once GHCR images are in use |
| `kaveon-kv` (Key Vault) | keep or delete; secrets live in the host's `.env` |

Deleting the database server is the one irreversible step; it waits for the
verified restore. The work-tenant AKS cluster and its ADLS account are
untouched by this plan.

## Operations

- Always Free instances are not evicted, but Oracle reclaims *idle* ones; the
  polling API and Engine heartbeats keep the host busy enough. Review the idle
  policy if the instance is ever reclaimed.
- Studio should show a "demo is restarting" state when the API is unreachable
  rather than raw errors; tracked separately.
- SSH is limited to the operator CIDR; rotate the variable when it changes.
