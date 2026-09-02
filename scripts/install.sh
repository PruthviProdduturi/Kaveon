#!/usr/bin/env bash
# Install Kaveon Engine CLI (macOS / Linux)
# Usage: curl -sSf https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | sh
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
    Darwin-x86_64)  ASSET="kaveon-macos-arm64" ;;  # Rosetta
    *)
        echo "  Unsupported platform: ${OS}-${ARCH}"
        echo "  Build from source: cd engine && cargo install --path crates/cli"
        exit 1
        ;;
esac

URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"

mkdir -p "${INSTALL_DIR}"

echo "  Downloading ${ASSET}..."
curl -sSL "${URL}" -o "${INSTALL_DIR}/kaveon"
chmod +x "${INSTALL_DIR}/kaveon"

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
echo "    kaveon --data-dir /path/to/parquet/files"
echo ""
