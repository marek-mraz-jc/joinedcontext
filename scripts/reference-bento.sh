#!/bin/sh
# Fetch the reference Bento (v1.22.0, the version deployment runs) into ./.bento/bento, checked by SHA-256.
# Every parity test compares ekiben's output with this binary's, byte for byte.
set -eu
V=1.22.0
SUM=42d124c5011e29d0c8a6dab20be9336721587da6b87209e1c5d4b23b7ee3b19e
mkdir -p .bento
[ -x .bento/bento ] && exit 0
curl -sSLo .bento/bento.tgz "https://github.com/warpstreamlabs/bento/releases/download/v$V/bento_${V}_linux_amd64.tar.gz"
echo "$SUM  .bento/bento.tgz" | sha256sum --check --status
tar xzf .bento/bento.tgz -C .bento bento && rm .bento/bento.tgz
.bento/bento --version | head -1
