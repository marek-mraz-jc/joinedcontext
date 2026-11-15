#!/usr/bin/env bash
# Loads an air-gap archive into a registry inside the zone (OPS-19, OPS-20, OPS-21).
#
#   scripts/load-airgap.sh --archive joinedcontext-airgap-<date>.tar.gz \
#                          --registry registry.internal:5000 \
#                          [--charts-dir deployment/airgap-charts] [--tls-verify=false]
#
# Every byte it writes comes out of the archive and every byte it sends goes to --registry.
# It resolves no other host: there is no fallback to a public registry, no chart repository
# to add, no update check. That is the property OPS-21 asks for, and the test asserts it by
# recording every destination the script ever names.
#
# Images keep the digest they were packaged with, so the digests in `components/*/images.yaml`
# still identify the same bytes and signature verification still applies. Point
# `global.imageRegistry` at --registry and the installation proceeds as written.
set -euo pipefail

archive=""
registry=""
charts_dir="deployment/airgap-charts"
tls=()
while [ $# -gt 0 ]; do
	case "$1" in
	--archive)
		archive="${2:?--archive needs a file}"
		shift 2
		;;
	--registry)
		registry="${2:?--registry needs a host}"
		shift 2
		;;
	--charts-dir)
		charts_dir="${2:?--charts-dir needs a directory}"
		shift 2
		;;
	--tls-verify=false)
		# A registry inside a zone is often served with a certificate no public root
		# signed. This weakens the transport to that one registry and nothing else.
		tls=(--dest-tls-verify=false)
		shift
		;;
	*)
		echo "usage: $0 --archive FILE --registry HOST [--charts-dir DIR] [--tls-verify=false]" >&2
		exit 64
		;;
	esac
done
[ -n "$archive" ] && [ -n "$registry" ] || {
	echo "usage: $0 --archive FILE --registry HOST [--charts-dir DIR] [--tls-verify=false]" >&2
	exit 64
}
[ -f "$archive" ] || {
	echo "$0: $archive is not a file" >&2
	exit 66
}
case "$registry" in
*://*)
	echo "$0: --registry is a host, not a URL: drop the scheme from \"$registry\"" >&2
	exit 64
	;;
esac

for tool in skopeo python3 tar; do
	command -v "$tool" >/dev/null || {
		echo "$0: $tool is not installed" >&2
		exit 69
	}
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tar -C "$work" -xzf "$archive"
[ -f "$work/manifest.yaml" ] || {
	echo "$0: $archive carries no manifest.yaml, so there is no way to know what is in it" >&2
	exit 65
}

# One line per image: the directory it was packaged into, and the repository path to push it
# to under the target registry. The path is the source repository without its registry host,
# so `docker.io/busybox` and `alpine/kubectl` both land where the values files expect them.
python3 -c '
import sys, yaml
for image in yaml.safe_load(open(sys.argv[1]))["images"]:
    host, _, rest = image["repository"].partition("/")
    path = rest if ("." in host or ":" in host) and rest else image["repository"]
    print(image["digest"].replace(":", "_"), path, image["tag"] or "packaged")
' "$work/manifest.yaml" | while read -r directory path tag; do
	[ -d "$work/images/$directory" ] || {
		echo "$0: the archive names $path but carries no $directory" >&2
		exit 65
	}
	echo "image  $path:$tag"
	# A push names a tag; the digest comes out the same because the manifest is copied
	# rather than rebuilt, which is what keeps the pins in images.yaml valid.
	skopeo copy --all ${tls[@]+"${tls[@]}"} \
		"oci:$work/images/$directory:packaged" "docker://$registry/$path:$tag"
done

mkdir -p "$charts_dir"
if [ -d "$work/charts" ]; then
	cp "$work/charts"/*.tgz "$charts_dir"/ 2>/dev/null || true
	echo "charts unpacked into $charts_dir"
fi

# The values override that points the deployment at the registry the images now live in.
# Every environment already reads `images.<component>.<part>.repository`, so this needs no
# new setting anywhere: the operator copies this file into their environment directory and
# the six installation steps proceed unchanged.
overrides="$charts_dir/airgap-images.yaml.gotmpl"
python3 -c '
import sys, yaml
registry = sys.argv[2]
images = {}
for image in yaml.safe_load(open(sys.argv[1]))["images"]:
    host, _, rest = image["repository"].partition("/")
    path = rest if ("." in host or ":" in host) and rest else image["repository"]
    part = {"repository": f"{registry}/{path}", "digest": image["digest"]}
    if image["tag"]:
        part["tag"] = str(image["tag"])
    images.setdefault(image["component"], {})[image["part"]] = part
header = (
    "---\n"
    "# Written by scripts/load-airgap.sh. Every image now resolves inside the zone, at the\n"
    f"# digest it was packaged with, so nothing here reaches a public registry (OPS-19, OPS-21).\n"
)
with open(sys.argv[3], "w") as out:
    out.write(header)
    yaml.safe_dump({"images": images}, out, default_flow_style=False, sort_keys=True)
' "$work/manifest.yaml" "$registry" "$overrides"

echo "loaded into $registry"
echo "copy $overrides into deployment/environments/<env>/ and point helmfile at $charts_dir"
