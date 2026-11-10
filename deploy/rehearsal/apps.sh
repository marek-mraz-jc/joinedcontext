# shellcheck shell=bash
# deploy/rehearsal/apps.sh: the rehearsal's Apps, sourced by up.sh right before jc starts
# (up.sh's variables: here, home, port, bind, export_dir, secret(); owner of this file: jc-apps-export).
#
# 1. The App bundles the jc image ships (/srv/apps, bins/jc/Dockerfile's `apps` stage), copied out
#    of that stage built from this clone (REHEARSAL_JC_IMAGE: copied out of that built jc image
#    instead), never built by hand: JC_PORTAL_APPS_DIR.
# 2. The cache the Portal fetches published builds into and serves them from (apps/fetch.rs, which
#    checks each package's SHA-256 against status.build before it unpacks): JC_PORTAL_APPS_CACHE_DIR.
#    Nothing here writes into it.
# 3. The export's prod builds (<export>/apps, made read only by prod-deployer) published as generic
#    packages of shako's jc-apps owner, once jc answers, in the background (apps-publish.log), and
#    rehearsal-app-roles named in the roles Apps' Group manifests of the rehearsal's own repository.
# 4. rehearsal-app-roles: a person of the imported realm in the groups of every viewer RoleBinding,
#    and (3) in the roles Apps' groups, for the suite's refused/served pairs (rehearsal-tester is in
#    none).
# 5. jc-functions, the platform image's runtime of App functions (deployment components/functions; a
#    platform component CUTOVER.md keeps beside jc), on loopback: it takes the Portal's portal-api
#    token of the imported realm and sends each function's reads to jc's gateway. JC_FUNCTIONS_URL.
#    REHEARSAL_FUNCTIONS_IMAGE / REHEARSAL_FUNCTIONS_PORT (default the public port + 1).
# 6. REHEARSAL_APP_PODS="hsl-transport=<image>": each ui-rust App's image (the monorepo's
#    image-apps.yml, or the same target built locally) on loopback, at the public port + 2, +3, ...,
#    reading its Endpoint `app-{name}` of the export on jc's own gateway, as deployment
#    components/app-pods runs it on jc's Service: JC_PORTAL_APP_PODS.
# REHEARSAL_APPS=off leaves all of it out.
# shellcheck disable=SC2154  # here, home, port, export_dir: up.sh's

