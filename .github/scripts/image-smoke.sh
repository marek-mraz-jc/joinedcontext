#!/bin/sh
# Starts the jc image as the cluster runs it (nonroot, read-only root filesystem, every capability
# dropped, /tmp and the state dir writable) with `--role all` in the config shape of deployment
# components/jc (slug mode, the Portal at the root), then checks that the admin listener answers
# /healthz, refuses /metrics without its token, and that a health or discovery route of every
# module answers on the public listener. base_host is an address jc reaches itself on: the
# Portal reads kaisatsu's discovery at the public issuer before it serves (the cluster sends that
# read through the edge, deployment components/jc/apisix-routes.yaml jc-identity).
#   sh image-smoke.sh <image>
set -eu
image=$1
dir=$(mktemp -d)
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$dir/admin-token"
chmod 0644 "$dir/admin-token"
cat > "$dir/jc.toml" <<'TOML'
role = "all"
listen = "0.0.0.0:8080"
admin_listen = "0.0.0.0:9091"
admin_token_file = "/run/secrets/jc/admin-token"
mode = "slug"
base_host = "localhost:8080"
scheme = "http"
state_dir = "/var/lib/jc"

[mounts.portal]
path_prefix = ""
TOML
chmod 0644 "$dir/jc.toml"
docker volume create jc-smoke-state >/dev/null
docker run -d --name jc-smoke -p 18080:8080 -p 19091:9091 \
  --read-only --tmpfs /tmp:rw,size=256m --cap-drop ALL --security-opt no-new-privileges \
  -v jc-smoke-state:/var/lib/jc -v "$dir/admin-token:/run/secrets/jc/admin-token:ro" \
  -v "$dir/jc.toml:/etc/jc/jc.toml:ro" \
  "$image" --config /etc/jc/jc.toml >/dev/null
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
# The gateway checked out shako's organization repository over shako's HTTP API (jc stops at start
# when that fails, bins/jc/src/modules/gateway.rs sync_forge): the tree lies in the state volume
# under gateway/org-cache, behind the link gateway/repo/current. The volume is mounted where jc
# mounts it, since jc may write the link with its absolute path.
busybox=docker.io/busybox:1.38@sha256:dc2d74b28e4cf8984fa52af1f39bc7c3d9c73760b41a74d629f5d11b1ab28616
docker run --rm -v jc-smoke-state:/var/lib/jc:ro "$busybox" sh -c \
  'c=/var/lib/jc/gateway/repo/current; test -L $c && ls -A $c/ | grep -q . && case $(readlink -f $c) in /var/lib/jc/gateway/org-cache/*) ;; *) exit 1 ;; esac' \
  || fail "the gateway's forge checkout is missing from the state dir"
echo "ok gateway checkout: $(docker run --rm -v jc-smoke-state:/var/lib/jc:ro "$busybox" ls -A /var/lib/jc/gateway/repo/current/ | tr '\n' ' ')"
if docker logs jc-smoke 2>&1 | grep -q 'forge checkout failed\|project checkout failed'; then fail "a gateway checkout failed"; fi
echo "SMOKE: ok"
