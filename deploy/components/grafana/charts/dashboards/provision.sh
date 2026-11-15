#!/bin/bash
# Gives each project its Grafana organization, folder, datasources and dashboard (T-6504). The
# postStart hook of the Grafana container runs this: Grafana keeps organizations in a database
# that starts empty with the pod, and its provisioning files refuse an organization that does not
# exist yet, so these go in through the local admin API on every start. Every step is safe to
# repeat: a container restart keeps the database and runs this again.
set -euo pipefail

api=${GRAFANA_API:-http://127.0.0.1:3000/grafana/api}
here=$(dirname "$0")
answer=$(mktemp)
# Name order, the one org_mapping numbers the organizations in (values/grafana/base-values).
export LC_ALL=C
expected=1

# The password stays out of argv: curl reads it from a descriptor.
call() { # method path [org] [data]; prints the HTTP status, keeps the body in $answer
  curl -sS -o "$answer" -w '%{http_code}' -K <(printf 'user = "%s:%s"\n' "$GF_SECURITY_ADMIN_USER" "$GF_SECURITY_ADMIN_PASSWORD") \
    -X "$1" "$api$2" -H 'Content-Type: application/json' ${3:+-H "X-Grafana-Org-Id: $3"} ${4:+--data-binary "$4"}
}

refused() {
  echo "provision: $1 answered $2: $(head -c 300 "$answer")" >&2
  exit 1
}

for _ in $(seq 85); do
  curl -fsS -o /dev/null "$api/health" && break
  sleep 2
done

for folder in "$here"/*.folder.json; do
  project=$(basename "$folder" .folder.json)

  status=$(call POST /orgs "" "{\"name\":\"$project\"}")
  case $status in
    200) ;;
    409) status=$(call GET "/orgs/name/$project")
         [ "$status" = 200 ] || refused "the organization $project" "$status" ;;
    *) refused "creating the organization $project" "$status" ;;
  esac
  org=$(grep -oE '"(orgId|id)":[0-9]+' "$answer" | head -1 | cut -d: -f2)
  expected=$((expected + 1))
  if [ "$org" != "$expected" ]; then
    echo "provision: organization $project is $org, org_mapping says $expected: people would land in another project's" >&2
    exit 1
  fi

  for source in "$here/$project".datasource.*.json; do
    status=$(call POST /datasources "$org" "@$source")
    if [ "$status" = 409 ]; then
      uid=$(basename "$source" .json)
      status=$(call PUT "/datasources/uid/${uid#"$project".datasource.}" "$org" "@$source")
    fi
    [ "$status" = 200 ] || refused "datasource $source" "$status"
  done

  # An existing folder answers 409 or 412, depending on which of its uid and title matched.
  status=$(call POST /folders "$org" "@$folder")
  case $status in 200 | 409 | 412) ;; *) refused "folder $project" "$status" ;; esac

  status=$(call POST /dashboards/db "$org" "@$here/$project.dashboard.json")
  [ "$status" = 200 ] || refused "dashboard $project" "$status"
done
echo "provision: every project is in place"
