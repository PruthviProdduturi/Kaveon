#!/usr/bin/env bash
# Kaveon — local development setup (macOS / Linux)
# Usage: ./scripts/setup.sh
set -euo pipefail

CYAN='\033[0;36m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m'

step()  { echo -e "\n${CYAN}==> $1${NC}"; }
ok()    { echo -e "    ${GREEN}$1${NC}"; }
warn()  { echo -e "    ${YELLOW}$1${NC}"; }

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo ""
echo "  Kaveon — Local Development Setup"
echo "  Talk to your data."
echo ""

# --- Prerequisites ---

step "Checking prerequisites"

# Node.js
if command -v node &>/dev/null; then
    ok "Node.js $(node --version)"
else
    warn "Node.js not found — install: https://nodejs.org or brew install node"
fi

# pnpm
if command -v pnpm &>/dev/null; then
    ok "pnpm $(pnpm --version)"
else
    warn "pnpm not found — installing"
    npm install -g pnpm
fi

# Python
if command -v python3 &>/dev/null; then
    ok "$(python3 --version)"
else
    warn "Python not found — install: brew install python@3.11"
fi

# Rust
if command -v cargo &>/dev/null; then
    ok "$(rustc --version)"
else
    warn "Rust not found — installing via rustup"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

# --- Studio ---

step "Setting up Studio (Next.js)"
if [ -f "studio/package.json" ]; then
    cd studio
    pnpm install --no-frozen-lockfile
    ok "Studio dependencies installed"
    cd "$ROOT"
else
    warn "studio/package.json not found — skipping"
fi

# --- API ---

step "Setting up API (Python)"
if [ -f "api/requirements.txt" ]; then
    cd api
    if [ ! -d ".venv" ]; then
        python3 -m venv .venv
    fi
    source .venv/bin/activate
    pip install -q -r requirements.txt
    ok "API dependencies installed"
    cd "$ROOT"
else
    warn "api/requirements.txt not found — skipping"
fi

# --- Engine ---

step "Building Kaveon Engine CLI"
if command -v cargo &>/dev/null; then
    cd engine
    cargo build --release --package kaveon-cli
    ok "Built: engine/target/release/kaveon"

    # Symlink to ~/.local/bin
    mkdir -p "$HOME/.local/bin"
    ln -sf "$(pwd)/target/release/kaveon" "$HOME/.local/bin/kaveon"
    ok "Linked: ~/.local/bin/kaveon"

    if [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then
        warn "Add to your shell profile: export PATH=\"\$HOME/.local/bin:\$PATH\""
    fi
    cd "$ROOT"
else
    echo -e "    ${RED}cargo not found — restart terminal after Rust install, then re-run${NC}"
fi

# --- Done ---

echo ""
echo -e "  ${GREEN}Setup complete!${NC}"
echo ""
echo "  Quick start:"
echo "    kaveon --data-dir <path>     # SQL shell"
echo "    cd studio && pnpm dev        # Studio on localhost:3000"
echo "    cd api && python main.py     # API on localhost:8000"
echo ""
