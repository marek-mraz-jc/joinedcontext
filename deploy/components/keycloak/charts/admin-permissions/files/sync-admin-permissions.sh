#!/usr/bin/env bash
# The Portal's client administration, narrowed to its own clients (T-2725, AP-27, AP-111).
#
# The realm runs Keycloak's fine-grained admin permissions (v2). `portal-api` holds the
# read-only `view-clients` role, so it lists and finds clients, and this script grants it the
# write scopes on every client of the realm and takes them away again on every client that does
# not carry `managed-by: joinedcontext`: the edge client, the platform's own clients and
# Keycloak's built-in ones. The App and workload clients the Portal creates carry the mark, so
# it keeps creating, changing and deleting those.
#
# It then links the Portal's `endpoint:{slug}` scopes to the MCP hub, which the deny keeps the
# Portal from doing itself (T-3324, below).
#
# Run after every realm import and idempotent: it finds its two policies and two permissions by
# name and writes them as they should be, and the deny covers the clients the realm holds now.
# A client somebody adds by hand between two runs is the Portal's to change until the next.
#
# Reading a client's secret stays with `view-clients` (Keycloak 26.6 lists clients with their
# secrets to a caller that may view them): confidential platform clients move off shared
# secrets (T-2868), which is what leaves the list nothing to show.
set -euo pipefail

: "${KEYCLOAK_URL:?the Keycloak base URL}"
: "${KEYCLOAK_USER:?the realm admin}"
: "${KEYCLOAK_PASSWORD:?the realm admin password}"
: "${REALM:?the realm}"
: "${PRINCIPAL:?the client whose service account administers clients}"
MANAGED_KEY=${MANAGED_KEY:-managed-by}
MANAGED_VALUE=${MANAGED_VALUE:-joinedcontext}
# The client scopes of the permissions v2 resource type `Clients` that change something.
WRITE_SCOPES='["manage","map-roles","map-roles-client-scope","map-roles-composite"]'

CONFIG=$(mktemp)
trap 'rm -f "$CONFIG"' EXIT
kc() { /opt/keycloak/bin/kcadm.sh "$@" --config "$CONFIG"; }
fail() { echo "admin permissions: $*" >&2; exit 1; }

login() {
  kc config credentials --server "$KEYCLOAK_URL" --realm master \
    --user "$KEYCLOAK_USER" --password "$KEYCLOAK_PASSWORD" >/dev/null
}
login

client_id() {
  kc get clients -r "$REALM" -q "clientId=$1" --fields id --format csv --noquotes | head -n 1
}

permissions=$(client_id admin-permissions)
[ -n "$permissions" ] || fail "realm $REALM has no admin-permissions client: adminPermissionsEnabled is off"
principal=$(client_id "$PRINCIPAL")
[ -n "$principal" ] || fail "realm $REALM has no client $PRINCIPAL"
server="clients/$permissions/authz/resource-server"

# The id of the policy or permission of that exact name, or nothing. Plain bash: the Keycloak
# image carries no awk, sed or jq.
named() {
  local id name
  while IFS=, read -r id name; do
    if [ "$name" = "$2" ]; then
      echo "$id"
      return
    fi
  done < <(kc get "$server/$1" -r "$REALM" -q "name=$2" --fields id,name --format csv --noquotes)
}

# Creates or rewrites one policy or permission; `$1` is its path under the resource server
# (`policy/client`, `permission/scope`), `$2` its name, the rest its fields.
ensure() {
  local path=$1 name=$2 id
  shift 2
  id=$(named "${path%%/*}" "$name")
  if [ -z "$id" ]; then
    kc create "$server/$path" -r "$REALM" -s "name=$name" "$@" >/dev/null
    echo "created $name"
  else
    kc update "$server/$path/$id" -r "$REALM" -s "id=$id" -s "name=$name" "$@"
    echo "updated $name"
  fi
}

ensure policy/client "$PRINCIPAL" -s logic=POSITIVE -s "clients=[\"$principal\"]"
ensure policy/client "not-$PRINCIPAL" -s logic=NEGATIVE -s "clients=[\"$principal\"]"
granted=$(named policy "$PRINCIPAL")
denied=$(named policy "not-$PRINCIPAL")

# Every client of the realm without the mark, the principal itself included.
unmanaged=""
count=0
while IFS=, read -r id _client mark; do
  if [ -n "$id" ] && [ "$mark" != "$MANAGED_VALUE" ]; then
    unmanaged+="${unmanaged:+,}\"$id\""
    count=$((count + 1))
  fi
done < <(kc get clients -r "$REALM" -q first=0 -q max=100000 \
  --fields "id,clientId,attributes($MANAGED_KEY)" --format csv --noquotes)
[ "$count" -gt 0 ] || fail "realm $REALM lists no client without $MANAGED_KEY: refusing to grant every client"

ensure permission/scope "$PRINCIPAL-manages-clients" -s resourceType=Clients \
  -s "scopes=$WRITE_SCOPES" -s "policies=[\"$granted\"]"
ensure permission/scope "$PRINCIPAL-not-on-unmanaged-clients" -s resourceType=Clients \
  -s "scopes=$WRITE_SCOPES" -s "resources=[$unmanaged]" -s "policies=[\"$denied\"]"
echo "denied $PRINCIPAL the writes on $count clients without $MANAGED_KEY: $MANAGED_VALUE"

