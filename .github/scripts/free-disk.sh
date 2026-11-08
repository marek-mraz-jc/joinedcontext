#!/bin/sh
# A hosted runner ships ~14 GB free; the Portal and jc test builds need more. Drop the SDKs no job
# here uses (as crates/portal's ci-full does before its --all-targets link).
set -eu
df -h / | tail -1
sudo rm -rf /usr/share/dotnet /usr/local/lib/android /opt/ghc /usr/local/.ghcup /opt/hostedtoolcache/CodeQL
df -h / | tail -1
