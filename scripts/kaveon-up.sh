#!/usr/bin/env bash
# Stand up a complete Kaveon deployment on any machine with Docker.
#
#   curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/kaveon-up.sh | bash
#   ./scripts/kaveon-up.sh                      # local storage, nothing to configure
#   ./scripts/kaveon-up.sh --storage s3://my-bucket/kaveon
#   ./scripts/kaveon-up.sh --storage adls://myaccount/kaveon/system
#
# What it brings up: the Engine coordinator, two Engine workers, the API and
# Studio. No PostgreSQL — KaveonDB is the system of record.
#
# The one decision worth making is --storage: the single store that holds
# everything Kaveon knows about itself (catalogs, schemas, tables, statistics,
# dashboards, charts, permissions). Table *data* can live anywhere a catalog
# points; this is where the record of it lives. Pick it once — moving it later
# is a migration, not a setting. See docs/engineering/system-storage.md.
#
# Safe to re-run. Existing secrets in .env are preserved, never regenerated.
set -euo pipefail

STORAGE="${KAVEON_SYSTEM_STORAGE:-}"
DATA_PATH="${KAVEON_DATA_PATH:-}"
ENV_FILE=".env"
PROFILE=""
PULL=1

usage() {
    sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
    cat <<'EOF'

Options:
  --storage <url>    System storage: file:///path, s3://bucket/prefix,
                     adls://account/container/prefix.
                     Default: file:///var/lib/kaveon/system (a Docker volume).
  --data <path>      Host directory holding table data to mount read-only.
  --env <file>       Environment file to read and write (default .env).
  --build            Build images from source instead of pulling.
  -h, --help         This text.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --storage) STORAGE="${2:?--storage needs a URL}"; shift 2 ;;
        --data) DATA_PATH="${2:?--data needs a path}"; shift 2 ;;
        --env) ENV_FILE="${2:?--env needs a path}"; shift 2 ;;
        --build) PULL=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

fail() { echo "kaveon: $*" >&2; exit 1; }
note() { printf '  %s\n' "$*"; }

# ── Prerequisites ────────────────────────────────────────────────────────────
command -v docker >/dev/null 2>&1 || fail "Docker is required: https://docs.docker.com/get-docker/"
docker compose version >/dev/null 2>&1 || fail "Docker Compose v2 is required (docker compose version)"
docker info >/dev/null 2>&1 || fail "the Docker daemon is not reachable; start Docker and try again"
[ -f docker-compose.yml ] || fail "run this from a Kaveon checkout (docker-compose.yml not found)"

# ── System storage ───────────────────────────────────────────────────────────
STORAGE="${STORAGE:-file:///var/lib/kaveon/system}"
case "$STORAGE" in
    file://*|s3://*|adls://*) ;;
    *) fail "--storage must begin with file://, s3:// or adls:// (got: ${STORAGE%%:*}…)" ;;
esac

# Credentials are never written into the storage URL or this file. ADLS uses
# workload then managed identity; S3 uses the provider's own chain. If the
# machine cannot already reach the store, say so now rather than at first write.
case "$STORAGE" in
    adls://*) [ -n "${AZURE_CLIENT_ID:-}${MSI_ENDPOINT:-}${IDENTITY_ENDPOINT:-}" ] \
        || note "note: no Azure identity in the environment; the host must provide one" ;;
    s3://*) [ -n "${AWS_ACCESS_KEY_ID:-}${AWS_WEB_IDENTITY_TOKEN_FILE:-}${AWS_ROLE_ARN:-}" ] \
        || note "note: no AWS credentials in the environment; the host must provide them" ;;
esac

# ── Translate the choice into what the Engine reads ──────────────────────────
# One URL is what a person should have to think about; these are the variables
# the Engine takes today. Written into the environment file so the running
# deployment and the file agree, and so a reader can see what was derived.
STORAGE_MODE="local"
STORAGE_LOCAL_PATH="/var/lib/kaveon/product-transactions"
STORAGE_ACCOUNT=""
STORAGE_CONTAINER=""
STORAGE_PREFIX="kaveon/product-catalog"

