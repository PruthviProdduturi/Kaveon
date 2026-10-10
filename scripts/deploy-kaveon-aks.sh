#!/usr/bin/env bash
set -Eeuo pipefail

# Fail-closed AKS installer. It validates identity, TLS, immutable images,
# migration evidence and PostgreSQL-free rendering before changing a release.
usage() {
  cat <<'EOF'
Usage: deploy-kaveon-aks.sh [--mode retirement|rehearsal]

Required environment:
  AZURE_SUBSCRIPTION_ID AZURE_RESOURCE_GROUP AZURE_AKS_NAME KAVEON_ACR_NAME
  API_IMAGE_DIGEST STUDIO_IMAGE_DIGEST KAVEON_API_WORKLOAD_IDENTITY_CLIENT_ID
  KAVEON_KEY_VAULT_URL KAVEON_ADLS_ACCOUNT KAVEON_ADLS_CONTAINER KAVEON_ADLS_ROOT_PATH
  KAVEON_ENGINE_CA_FILE or KAVEON_ENGINE_CA_B64
  KAVEON_ENGINE_BRIDGE_TOKEN KAVEON_ENGINE_CATALOG_TOKEN
  KAVEON_CREDENTIAL_ACTIVE_KEY KAVEON_CREDENTIAL_KEYS
  AUTH_MICROSOFT_ENTRA_ID_ID AUTH_MICROSOFT_ENTRA_ID_ISSUER AUTH_ENTRA_ADMIN_OBJECT_IDS

For retirement or rehearsal mode, KAVEON_EVIDENCE_PVC, KAVEON_AUTHORITY_FAMILIES,
and KAVEON_READ_AUTHORITY_FAMILIES are required. Secret values are never printed.
EOF
}
MODE=retirement
while (($#)); do
  case "$1" in
    --mode) MODE="${2:?missing mode}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done
[[ "$MODE" == retirement || "$MODE" == rehearsal ]] || { echo 'mode must be retirement or rehearsal' >&2; exit 2; }
need_cmd() { command -v "$1" >/dev/null || { echo "Missing required command: $1" >&2; exit 1; }; }
for c in az kubectl kubelogin helm jq openssl base64; do need_cmd "$c"; done
need() { [[ -n "${!1:-}" ]] || { echo "Missing required environment variable: $1" >&2; exit 1; }; }
for v in AZURE_SUBSCRIPTION_ID AZURE_RESOURCE_GROUP AZURE_AKS_NAME KAVEON_ACR_NAME API_IMAGE_DIGEST STUDIO_IMAGE_DIGEST KAVEON_API_WORKLOAD_IDENTITY_CLIENT_ID KAVEON_KEY_VAULT_URL KAVEON_ADLS_ACCOUNT KAVEON_ADLS_CONTAINER KAVEON_ADLS_ROOT_PATH KAVEON_ENGINE_BRIDGE_TOKEN KAVEON_ENGINE_CATALOG_TOKEN KAVEON_CREDENTIAL_ACTIVE_KEY KAVEON_CREDENTIAL_KEYS AUTH_MICROSOFT_ENTRA_ID_ID AUTH_MICROSOFT_ENTRA_ID_ISSUER AUTH_ENTRA_ADMIN_OBJECT_IDS; do need "$v"; done
[[ "$API_IMAGE_DIGEST" =~ ^sha256:[a-f0-9]{64}$ ]] || { echo 'API_IMAGE_DIGEST must be an immutable sha256 digest' >&2; exit 1; }
[[ "$STUDIO_IMAGE_DIGEST" =~ ^sha256:[a-f0-9]{64}$ ]] || { echo 'STUDIO_IMAGE_DIGEST must be an immutable sha256 digest' >&2; exit 1; }
if [[ "$MODE" == retirement || "$MODE" == rehearsal ]]; then
  for v in KAVEON_EVIDENCE_PVC KAVEON_AUTHORITY_FAMILIES KAVEON_READ_AUTHORITY_FAMILIES; do need "$v"; done
fi
NAMESPACE="${KAVEON_NAMESPACE:-kaveon}"
RELEASE="${KAVEON_HELM_RELEASE:-kaveon-platform}"
CHART="${KAVEON_CHART_DIR:-infra/helm/kaveon-platform}"
EVIDENCE_PVC="${KAVEON_EVIDENCE_PVC:-kaveon-retirement-evidence}"
AUTH_SECRET_NAME="${KAVEON_AUTH_SECRET_NAME:-kaveon-portal-auth}"
CA_CONFIGMAP="${KAVEON_ENGINE_CA_CONFIGMAP:-kaveon-engine-ca}"
CA_KEY="${KAVEON_ENGINE_CA_KEY:-ca.crt}"
AUTH_SECRET="${KAVEON_AUTH_SECRET:-}"
PROXY_SECRET="${KAVEON_PROXY_SECRET:-}"
tmp="$(mktemp -d)"
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT

echo "Checking Azure and cluster prerequisites..."
az account set --subscription "$AZURE_SUBSCRIPTION_ID"
az aks show -g "$AZURE_RESOURCE_GROUP" -n "$AZURE_AKS_NAME" --query '{state:provisioningState,power:powerState.code}' -o tsv | grep -q $'Succeeded\tRunning' || { echo 'AKS is not in Succeeded/Running state' >&2; exit 1; }
az aks get-credentials -g "$AZURE_RESOURCE_GROUP" -n "$AZURE_AKS_NAME" --overwrite-existing >/dev/null
if command -v kubelogin >/dev/null; then kubelogin convert-kubeconfig -l azurecli >/dev/null; fi
kubectl get namespace "$NAMESPACE" >/dev/null 2>&1 || kubectl create namespace "$NAMESPACE" >/dev/null
read_existing_secret() {
  local key="$1"
  kubectl -n "$NAMESPACE" get secret "$AUTH_SECRET_NAME" -o "jsonpath={.data.$key}" 2>/dev/null | base64 --decode 2>/dev/null || true
}
AUTH_SECRET="${AUTH_SECRET:-$(read_existing_secret AUTH_SECRET)}"
PROXY_SECRET="${PROXY_SECRET:-$(read_existing_secret KAVEON_PROXY_SECRET)}"
AUTH_SECRET="${AUTH_SECRET:-$(openssl rand -base64 32 | tr -d '\n')}"
PROXY_SECRET="${PROXY_SECRET:-$(openssl rand -hex 32)}"

echo "Checking Engine and evidence prerequisites..."
kubectl -n "$NAMESPACE" get service kaveon-coordinator >/dev/null || { echo 'Engine service kaveon-coordinator is missing' >&2; exit 1; }
# The evidence volume must exist before the API mounts it, but the chart
# creates it unless told otherwise, so requiring it up front failed a first
# deploy on a volume that run was about to make. Insist only when we are not
# the ones creating it.
if [[ ( "$MODE" == retirement || "$MODE" == rehearsal ) && "${KAVEON_CREATE_EVIDENCE_PVC:-true}" != "true" ]]; then
  kubectl -n "$NAMESPACE" get pvc "$EVIDENCE_PVC" >/dev/null || { echo "Evidence PVC $EVIDENCE_PVC is missing and KAVEON_CREATE_EVIDENCE_PVC is not true; run the reviewed replay/evidence job first" >&2; exit 1; }
fi
if [[ -n "${KAVEON_ENGINE_CA_FILE:-}" ]]; then
  [[ -f "$KAVEON_ENGINE_CA_FILE" ]] || { echo "CA file does not exist: $KAVEON_ENGINE_CA_FILE" >&2; exit 1; }
  cp "$KAVEON_ENGINE_CA_FILE" "$tmp/ca.crt"
elif [[ -n "${KAVEON_ENGINE_CA_B64:-}" ]]; then
  printf '%s' "$KAVEON_ENGINE_CA_B64" | base64 --decode > "$tmp/ca.crt"
else
  echo 'Provide KAVEON_ENGINE_CA_FILE or KAVEON_ENGINE_CA_B64' >&2; exit 1
fi
test -s "$tmp/ca.crt" || { echo 'Engine CA is empty' >&2; exit 1; }
kubectl -n "$NAMESPACE" create configmap "$CA_CONFIGMAP" --from-file="$CA_KEY=$tmp/ca.crt" --dry-run=client -o yaml | kubectl apply -f - >/dev/null

echo "Applying portal authentication Secret (values are not printed)..."
kubectl -n "$NAMESPACE" create secret generic "$AUTH_SECRET_NAME" \
  --from-literal=AUTH_SECRET="$AUTH_SECRET" --from-literal=KAVEON_PROXY_SECRET="$PROXY_SECRET" \
  --from-literal=KAVEON_ENGINE_BRIDGE_TOKEN="$KAVEON_ENGINE_BRIDGE_TOKEN" \
  --from-literal=KAVEON_ENGINE_CATALOG_TOKEN="$KAVEON_ENGINE_CATALOG_TOKEN" \
  --from-literal=KAVEON_CREDENTIAL_ACTIVE_KEY="$KAVEON_CREDENTIAL_ACTIVE_KEY" \
  --from-literal=KAVEON_CREDENTIAL_KEYS="$KAVEON_CREDENTIAL_KEYS" \
  --from-literal=AUTH_MICROSOFT_ENTRA_ID_ID="$AUTH_MICROSOFT_ENTRA_ID_ID" \
  --from-literal=AUTH_MICROSOFT_ENTRA_ID_ISSUER="$AUTH_MICROSOFT_ENTRA_ID_ISSUER" \
  --from-literal=AUTH_ENTRA_ADMIN_OBJECT_IDS="$AUTH_ENTRA_ADMIN_OBJECT_IDS" \
  --from-literal=GITHUB_ID="${GITHUB_ID:-}" \
  --from-literal=GITHUB_SECRET="${GITHUB_SECRET:-}" \
  --from-literal=AUTH_ADMIN_EMAILS="${AUTH_ADMIN_EMAILS:-}" \
  --dry-run=client -o yaml | kubectl apply -f - >/dev/null

RETIREMENT=$( [[ "$MODE" == retirement ]] && echo true || echo false )
REHEARSAL=$( [[ "$MODE" == rehearsal ]] && echo true || echo false )
cat > "$tmp/values.yaml" <<EOF
namespace: $NAMESPACE
images:
  api: {repository: ${KAVEON_ACR_NAME}.azurecr.io/kaveon-api, digest: $API_IMAGE_DIGEST}
  studio: {repository: ${KAVEON_ACR_NAME}.azurecr.io/kaveon-studio, digest: $STUDIO_IMAGE_DIGEST}
secrets: {portalAuth: $AUTH_SECRET_NAME}
# The one published door. Off unless the installation names a host, so a
# deployment is never exposed by default; the API stays cluster-internal
# either way and is reached only through Studio's proxy.
ingress:
  enabled: ${KAVEON_INGRESS_ENABLED:-false}
  host: "${KAVEON_INGRESS_HOST:-}"
  className: "${KAVEON_INGRESS_CLASS:-}"
  annotations:
    cert-manager.io/cluster-issuer: "${KAVEON_CERT_ISSUER:-}"
  tls:
    enabled: ${KAVEON_INGRESS_TLS_ENABLED:-false}
    secretName: "${KAVEON_INGRESS_TLS_SECRET:-}"
engine: {service: kaveon-coordinator.$NAMESPACE.svc.cluster.local, port: 8080, caConfigMap: $CA_CONFIGMAP, caKey: $CA_KEY}
seed:
  catalog: OpenSource
  name: OpenSource
  adls: {account: $KAVEON_ADLS_ACCOUNT, container: opensource, rootPath: $KAVEON_ADLS_ROOT_PATH}
api:
  evidenceStorage: {create: ${KAVEON_CREATE_EVIDENCE_PVC:-true}, name: $EVIDENCE_PVC, storageClass: azurefile-csi, size: 1Gi, accessModes: [ReadWriteMany], retain: true}
  cutover:
    retirementMode: $RETIREMENT
    restartRehearsalMode: $REHEARSAL
    authorityFamilies: "$KAVEON_AUTHORITY_FAMILIES"
    readAuthorityFamilies: "$KAVEON_READ_AUTHORITY_FAMILIES"
    evidencePvc: $EVIDENCE_PVC
    postgresqlWriteFence: true
    liveDlmArtifactPublish: true
    artifactAccount: $KAVEON_ADLS_ACCOUNT
    artifactContainer: $KAVEON_ADLS_CONTAINER
  workloadIdentity: {clientId: $KAVEON_API_WORKLOAD_IDENTITY_CLIENT_ID}
  keyVaultUrl: $KAVEON_KEY_VAULT_URL
EOF

echo "Rendering and validating PostgreSQL-free release..."
render="$tmp/rendered.yaml"
helm lint "$CHART" -n "$NAMESPACE" -f "$tmp/values.yaml" >/dev/null
helm template "$RELEASE" "$CHART" -n "$NAMESPACE" -f "$tmp/values.yaml" > "$render"
if grep -q 'kind: StatefulSet' "$render" || grep -q 'kaveon-postgres' "$render"; then echo 'Refusing deployment: rendered PostgreSQL resources are present' >&2; exit 1; fi
grep -q 'KAVEON_POSTGRESQL_RETIREMENT_MODE' "$render" || { echo 'Retirement flag missing from render' >&2; exit 1; }

echo "Deploying $RELEASE atomically..."
helm upgrade --install "$RELEASE" "$CHART" -n "$NAMESPACE" -f "$tmp/values.yaml" --create-namespace --atomic --wait --timeout "${KAVEON_HELM_TIMEOUT:-15m}"
kubectl -n "$NAMESPACE" rollout status deployment/kaveon-api --timeout=5m
kubectl -n "$NAMESPACE" rollout status deployment/kaveon-portal --timeout=5m
kubectl -n "$NAMESPACE" get pods -o wide

echo "Running in-cluster smoke checks..."
SMOKE="kaveon-smoke-$RANDOM"
kubectl -n "$NAMESPACE" run "$SMOKE" --rm -i --restart=Never --labels app=kaveon-portal --image=curlimages/curl:8.10.1 --command -- curl --fail --silent --show-error http://kaveon-api:8080/api/health >/dev/null
kubectl -n "$NAMESPACE" get deployment kaveon-api kaveon-portal -o json | jq -e 'all(.items[]; .status.availableReplicas >= 1)' >/dev/null
echo "Kaveon AKS deployment passed preflight, rollout, and health checks."


