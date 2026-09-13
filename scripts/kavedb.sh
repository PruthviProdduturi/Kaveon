#!/usr/bin/env bash
set -euo pipefail

compose_file="$(cd "$(dirname "$0")/.." && pwd)/docker-compose.kavedb.yml"
case "${1:-up}" in
  up) shift; docker compose -f "$compose_file" up -d "$@" ;;
  build) docker compose -f "$compose_file" up -d --build ;;
  down) docker compose -f "$compose_file" down ;;
  logs) docker compose -f "$compose_file" logs -f kaveon-db ;;
  status) docker compose -f "$compose_file" ps ;;
  *) echo "Usage: $0 {up|build|down|logs|status}" >&2; exit 2 ;;
esac
