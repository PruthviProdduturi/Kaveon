#!/usr/bin/env bash
# Install Kaveon Engine CLI (macOS / Linux)
# Usage: curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | bash
#    or: ./scripts/install.sh
set -euo pipefail

REPO="PruthviProdduturi/Kaveon"
TAG="engine-dev"
INSTALL_DIR="${HOME}/.local/bin"

echo ""
echo "  Installing Kaveon Engine CLI"
echo ""

# Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}-${ARCH}" in
    Linux-x86_64)   ASSET="kaveon-linux-x64" ;;
    Darwin-arm64)   ASSET="kaveon-macos-arm64" ;;
    *)
        echo "  Unsupported platform: ${OS}-${ARCH}"
        echo "  Build from source: cd engine && cargo install --path crates/cli"
        exit 1
        ;;
esac

URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"

mkdir -p "${INSTALL_DIR}"
TEMP_BINARY="$(mktemp "${INSTALL_DIR}/.kaveon.XXXXXX")"
trap 'rm -f -- "${TEMP_BINARY}"' EXIT

echo "  Downloading ${ASSET}..."
curl -fsSL --retry 3 "${URL}" -o "${TEMP_BINARY}"
test -s "${TEMP_BINARY}"
chmod +x "${TEMP_BINARY}"
"${TEMP_BINARY}" --version
mv -f -- "${TEMP_BINARY}" "${INSTALL_DIR}/kaveon"

echo "  Installed: ${INSTALL_DIR}/kaveon"

# Check PATH
if [[ ":$PATH:" != *":${INSTALL_DIR}:"* ]]; then
    echo ""
    echo "  Add to your shell profile:"
    echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
fi

echo ""
echo "  Done! Run:"
echo "    kaveon --version"
echo "    kaveon --local --data-dir /path/to/parquet/files"
echo "    kaveon --server https://localhost:8080 --ca-cert /path/to/ca.crt"
echo "  For Microsoft sign-in, follow the CLI prompt when your server enables it."
echo ""
