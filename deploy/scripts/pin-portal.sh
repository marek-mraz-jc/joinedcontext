#!/usr/bin/env bash
# Pin the portal, its UI bundle and both app-builder images to one portal commit (T-2633, AP-82,
# AP-106, T-5297). The binary and the bundle each come from the newest commit at or before it that
# built them (scripts/portal-ui-pin.sh, T-6158); `commit:` in the portal block names the binary's.
#
# usage: scripts/pin-portal.sh <portal commit, 40 hex>
#
# The portal's image.yml tags each image it built with the commit. The builder image has to match
# the workflows the seeded applications carry (components/gitea/apps/*/.gitea/workflows), so the
# script reads lane.mjs and build-app out of it and refuses to pin a builder that cannot run them.
set -euo pipefail

commit=${1:-}
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || { echo "usage: $0 <portal commit, 40 hex>" >&2; exit 2; }
cd "$(dirname "$0")/.."
registry=ghcr.io/marek-mraz-jc

digest() {
  # A missing image is an empty digest, refused below with words, never a bare exit of set -e.
  { docker buildx imagetools inspect "$registry/$1:$commit" 2>/dev/null || true; } | awk '/^Digest:/ { print $2; exit }'
}
# The builder images' own pins, kept when the commit did not rebuild them.
pinned() {
  awk -v repo="$registry/$1" '$0 ~ "repository: .*" repo "\x27" { mine = 1 } mine && /digest:/ { gsub(/\x27/, "", $2); print $2; exit }' components/gitea-runner/images.yaml
}
# The binary of the same code (T-6158): this commit's, or the pinned or newest one built since when
# only ui/ changed after it, so a ui/-only commit moves the bundle alone.
read -r portal_commit portal < <(scripts/portal-ui-pin.sh --binary "$commit") || true
[[ "$portal_commit" =~ ^[0-9a-f]{40}$ && "$portal" =~ ^sha256:[0-9a-f]{64}$ ]] ||
  { echo "no portal image is published for $commit or fits it" >&2; exit 1; }
# image.yml builds only what a push's paths need (T-2485): a commit that changed no builder
# publishes no builder, and the builders pinned now are still the ones the workflows run.
builder=$(digest joinedcontext-app-builder)
rust=$(digest joinedcontext-app-builder-rust)
[[ -n "$builder" ]] || builder=$(pinned joinedcontext-app-builder)
[[ -n "$rust" ]] || rust=$(pinned joinedcontext-app-builder-rust)
for d in "$builder" "$rust"; do
  [[ "$d" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "no builder image is published for $commit and none is pinned" >&2; exit 1; }
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
docker pull -q "$registry/joinedcontext-app-builder@$builder" >/dev/null
docker run --rm --entrypoint cat "$registry/joinedcontext-app-builder@$builder" /opt/template/lane.mjs >"$work/lane.mjs"
docker run --rm --entrypoint cat "$registry/joinedcontext-app-builder@$builder" /usr/local/bin/build-app >"$work/build-app"
python3 scripts/check-builder-contract.py "$work/lane.mjs" "$work/build-app" components/gitea/apps/*/.gitea/workflows/build.yml

# The UI bundle of the same files (T-5297, T-6050): this commit's, or the pinned or newest one built
# since, when ui/ and sdk/ did not change after it.
read -r ui_commit ui < <(scripts/portal-ui-pin.sh "$commit") || true
[[ "$ui_commit" =~ ^[0-9a-f]{40}$ && "$ui" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "no portal-ui bundle matches $commit" >&2; exit 1; }

# The docs commit the image's /docs pages are, beside the digest (T-4483).
docs=$(curl -fsSL "https://raw.githubusercontent.com/marek-mraz-jc/joinedcontext-portal/$portal_commit/public-docs/COMMIT")
[[ "$docs" =~ ^[0-9a-f]{40}$ ]] || { echo "portal $portal_commit names no docs commit in public-docs/COMMIT" >&2; exit 1; }

# The platform commit the image's jc-core is built from (the Cargo.toml rev): the seed is held to
# it (tests/test_portal_pin_reads_the_seed.py), so it moves with the portal.
platform=$(curl -fsSL "https://raw.githubusercontent.com/marek-mraz-jc/joinedcontext-portal/$portal_commit/Cargo.toml" |
  sed -nE 's/^jc-core = .*joinedcontext-platform\.git", rev = "([0-9a-f]{40})".*/\1/p' | head -1)
[[ "$platform" =~ ^[0-9a-f]{40}$ ]] || { echo "portal $portal_commit builds jc-core from no platform rev in Cargo.toml" >&2; exit 1; }

short=${commit:0:7}
python3 - "$short" "$portal" "$builder" "$rust" "$portal_commit" "$docs" "$ui" "$ui_commit" "$platform" <<'EOF'
import re
import sys

short, portal, builder, rust, commit, docs, ui, ui_commit, platform = sys.argv[1:]

def pin(path, pins):
    text = open(path).read()
    for repository, digest in pins:
        text, n = re.subn(
            rf"(repository: '[^']*/{repository}'\n(?:    .*\n)*?    digest: )'sha256:[0-9a-f]{{64}}'",
            rf"\g<1>'{digest}'", text)
        if n != 1:
            sys.exit(f"{path}: no single pin for {repository}")
    open(path, "w").write(text)

pin("components/portal/images.yaml", [("joinedcontext-portal", portal), ("joinedcontext-portal-ui", ui)])
text = open("components/portal/images.yaml").read()
for key, value in (("commit", commit), ("docs", docs), ("platform", platform)):
    text, n = re.subn(rf"(\n    {key}: )'[0-9a-f]{{40}}'", rf"\g<1>'{value}'", text, count=1)
    if n != 1:
        sys.exit(f"components/portal/images.yaml: no {key} pin")
text, n = re.subn(r"(\n  ui:\n(?:    .*\n)*?    commit: )'[0-9a-f]{40}'", rf"\g<1>'{ui_commit}'", text)
if n != 1:
    sys.exit("components/portal/images.yaml: no ui commit pin")
open("components/portal/images.yaml", "w").write(text)
before = open("components/gitea-runner/images.yaml").read()
pin("components/gitea-runner/images.yaml",
    [("joinedcontext-app-builder", builder), ("joinedcontext-app-builder-rust", rust)])
text = open("components/gitea-runner/images.yaml").read()
# The comment names the portal commit the builders are of: it moves only with them.
if text != before:
    open("components/gitea-runner/images.yaml", "w").write(re.sub(r"Pinned to portal [0-9a-f]{7}", f"Pinned to portal {short}", text))
EOF
echo "pinned portal $short: portal ${portal_commit:0:7} $portal, ui ${ui_commit:0:7} $ui, docs ${docs:0:7}, builder $builder, rust $rust"
scripts/check-docs-pin.sh