case "$STORAGE" in
    file://*)
        STORAGE_MODE="local"
        STORAGE_LOCAL_PATH="/${STORAGE#file:///}"
        ;;
    adls://*)
        rest="${STORAGE#adls://}"
        STORAGE_MODE="adls"
        STORAGE_ACCOUNT="${rest%%/*}"
        rest="${rest#*/}"
        STORAGE_CONTAINER="${rest%%/*}"
        case "$rest" in
            */*) STORAGE_PREFIX="${rest#*/}" ;;
            *) STORAGE_PREFIX="" ;;
        esac
        [ -n "$STORAGE_ACCOUNT" ] && [ -n "$STORAGE_CONTAINER" ] \
            || fail "adls:// needs <account>/<container>[/<prefix>]"
        ;;
    s3://*)
        # The Engine's object layer speaks S3 through the same conditional-write
        # protocol, but the server still reads the ADLS-named variables. Until
        # that rename lands, say so plainly rather than starting a deployment
        # that silently writes to a local directory.
        fail "s3:// system storage is not wired into the server yet; use adls:// or file:// (see docs/engineering/system-storage.md)"
        ;;
esac

# ── Secrets ──────────────────────────────────────────────────────────────────
# Generated once and then left alone, so re-running never invalidates a running
# deployment's sessions or breaks the Studio-to-API trust.
secret() {
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -hex 32
    else
        head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n'
    fi
}

read_existing() {
    [ -f "$ENV_FILE" ] || return 1
    sed -n "s/^$1=//p" "$ENV_FILE" | tail -1 | grep . || return 1
}

ensure() {
    local key="$1" value
    if value="$(read_existing "$key")"; then
        printf '%s' "$value"
    else
        printf '%s' "$(secret)"
    fi
}

AUTH_SECRET_VALUE="$(ensure AUTH_SECRET)"
PROXY_SECRET_VALUE="$(ensure KAVEON_PROXY_SECRET)"
ADMIN_TOKEN_VALUE="$(ensure KAVEON_ENGINE_ADMIN_TOKEN)"
BRIDGE_TOKEN_VALUE="$(ensure KAVEON_ENGINE_BRIDGE_TOKEN)"
EXCHANGE_TOKEN_VALUE="$(ensure KAVEON_EXCHANGE_TOKEN)"
CATALOG_TOKEN_VALUE="$(ensure KAVEON_CATALOG_ADMIN_TOKEN)"

# ── Environment file ─────────────────────────────────────────────────────────
# Only the keys below are this script's to write. Everything else in the file
# belongs to the operator and is carried through verbatim.
#
# This used to write the file whole, reading back just the six generated
# secrets. That silently deleted every other key on a re-run — and the keys it
# deleted were the ones docs/guides/self-hosting.md tells you to add by hand:
# AUTH_MICROSOFT_ENTRA_ID_ID, AUTH_MICROSOFT_ENTRA_ID_SECRET and
# AUTH_ADMIN_EMAILS. Losing the last of those leaves a deployment with no
# administrator at all, and the same guide tells you to re-run this script to
# restart ("Day two") and to upgrade. Provider keys, KAVEON_CREDENTIAL_KEYS,
# AUTH_URL, WEB_URL and every Engine tuning value went the same way.
MANAGED_KEYS="NODE_ENV KAVEON_SYSTEM_STORAGE KAVEON_PRODUCT_STORAGE_MODE
KAVEON_PRODUCT_LOCAL_PATH KAVEON_PRODUCT_ADLS_ACCOUNT KAVEON_PRODUCT_ADLS_CONTAINER
KAVEON_PRODUCT_ADLS_PREFIX KAVEON_DATA_PATH AUTH_SECRET KAVEON_PROXY_SECRET
KAVEON_ENGINE_ADMIN_TOKEN KAVEON_ENGINE_BRIDGE_TOKEN KAVEON_EXCHANGE_TOKEN
KAVEON_CATALOG_ADMIN_TOKEN"

is_managed() {
    local candidate="$1" key
    for key in $MANAGED_KEYS; do
        [ "$key" = "$candidate" ] && return 0
    done
    return 1
}

# A data path given on this run wins; one already in the file is kept. Without
# this, re-running without --data dropped the mount and the workers came up
# with nothing to read.
if [ -z "$DATA_PATH" ]; then
    DATA_PATH="$(read_existing KAVEON_DATA_PATH || true)"
fi

# Lines the operator owns, in the order they were written, comments between
# them dropped only where they introduced a managed key. Read before the file
# is touched.
CARRIED=""
if [ -f "$ENV_FILE" ]; then
    CARRIED="$(awk -v managed="$MANAGED_KEYS" '
        BEGIN { split(managed, list, /[ \n]+/); for (i in list) if (list[i] != "") own[list[i]] = 1 }
        /^[ \t]*#/ { next }
        /^[ \t]*$/ { next }
        {
            line = $0
            eq = index(line, "=")
            if (eq == 0) { print line; next }
            key = substr(line, 1, eq - 1)
            gsub(/^[ \t]+|[ \t]+$/, "", key)
            if (!(key in own)) print line
        }
    ' "$ENV_FILE")"
