#!/usr/bin/env bash
# Rotate one in-cluster credential without downtime, then MEASURE that the old one is refused
# (OPS-45, T-1719; Operations/01 Runbook 5).
#
#   scripts/rotate-secret.sh --instance dev --secret keycloak-client-activity-ingest \
#       --idm https://idm.<domain> --realm <realm>
#   scripts/rotate-secret.sh --instance dev --secret db-portal
#   scripts/rotate-secret.sh --instance dev --secret gitea-token-portal --forge https://<domain>/git
#   scripts/rotate-secret.sh --instance dev --secret portal-cookie-key
#   scripts/rotate-secret.sh --instance dev --secret portal-cookie-key --drop-previous
#   scripts/rotate-secret.sh --instance dev --secret ckan-api-token --catalogue https://opendata.<domain>
#   scripts/rotate-secret.sh --instance dev --secret agent-runner-model-key --from-file <file> \
#       --model-base https://openrouter.ai/api/v1
#   scripts/rotate-secret.sh ... --dry-run     # prints the plan, changes nothing (T-5106)
#
# The generator writes a Secret only where no copy exists and copies the first one it finds to
# every namespace, so deleting one copy changes nothing. The script therefore:
#
#   1. reads the old value (never printed; only its length and whether it changed),
#   2. deletes EVERY copy of the Secret,
#   3. applies the release that makes it again: the Secret's own generator release, or for a
#      forge token the forge bootstrap, which deletes the old token in Gitea and mints one,
#   4. applies the release that pushes it to the other side: keycloak-config-cli for a client
#      secret or a demo person's password; CloudNativePG applies a role password itself,
#   5. restarts every workload in those namespaces whose pods read the Secret, and waits,
#   6. measures: the old value is refused and the new one accepted, within --bound seconds.
#
# The Portal's session key is read on one side only, so there is nothing to measure, but every
# signed-in person's cookie is sealed with it. Its rotation keeps the old value under the
# Secret's `previous` key, which the Portal reads as JC_PORTAL_COOKIE_KEY_PREVIOUS and still
# opens cookies with; nobody is signed out. Once a session has outlived the window,
# `--drop-previous` removes it and restarts the readers (T-2842, Deployment/08). The next
# rotation replaces it, so at most one retired key is ever held.
#
# The catalogue's API token is minted by CKAN, not generated: its own Job revokes the token by
# name and mints one when the Secret is gone, so the run deletes the Secret and applies the
# catalogue release the Job's ServiceAccount belongs to. The old token is refused from that
# moment; the Portal's CKAN writes fail until its restart a few seconds later and the next
# reconcile retries them. Catalogue reads never use the token.
#
# The model key comes from the provider: the owner creates a new key in the provider's account
# and hands it over in a file (--from-file, read once, never echoed). The run writes it into
# every copy, restarts the readers and measures the new key at the provider's GET /key. The old
# key keeps working until the owner deletes it in the provider's account; the run says so.
#
# Other credentials the platform does not generate are refused and named with who rotates them:
# the Gitea and Keycloak administrators are written once at install (initialOnlyNoReset), and
# operator-supplied Secrets (SMTP, backup S3) belong to Runbook 5's table.
set -uo pipefail

instance=""
secret=""
idm=""
realm=""
forge=""
bound=60
drop_previous=0
dry_run=0
catalogue=""
from_file=""
model_base=""
helmfile_file="deployment/helmfile.yaml"

die() { printf 'rotate-secret: %s\n' "$1" >&2; exit 2; }
say() { printf 'rotate-secret: %s\n' "$1"; }

while [ $# -gt 0 ]; do
	case "$1" in
	--instance) instance="${2:?}"; shift 2 ;;
	--secret) secret="${2:?}"; shift 2 ;;
	--idm) idm="${2:?}"; shift 2 ;;
	--realm) realm="${2:?}"; shift 2 ;;
	--forge) forge="${2:?}"; shift 2 ;;
	--bound) bound="${2:?}"; shift 2 ;;
	--drop-previous) drop_previous=1; shift ;;
	--dry-run) dry_run=1; shift ;;
	--catalogue) catalogue="${2:?}"; shift 2 ;;
	--from-file) from_file="${2:?}"; shift 2 ;;
	--model-base) model_base="${2:?}"; shift 2 ;;
	-h | --help) sed -n '2,50p' "$0"; exit 0 ;;
	*) die "unknown argument: $1" ;;
	esac
