#!/usr/bin/env bash
#
# This work is licensed under [EU PL 1.2]
# (https://gitlab.com/civitas-connect/civitas-core/civitas-core/-/blob/main/LICENSE)
# by Civitas Connect e. V., Hafenweg 7, 48155 Münster, Germany, and [other authors].
#
# You may not use this work except in compliance with the Licence.
#
# -----------------------------------------------------------------------------
# Local pull-through image caches for the k3d dev cluster.
#
# Each upstream registry gets its own `registry:2` running in proxy mode, with a
# persistent named volume. The containers live OUTSIDE k3d (plain Docker, with
# --restart unless-stopped), so they survive `k3d cluster delete` / recreation —
# which is exactly what the variant test harness does between runs.
#
# The cluster reaches them via `host.k3d.internal` (injected by k3d into every
# node) — see the `registries.config` mirrors block in k3d-jc-local.yaml.
# Those mirror endpoints list the real upstream as a fallback, so the cluster
# keeps working even when these caches are stopped.
#
#   ./dev-deployment/registry-cache.sh up       # start caches (run once)
#   ./dev-deployment/registry-cache.sh status
#   ./dev-deployment/registry-cache.sh down     # stop+remove containers (keep data)
#   ./dev-deployment/registry-cache.sh prune    # also drop the cached data volumes
#
# First pull of an image is slow (populates the cache); every fresh cluster
# afterwards pulls from the local cache and starts in seconds.
# -----------------------------------------------------------------------------

set -euo pipefail

# name | host-port | upstream remote URL
#
# Ports must match the mirror endpoints in k3d-jc-local.yaml.
# registry.gitlab.com is included for legacy images pulled from GitLab; if those are private,
# either `docker login registry.gitlab.com` is not enough for the proxy — set
# REGISTRY_PROXY_USERNAME/PASSWORD below — but thanks to the upstream fallback in
# the mirror config a 401 from the cache simply falls back to a direct pull.
CACHES=(
  "pullcache-dockerio|5001|https://registry-1.docker.io"
  "pullcache-ghcr|5002|https://ghcr.io"
  "pullcache-quay|5003|https://quay.io"
  "pullcache-k8s|5004|https://registry.k8s.io"
  "pullcache-gitlab|5005|https://registry.gitlab.com"
)

command -v docker >/dev/null 2>&1 || { echo "docker not found" >&2; exit 1; }

up() {
  local c name port upstream running
  for c in "${CACHES[@]}"; do
    IFS='|' read -r name port upstream <<<"$c"
    running="$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null || echo false)"
    if [ "$running" = "true" ]; then
      echo "✓ $name already running (:$port -> $upstream)"
      continue
    fi
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker run -d --restart unless-stopped --name "$name" \
      -p "${port}:5000" \
      -v "${name}-data:/var/lib/registry" \
      -e "REGISTRY_PROXY_REMOTEURL=${upstream}" \
      registry:2 >/dev/null
    echo "✓ started $name (:$port -> $upstream)"
  done
  echo
  echo "Caches are reachable from the cluster as host.k3d.internal:<port>."
  echo "They are wired into k3d-jc-local.yaml — just (re)create the cluster."
}

down() {
  local c name
  for c in "${CACHES[@]}"; do
    IFS='|' read -r name _ _ <<<"$c"
    docker rm -f "$name" >/dev/null 2>&1 && echo "removed $name" || true
  done
}

prune() {
  down
  local c name
  for c in "${CACHES[@]}"; do
    IFS='|' read -r name _ _ <<<"$c"
    docker volume rm "${name}-data" >/dev/null 2>&1 && echo "dropped volume ${name}-data" || true
  done
}

status() {
  docker ps -a --filter 'name=pullcache-' \
    --format 'table {{.Names}}\t{{.Ports}}\t{{.Status}}'
}

case "${1:-up}" in
  up)     up ;;
  down)   down ;;
  prune)  prune ;;
  status) status ;;
  -h|--help) sed -n '28,33p' "$0" | sed 's/^# \{0,1\}//' ;;
  *) echo "usage: $0 {up|down|status|prune}" >&2; exit 1 ;;
esac
