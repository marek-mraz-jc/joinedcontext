#!/usr/bin/env bash
# The presenter's password: the owner's own dev admin password, copied as Keycloak stores it (T-6542).
#
#   # once, on dev: copy the owner's dev admin credential (hashed) into the sops file
#   KUBECONFIG=.secrets/kubeconfig-dev.yaml \
#       scripts/presenter-secret.sh capture --realm <dev instanceSlug> --from <the owner's dev username>
#
#   # per instance (dev, stage, smartcity): write it into the presenter's account
#   SOPS_AGE_KEY_FILE=.secrets/age-<instance>.key KUBECONFIG=.secrets/kubeconfig-<instance>.yaml \
#       scripts/presenter-secret.sh load --instance <instance> --realm <instanceSlug>
#
#   add --dry-run to either: it reads and prints the plan, writes nothing
#
# Owner order 2026-11-03 04:48Z: the presenter signs in with the owner's dev password; 04:52Z: no
# TOTP. Nobody reads or types the plain password. `capture` reads the password credential row of
# the named dev account from Keycloak's database (secret_data holds the salted hash, credential_data
# the algorithm and iterations) and encrypts both with sops to the recipients of .sops.yaml straight
# from memory into components/keycloak/presenter.enc.yaml; commit that file. `load` decrypts it and
# writes the credential into the presenter (components/keycloak/presenter.yaml) through the realm's
# partial import, the one admin call that takes a stored hash, then applies keycloak-config, which
# puts the person's groups back and never touches a password it was not given.
#
# The owner changing their dev password changes nothing here until `capture` and `load` run again.
set -uo pipefail

username="presenter@hel.fi"
sops_file="components/keycloak/presenter.enc.yaml"
helmfile_file="deployment/helmfile.yaml"
id_file_for() { printf 'ci/example-deployments/environments/%s/global.yaml.gotmpl' "$1"; }

die() { printf 'presenter-secret: %s\n' "$1" >&2; exit 2; }
say() { printf 'presenter-secret: %s\n' "$1"; }

command="${1:-}"
[ $# -gt 0 ] && shift
instance=""
realm=""
from=""
dry_run=0
while [ $# -gt 0 ]; do
	case "$1" in
	--instance) instance="${2:?}"; shift 2 ;;
	--realm) realm="${2:?}"; shift 2 ;;
	--from) from="${2:?}"; shift 2 ;;
	--dry-run) dry_run=1; shift ;;
	*) die "unknown argument $1 (see the header of this script)" ;;
	esac
done
case "$command" in capture | load) ;; *) die "usage: presenter-secret.sh capture|load ... (see the header of this script)" ;; esac
[ -n "$realm" ] || die "--realm is required (the instance's global.instanceSlug)"
case "$realm" in *[!a-z0-9-]*) die "--realm takes lowercase letters, digits and dashes" ;; esac
[ -f .sops.yaml ] || die "run from the deployment repository root"

capture() {
	[ -n "$from" ] || die "--from is required: the owner's dev admin username"
	case "$from" in *[!A-Za-z0-9@._+-]*) die "--from takes a Keycloak username" ;; esac
	local pod row algorithm
	pod=$(kubectl get pod -A -l cnpg.io/cluster=postgres-cluster,cnpg.io/instanceRole=primary \
		-o 'jsonpath={.items[0].metadata.namespace}/{.items[0].metadata.name}') || die "cannot list the database pods"
	case "$pod" in '' | /*) die "postgres-cluster has no primary in this cluster" ;; esac
	# One row or nothing; psql reads the names as variables, never as SQL text.
	row=$(printf '%s\n' "select json_build_object('secretData', c.secret_data, 'credentialData', c.credential_data)
		from credential c join user_entity u on u.id = c.user_id join realm r on r.id = u.realm_id
		where r.name = :'realm' and u.username = :'from' and c.type = 'password';" \
		| kubectl -n "${pod%/*}" exec -i "${pod#*/}" -c postgres -- \
			psql -X -q -v ON_ERROR_STOP=1 -tA -d keycloak -v realm="$realm" -v from="$from") \
		|| die "cannot read the credential of $from in realm $realm"
	[ "$(printf '%s\n' "$row" | grep -c .)" = 1 ] || die "$from holds no single password credential in realm $realm"
	algorithm=$(printf '%s' "$row" | python3 -c 'import json,sys; print(json.loads(json.loads(sys.stdin.read())["credentialData"]).get("algorithm","?"))') \
		|| die "the credential of $from is not Keycloak's JSON"
	say "read the $algorithm password credential of $from in realm $realm"
	if [ "$dry_run" = 1 ]; then
		say "would encrypt it into $sops_file"
		return
	fi
	printf '%s' "$row" | python3 -c '
