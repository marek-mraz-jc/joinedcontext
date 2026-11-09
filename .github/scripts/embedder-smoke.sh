#!/bin/sh
# Starts the jc-embedder image as the cluster runs it (nonroot, read-only root filesystem, every
# capability dropped) with one example deployment, then checks that /healthz answers, that the
# model loaded (the log names its SHA-256), and that an anonymous embedding call is refused 401.
#   sh embedder-smoke.sh <image>
set -eu
image=$1
dir=$(mktemp -d)
cat > "$dir/embedder.toml" <<'TOML'
listen_addr = "0.0.0.0:8089"
model_path = "/models/e5-small"
threads = 1
[[deployments]]
name = "smoke"
issuer = "https://id.smoke.invalid/realms/jc"
jwks_url = "https://id.smoke.invalid/realms/jc/protocol/openid-connect/certs"
clients = ["jc-assistant"]
max_texts_per_minute = 60
TOML
chmod 0644 "$dir/embedder.toml"
docker run -d --name embedder-smoke -p 18089:8089 \
  --read-only --tmpfs /tmp:rw,size=16m --cap-drop ALL --security-opt no-new-privileges \
  -v "$dir/embedder.toml:/etc/jc-embedder/embedder.toml:ro" "$image" >/dev/null
cleanup() { docker rm -f embedder-smoke >/dev/null 2>&1 || true; rm -rf "$dir"; }
trap cleanup EXIT
fail() { echo "SMOKE: $1"; docker logs embedder-smoke 2>&1 | tail -40; exit 1; }
for _ in $(seq 60); do curl -sf localhost:18089/healthz >/dev/null && break; sleep 1; done
curl -sf localhost:18089/healthz >/dev/null || fail "no /healthz after 60 s"
[ "$(docker inspect -f '{{.Config.User}}' "$image")" = nonroot:nonroot ] || fail "the image does not run as nonroot"
docker logs embedder-smoke 2>&1 | grep -q '"sha256":"dd476dd0' || fail "the model did not load"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' \
  -d '{"model":"multilingual-e5-small-int8","kind":"query","input":["q"]}' localhost:18089/v1/embeddings)
[ "$code" = 401 ] || fail "an anonymous call answered $code, not 401"
echo "SMOKE: jc-embedder ok"