done

[ -n "$instance" ] || die "--instance is required"
[ -n "$secret" ] || die "--secret is required"
case "$bound" in '' | *[!0-9]*) die "--bound is a number of seconds" ;; esac

# The class decides which key holds the value, what makes it again and how the old one is tested.
case "$secret" in
gitea-admin-credentials | keycloak-admin-user)
	die "$secret is written once at install and not reset by an apply; rotate it by Runbook 5 (the administrator's own password change), not here" ;;
agent-runner-model-key)
	class=external key=key
	[ -n "$from_file" ] || die "the model key comes from the provider: create a new key there and pass it with --from-file (Runbook 5)"
	[ -n "$model_base" ] || die "the model key is measured at the provider: pass --model-base (https://openrouter.ai/api/v1)" ;;
keycloak-smtp | postgres-backup-s3 | github-app-mirror | portal-age-identity)
	die "$secret is supplied by the operator; rotate it at its source and re-create it by Runbook 5" ;;
ckan-api-token | ckan-api-token-*)
	class=catalogue key=token
	[ -n "$catalogue" ] || die "a catalogue token is measured at the catalogue's action API: pass --catalogue https://opendata.<domain>" ;;
keycloak-client-*)
	class=client key=client-secret
	[ -n "$idm" ] && [ -n "$realm" ] || die "a client secret is measured at the token endpoint: pass --idm and --realm" ;;
db-*) class=database key=password ;;
keycloak-user-*) class=generic key="" push_config=1 ;;
gitea-token-portal | gitea-token-gateway | gitea-token-lane-secret)
	class=forge key=token
	[ -n "$forge" ] || die "a forge token is measured against the forge: pass --forge" ;;
portal-cookie-key) class=generic key=key keeps_previous=1 ;;
*) class=generic key="" ;;
esac
[ "$class" = client ] && push_config=1
[ -z "$from_file" ] || [ "$class" = external ] || die "--from-file is for agent-runner-model-key"
[ "$drop_previous" = 0 ] || [ "${keeps_previous:-0}" = 1 ] || die "$secret keeps no previous value; --drop-previous is for portal-cookie-key"
if [ "$class" = external ]; then
	if [ ! -f "$from_file" ] || [ ! -r "$from_file" ]; then die "cannot read $from_file"; fi
	# One line; a trailing newline from an editor is not part of the key.
	supplied=$(head -c 4096 "$from_file" | tr -d '\r\n')
	[ -n "$supplied" ] || die "$from_file is empty"
	case "$supplied" in *[[:space:]]*) die "$from_file holds more than one key or whitespace inside the key" ;; esac
fi

namespaces=$(kubectl get secret -A --field-selector "metadata.name=$secret" \
	-o 'jsonpath={range .items[*]}{.metadata.namespace}{"\n"}{end}' 2>/dev/null) || die "cannot list Secrets named $secret"
[ -n "$namespaces" ] || die "no Secret named $secret in the cluster"
first=$(printf '%s\n' "$namespaces" | head -n1)

# Only what an apply makes again is deleted: a generated Secret carries its Helm release. The
# catalogue token is written by a hook Job, whose ServiceAccount of the same name carries it.
release=$(kubectl -n "$first" get secret "$secret" -o 'jsonpath={.metadata.annotations.meta\.helm\.sh/release-name}' 2>/dev/null)
if [ "$class" = forge ]; then
	release="gitea-bootstrap"
elif [ "$class" = catalogue ]; then
	release=$(kubectl -n "$first" get serviceaccount "$secret" -o 'jsonpath={.metadata.annotations.meta\.helm\.sh/release-name}' 2>/dev/null)
	[ -n "$release" ] || die "no catalogue release owns the token Job's ServiceAccount $first/$secret; nothing would mint a new token"
elif [ "$class" = external ]; then
	release=""
elif [ -z "$release" ]; then
	die "$secret in $first carries no Helm release, so no apply would make it again; it is not the platform's to rotate"
fi

