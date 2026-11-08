#!/bin/sh
# Starts the jc image as the cluster runs it (nonroot, read-only root filesystem, every capability
# dropped, /tmp and the state dir writable) with `--role all`, then checks that the admin
# listener answers /healthz, refuses /metrics without its token, and that a health or discovery
# route of every module answers on the public listener.
#   sh image-smoke.sh <image>
set -eu
image=$1
dir=$(mktemp -d)
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$dir/admin-token"
chmod 0644 "$dir/admin-token"
docker volume create jc-smoke-state >/dev/null
docker run -d --name jc-smoke -p 18080:8080 -p 19091:9091 \
  --read-only --tmpfs /tmp:rw,size=256m --cap-drop ALL --security-opt no-new-privileges \
  -v jc-smoke-state:/var/lib/jc -v "$dir/admin-token:/run/secrets/jc/admin-token:ro" \
  -e JCBIN_ADMIN_LISTEN=0.0.0.0:9091 -e JCBIN_ADMIN_TOKEN_FILE=/run/secrets/jc/admin-token \
  -e JCBIN_SCHEME=http \
  "$image" --role all >/dev/null
cleanup() { docker rm -f jc-smoke >/dev/null 2>&1 || true; docker volume rm jc-smoke-state >/dev/null 2>&1 || true; rm -rf "$dir"; }
trap cleanup EXIT
fail() { echo "SMOKE: $1"; docker logs jc-smoke 2>&1 | tail -60; exit 1; }
for _ in $(seq 90); do curl -sf localhost:19091/healthz >/dev/null && break; sleep 1; done
curl -sf localhost:19091/healthz >/dev/null || fail "no admin /healthz after 90 s"
[ "$(docker inspect -f '{{.Config.User}}' "$image")" = nonroot:nonroot ] || fail "the image does not run as nonroot"
code=$(curl -s -o /dev/null -w '%{http_code}' localhost:19091/metrics)
[ "$code" = 401 ] || fail "admin /metrics without a token answered $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $(cat "$dir/admin-token")" localhost:19091/metrics)
[ "$code" = 200 ] || fail "admin /metrics with its token answered $code"
# Each module's own health or discovery route (bins/jc/tests/one_binary.rs probe()), waiting for
# the Portal, which jc builds after it serves (503 until then).
check() { # path needle
  for _ in $(seq 60); do
    body=$(curl -s "localhost:18080$1" || true)
    case "$body" in *"$2"*) echo "ok $1"; return 0 ;; esac
    sleep 1
  done
  fail "$1 did not answer with $2: $(echo "$body" | head -c 300)"
}
check /id/realms/jc/.well-known/openid-configuration '"issuer"'
check /opendata/api/3/action/status_show '"success"'
check /dashboards/api/health '"database"'
check /streams/ping pong
check /broker/ngsi-ld/v1/info/sourceIdentity antares
check /git/api/healthz '"status":"ok"'
check /api/v1/health '"status":"ok"'
echo "SMOKE: ok"
