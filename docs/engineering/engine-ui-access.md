# Engine UI access

The native Engine dashboard is `/ui`. Its static HTML shell accepts unauthenticated
GET requests so browsers can show the token sign-in form. Cluster and query APIs
remain authenticated. Connect keeps the supplied token only in page memory;
Disconnect clears query/telemetry displays, cancels polling and invalidates pending
responses. Tokens are not saved in browser storage or URLs.

## This workstation and AKS

Direct AKS API access still times out from this workstation despite resource-level
allowlist updates for observed outbound IPs. No subscription policies were changed.
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

When the network path is restored, use ordinary port-forwarding and the native TLS
endpoint at `https://localhost:18443/ui`. Trust the test CA and enter the same token.
The local viewer is for inspection; it does not resolve the corporate network path.

## Validation and deployment

Six server security tests pass, including public GET `/ui`, rejection of unauthenticated
POST `/ui`, and missing/incorrect/correct credentials for both dashboard APIs.
A real headless Edge browser test verified wrong-token rejection, three workers
visible after authentication, empty browser storage, cleared data on Disconnect,
and no JavaScript errors. The live viewer reads the deployed AKS cluster.

The coordinator UI/auth image digest is
`sha256:61663d0d4d87310bad297c4594db39bea053d0c9903958d8f2e79540288b0c7e`.
The chart supports `coordinator.imageDigest` for this compatible coordinator-only
update; workers retain the previously qualified engine image. Future full engine
upgrades should update both roles as required by their protocol compatibility.

The coordinator-only AKS rollout completed successfully. A verified HTTPS GET
of its native `/ui` returned the new sign-in form; coordinator and all three
worker pods were Ready. Raw rollout evidence is `tmp/aks-ui-rollout.json`.

This workstation's default kubectl context is a separate cluster,
`aks-helio-orch-DSEng-dev`. Keep `--kubeconfig tmp/aks-kubeconfig` on direct
Kaveon kubectl commands; the dedicated file selects `kaveon-test-aks`.
The viewer always names the Kaveon subscription/resource group/cluster explicitly.