# Every workload whose pod template names the Secret: env, envFrom, a volume or a pull secret,
# one `<namespace> <kind>/<name>` per line.
readers() {
	for ns in $namespaces; do
		workloads=$(kubectl -n "$ns" get deployment,statefulset,daemonset -o json 2>/dev/null | SECRET="$secret" python3 -c '
import json, os, sys
name = os.environ["SECRET"]
for item in json.load(sys.stdin).get("items", []):
    spec = item["spec"]["template"]["spec"]
    refs = [p.get("name") for p in spec.get("imagePullSecrets", [])]
    refs += [v.get("secret", {}).get("secretName") for v in spec.get("volumes", [])]
    for c in spec.get("containers", []) + spec.get("initContainers", []):
        refs += [e.get("valueFrom", {}).get("secretKeyRef", {}).get("name") for e in c.get("env", [])]
        refs += [e.get("secretRef", {}).get("name") for e in c.get("envFrom", [])]
    if name in refs:
        print(item["kind"].lower() + "/" + item["metadata"]["name"])
') || die "cannot list the workloads of $ns"
		for workload in $workloads; do printf '%s %s\n' "$ns" "$workload"; done
	done
}

restart_readers() {
	restarted=0
	list=$(readers) || exit 2
	while read -r ns workload; do
		[ -n "$workload" ] || continue
		kubectl -n "$ns" rollout restart "$workload" >/dev/null || die "restarting $workload in $ns failed"
		kubectl -n "$ns" rollout status "$workload" --timeout=300s >/dev/null || die "$workload in $ns did not come back"
		say "restarted $ns/$workload"
		restarted=$((restarted + 1))
	done <<<"$list"
	say "$restarted workload(s) restarted"
}

# The plan, from the same reads the run makes; nothing is deleted, applied, patched or restarted.
if [ "$dry_run" = 1 ]; then
	list=$(readers) || exit 2
	say "dry run: $secret ($class) in: $(printf '%s ' $namespaces)"
	if [ "$drop_previous" = 1 ]; then
		say "would remove the previous value from every copy"
	elif [ "$class" = external ]; then
		say "would write the key from $from_file into every copy and measure it at $model_base/key"
	else
		say "would delete every copy and apply release ${release}"
		[ "${push_config:-0}" = 1 ] && say "would apply release keycloak-config to push the new value"
		[ "${keeps_previous:-0}" = 1 ] && say "would keep the old value as previous"
	fi
	while read -r ns workload; do
		[ -n "$workload" ] && say "would restart $ns/$workload"
	done <<<"$list"
	case "$class" in
	client) say "would measure the old value refused at $idm/realms/$realm" ;;
	database) say "would measure the old password refused on the CloudNativePG primary" ;;
	forge) say "would measure the old token refused at $forge/api/v1/user" ;;
	catalogue) say "would measure the old token refused at $catalogue/api/3/action/api_token_list" ;;
	esac
	say "dry run: nothing changed"
	exit 0
fi

value_of() { # <namespace> -> the decoded value of $key (the whole encoded data without one), on stdout only
	if [ -z "$key" ]; then
		kubectl -n "$1" get secret "$secret" -o 'jsonpath={.data}' 2>/dev/null
		return
	fi
	kubectl -n "$1" get secret "$secret" -o "jsonpath={.data.${key//./\\.}}" 2>/dev/null | base64 -d 2>/dev/null
}

if [ "$drop_previous" = 1 ]; then
	dropped=0
	for ns in $namespaces; do
		[ -n "$(kubectl -n "$ns" get secret "$secret" -o 'jsonpath={.data.previous}' 2>/dev/null)" ] || continue
		kubectl -n "$ns" patch secret "$secret" --type json -p '[{"op":"remove","path":"/data/previous"}]' >/dev/null ||
			die "removing the previous value of $secret in $ns failed"
		dropped=$((dropped + 1))
	done
	[ "$dropped" -gt 0 ] || die "$secret holds no previous value in any copy: no rotation is open"
	restart_readers
	say "done: the previous value of $secret is dropped and every reader restarted; a cookie only it opened now asks for a login"
	exit 0
fi

old=$(value_of "$first")
[ -n "$old" ] || die "$secret in $first holds no ${key:-data}"
say "rotating $secret ($class) in: $(printf '%s ' $namespaces)"