# The MCP hub's links (T-3324, EP-88). The deny above covers `mcp-hub`, and Keycloak checks
# `manage` on a client to link a scope to it, so the Portal cannot link its `endpoint:{slug}`
# scopes there itself. The realm admin does it on each run: every `endpoint:` scope carrying the
# mark is an optional scope of the hub, and an `endpoint:` scope without the mark, or linked as
# a default scope, is unlinked, because it would widen what hub tokens reach. A new Endpoint
# reaches the hub after the next run.
# `endpoint:*` is not one of them: the realm import writes it, unmarked, as the hub's one
# default scope (ADR-N-025 4b), so it is left as it is (T-4459).
# The command line's client gets the same optional links (T-5614): `jcctl token --resource` asks
# for one of them, and the gateway admits a `jcctl` token on the one Endpoint its scope names.
# There `endpoint:*` is unlinked like any stray, so no jcctl token names every Endpoint.
# The Portal's own client gets them too (T-6063): a connector's `jc_data_query` exchanges the
# person's token for `endpoint:{slug}` (RFC 8693), and Keycloak's standard token exchange grants
# only a scope the requester holds as optional. No `endpoint:*` there either.
HUB_CLIENT=${HUB_CLIENT:-mcp-hub}
CLI_CLIENT=${CLI_CLIENT:-jcctl}
SCOPE_PREFIX=endpoint:
declare -A ours=()
while IFS=, read -r id name mark; do
  if [ -n "$id" ] && [ "${name#"$SCOPE_PREFIX"}" != "$name" ] && [ "$mark" = "$MANAGED_VALUE" ]; then
    ours[$id]=$name
  fi
done < <(kc get client-scopes -r "$REALM" --fields "id,name,attributes($MANAGED_KEY)" --format csv --noquotes)
# The links themselves go over bash's /dev/tcp with the token kcadm keeps in its config: a
# kcadm JVM per link took a second each, so a fresh realm of 10,000 Endpoints never converged
# inside the Job's timeout, and one process does a link in milliseconds (T-4474, ADR-N-025 §4d).
# Plain http only: KEYCLOAK_URL is the in-cluster Service, which the mesh carries as mTLS.
[[ $KEYCLOAK_URL =~ ^http://([^/:]+)(:([0-9]+))?(/.*)?$ ]] ||
  fail "KEYCLOAK_URL is not an in-cluster http URL: $KEYCLOAK_URL"
kc_host=${BASH_REMATCH[1]}
kc_port=${BASH_REMATCH[3]:-80}
kc_prefix=${BASH_REMATCH[4]%/}
# The Host the token was issued through, as kcadm sends it: the port unless it is 80.
kc_authority=$kc_host
[ "$kc_port" = 80 ] || kc_authority=$kc_host:$kc_port
admin_token() {
  local config pattern='"token" *: *"([^"]+)"'
  config=$(<"$CONFIG")
  [[ $config =~ $pattern ]] || fail "kcadm keeps no token"
  echo "${BASH_REMATCH[1]}"
}
# The status of one PUT under the realm's admin API.
put_status() {
  local status
  exec 3<>"/dev/tcp/$kc_host/$kc_port"
  printf 'PUT %s/admin/realms/%s/%s HTTP/1.1\r\nHost: %s\r\nAuthorization: Bearer %s\r\nContent-Length: 0\r\nConnection: close\r\n\r\n' \
    "$kc_prefix" "$REALM" "$1" "$kc_authority" "$token" >&3
  read -r _ status _ <&3
  exec 3<&-
  echo "$status"
}

# Links every marked scope to client $1 as optional and unlinks every other `endpoint:` link;
# with $2 = keep-star, the realm's own `endpoint:*` stays where the import put it.
link_endpoint_scopes() {
  local client=$1 star=$2 client_id_ path kind id name status added=0
  client_id_=$(client_id "$client")
  if [ -z "$client_id_" ]; then
    echo "no client $client: no endpoint scope to link"
    return 0
  fi
  path="clients/$client_id_"
  local -A linked=()
  for kind in default optional; do
    while IFS=, read -r id name; do
      if [ -z "$id" ] || [ "${name#"$SCOPE_PREFIX"}" = "$name" ]; then
        continue
      fi
      if [ "$name" = "${SCOPE_PREFIX}*" ] && [ "$star" = keep-star ]; then
        continue
      fi
      if [ "$kind" = optional ] && [ -n "${ours[$id]:-}" ]; then
        linked[$id]=1
      else
        kc delete "$path/$kind-client-scopes/$id" -r "$REALM"
        echo "unlinked $name from $client ($kind)"
      fi
    done < <(kc get "$path/$kind-client-scopes" -r "$REALM" --fields id,name --format csv --noquotes)
  done
  token=$(admin_token)
  for id in "${!ours[@]}"; do
    if [ -z "${linked[$id]:-}" ]; then
      status=$(put_status "$path/optional-client-scopes/$id")
      if [ "$status" = 401 ]; then
        login
        token=$(admin_token)
        status=$(put_status "$path/optional-client-scopes/$id")
      fi
      [ "$status" = 204 ] || fail "linking ${ours[$id]} to $client answered $status"
      added=$((added + 1))
    fi
  done
  echo "linked $added of ${#ours[@]} $SCOPE_PREFIX scopes to $client"
}

link_endpoint_scopes "$HUB_CLIENT" keep-star
link_endpoint_scopes "$CLI_CLIENT" unlink-star
link_endpoint_scopes "$PRINCIPAL" unlink-star
