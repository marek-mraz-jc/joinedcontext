#!/usr/bin/env bash
# =============================================================================
# set-initial-user-password.sh
#
# After a fresh deployment the initial user has no password set. This
# script force-sets a password for that user via kcadm.sh (run inside the
# keycloak pod) and clears its requiredActions, so a client can obtain an OIDC
# token via the ROPC grant.
#
# Callable from:
#   * seed-platform-data.sh — to establish AUTH_PASSWORD before seeding.
#   * GitLab CI (system-test setup) — to give the system tests a usable login.
#
# Behaviour:
#   * If AUTH_PASSWORD is already set in the env, it is trusted and no keycloak
#     reset is performed.
#   * Otherwise an ephemeral 32-char password is generated and force-set.
#
# On success AUTH_USER / AUTH_PASSWORD are exported for the caller (source it),
# and — if AUTH_DOTENV_FILE is set — written there as a dotenv artifact.
#
# Required env: NAMESPACE, KEYCLOAK_REALM, AUTH_USER
# =============================================================================
set -Eeuo pipefail

NAMESPACE="${NAMESPACE:-dev}"
KEYCLOAK_REALM="${KEYCLOAK_REALM:-dev}"
AUTH_USER="${AUTH_USER:-${GITLAB_USER_EMAIL:-admin@joinedcontext.test}}"

# Never leak AUTH_PASSWORD / KC_ADMIN_PASSWORD into the job log.
set +x

if [ -n "${AUTH_PASSWORD:-}" ]; then
  echo "AUTH_PASSWORD provided via env — skipping keycloak admin reset steps."
else
  # Generate a 32-char alphanumeric password.
  AUTH_PASSWORD="$(openssl rand -base64 288 | LC_ALL=C tr -dc 'A-Za-z0-9' | dd bs=32 count=1 2>/dev/null)"
  if [ "${#AUTH_PASSWORD}" -lt 16 ]; then
    echo "ERROR: failed to generate AUTH_PASSWORD (got length ${#AUTH_PASSWORD})" >&2
    exit 1
  fi

  KC_ADMIN_PASSWORD="$(kubectl -n "${NAMESPACE}" get secret keycloak-admin-user \
    -o jsonpath='{.data.password}' | base64 -d)"

  KC_POD="$(kubectl -n "${NAMESPACE}" get pod \
    -l app.kubernetes.io/name=keycloakx,app.kubernetes.io/instance=keycloak-app \
    -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)"
  if [ -z "${KC_POD}" ]; then
    echo "ERROR: could not locate keycloak pod in namespace ${NAMESPACE}" >&2
    exit 1
  fi

  echo "Resetting password + clearing required actions for ${AUTH_USER} via pod ${KC_POD}..."

  # Clear required actions (verifiyMail) and force-set the password.
  kubectl -n "${NAMESPACE}" exec -i "${KC_POD}" -c keycloak -- /bin/sh -c '
    set -e
    KC_ADMIN_PASSWORD="$1"
    KEYCLOAK_REALM="$2"
    AUTH_USER="$3"
    AUTH_PASSWORD="$4"
    /opt/keycloak/bin/kcadm.sh config credentials \
      --server http://localhost:8080 \
      --realm master \
      --user admin \
      --password "$KC_ADMIN_PASSWORD" >/dev/null
    /opt/keycloak/bin/kcadm.sh set-password \
      -r "$KEYCLOAK_REALM" \
      --username "$AUTH_USER" \
      --new-password "$AUTH_PASSWORD"
    USER_ID="$(/opt/keycloak/bin/kcadm.sh get users -r "$KEYCLOAK_REALM" \
                 -q "email=$AUTH_USER" --fields id --format csv --noquotes \
                 | head -n1 | tr -d "\r")"
    if [ -z "$USER_ID" ]; then
      echo "ERROR: user $AUTH_USER not found in realm $KEYCLOAK_REALM" >&2
      exit 1
    fi
    /opt/keycloak/bin/kcadm.sh update "users/$USER_ID" -r "$KEYCLOAK_REALM" \
      -s "emailVerified=true" \
      -s "requiredActions=[]" \
      -s "enabled=true"
  ' _ "${KC_ADMIN_PASSWORD}" "${KEYCLOAK_REALM}" "${AUTH_USER}" "${AUTH_PASSWORD}"
fi

if [ -n "${AUTH_DOTENV_FILE:-}" ]; then
  mkdir -p "$(dirname "${AUTH_DOTENV_FILE}")"
  cat > "${AUTH_DOTENV_FILE}" <<EOF
AUTH_USER=${AUTH_USER}
AUTH_PASSWORD=${AUTH_PASSWORD}
KEYCLOAK_URL=${KEYCLOAK_URL:-}
KEYCLOAK_REALM=${KEYCLOAK_REALM}
SYSTEM_TEST_AUTH_USER=${AUTH_USER}
SYSTEM_TEST_AUTH_PASSWORD=${AUTH_PASSWORD}
EOF
  echo "Auth dotenv written to ${AUTH_DOTENV_FILE}"
fi

# Export for callers that source this script.
export AUTH_USER AUTH_PASSWORD
