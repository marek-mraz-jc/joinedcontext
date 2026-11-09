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
#    packages of shako's jc-apps owner, once jc answers, in the background (apps-publish.log).
# 4. rehearsal-app-roles: a person of the imported realm holding the access groups of every
#    `visibility: roles` App, for the suite's 403/200 pair (rehearsal-tester holds none).
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
  echo "apps.sh: $(find "$JC_PORTAL_APPS_DIR" -mindepth 1 -maxdepth 1 -type d | wc -l) shipped bundles from ${REHEARSAL_JC_IMAGE:-the apps stage} (${apps_id:7:12})"

  realm_file=$(find "$home/state/realms" -maxdepth 1 -name '*.json' 2>/dev/null | head -1)
  if [ -n "$realm_file" ] && [ -d "$export_dir/gitea/files" ]; then
    secret realm/REHEARSAL_APP_ROLES_PASSWORD > /dev/null
    ln -sfn realm/REHEARSAL_APP_ROLES_PASSWORD "$home/secrets/realm-REHEARSAL_APP_ROLES_PASSWORD"
    python3 -I "$here/apps.py" person --realm-file "$realm_file" --files "$export_dir/gitea/files" \
      --password-file "$home/secrets/realm/REHEARSAL_APP_ROLES_PASSWORD"
  fi

  if [ -s "$export_dir/apps/builds.json" ]; then
    nohup python3 -I "$here/apps.py" publish --base "http://localhost:$port" --export "$export_dir" \
      --token-file "$JCBIN_SHAKO_TOKEN_FILE" --report "$home/apps-publish.json" > "$home/apps-publish.log" 2>&1 &
  fi
fi