model_status() { # <key on stdin> -> HTTP status of the provider's GET /key with it
	sed 's/^/header = "Authorization: Bearer /; s/$/"/' | curl -sS --max-time 10 -o /dev/null -w '%{http_code}' \
		-K - "$model_base/key" 2>/dev/null || echo curl-error
}

# The supplied key: measured before anything changes, so a mistyped key never replaces a working one.
if [ "$class" = external ]; then
	[ "$supplied" != "$old" ] || die "$from_file holds the key already in use"
	status=$(printf '%s\n' "$supplied" | model_status)
	[ "$status" = 200 ] || die "the provider answers $status to the supplied key; nothing changed"
	encoded=$(printf '%s' "$supplied" | base64 | tr -d '\n')
	for ns in $namespaces; do
		printf '{"data":{"%s":"%s"}}' "$key" "$encoded" |
			kubectl -n "$ns" patch secret "$secret" --type merge --patch-file /dev/stdin >/dev/null ||
			die "writing the new key into $secret in $ns failed"
	done
	for ns in $namespaces; do
		[ "$(value_of "$ns")" = "$supplied" ] || die "the copy in $ns does not hold the new key"
	done
	say "new value written in every copy"
	restart_readers
	if [ "$(printf '%s\n' "$old" | model_status)" = 200 ]; then
		say "the new key is accepted; the old key still answers 200: delete it in the provider's account now"
	else
		say "the new key is accepted and the old key is refused; $secret rotated"
	fi
	exit 0
fi

