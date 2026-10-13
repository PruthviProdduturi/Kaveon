#!/usr/bin/env bash
# Run the `kaveon` CLI against an AKS installation's Engine.
#
# The Engine is not published. Studio's ingress is the only door into the
# cluster, and the API behind it is reached only through Studio's proxy, which
# is what stamps the identity the API trusts — so there is no public endpoint
# for the coordinator and `--server https://kaveon-coordinator:8080` from a
# laptop fails with a DNS error, not an authentication one.
#
# This forwards the coordinator's port to the loopback address and points the
# CLI at it. The serving certificate carries `localhost` and `127.0.0.1` among
# its names, so the connection is verified against the Engine's own CA rather
# than skipping verification — the point is to exercise the real TLS path, not
# to tunnel around it.
#
# Arguments are passed through to `kaveon`, so:
#
#   scripts/kaveon-cli-aks.sh                                  # interactive
#   scripts/kaveon-cli-aks.sh -e "SHOW CATALOGS"
#   scripts/kaveon-cli-aks.sh --catalog OpenSource --schema climate_energy \
#     -e "SELECT count(*) FROM energy"
#
# Requires kubectl credentials for the cluster and `kaveon` on PATH.
set -Eeuo pipefail

NAMESPACE="${KAVEON_NAMESPACE:-kaveon}"
LOCAL_PORT="${KAVEON_CLI_LOCAL_PORT:-18080}"
TLS_SECRET="${KAVEON_ENGINE_TLS_SECRET:-kaveon-engine-tls}"
AUTH_SECRET="${KAVEON_ENGINE_AUTH_SECRET:-kaveon-engine-auth}"

command -v kubectl >/dev/null || { echo 'kubectl is required' >&2; exit 1; }
command -v kaveon  >/dev/null || { echo 'kaveon is not on PATH; see docs/guides/engine-cli.md' >&2; exit 1; }

work="$(mktemp -d)"
forward_pid=""
cleanup() {
  [[ -n "$forward_pid" ]] && kill "$forward_pid" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

# The CA the Engine's certificate is signed by. Taken from the cluster rather
# than a checked-in copy, so rotating the CA does not leave this stale.
kubectl -n "$NAMESPACE" get secret "$TLS_SECRET" -o jsonpath='{.data.ca\.crt}' \
  | base64 --decode > "$work/ca.pem"
test -s "$work/ca.pem" || { echo "could not read the Engine CA from $TLS_SECRET" >&2; exit 1; }

# An operator token from the Engine's own security document. Never printed and
# never written to disk: it is exported into this process only.
KAVEON_ACCESS_TOKEN="$(
  kubectl -n "$NAMESPACE" get secret "$AUTH_SECRET" -o jsonpath='{.data.security\.json}' \
    | base64 --decode \
    | python -c 'import json,sys; print(json.load(sys.stdin)["principals"][0]["token"], end="")'
)"
[[ -n "$KAVEON_ACCESS_TOKEN" ]] || { echo "could not read an operator token from $AUTH_SECRET" >&2; exit 1; }
export KAVEON_ACCESS_TOKEN
export KAVEON_CA_CERT="$work/ca.pem"

kubectl -n "$NAMESPACE" port-forward service/kaveon-coordinator "$LOCAL_PORT:8080" >"$work/forward.log" 2>&1 &
forward_pid=$!

# Wait for the forward rather than sleeping a guessed interval: a fixed sleep
# is either a wasted second or a race, and the failure mode of the race is a
# connection error that looks like a TLS problem.
for _ in $(seq 50); do
  grep -q "Forwarding from" "$work/forward.log" 2>/dev/null && break
  kill -0 "$forward_pid" 2>/dev/null || { echo 'port-forward exited:' >&2; cat "$work/forward.log" >&2; exit 1; }
  sleep 0.2
done
grep -q "Forwarding from" "$work/forward.log" 2>/dev/null \
  || { echo 'port-forward did not become ready:' >&2; cat "$work/forward.log" >&2; exit 1; }

# 127.0.0.1 rather than localhost: `kubectl port-forward` listens on IPv4, and
# on Windows localhost resolves to ::1 first, so the name fails to connect while
# the address succeeds. The certificate carries both, so either verifies — the
# difference is only which one can be reached.
#
# Run rather than exec. `exec` replaces this shell, which discards the EXIT
# trap along with it, so the forward outlived the CLI and held the port — the
# next run then failed to bind and reported it as the Engine being
# unreachable. The CLI's exit status is passed through instead.
status=0
kaveon --server "https://127.0.0.1:$LOCAL_PORT" "$@" || status=$?
exit "$status"