if [ "${REHEARSAL_APPS:-on}" != off ]; then
  apps_root=$home/apps
  mkdir -p "$apps_root"
  apps_image=${REHEARSAL_JC_IMAGE:-}
  if [ -z "$apps_image" ]; then
    apps_image=$(docker build -q --target apps -f "$here/../../bins/jc/Dockerfile" "$here/../..") || {
      echo "apps.sh: the jc image's apps stage did not build; REHEARSAL_APPS=off starts jc without Apps" >&2
      exit 1
    }
  fi
  apps_id=$(docker image inspect -f '{{.Id}}' "$apps_image")
  if [ "$(cat "$apps_root/image-id" 2>/dev/null)" != "$apps_id" ]; then
    rm -rf "$apps_root/srv.new"
    apps_cid=$(docker create "$apps_id" /nonexistent)
    docker cp "$apps_cid:/srv/apps" "$apps_root/srv.new" >/dev/null
    docker rm "$apps_cid" >/dev/null
    rm -rf "$apps_root/srv"
    mv "$apps_root/srv.new" "$apps_root/srv"
    printf '%s\n' "$apps_id" > "$apps_root/image-id"
  fi
  # A bundle without integrity.json serves nothing (static_host.rs): refuse a copy that lost one.
  for d in "$apps_root"/srv/*/; do
    if [ ! -s "$d/index.html" ] || [ ! -s "$d/integrity.json" ]; then
      echo "apps.sh: $d has no index.html or integrity.json" >&2; exit 1
    fi
  done
  export JC_PORTAL_APPS_DIR=$apps_root/srv
  mkdir -p "$home/state/apps-cache"
  chmod 700 "$home/state/apps-cache"
  export JC_PORTAL_APPS_CACHE_DIR=$home/state/apps-cache
  # The Portal behind prod's edge believes the edge's X-Access-Token (components/portal
  # base-values JC_TRUST_EDGE_TOKEN), the one credential an App function's data calls carry;
  # the rehearsal listens on loopback only, and the charts-apps suite stands in for the edge.
  if [ "$bind" = 127.0.0.1 ]; then export JC_TRUST_EDGE_TOKEN=true; fi
  echo "apps.sh: $(find "$JC_PORTAL_APPS_DIR" -mindepth 1 -maxdepth 1 -type d | wc -l) shipped bundles from ${REHEARSAL_JC_IMAGE:-the apps stage} (${apps_id:7:12})"

  realm_file=$(find "$home/state/realms" -maxdepth 1 -name '*.json' 2>/dev/null | head -1)
  if [ -n "$realm_file" ]; then
    # the pin of deployment components/functions/images.yaml
    functions_image=${REHEARSAL_FUNCTIONS_IMAGE:-ghcr.io/marek-mraz-jc/joinedcontext-platform@sha256:471d88ddd1b54f934d9ceb9b3fd7fae1650e0e4cb42b438961bc91d5a093018d}
    functions_port=${REHEARSAL_FUNCTIONS_PORT:-$((port + 1))}
    functions_realm=$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))["realm"])' "$realm_file")
    docker rm -f "rh-functions-$port" >/dev/null 2>&1 || true
    # It reads the realm's keys from jc, which starts after it: it retries in the background.
    docker run -d --name "rh-functions-$port" --network host --restart on-failure:5 --read-only \
      --cap-drop ALL --security-opt no-new-privileges --memory 1g --pids-limit 256 \
      --entrypoint /usr/local/bin/jc-functions \
      -e JC_FUNCTIONS_BIND="127.0.0.1:$functions_port" \
      -e JC_OIDC_ISSUER="http://localhost:$port/id/realms/$functions_realm" \
      -e JC_OIDC_JWKS_URL="http://localhost:$port/id/realms/$functions_realm/protocol/openid-connect/certs" \
      -e JC_FUNCTIONS_CALLER=portal-api -e JC_FUNCTIONS_ASSISTANT_CALLER=jc-assistant \
      -e JC_FUNCTIONS_AUDIENCE=jc-functions -e JC_GATEWAY_URL="http://localhost:$port" \
      "$functions_image" >/dev/null || { echo "apps.sh: jc-functions did not start" >&2; exit 1; }
    export JC_FUNCTIONS_URL=http://127.0.0.1:$functions_port
    echo "apps.sh: jc-functions on $JC_FUNCTIONS_URL (${functions_image##*@})"
  fi
  if [ -n "$realm_file" ] && [ -d "$export_dir/gitea/files" ]; then
    secret realm/REHEARSAL_APP_ROLES_PASSWORD > /dev/null
    ln -sfn realm/REHEARSAL_APP_ROLES_PASSWORD "$home/secrets/realm-REHEARSAL_APP_ROLES_PASSWORD"
    # REHEARSAL_APP_HOLDER_ID: the holder takes the user id a prod read Policy names
    # (banskabystrica-zaznamy reads only as fc3b55df, demo.steward), on a loopback rehearsal only.
    holder_id=()
    if [ -n "${REHEARSAL_APP_HOLDER_ID:-}" ]; then
      case $bind in
        127.*|::1) holder_id=(--id "$REHEARSAL_APP_HOLDER_ID") ;;
        *) echo "apps.sh: REHEARSAL_APP_HOLDER_ID only on a loopback rehearsal, not $bind" >&2; exit 1 ;;
      esac
    fi
    python3 -I "$here/apps.py" person --realm-file "$realm_file" --files "$export_dir/gitea/files" \
      --password-file "$home/secrets/realm/REHEARSAL_APP_ROLES_PASSWORD" "${holder_id[@]}"
  fi

  pods=() pod_port=$((port + 2))
  for pod in ${REHEARSAL_APP_PODS:-}; do
    name=${pod%%=*} image=${pod#*=}
    endpoint=$(find "$export_dir/gitea/files" -path "*/spaces/*/endpoints/app-$name.yaml" | head -1)
    slug=$(sed -n 's/^  slug: //p' "$endpoint" 2>/dev/null)
    [ -n "$slug" ] || { echo "apps.sh: no Endpoint app-$name with a slug in $export_dir/gitea/files" >&2; exit 1; }
    project=$(sed -n 's/^  namespace: //p' "$endpoint")
    space=$(basename "$(dirname "$(dirname "$endpoint")")")
    config=$(python3 -I -c 'import json, sys; n, s, sp, p, port = sys.argv[1:]; print(json.dumps({"appName": n, "endpointName": f"app-{n}", "endpoints": [{"name": f"app-{n}", "slug": s, "space": sp}], "portal": f"http://localhost:{port}/projects/{p}", "slug": s, "space": sp, "transport": "origin"}))' "$name" "$slug" "$space" "$project" "$port")
    docker rm -f "rh-app-$name-$port" >/dev/null 2>&1 || true
    docker run -d --name "rh-app-$name-$port" --network host --restart on-failure:5 --read-only \
      --tmpfs /tmp:rw,size=16m --cap-drop ALL --security-opt no-new-privileges --user 65532:65532 \
      -e JC_BIND_ADDRESS="127.0.0.1:$pod_port" -e JC_BASE_PATH=/ -e JC_APP_CONFIG="$config" \
      -e JC_ENDPOINT_URL="http://localhost:$port/api/endpoint/$slug/" \
      "$image" >/dev/null || { echo "apps.sh: the pod of $name did not start" >&2; exit 1; }
    pods+=("$name=http://127.0.0.1:$pod_port")
    echo "apps.sh: $name's pod on 127.0.0.1:$pod_port ($image, endpoint $slug)"
    pod_port=$((pod_port + 1))
  done
  if [ ${#pods[@]} -gt 0 ]; then
    JC_PORTAL_APP_PODS=$(IFS=,; echo "${pods[*]}")
    export JC_PORTAL_APP_PODS
  fi

  # Once jc answers: the prod builds into shako, then the holder into the roles Apps' groups.
  (
    if [ -d "$export_dir/apps" ]; then
      python3 -I "$here/apps.py" publish --base "http://localhost:$port" --export "$export_dir" \
        --token-file "$JCBIN_SHAKO_TOKEN_FILE" --report "$home/apps-publish.json"
    fi
    if [ -n "$realm_file" ]; then
      python3 -I "$here/apps.py" grant --base "http://localhost:$port" --export "$export_dir" \
        --token-file "$JCBIN_SHAKO_TOKEN_FILE" --realm-file "$realm_file" --secrets-dir "$home/secrets/realm"
    fi
  ) > "$home/apps-publish.log" 2>&1 &
fi