import json, sys, yaml
row = json.loads(sys.stdin.read())
yaml.safe_dump({"from": sys.argv[1], "secretData": row["secretData"], "credentialData": row["credentialData"]}, sys.stdout)
' "$from" | sops --encrypt --input-type yaml --output-type yaml --filename-override "$sops_file" /dev/stdin >"$sops_file.new" \
		|| { rm -f "$sops_file.new"; die "sops could not encrypt the credential"; }
	mv "$sops_file.new" "$sops_file"
	say "wrote $sops_file: commit it, then run \`load\` on each instance"
}

load() {
	[ -n "$instance" ] || die "--instance is required"
	[ -n "${SOPS_AGE_KEY_FILE:-}${SOPS_AGE_KEY:-}" ] || die "set SOPS_AGE_KEY_FILE to this instance's age key"
	[ -f "$sops_file" ] || die "$sops_file is missing: run \`capture\` on dev first"
	local id body pod
	id=$(sed -n 's/^  presenterUserId: \([0-9a-f-]*\)$/\1/p' "$(id_file_for "$instance")" 2>/dev/null)
	[ -n "$id" ] || die "$instance sets no global.presenterUserId"
	body=$(sops -d "$sops_file" | python3 -c '
import json, sys, yaml
copied = yaml.safe_load(sys.stdin)
if not json.loads(copied["credentialData"]).get("algorithm"):
    sys.exit("the copied credential names no algorithm")
user = {"id": sys.argv[1], "username": sys.argv[2], "email": sys.argv[2], "enabled": True, "emailVerified": True,
        "credentials": [{"type": "password", "secretData": copied["secretData"], "credentialData": copied["credentialData"]}]}
print(json.dumps({"ifResourceExists": "OVERWRITE", "users": [user]}))
' "$id" "$username") || die "cannot decrypt $sops_file with this key"
	say "decrypted the copied password credential for $username ($id)"
	if [ "$dry_run" = 1 ]; then
		say "would write it into realm $realm by partial import and apply keycloak-config"
		return
	fi
	pod=$(kubectl get pod -A -l app.kubernetes.io/name=keycloakx,app.kubernetes.io/instance=keycloak-app \
		-o 'jsonpath={.items[0].metadata.namespace}/{.items[0].metadata.name}') || die "cannot find the Keycloak pod"
	case "$pod" in '' | /*) die "no Keycloak pod in the cluster" ;; esac
	# The admin password is the first line of stdin inside the Keycloak pod (as scripts/preview.sh),
	# the import the rest, so neither reaches a command line.
	# shellcheck disable=SC2016 # the script runs inside the pod, which expands it
	{
		kubectl -n "${pod%/*}" get secret keycloak-admin-user -o 'jsonpath={.data.password}' | base64 -d
		echo
		printf '%s\n' "$body"
	} | kubectl -n "${pod%/*}" exec -i "${pod#*/}" -c keycloak -- sh -c '
set -eu
kc() { /opt/keycloak/bin/kcadm.sh "$@" --config /tmp/kcadm-presenter.config; }
trap "rm -f /tmp/kcadm-presenter.config" EXIT
IFS= read -r password
kc config credentials --server http://localhost:8080 --realm master --user admin --password "$password" >/dev/null
kc create partialImport -r "$1" -f - >/dev/null
' sh "$realm" || die "the partial import into realm $realm failed"
	say "wrote the copied credential into $username in realm $realm"
	# The overwrite dropped the person's groups; keycloak-config sets them again.
	helmfile -f "$helmfile_file" -e "$instance" -l release=keycloak-config sync >/dev/null || die "release keycloak-config did not apply"
	say "applied keycloak-config: $username signs in on $instance with the owner's dev password"
}

"$command"
