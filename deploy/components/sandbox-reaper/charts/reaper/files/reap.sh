#!/bin/sh
# Deletes sandbox namespaces whose age has passed their Time-To-Live (OPS-44, PF-19, CC-67).
#
# The contract is docs/Architecture/06-configuration-as-code.md section 4: a sandbox namespace
# carries `sandbox.joinedcontext.com/lifecycle` with the value `unmanaged` or `preview`, and
# may carry a shorter `sandbox.joinedcontext.com/ttl`. Age is `metadata.creationTimestamp`,
# the one timestamp nothing inside the namespace can rewrite.
#
# Everything this reads and every decision it makes is on stdout, because the record of what
# an unattended deletion loop removed is the only thing anyone has afterwards.
set -eu

LABEL="${LABEL:-sandbox.joinedcontext.com/lifecycle}"
TTL_ANNOTATION="${TTL_ANNOTATION:-sandbox.joinedcontext.com/ttl}"
# The ceiling of OPS-44 and PF-19. A `ttl` annotation may shorten a sandbox's life; it can
# never extend it past this, which is why the annotation is clamped rather than trusted.
CEILING_DAYS="${CEILING_DAYS:-14}"
DRY_RUN="${DRY_RUN:-false}"

ceiling=$((CEILING_DAYS * 24 * 60 * 60))
now="$(date -u +%s)"

# A Go duration as the annotation writes it, in seconds. Empty output means unreadable, and
# an unreadable duration falls back to the ceiling rather than to no limit at all.
ttl_seconds() {
	value="$1"
	number="${value%[smhd]}"
	case "$number" in
	'' | *[!0-9]*) return 0 ;;
	esac
	case "$value" in
	*s) echo "$number" ;;
	*m) echo $((number * 60)) ;;
	*h) echo $((number * 60 * 60)) ;;
	*d) echo $((number * 24 * 60 * 60)) ;;
	esac
}

# An RFC 3339 instant as seconds since the epoch. GNU date reads the format directly and
# busybox date needs the layout spelled out, so both forms are tried: the reaper runs on
# busybox in the cluster and on GNU date wherever its tests run.
rfc3339_epoch() {
	date -u -d "$1" +%s 2>/dev/null && return 0
	date -u -D '%Y-%m-%dT%H:%M:%SZ' -d "$1" +%s 2>/dev/null
}

# One line per candidate: name, creationTimestamp, and the ttl annotation or `-`. The label
# selector is what keeps a permanent namespace out of the list entirely: it is not filtered
# out further down, it is never fetched.
template='{{range .items}}{{.metadata.name}} {{.metadata.creationTimestamp}} {{if .metadata.annotations}}{{with index .metadata.annotations "TTL_ANNOTATION"}}{{.}}{{else}}-{{end}}{{else}}-{{end}}
{{end}}'
template="$(echo "$template" | sed "s|TTL_ANNOTATION|$TTL_ANNOTATION|")"

candidates="$(kubectl get namespaces \
	--selector "$LABEL in (unmanaged,preview)" \
	--output "go-template=$template")"

echo "$candidates" | while read -r name created ttl; do
	[ -n "$name" ] || continue

	if ! epoch="$(rfc3339_epoch "$created")"; then
		echo "keep   $name: creationTimestamp \"$created\" did not parse, so its age is unknown"
		continue
	fi
	age=$((now - epoch))

	limit="$ceiling"
	if [ "$ttl" != "-" ]; then
		seconds="$(ttl_seconds "$ttl")"
		if [ -z "$seconds" ] || [ "$seconds" -le 0 ]; then
			echo "warn   $name: ttl \"$ttl\" is not a duration this reads, the ${CEILING_DAYS} day ceiling applies"
		elif [ "$seconds" -lt "$ceiling" ]; then
			limit="$seconds"
		else
			echo "warn   $name: ttl \"$ttl\" is longer than the ${CEILING_DAYS} day ceiling and is clamped to it"
		fi
	fi

	if [ "$age" -le "$limit" ]; then
		echo "keep   $name: ${age}s old, limit ${limit}s"
		continue
	fi

	if [ "$DRY_RUN" = "true" ]; then
		echo "would  $name: ${age}s old, limit ${limit}s"
		continue
	fi

	# Foreground cascade: the namespace stays until the workloads, Secrets and
	# ServiceAccount tokens inside it are gone, which is what OPS-44 means by cascading
	# cleanly. `--wait=false` returns without blocking the run on the slowest namespace.
	echo "reap   $name: ${age}s old, limit ${limit}s"
	kubectl delete namespace "$name" --cascade=foreground --wait=false
done
