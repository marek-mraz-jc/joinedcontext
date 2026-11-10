#!/bin/sh
# shellcheck disable=SC2016 # "$ctx" is the literal text of image.yml, not a variable here
# Every path bins/jc/Dockerfile copies from the `build` stage must be one image.yml's `bin` step
# writes into the named context it hands in as `build` (the context replaces the whole stage, so a
# path only the Dockerfile's own build stage makes is "not found" in ship: main ci 42671128154).
# Checks the first path element of each `COPY --from=build <src>` against the `"$ctx/<name>"`
# paths of image.yml.
#   sh check-build-context.sh [Dockerfile image.yml]
#   sh check-build-context.sh --selftest
set -eu

# srcs <Dockerfile>: the first path element of every COPY --from=build source, one per line
srcs() {
  awk '$1 == "COPY" { from = ""; n = 0
      for (i = 2; i <= NF; i++) {
        if ($i ~ /^--from=/) { from = substr($i, 8); continue }
        if ($i ~ /^--/) continue
        a[++n] = $i
      }
      if (from == "build") for (i = 1; i < n; i++) { p = a[i]; sub(/^\//, "", p); sub(/\/.*/, "", p); print p }
    }' "$1" | sort -u
}

# made <image.yml>: the names the bin step writes under "$ctx/"
made() { grep -o '"\$ctx/[^/" ]*' "$1" | sed 's|"\$ctx/||' | sort -u; }

check() { # Dockerfile image.yml
  missing=$(srcs "$1" | while read -r p; do made "$2" | grep -qx "$p" || echo "$p"; done)
  if [ -n "$missing" ]; then
    echo "::error file=$1::COPY --from=build of a path image.yml's bin step does not put in the build context: $(echo "$missing" | tr '\n' ' ')"
    return 1
  fi
  echo "build context: $(srcs "$1" | tr '\n' ' ')all made by $2"
}

if [ "${1:-}" = --selftest ]; then
  d=$(mktemp -d); trap 'rm -rf "$d"' EXIT
  printf 'COPY --from=build /jc /usr/local/bin/jc\nCOPY --from=build --chown=1:1 /state /var/lib/jc\nCOPY --from=rust /opt/x /opt/x\nCOPY . .\n' > "$d/Dockerfile"
  printf '          strip -o "$ctx/jc" x\n          mkdir -p "$ctx/state"\n' > "$d/image.yml"
  check "$d/Dockerfile" "$d/image.yml" >/dev/null || { echo "FAIL: good context refused"; exit 1; }
  echo 'COPY --from=build /opt/e5-small /opt/e5-small' >> "$d/Dockerfile"
  if check "$d/Dockerfile" "$d/image.yml" >/dev/null; then echo "FAIL: /opt from build passed"; exit 1; fi
  echo "check-build-context selftest: ok"
  exit 0
fi

check "${1:-bins/jc/Dockerfile}" "${2:-.github/workflows/image.yml}"
