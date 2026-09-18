#!/usr/bin/env bash
# Install the Kaveon CLI (macOS / Linux)
#
# Preview build (the moving engine-dev prerelease, default):
#   curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | bash
# Tagged release, verified against its SHA256SUMS:
#   curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | KAVEON_VERSION=0.3.0 bash
#   ./scripts/install.sh --version 0.3.0
#
# Environment: KAVEON_VERSION (release to install; empty means the preview),
# KAVEON_INSTALL_DIR (default ~/.local/bin), KAVEON_DOWNLOAD_BASE (a mirror of
# https://github.com/PruthviProdduturi/Kaveon/releases/download).
set -euo pipefail

REPO="PruthviProdduturi/Kaveon"
PREVIEW_TAG="engine-dev"
VERSION="${KAVEON_VERSION:-}"
INSTALL_DIR="${KAVEON_INSTALL_DIR:-${HOME}/.local/bin}"
DOWNLOAD_BASE="${KAVEON_DOWNLOAD_BASE:-https://github.com/${REPO}/releases/download}"

usage() {
    cat <<'USAGE'
Usage: install.sh [--version X.Y.Z]

  --version, -v   Install the tagged release cli-vX.Y.Z (verified against its
                  SHA256SUMS) instead of the engine-dev preview build.
  --help, -h      Show this help.

Environment: KAVEON_VERSION, KAVEON_INSTALL_DIR, KAVEON_DOWNLOAD_BASE.
USAGE
}

fail() {
    echo "  $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version|-v)
            [[ $# -ge 2 ]] || fail "--version needs a value"
            VERSION="$2"
            shift 2
            ;;
        --version=*)
            VERSION="${1#*=}"
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            usage >&2
            fail "Unknown option: $1"
            ;;
    esac
done

# Accept 0.3.0, v0.3.0 or cli-v0.3.0.
VERSION="${VERSION#cli-v}"
VERSION="${VERSION#v}"
if [[ -n "${VERSION}" && ! "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    fail "'${VERSION}' is not a release version (expected X.Y.Z)"
fi

echo ""
echo "  Installing Kaveon CLI"
echo ""

# Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}-${ARCH}" in
    Linux-x86_64)   TARGET="x86_64-unknown-linux-gnu"; PREVIEW_ASSET="kaveon-linux-x64" ;;
    Darwin-arm64)   TARGET="aarch64-apple-darwin";     PREVIEW_ASSET="kaveon-macos-arm64" ;;
    Darwin-x86_64)  TARGET="x86_64-apple-darwin";      PREVIEW_ASSET="" ;;
    *)
        echo "  Unsupported platform: ${OS}-${ARCH}" >&2
        fail "Build from source: cd engine && cargo install --path crates/cli"
        ;;
esac

sha256_of() {
    if command -v sha256sum > /dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum > /dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif command -v openssl > /dev/null 2>&1; then
        openssl dgst -sha256 "$1" | awk '{print $NF}'
    else
        fail "No SHA-256 tool found (sha256sum, shasum or openssl)."
    fi
}

download() {
    echo "  Downloading $2 from $1..."
    curl -fsSL --retry 3 "${DOWNLOAD_BASE}/$1/$2" -o "$3"
    test -s "$3" || fail "The download of $2 is empty."
}

# Copies $1 into INSTALL_DIR atomically (a rename on the same filesystem).
install_binary() {
    local staged
    staged="$(mktemp "${INSTALL_DIR}/.kaveon.XXXXXX")"
    cp -f -- "$1" "${staged}"
    chmod 755 "${staged}"
    mv -f -- "${staged}" "${INSTALL_DIR}/kaveon"
}

mkdir -p "${INSTALL_DIR}"
WORK="$(mktemp -d)"
trap 'rm -rf -- "${WORK}"' EXIT

if [[ -z "${VERSION}" ]]; then
    if [[ -z "${PREVIEW_ASSET}" ]]; then
        fail "The ${PREVIEW_TAG} preview has no Intel macOS build; install a release with KAVEON_VERSION=<X.Y.Z>."
    fi
    download "${PREVIEW_TAG}" "${PREVIEW_ASSET}" "${WORK}/kaveon"
    chmod +x "${WORK}/kaveon"
    "${WORK}/kaveon" --version
else
    TAG="cli-v${VERSION}"
    ASSET="kaveon-${VERSION}-${TARGET}.tar.gz"
    download "${TAG}" "${ASSET}" "${WORK}/${ASSET}"
    download "${TAG}" "SHA256SUMS" "${WORK}/SHA256SUMS"

    EXPECTED="$(awk -v name="${ASSET}" '{ file = $2; sub(/^\*/, "", file); if (file == name) print $1 }' "${WORK}/SHA256SUMS")"
    [[ -n "${EXPECTED}" ]] || fail "SHA256SUMS in ${TAG} has no entry for ${ASSET}."
    ACTUAL="$(sha256_of "${WORK}/${ASSET}")"
    if [[ "${ACTUAL}" != "${EXPECTED}" ]]; then
        echo "  SHA-256 mismatch for ${ASSET}" >&2
        echo "    expected ${EXPECTED}" >&2
        echo "    actual   ${ACTUAL}" >&2
        fail "The download was not installed."
    fi
    echo "  Verified SHA-256 ${ACTUAL}"

    tar -xzf "${WORK}/${ASSET}" -C "${WORK}" kaveon
    chmod +x "${WORK}/kaveon"
    REPORTED="$("${WORK}/kaveon" --version)"
    echo "  ${REPORTED}"
    [[ "${REPORTED}" == "kaveon ${VERSION}" ]] || fail "The binary reports '${REPORTED}', expected 'kaveon ${VERSION}'."
fi

install_binary "${WORK}/kaveon"
echo "  Installed: ${INSTALL_DIR}/kaveon"

# Check PATH
if [[ ":$PATH:" != *":${INSTALL_DIR}:"* ]]; then
    echo ""
    echo "  Add to your shell profile:"
    echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
fi

echo ""
echo "  Done. Run:"
echo "    kaveon --version"
echo "    kaveon --local --data-dir /path/to/parquet/files"
echo "    kaveon --server https://localhost:8080 --ca-cert /path/to/ca.crt"
echo "  For Microsoft sign-in, follow the CLI prompt when your server enables it."
echo ""
