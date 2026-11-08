#!/bin/sh
# Writes the jc pin (deploy-pin.yml) into the deployment repository: components/jc/images.yaml
# `jc.server` set to IMAGE@DIGEST of commit SHA, committed on pin/jc-<sha> (pin-proof/jc-<sha>
# for a ship-proof/** BRANCH) on top of DEPLOY_BASE, pushed, and for main a pull request opened.
#   SHA= DIGEST= IMAGE= BRANCH= DEPLOYMENT=<owner/repo> DEPLOY_BASE= PIN_TOKEN= sh deploy-pin.sh
#   sh deploy-pin.sh --selftest
set -eu

# pin <images.yaml> <repository> <digest> <commit>: the file with jc.server pinned
pin() {
  python3 - "$@" <<'PY'
import re, sys
path, repo, digest, commit = sys.argv[1:5]
text = open(path).read()
if not re.search(r"^jc:\s*$", text, re.M):
    sys.exit(f"{path}: no top-level jc: block")
block = (f"  # jc {commit}, signed by image.yml and scanned by image-scan.yml (deploy-pin.yml).\n"
         f"  server:\n    repository: '{repo}'\n    tag: 'main'\n    digest: '{digest}'\n")
# the old block with the comment lines right above it
text = re.sub(r"^(  #[^\n]*\n)*  server:\n(    [^\n]*\n)*", "", text, flags=re.M)
text = re.sub(r"^jc:\s*\n", lambda m: m.group(0) + block, text, count=1, flags=re.M)
open(path, "w").write(text)
PY
}

if [ "${1:-}" = --selftest ]; then
  t=$(mktemp -d)
  printf -- "---\n# head\njc:\n  seed:\n    repository: 'docker.io/busybox'\n    tag: '1.38'\n" > "$t/a.yaml"
  pin "$t/a.yaml" ghcr.io/o/jc "sha256:$(printf '%064d' 1)" abc
  pin "$t/a.yaml" ghcr.io/o/jc "sha256:$(printf '%064d' 2)" def
  [ "$(grep -c 'server:' "$t/a.yaml")" = 1 ] || { cat "$t/a.yaml"; echo "FAIL: one server block"; exit 1; }
  grep -q "digest: 'sha256:0*2'" "$t/a.yaml" || { echo "FAIL: newest digest"; exit 1; }
  if ! grep -q "# jc def," "$t/a.yaml" || grep -q "# jc abc," "$t/a.yaml"; then echo "FAIL: comment"; exit 1; fi
  grep -q "tag: '1.38'" "$t/a.yaml" || { echo "FAIL: seed kept"; exit 1; }
  python3 -c 'import sys, yaml; d = yaml.safe_load(open(sys.argv[1])); assert d["jc"]["server"]["digest"].endswith("2"), d' "$t/a.yaml"
  rm -rf "$t"
  echo "deploy-pin.sh selftest ok"
  exit 0
fi

printf '%s' "$SHA" | grep -qxE '[0-9a-f]{40}' || { echo "::error::'$SHA' is no commit"; exit 1; }
printf '%s' "$DIGEST" | grep -qxE 'sha256:[0-9a-f]{64}' || { echo "::error::'$DIGEST' is no digest"; exit 1; }
short=$(printf '%s' "$SHA" | cut -c1-12)
case "$BRANCH" in
  main) pin_branch="pin/jc-$short" ;;
  ship-proof/*) pin_branch="pin-proof/jc-$short" ;;
  *) echo "::error::no pin for branch $BRANCH"; exit 1 ;;
esac
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
if [ -z "${PIN_TOKEN:-}" ]; then
  {
    echo "### jc $short is signed and scanned, but not pinned"
    echo "The secret DEPLOY_PIN_TOKEN is not set. Pin by hand in $DEPLOYMENT components/jc/images.yaml:"
    # shellcheck disable=SC2016
    printf '```yaml\njc:\n  server:\n    repository: %s\n    tag: main\n    digest: %s\n```\n' "'$IMAGE'" "'$DIGEST'"
  } >> "$summary"
  echo "::error::DEPLOY_PIN_TOKEN is not set: $IMAGE@$DIGEST is signed and scanned but not pinned (the run summary holds the pin)"
  exit 1
fi
auth="AUTHORIZATION: basic $(printf 'x-access-token:%s' "$PIN_TOKEN" | base64 -w0)"
work=$(mktemp -d)
git -c http.extraheader="$auth" clone -q --depth 1 --branch "$DEPLOY_BASE" "https://github.com/$DEPLOYMENT.git" "$work"
cd "$work"
[ -f components/jc/images.yaml ] || { echo "::error::$DEPLOYMENT $DEPLOY_BASE has no components/jc/images.yaml"; exit 1; }
pin components/jc/images.yaml "$IMAGE" "$DIGEST" "$SHA"
if git diff --quiet; then
  echo "::notice::$DEPLOYMENT $DEPLOY_BASE already pins $DIGEST"
  exit 0
fi
git config user.name "deploy-pin"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git checkout -q -b "$pin_branch"
git commit -q -m "Pin jc $short ($DIGEST) for dev: signed by the monorepo's image.yml, scanned by image-scan.yml" \
  -m "ci: ${CI_URL:-}" -m "pin: ${RUN_URL:-}" -- components/jc/images.yaml
git -c http.extraheader="$auth" push -q origin "HEAD:refs/heads/$pin_branch"
echo "pushed $DEPLOYMENT $pin_branch" | tee -a "$summary"
if [ "$BRANCH" = main ]; then
  url=$(GH_TOKEN="$PIN_TOKEN" gh pr create -R "$DEPLOYMENT" --base "$DEPLOY_BASE" --head "$pin_branch" \
    --title "Pin jc $short for dev" \
    --body "Digest \`$DIGEST\` of ghcr.io jc \`$SHA\`: smoke-tested, scanned (HIGH/CRITICAL with a fix: none), signed. dev only; prod stays a manual pin. ci: ${CI_URL:-} · pin: ${RUN_URL:-}")
  echo "opened $url" | tee -a "$summary"
fi
