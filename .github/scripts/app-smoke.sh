#!/bin/sh
# Starts a ui-rust App's image as the cluster runs it (uid 65532, read-only root filesystem,
# every capability dropped) with an endpoint nothing answers, then checks that /healthz answers,
# that / serves the App's page with its title, and that the image runs as 65532.
#   sh app-smoke.sh <image> <page title>
set -eu
image=$1
title=$2
docker run -d --name app-smoke -p 18080:8080 \
  --read-only --tmpfs /tmp:rw,size=16m --cap-drop ALL --security-opt no-new-privileges \
  -e JC_BIND_ADDRESS=0.0.0.0:8080 -e JC_BASE_PATH=/ \
  -e JC_ENDPOINT_URL=http://endpoint.smoke.invalid/api/endpoint/smoke/ "$image" >/dev/null
cleanup() { docker rm -f app-smoke >/dev/null 2>&1 || true; }
trap cleanup EXIT
fail() { echo "SMOKE: $1"; docker logs app-smoke 2>&1 | tail -40; exit 1; }
for _ in $(seq 30); do curl -sf localhost:18080/healthz >/dev/null && break; sleep 1; done
curl -sf localhost:18080/healthz >/dev/null || fail "no /healthz after 30 s"
[ "$(docker inspect -f '{{.Config.User}}' "$image")" = 65532:65532 ] || fail "the image does not run as 65532"
curl -sf localhost:18080/ | grep -qF "<title>$title</title>" || fail "/ is not the page titled $title"
echo "SMOKE: $image ok"
