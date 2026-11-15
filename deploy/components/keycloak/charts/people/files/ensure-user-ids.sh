#!/usr/bin/env bash
# The seeded people under the fixed Keycloak ids the manifests name them by (PF-126, ADR-N-061).
#
# keycloak-config-cli creates a user through the admin API, which drops the `id` it is given, so
# this runs before the realm import and creates each person through the realm's partial import,
# which keeps it. The import then sets their password, second factor, roles and groups as before.
#
# One line per person in $USERS_FILE: `<id> <username> <recreate>`. A person who is missing is
# created; one who holds the id already is left alone; one who holds another id is deleted and
# created again only when `recreate` is true (the demo people of the development profile, whose
# password and second factor the import sets again from their Secrets). Otherwise the Job fails
# and names the id the deployment has to carry: an owner's account is never deleted.
#
# Run on every apply and idempotent. On a fresh installation the realm does not exist yet: it is
# created bare, and the import fills it in.
set -euo pipefail

: "${KEYCLOAK_URL:?the Keycloak base URL}"
: "${KEYCLOAK_USER:?the realm admin}"
: "${KEYCLOAK_PASSWORD:?the realm admin password}"
: "${REALM:?the realm}"
USERS_FILE=${USERS_FILE:-/config/users}

CONFIG=$(mktemp)
BODY=$(mktemp)
HELD=$(mktemp)
trap 'rm -f "$CONFIG" "$BODY" "$HELD"' EXIT
kc() { /opt/keycloak/bin/kcadm.sh "$@" --config "$CONFIG"; }
fail() { echo "user ids: $*" >&2; exit 1; }

kc config credentials --server "$KEYCLOAK_URL" --realm master \
  --user "$KEYCLOAK_USER" --password "$KEYCLOAK_PASSWORD" >/dev/null

if ! kc get "realms/$REALM" --fields realm >/dev/null 2>&1; then
  kc create realms -s realm="$REALM" -s enabled=true
  echo "realm $REALM created; the import fills it in"
fi

# Every kcadm.sh call starts a JVM (about 5 s under the Job's CPU limit): one call per person and
# check took 234 s for the 14 demo people in CI (T-5846). So the realm's users are read in pages
# of $PAGE, every line is checked before anything changes, and the people go in at most two
# partial imports.
PAGE=500
read_held() {
  : > "$HELD"
  local first=0 page rows
  while :; do
    page=$(kc get users -r "$REALM" -q briefRepresentation=true -q first="$first" -q max="$PAGE" \
      --fields id,username --format csv --noquotes)
    [ -n "$page" ] || break
    printf '%s\n' "$page" >> "$HELD"
    mapfile -t rows <<< "$page"
    [ ${#rows[@]} -ge "$PAGE" ] || break
    first=$((first + PAGE))
  done
}
# The id a username holds in the realm, or nothing. Keycloak keeps usernames in lower case.
# The image has no awk: plain bash.
id_of() {
  local id username
  while IFS=, read -r id username; do
    if [ "$username" = "${1,,}" ]; then echo "$id"; return; fi
  done < "$HELD"
}

UUID='^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
read_held
missing=() replaced=() created=()
while read -r id username recreate; do
  [ -n "$id" ] || continue
  [[ $id =~ $UUID ]] || fail "'$id' is not a lower-case Keycloak user id"
  [[ $username =~ ^[A-Za-z0-9._@+-]+$ ]] || fail "a username of $id holds a character Keycloak refuses"
  held=$(id_of "$username")
  if [ "$held" = "$id" ]; then
    echo "$id is in place"
  elif [ -z "$held" ]; then
    missing+=("$id $username")
  elif [ "$recreate" = true ]; then
    replaced+=("$id $username $held")
  else
    fail "the person meant to hold $id holds $held: set that id where the deployment names them (global.initialUserId); nobody was changed"
  fi
done < "$USERS_FILE"

# One partial import of these people: SKIP leaves anyone else alone, OVERWRITE replaces the
# person under the old id.
import_people() { # mode "id username [held]"...
  local mode=$1 sep="" entry id username
  shift
  [ $# -gt 0 ] || return 0
  {
    printf '{"ifResourceExists":"%s","users":[' "$mode"
    for entry in "$@"; do
      read -r id username _ <<< "$entry"
      printf '%s{"id":"%s","username":"%s","email":"%s","enabled":true,"emailVerified":true}' \
        "$sep" "$id" "$username" "$username"
      sep=,
    done
    printf ']}'
  } > "$BODY"
  kc create partialImport -r "$REALM" -f "$BODY" >/dev/null
  created+=("$@")
}
import_people SKIP "${missing[@]}"
import_people OVERWRITE "${replaced[@]}"
[ ${#created[@]} -gt 0 ] || exit 0

read_held
for entry in "${created[@]}"; do
  read -r id username was <<< "$entry"
  [ "$(id_of "$username")" = "$id" ] || fail "Keycloak did not create $id"
  echo "$id created${was:+ in place of $was}"
done