# Each copy as it stands, held in memory (never on disk or a command line) until the new value
# is in place: a failed apply puts the old Secret back instead of leaving its readers with none
# at their next restart (F-R2DEP3-010).
declare -A saved
for ns in $namespaces; do
	saved[$ns]=$(kubectl -n "$ns" get secret "$secret" -o json | python3 -c 'import json, sys
s = json.load(sys.stdin)
m = s["metadata"]
s["metadata"] = {k: m[k] for k in ("name", "namespace", "labels", "annotations") if k in m}
print(json.dumps({k: s[k] for k in ("apiVersion", "kind", "metadata", "type", "data") if k in s}))') ||
		die "reading $secret in $ns failed; nothing was deleted"
done
restore_and_die() {
	local ns
	for ns in $namespaces; do
		kubectl -n "$ns" delete secret "$secret" --ignore-not-found --wait=true >/dev/null
		kubectl -n "$ns" create -f - <<<"${saved[$ns]}" >/dev/null ||
			printf 'rotate-secret: %s in %s could not be put back: re-create it before any reader restarts\n' "$secret" "$ns" >&2
	done
	die "$1; the old $secret is back in every copy"
}

for ns in $namespaces; do
	kubectl -n "$ns" delete secret "$secret" --wait=true >/dev/null || restore_and_die "deleting $secret in $ns failed"
done

if [ "$class" = forge ]; then
	helmfile -f "$helmfile_file" -e "$instance" -l release=gitea-bootstrap sync >/dev/null || restore_and_die "the forge bootstrap did not mint a new token"
elif [ "$class" = catalogue ]; then
	helmfile -f "$helmfile_file" -e "$instance" -l "release=$release" sync >/dev/null || restore_and_die "the catalogue release $release did not mint a new token"
else
	helmfile -f "$helmfile_file" -e "$instance" -l "secret-name=$secret" sync >/dev/null || restore_and_die "the generator release of $secret did not apply"
fi
if [ "${push_config:-0}" = 1 ]; then
	helmfile -f "$helmfile_file" -e "$instance" -l release=keycloak-config sync >/dev/null || restore_and_die "keycloak-config-cli did not push the new client secret"
fi

new=$(value_of "$first")
[ -n "$new" ] || restore_and_die "$secret was not made again in $first"
unset saved
[ "$new" != "$old" ] || die "$secret holds the old value after the apply; a copy survived somewhere"
for ns in $namespaces; do
	[ "$(value_of "$ns")" = "$new" ] || die "the copy in $ns differs from the one in $first"
done
say "new value written in every copy"

# Kept before any reader restarts, so no pod ever starts with the new key alone. Sent on stdin:
# printf is a builtin, so the value is on no process's command line.
if [ "${keeps_previous:-0}" = 1 ]; then
	encoded=$(printf '%s' "$old" | base64 | tr -d '\n')
	for ns in $namespaces; do
		printf '{"data":{"previous":"%s"}}' "$encoded" |
			kubectl -n "$ns" patch secret "$secret" --type merge --patch-file /dev/stdin >/dev/null ||
			die "keeping the old value of $secret as previous in $ns failed; its readers were not restarted"
	done
	say "the old value stays readable as previous; run again with --drop-previous once every session has outlived it"
fi

restart_readers

# The measurement: the old value refused, the new one accepted. Values go to curl and psql on
# stdin, never on a command line another process could read.
client_status() { # <secret on stdin> -> HTTP status of a client_credentials grant
	curl -sS --max-time 10 -o /dev/null -w '%{http_code}' \
		-d grant_type=client_credentials -d "client_id=${secret#keycloak-client-}" \
		--data-urlencode client_secret@- \
		"$idm/realms/$realm/protocol/openid-connect/token" 2>/dev/null || echo curl-error
}
forge_status() { # <token on stdin> -> HTTP status of the forge's /user route with it (401 only for a token it does not know)
	sed 's/^/header = "Authorization: token /; s/$/"/' | curl -sS --max-time 10 -o /dev/null -w '%{http_code}' \
		-K - "$forge/api/v1/user" 2>/dev/null || echo curl-error
}
catalogue_status() { # <token on stdin> -> HTTP status of listing the site administrator's tokens with it
	local user
	user=$(kubectl -n "$first" get secret ckan-admin-credentials -o 'jsonpath={.data.username}' 2>/dev/null | base64 -d 2>/dev/null)
	[ -n "$user" ] || user="admin"
	sed 's/^/header = "Authorization: /; s/$/"/' | curl -sS --max-time 10 -o /dev/null -w '%{http_code}' \
		-K - -H 'Content-Type: application/json' --data "{\"user_id\":\"$user\"}" \
		"$catalogue/api/3/action/api_token_list" 2>/dev/null || echo curl-error
}
database_login() { # <password on stdin> -> "accepted", "refused", or "unknown" when there is no primary to ask
	local pod user
	pod=$(kubectl get pod -A -l cnpg.io/instanceRole=primary -o 'jsonpath={.items[0].metadata.namespace}/{.items[0].metadata.name}' 2>/dev/null)
	case "$pod" in '' | /*) cat >/dev/null; echo unknown; return ;; esac
	user=$(kubectl -n "$first" get secret "$secret" -o 'jsonpath={.data.username}' 2>/dev/null | base64 -d 2>/dev/null)
	[ -n "$user" ] || user="${secret#db-}"
	if kubectl -n "${pod%/*}" exec -i "${pod#*/}" -c postgres -- sh -c \
		"IFS= read -r PGPASSWORD; export PGPASSWORD; psql -h localhost -U '$user' -d postgres -tAc 'select 1'" >/dev/null 2>&1; then
		echo accepted
	else
		echo refused
	fi
}

refused() { # <value> -> exit 0 when the other side refuses it
	case "$class" in
	client) [ "$(printf '%s' "$1" | client_status)" = 401 ] ;;
	forge) [ "$(printf '%s\n' "$1" | forge_status)" = 401 ] ;;
	# CKAN reads a token it does not know as anonymous, and refuses anonymous this list.
	catalogue) [ "$(printf '%s\n' "$1" | catalogue_status)" = 403 ] ;;
	database)
		case "$(printf '%s\n' "$1" | database_login)" in
		refused) return 0 ;;
		accepted) return 1 ;;
		*) say "FAIL: no CloudNativePG primary to measure the login on"; exit 1 ;;
		esac ;;
	esac
}

case "$class" in
generic)
	say "done: $secret has a new value and every reader restarted; nothing outside the cluster checks it"
	exit 0 ;;
esac

deadline=$(($(date +%s) + bound))
until refused "$old"; do
	[ "$(date +%s)" -lt "$deadline" ] || {
		say "FAIL: the old value of $secret is still accepted after ${bound}s"
		exit 1
	}
	sleep 2
done
say "the old value is refused"
if refused "$new"; then
	say "FAIL: the new value of $secret is refused too"
	exit 1
fi
say "the new value is accepted; $secret rotated"
