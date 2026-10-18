# Restoring the hosted demo cluster

The hosted demo is deliberately offline. This document records why, what it will
take to bring it back, and the one decision that has to be made *before* it is
brought back -- because restarting the cluster unchanged recreates the outage
about eight days later.

Everything below was measured on 2026-10-18 against subscription
`4ed07f02-b111-4eea-98ce-1c177d573a51`.

## State

| Thing | Value |
|---|---|
| Subscription state (ARM) | `Disabled` |
| Offer | `MS-AZR-0063P` -- Visual Studio Enterprise, USD 150/month credit |
| `subscriptionPolicies.spendingLimit` | `On` |
| Billing period | 2026-10-12 to 2026-11-11 |
| Credit resets, subscription re-enables | **2026-11-12** |
| `kaveon-aks` `powerState` | `Deallocated` |
| `kaveon-aks` `provisioningState` | `Failed` |
| Node pools | `system` 1x, `workers` 3x, all `Standard_D4s_v3` |

The cluster is not broken. The spending limit is on, the monthly credit was
exhausted, Azure disabled the subscription, and disabling a subscription
deallocates its compute. `Failed` and `Deallocated` are consequences of that,
not an independent fault, and no cluster configuration needs repair.

Studio stays up throughout on Vercel, which bills separately and is unaffected.
It carries a maintenance notice naming 2026-11-12 -- a date chosen to match the
credit reset, so the notice and the billing cycle agree.

## Why it lasted four days

This is the part that matters, and it is arithmetic rather than an incident.
`Standard_D4s_v3` in `westus2` is USD 0.1920/hour at retail (verified against
`prices.azure.com`, 2026-10-18). The cluster runs four of them:

| | |
|---|---|
| Burn rate | **USD 0.768/hour** |
| Per day | USD 18.43 |
| Per 730-hour month | **USD 560** |
| Monthly credit | **USD 150** |
| Runtime the credit buys | **195 hours, about 8 days** |

The cluster costs 3.7x the credit that funds it. Nothing anomalous happened in
October: four `D4s_v3` running continuously cannot survive a USD 150 credit, and
will not survive it next month either. A restart on the current shape exhausts
the credit roughly eight days later and disables the subscription again.

So the shape, the schedule, or the funding has to change. Restarting without
changing one of the three is choosing to repeat this.

## Choose one before restarting

Retail `westus2` hourly prices, verified 2026-10-18.

1. **Run it only when it is being used.** `az aks stop` when idle, `az aks start`
   to demo. Keeps `D4s_v3` nodes, so published qualification timings stay
   reproducible. The credit buys about 195 hours a month -- roughly six hours a
   day, every day. Recommended for a demo cluster: it is the only option that
   preserves both the numbers and the budget.

2. **Reshape to run continuously.** 1x `Standard_D2s_v3` (USD 0.0960/hr) plus
   2x `Standard_B2s` (USD 0.0416/hr each) is USD 0.179/hour, about USD 131/month,
   which fits the credit with roughly USD 19 of headroom for storage, registry
   and egress. The cost is real: `B2s` is 2 vCPU and 4 GB, and burstable.
   Distributed joins over the billion-row Delta table would be severely
   constrained and the timings in `docs/qualification/` would no longer
   reproduce. This trades the ability to demonstrate the engine for uptime.

3. **Lift the spending limit.** The account admin can do this in the portal, and
   it restores service immediately rather than waiting for the reset. Overage
   beyond the credit runs about USD 410/month at the current shape and bills to
   a payment method.

Options 1 and 3 keep the cluster honest about performance. Option 2 does not,
and if it is taken, the qualification documents must say so.

## Restart procedure

1. **Confirm the subscription is actually enabled.** Use ARM, not the CLI
   profile:

   ```bash
   az rest --method get \
     --url "https://management.azure.com/subscriptions/<SUB>?api-version=2020-01-01" \
     --query "{state:state,spendingLimit:subscriptionPolicies.spendingLimit}"
   ```

   `az account show` reports a locally cached profile. On 2026-10-18 it said
   `Enabled` while ARM said `Disabled`; ARM is authoritative. Do not use
   `az account show` to decide whether the subscription is live.

2. Apply the decision from the section above -- resize or rescale the node pools
   *before* starting, so the cluster never runs a shape that cannot be afforded.

3. `az aks start -g kaveon-rg -n kaveon-aks`, then confirm `powerState` is
   `Running` and `provisioningState` is `Succeeded`. The `Failed` provisioning
   state should clear on a successful start; if it does not, that is a real
   fault and worth investigating rather than retrying.

