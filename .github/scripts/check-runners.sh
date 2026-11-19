#!/bin/sh
# Every job names a fixed runner image (ubuntu-24.04), never a moving label (ubuntu-latest,
# windows-latest, macos-latest): ubuntu-latest became 26.04 (glibc 2.43) on 2026-11-19 and the jc
# binary image.yml built on it failed the trixie image smoke with "GLIBC_2.43 not found" (T-7287).
#   sh check-runners.sh [workflow dir]
#   sh check-runners.sh --selftest
set -eu

# moving <dir>: every runs-on line naming a *-latest label, as file:line: text
moving() { grep -rnE --include='*.yml' --include='*.yaml' '^[[:space:]]*runs-on:.*-latest' "$1" || true; }

check() { # dir
  bad=$(moving "$1")
  if [ -n "$bad" ]; then
    echo "$bad" | while IFS=: read -r f n text; do
      echo "::error file=$f,line=$n::runs-on names a moving runner label; pin an image such as ubuntu-24.04:$text"
    done
    return 1
  fi
  echo "runners: every runs-on in $1 names a fixed image"
}

if [ "${1:-}" = --selftest ]; then
  d=$(mktemp -d); trap 'rm -rf "$d"' EXIT
  printf 'jobs:\n  a:\n    runs-on: ubuntu-24.04\n  b:\n    runs-on: [self-hosted, linux]\n' > "$d/ok.yml"
  check "$d" >/dev/null || { echo "FAIL: fixed runners refused"; exit 1; }
  printf 'jobs:\n  c:\n    runs-on: ubuntu-latest\n' > "$d/bad.yml"
  ! check "$d" >/dev/null || { echo "FAIL: ubuntu-latest passed"; exit 1; }
  rm "$d/bad.yml"; printf 'jobs:\n  c:\n    runs-on:   macos-latest  # x\n' > "$d/bad.yaml"
  ! check "$d" >/dev/null || { echo "FAIL: macos-latest in a .yaml passed"; exit 1; }
  echo "check-runners.sh selftest ok"
  exit 0
fi
check "${1:-.github/workflows}"
