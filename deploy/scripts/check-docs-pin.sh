#!/usr/bin/env bash
# The docs this release serves agree with the code it runs (T-4483, TS-19).
#
# usage: scripts/check-docs-pin.sh [--docs <docs tree>]
#
# The Portal's /docs pages are the docs commit `docs:` in components/portal/images.yaml (its
# public-docs/COMMIT, written by scripts/pin-portal.sh). This checks out the code the release
# pins, the Portal at `commit:` beside its digest, the platform at `commit:` beside the gateway's
# digest and this deployment tree, and runs the docs repository's own checkers of that docs
# commit against it: check-docs-against-code.py, generate-config-reference.py (check mode) and
# verify-openapi-snippets.py with the Portal's OpenAPI. A pinned commit whose image is not the
# pinned digest is refused first, so a pin cannot name code it does not run. `--docs` checks
# another docs tree against the same code (the tests' fixture).
set -euo pipefail
cd "$(dirname "$0")/.."
docs_tree=
case "${1:-}" in
  --docs) docs_tree=$(cd "${2:?usage: $0 [--docs <docs tree>]}" && pwd) ;;
  "") ;;
  *) echo "usage: $0 [--docs <docs tree>]" >&2; exit 2 ;;
esac

read -r portal portal_digest platform platform_digest docs < <(python3 - <<'EOF'
import yaml
portal = yaml.safe_load(open("components/portal/images.yaml"))["portal"]["portal"]
gateway = yaml.safe_load(open("components/context-gateway/images.yaml"))["context-gateway"]["gateway"]
print(portal.get("commit", "-"), portal["digest"], gateway.get("commit", "-"), gateway["digest"], portal.get("docs", "-"))
EOF
)
for pin in "$portal" "$platform" "$docs"; do
  [[ "$pin" =~ ^[0-9a-f]{40}$ ]] || { echo "a pinned commit is not 40 hex: $pin (portal commit, gateway commit, portal docs)" >&2; exit 1; }
done

# The registry's digest for the image tagged with `commit` (image.yml tags every build with it).
registry_digest() {
  local token
  token=$(curl -fsSL "https://ghcr.io/token?scope=repository:marek-mraz-jc/$1:pull" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')
  curl -fsSI -H "Authorization: Bearer $token" \
    -H "Accept: application/vnd.oci.image.index.v1+json,application/vnd.docker.distribution.manifest.list.v2+json,application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.v2+json" \
    "https://ghcr.io/v2/marek-mraz-jc/$1/manifests/$2" | tr -d '\r' | awk 'tolower($1) == "docker-content-digest:" { print $2 }'
}
# The commits our own image workflow on main signed `digest` from (cosign githubWorkflowSha), one
# per line; nothing when the image carries no such signature or cosign is absent.
built_from() {
  command -v cosign >/dev/null || return 0
  cosign verify "ghcr.io/marek-mraz-jc/$1@$2" \
    --certificate-identity-regexp "^https://github.com/marek-mraz-jc/$1/.github/workflows/.+@refs/heads/main$" \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com 2>/dev/null |
    python3 -c 'import json,sys; print("\n".join(sorted({s["optional"].get("githubWorkflowSha", "") for s in json.load(sys.stdin)})))' 2>/dev/null || true
}
red=0
for pair in "joinedcontext-portal $portal $portal_digest components/portal/images.yaml" \
            "joinedcontext-platform $platform $platform_digest components/context-gateway/images.yaml"; do
  read -r repo commit digest file <<<"$pair"
  found=$(registry_digest "$repo" "$commit" || true)
  # A scheduled rebuild of the same commit moves its tag to a new digest (the platform's ship
  # run, 2026-11-12): the pinned digest still runs that commit when our image workflow signed it
  # from that commit, which is what the tag stood for.
  if [ "$found" != "$digest" ] && built_from "$repo" "$digest" | grep -qx "$commit"; then
    found=$digest
  fi
  if [ "$found" != "$digest" ]; then
    echo "$file: $repo $commit is ${found:-not in the registry}, the pin says $digest: move commit: with the digest" >&2
    red=1
  fi
done
[ "$red" = 0 ] || exit 1

work=$(mktemp -d)
trap 'rm -rf "${work:?}"' EXIT
fetch() {
  mkdir -p "$3"
  curl -fsSL --retry 3 "https://codeload.github.com/marek-mraz-jc/$1/tar.gz/$2" | tar -xz -C "$3" --strip-components=1
  # an empty tree would make every claim "agree" with nothing
  [ -n "$(ls -A "$3")" ]
}
code=$work/code
fetch joinedcontext-portal "$portal" "$code/joinedcontext-portal"
fetch joinedcontext-platform "$platform" "$code/joinedcontext-platform"
mkdir -p "$code/joinedcontext-deployment"
git archive HEAD | tar -x -C "$code/joinedcontext-deployment"
if [ -z "$docs_tree" ]; then
  docs_tree=$work/docs
  fetch joinedcontext-docs "$docs" "$docs_tree"
fi

echo "docs ${docs:0:7} against portal ${portal:0:7}, platform ${platform:0:7}, deployment $(git rev-parse --short HEAD)"
cd "$docs_tree"
python3 scripts/check-docs-against-code.py --code "$code" . || red=1
JC_CODE_DIR=$code python3 scripts/generate-config-reference.py || red=1
OPENAPI_JSON=$code/joinedcontext-portal/ui/openapi.json python3 scripts/verify-openapi-snippets.py . || red=1
if [ "$red" != 0 ]; then
  echo "the pinned docs do not hold for the pinned code: pin a docs commit that does (portal scripts/public-docs.sh, then scripts/pin-portal.sh)" >&2
  exit 1
fi
echo "the pinned docs hold for the pinned code"
