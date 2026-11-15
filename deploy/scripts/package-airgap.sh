#!/usr/bin/env bash
# Packages every pinned image and upstream chart into one archive to carry into an
# air-gapped zone (OPS-19, OPS-20, OPS-21).
#
#   scripts/package-airgap.sh [--out DIR] [--name NAME]
#
# Run it on a connected machine, from the root of this repository. It reads the pins that are
# already there — `components/*/images.yaml` and `components/*/charts.yaml` — so the archive
# and the deployment can never name different bytes, and writes:
#
#   <name>.tar.gz
#   ├── manifest.yaml   every image and chart in the archive, with the digest each was pulled at
#   ├── images/         one OCI layout per image, named by its digest
#   └── charts/         one .tgz per upstream chart, at its pinned version
#
# Images are copied with skopeo rather than saved with docker, because skopeo copies the
# manifest as it stands: the digest inside the zone is the digest in `images.yaml`, which is
# what keeps signature verification working after the mirror (OPS-20).
#
# The counterpart is scripts/load-airgap.sh.
set -euo pipefail

out="."
name="joinedcontext-airgap-$(date -u +%Y-%m-%d)"
while [ $# -gt 0 ]; do
	case "$1" in
	--out)
		out="${2:?--out needs a directory}"
		shift 2
		;;
	--name)
		name="${2:?--name needs a name}"
		shift 2
		;;
	*)
		echo "usage: $0 [--out DIR] [--name NAME]" >&2
		exit 64
		;;
	esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
for tool in skopeo helm python3 tar; do
	command -v "$tool" >/dev/null || {
		echo "$0: $tool is not installed" >&2
		exit 69
	}
done

mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/images" "$work/charts"

echo "reading the pins in $root/components"
python3 "$root/scripts/airgap-inventory.py" "$root" >"$work/manifest.yaml"

# One line per image: the reference to pull and the directory to pull it into. The directory
# is the digest with its colon replaced, so two tags of one repository never collide.
python3 -c '
import sys, yaml
for image in yaml.safe_load(open(sys.argv[1]))["images"]:
    print(image["reference"], image["digest"].replace(":", "_"))
' "$work/manifest.yaml" | while read -r reference directory; do
	echo "image  $reference"
	# --all copies every platform in the index, because the machine that carries the
	# archive in is rarely the architecture the cluster runs.
	skopeo copy --all "docker://$reference" "oci:$work/images/$directory:packaged"
done

python3 -c '
import sys, yaml
for chart in yaml.safe_load(open(sys.argv[1]))["charts"]:
    print(chart["repository"], chart["chart"], chart["version"])
' "$work/manifest.yaml" | while read -r repository chart version; do
	echo "chart  $chart $version"
	helm pull "$chart" --repo "$repository" --version "$version" --destination "$work/charts"
done

archive="$out/$name.tar.gz"
tar -C "$work" -czf "$archive" manifest.yaml images charts
echo "wrote $archive"
python3 -c '
import sys, yaml
inventory = yaml.safe_load(open(sys.argv[1]))
print(len(inventory["images"]), "images,", len(inventory["charts"]), "charts")
' "$work/manifest.yaml"