fi

umask 077
TEMP_ENV="${ENV_FILE}.kaveon-up.$$"
trap 'rm -f "$TEMP_ENV"' EXIT
{
    echo "# Managed block written by scripts/kaveon-up.sh on $(date -u +%Y-%m-%dT%H:%M:%SZ)."
    echo "# Secrets here are generated once; re-running preserves them, and"
    echo "# preserves every key below this block that the script does not own."
    echo
    echo "NODE_ENV=production"
    echo "KAVEON_SYSTEM_STORAGE=${STORAGE}"
    echo "KAVEON_PRODUCT_STORAGE_MODE=${STORAGE_MODE}"
    echo "KAVEON_PRODUCT_LOCAL_PATH=${STORAGE_LOCAL_PATH}"
    echo "KAVEON_PRODUCT_ADLS_ACCOUNT=${STORAGE_ACCOUNT}"
    echo "KAVEON_PRODUCT_ADLS_CONTAINER=${STORAGE_CONTAINER}"
    echo "KAVEON_PRODUCT_ADLS_PREFIX=${STORAGE_PREFIX}"
    [ -n "$DATA_PATH" ] && echo "KAVEON_DATA_PATH=${DATA_PATH}"
    echo
    echo "AUTH_SECRET=${AUTH_SECRET_VALUE}"
    echo "KAVEON_PROXY_SECRET=${PROXY_SECRET_VALUE}"
    echo "KAVEON_ENGINE_ADMIN_TOKEN=${ADMIN_TOKEN_VALUE}"
    echo "KAVEON_ENGINE_BRIDGE_TOKEN=${BRIDGE_TOKEN_VALUE}"
    echo "KAVEON_EXCHANGE_TOKEN=${EXCHANGE_TOKEN_VALUE}"
    echo "KAVEON_CATALOG_ADMIN_TOKEN=${CATALOG_TOKEN_VALUE}"
    echo
    if [ -n "$CARRIED" ]; then
        echo "# Yours. This script does not read or rewrite anything below here."
        printf '%s\n' "$CARRIED"
    else
        echo "# Sign-in. Without an OAuth provider the stack runs in local mode with"
        echo "# a single development identity — fine on a laptop, not on a network."
        echo "# Set these to require real sign-in; see docs/guides/self-hosting.md."
        echo "# AUTH_MICROSOFT_ENTRA_ID_ID="
        echo "# AUTH_MICROSOFT_ENTRA_ID_SECRET="
        echo "# AUTH_ADMIN_EMAILS=you@example.com"
    fi
} > "$TEMP_ENV"

# Replaced in one step: an interrupted run leaves the old file, never half of
# a new one. A deployment with half an environment file does not start.
mv -f "$TEMP_ENV" "$ENV_FILE"
trap - EXIT
carried_count=$(printf '%s' "$CARRIED" | grep -c . || true)
note "wrote ${ENV_FILE} (mode 600; ${carried_count} of your own keys kept)"

# ── Images ───────────────────────────────────────────────────────────────────
if [ "$PULL" = "1" ]; then
    note "pulling images…"
    docker compose pull --quiet 2>/dev/null || {
        note "no published images for this checkout; building from source instead"
        PULL=0
    }
fi
if [ "$PULL" = "0" ]; then
    note "building images (first build takes a while — the Engine is Rust)…"
    docker compose build
fi

# ── Up ───────────────────────────────────────────────────────────────────────
note "starting…"
docker compose up -d

# ── Wait, and say plainly whether it worked ──────────────────────────────────
deadline=$(( $(date +%s) + 300 ))
healthy() { docker compose ps --format '{{.Service}} {{.Health}}' 2>/dev/null | grep -c 'healthy' || true; }
want=$(docker compose ps --services 2>/dev/null | wc -l)
while [ "$(date +%s)" -lt "$deadline" ]; do
    if docker compose ps --format '{{.Service}} {{.State}} {{.Health}}' 2>/dev/null \
        | awk '$3 == "unhealthy" { bad = 1 } END { exit !bad }'; then
        docker compose ps
        fail "a service is unhealthy; 'docker compose logs' has the detail"
    fi
    [ "$(healthy)" -ge 2 ] && break
    sleep 5
done

echo
docker compose ps --format 'table {{.Service}}\t{{.State}}\t{{.Health}}'
echo
note "Studio    http://localhost:3000"
note "API       http://localhost:8082/api/health"
note "Engine    http://localhost:8081/v1/statement"
note "storage   ${STORAGE}"
echo
note "Next: open Studio, register a catalog over your data, and run ANALYZE on a"
note "table so it can answer counts and bounds without reading it."
