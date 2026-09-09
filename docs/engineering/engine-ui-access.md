# Engine UI access

Preferred ports: AKS Engine UI `https://localhost:8080/ui`, Studio
`http://localhost:3000`, local Docker API `http://localhost:8082`. Start the
engine tunnel with `kubectl -n kaveon port-forward service/kaveon 8080:8080
--address 127.0.0.1`. Use HTTPS; the AKS listener requires TLS. Each workstation
can use `18443:8080` and `https://localhost:18443/ui` when 8080 is occupied.
Do not terminate Docker Desktop merely because it owns a published port.

The native Engine dashboard is `/ui`. Its static HTML shell accepts unauthenticated
GET requests so browsers can show **Sign in with Microsoft** when Entra is configured.
The manual token form remains under Advanced for diagnostics. Cluster and query APIs
remain authenticated. Sign-in keeps tokens only in page memory;
Disconnect clears query/telemetry displays, cancels polling and invalidates pending
responses. Tokens are not saved in browser storage or URLs.

Query history shows **Kaveon CLI** for CLI submissions and the authenticated **User**.
The user label comes from validated Entra username/name claims and falls back to the
immutable principal when absent. The query detail view retains that principal.
Client/source labels are reported metadata; roles and ownership use authenticated
identity, never the CLI's editable `--user` field. A coordinator restart clears
in-memory query history, so newly submitted queries carry the new display fields.

## This workstation and AKS

Direct AKS API access is now working. The workstation uses varying outbound IPs;
the fixed API IP allowlist was the blocker. Removing that restriction on this
test cluster restored `kubectl get nodes` and actual service port-forwarding.
The control-plane endpoint is internet-reachable, with Entra authentication and
Azure RBAC still required and local accounts disabled. No subscription policies
were changed. `restrictApiToOperatorIps` in the Bicep template defaults to false;
enable it only with stable, verified egress ranges. Storage firewall rules and
the private Engine Services were not changed.

Before the network fix,
`scripts/aks-engine-ui.py` provides a read-only local viewer through the working,
authenticated Azure AKS Run Command channel. It is not a Kubernetes port-forward.

From the repository root:

```powershell
python scripts/aks-engine-ui.py
```

Open `http://localhost:18444/ui`. In another terminal, copy the existing Engine
token, paste it into the password field, and select Connect:

```powershell
(Get-Content tmp/aks-private-v2/tokens.json -Raw | ConvertFrom-Json).principal | Set-Clipboard
```

The viewer binds only to loopback, rejects unexpected Host headers, requires the
Engine token for data, and supports only dashboard read endpoints. It does not
expose a public engine endpoint or support SQL submission. Two reads are batched
into one Azure command every 60 seconds; browser polling uses the cached snapshot.
The banner shows the last successful refresh or an error. Failed/expired snapshots
are not returned as current API data. Refresh pauses after three minutes without
authenticated browser requests. Closing the viewer page therefore stops recurring
Azure calls after that idle period; Ctrl+C stops a foreground viewer immediately.

The bridge needs Azure CLI login with AKS Run Command permission, the repo UI file,
and the existing private token file. Temporary curl credentials inherit the private
directory's ACLs and are piped to curl through stdin, never command-line token
arguments. Keep `tmp/aks-private-v2` private and excluded from Git.

Use ordinary port-forwarding and the native TLS
endpoint at `https://localhost:18443/ui`. Trust the test CA and select Microsoft sign-in.
The local viewer remains an optional read-only fallback.

## Validation and deployment

Six server security tests pass, including public GET `/ui`, rejection of unauthenticated
POST `/ui`, and missing/incorrect/correct credentials for both dashboard APIs.
A real headless Edge browser test verified wrong-token rejection, three workers
visible after authentication, empty browser storage, cleared data on Disconnect,
and no JavaScript errors. The live viewer reads the deployed AKS cluster.

The earlier coordinator-only UI/auth image digest was
`sha256:909cf21c79cc87d5bf4085f7f155aa23e90a9ed00f1ea968c0fea10327e2883a`.
The chart supports `coordinator.imageDigest` for this compatible coordinator-only
update; workers retained the previously qualified engine image. Future full engine
upgrades should update both roles as required by their protocol compatibility.

The coordinator-only AKS rollout completed successfully. A verified HTTPS GET
of its native `/ui` returned the new sign-in form; coordinator and all three
worker pods were Ready. Raw rollout evidence is `tmp/aks-ui-rollout.json`.

The subsequent CLI 0.2.0 rollout updated both coordinator and all three workers
to `sha256:c50c400207244c9a5af0553420e9f6ecbfe52df3cf4276f4b958d55fabf539fd`.
All four are Ready. The portal is also deployed as `service/kaveon-portal` on
port 3000; see [deployment evidence](aks-test-deployment.md) for image pins and
the live `kavedb` SQL Lab check.

Always check `kubectl config current-context` before operating on a cluster.
The repository's dedicated `--kubeconfig tmp/aks-kubeconfig` selects `kaveon-test-aks`;
other machines can use their standard kubeconfig with that explicit context.
The viewer always names the Kaveon subscription/resource group/cluster explicitly.

The latest live browser check verified three active workers, **Kaveon CLI**, and
the signed username. An explicit spoofed request username did not replace the
authenticated identity. See the [complete connection guide](azure-deployment-guide.md).