4. Roll the Engine image that was built but never deployed. Resolve its digest
   from ACR rather than trusting a recorded prefix -- `kaveonacr` is unreachable
   while the subscription is disabled, so the digest could not be captured in
   full here:

   ```bash
   az acr repository show-manifests -n kaveonacr --repository kaveon-engine \
     --orderby time_desc --top 5 -o table
   ```

   `scripts/deploy-kaveon-aks.sh` refuses a tag and requires an immutable
   digest, which is deliberate -- pass the digest.

5. Verify a distributed query actually runs across workers, not just that pods
   are `Ready`. `docs/operations-troubleshooting.md` has the check for a
   statement that ran on the coordinator instead of the workers.

6. Clear the maintenance notice. It is configuration, not code: unset
   `NEXT_PUBLIC_KAVEON_SERVICE_NOTICE` and
   `NEXT_PUBLIC_KAVEON_SERVICE_NOTICE_RETURNS` on the Vercel project and
   redeploy. They are inlined at build time, so unsetting them without a rebuild
   leaves the notice showing.

## Waiting on the restart

- The Engine image built during the October work was never rolled; the cluster
  went down before it could be. See step 4.
- Two stale lake directories under the `product` container, `events/enriched`
  and `events/dashboard`, total roughly 6 GB and are no longer referenced by any
  dataset or chart. Their tables were dropped; the files were not. The
  subscription being disabled makes the storage read-only, so this could not be
  completed.
- DLM regeneration for the rebuilt Kaveon Events dataset should be re-verified
  against the live cluster once it serves again.

## Gotchas

- **`az account show` lies about subscription state.** It reads a cached
  profile. Use the ARM call in step 1.
- **Node pool `powerState` reads `Running` while the cluster is `Deallocated`.**
  The per-pool field is stale when the cluster is stopped. The cluster-level
  `powerState` is the truth; stopped nodes are not billing.
- **The AKS deploy workflow fails on every push while the cluster is down.**
  That is expected, not a regression -- there is nothing to deploy to. CI itself
  stays green.
- **Old Vercel production aliases keep serving their own frozen builds.** They
  predate the canonical-host redirect, so they cannot redirect to `kaveon.io`
  and will keep answering with whatever code they shipped with. Only deleting
  those deployments retires them.

## One host, and the variable that quietly broke it

Separately from the cluster, `kaveon.io` was serving the product page correctly
while **`/home` returned a 302 to `https://kaveon.vercel.app/login`, which
returns 404**. Clicking "Open Kaveon" on the brand domain therefore landed on a
dead page on a different domain. It presented as "the site takes too long to
load"; it was never loading.

The cause was `AUTH_URL`, a production environment variable set to
`https://kaveon.vercel.app`. Auth.js builds its sign-in redirect from
`AUTH_URL`, and an explicit value overrides host detection, so `AUTH_TRUST_HOST`
being set did not save it. It is now `https://kaveon.io`.

Two things follow:

- **`AUTH_URL` must track the canonical host.** The `next.config.ts` redirect
  that sends `*.vercel.app` to `kaveon.io` cannot fix this, because the
  off-domain hop is generated by auth middleware before any of that applies, and
  because the target host 404s rather than redirecting.
- **A browser console is worth more than a status code here.** `curl` on
  `kaveon.io/` returned 200 and the markup was byte-identical to a known-good
  build. The failure only appeared as a CORS error on an RSC prefetch of `/home`
  in a real browser, because the prefetch crossed origins.

Check both after any change to hosts or auth configuration:

```bash
curl -s -o /dev/null -w "%{http_code} -> %{redirect_url}\n" https://kaveon.io/home
curl -sL -o /dev/null -w "final: %{url_effective} (%{http_code})\n" https://kaveon.io/home
```

The first must not name a host other than `kaveon.io`; the second must end on
`kaveon.io` with a 200.

## Deploying Studio at all

`ci.yml`'s "Deploy to Vercel" job is gated on `vars.KAVEON_VERCEL_DEPLOY ==
'true'`. That repository variable did not exist, so the job reported `skipped`
on every push to `dev` and no push had deployed Studio since the variable was
introduced -- the live site was a one-off manual deploy, several hundred commits
behind. The variable is now set to `true`, so pushes to `dev` deploy Studio.

The three Vercel secrets the job needs (`VERCEL_TOKEN`, `VERCEL_ORG_ID`,
`VERCEL_PROJECT_ID`) were already present; only the gate was missing.

Note that `NEXT_PUBLIC_*` variables are inlined at build time, while server-side
variables such as `AUTH_URL` are read by the deployed function. Changing either
requires a redeploy to take effect on an existing deployment.
