# shellcheck shell=bash
# shellcheck disable=SC2154  # env_name, home, pg_port, origin, bind, port, jc_bin, part, profile_line, portal_mounts and realm_config are up.sh's
# split.sh, sourced by up.sh when REHEARSAL_LAYOUT=split: the rehearsal as separate services, one
# jc process per role group, as a deployment that scales them runs it.
#
#   front   nginx in docker on the public <bind>:<port>, the one origin: /git, /dashboards,
#           /opendata and /streams to their module's process, everything else to core
#   core    kaisatsu, antares, gateway, portal, model-tools, assistant, viz; REHEARSAL_CORE_REPLICAS
#           (default 1) processes on the same jc.toml and state dir, the broker on the NATS bus
#   data    with REHEARSAL_DATA_REPLICAS=1-9 (T-7226): gateway and antares leave core for that many
#           processes of `--role gateway,antares` (the data plane, `[gateway]`), each with a state
#           dir of its own; the front sends the gateway's root paths and /broker to them in turn,
#           core's Portal calls the broker through `[remote]` and names the gateway there
#   shako, daiya, dataroom, ekiben (ekiben not with REHEARSAL_HOLD_PIPELINES=1): one process each,
#           behind `[door]`; core's Portal calls them through `[remote]`
#   nats    NATS with JetStream in docker on loopback, user and password from a 0600 file
#
# Why the gateway and the broker stayed in core before T-7226 (T-7014; worker-t7007's module table in T-7008): the
# gateway reaches the broker on the in-process hop alone (modules/gateway.rs vars
# JC_GATEWAY_BROKER_URL, app.rs `hop_secret` only with both roles), verifies persons only with
# kaisatsu and the Portal in its process (gateway.rs vars, `portal_internal`), and reads a Portal
# session through PORTAL_STATE (gateway.rs session_reads). So core scaled as a whole: two core
# replicas are two brokers and two gateways. The data plane (T-7226) reaches the broker on its own
# hop, verifies persons against core's kaisatsu (`[gateway] kaisatsu`), reads revocations,
# connectors, usage and Portal sessions on core replica 1's internal listener with its
# `context-gateway` token, and follows the forge through `[remote.modules] shako`.
#
# Every cross-process call carries a kaisatsu token: the realm jc-split (state/realms) holds the
# client portal-remote, allowed one scope per remote module whose audience mapper names it; each
# module's door admits that client's token for its own audience alone (bins/jc/src/door.rs,
# tests/split_role.rs). No secret reaches argv: the client secret, the NATS password and the
# database passwords stay in 0600 files; the NATS URL with its password is in core's environment.
# Ports (loopback): core <pg_port>+400.., modules +410.., their admin listeners +200.. and +210..,
# NATS +500 (prometheus.sh holds +300 and +301).

split=$home/split
mkdir -p "$split"
chmod 700 "$split"
if [ -n "${REHEARSAL_ROLE:-}" ]; then
  echo "split.sh: REHEARSAL_ROLE names one process's roles; the split layout places every module itself" >&2
  exit 1
fi
if [ -n "${REHEARSAL_JC_TOML_EXTRA:-}" ]; then
  # [[organizations]] hosts name dataroom's and the gateway's hosts, which a front by path alone
  # does not route (host mode needs a front by host too).
  echo "split.sh: host mode (REHEARSAL_JC_TOML_EXTRA) is not in the split layout; run it on one jc" >&2
  exit 1
fi
replicas=${REHEARSAL_CORE_REPLICAS:-1}
case $replicas in [1-9]) ;; *) echo "split.sh: REHEARSAL_CORE_REPLICAS is 1-9" >&2; exit 1 ;; esac
data_replicas=${REHEARSAL_DATA_REPLICAS:-0}
case $data_replicas in [0-9]) ;; *) echo "split.sh: REHEARSAL_DATA_REPLICAS is 0-9" >&2; exit 1 ;; esac
remote_modules=(shako daiya dataroom)
[ "${REHEARSAL_HOLD_PIPELINES:-0}" = 1 ] || remote_modules+=(ekiben)
core_roles=kaisatsu,antares,gateway,portal,model-tools,assistant,viz
# The scopes of the realm jc-split: the remote modules, and the broker when it is in the data plane.
door_modules=("${remote_modules[@]}")
if [ "$data_replicas" -gt 0 ]; then
  core_roles=kaisatsu,portal,model-tools,assistant,viz
  door_modules+=(antares)
fi
# The gateway's root paths (bins/jc/src/module.rs `root_paths`) and the broker's slug.
data_paths=(/api/endpoint /api/endpoints /api/mcp /cs /catalog /catalog.jsonld /catalog.ttl /oauth
  /.well-known/oauth-authorization-server /.well-known/oauth-protected-resource/api/mcp
  /.well-known/oauth-protected-resource/api/endpoint /broker)
declare -A slug=([shako]=git [daiya]=dashboards [dataroom]=opendata [ekiben]=streams)
declare -A mport
i=0
for m in "${remote_modules[@]}"; do mport[$m]=$((pg_port + 410 + i)); i=$((i + 1)); done

# NATS, JetStream on, loopback only, one user.
nats_image=${REHEARSAL_NATS_IMAGE:-nats:2.10-alpine@sha256:b83efabe3e7def1e0a4a31ec6e078999bb17c80363f881df35edc70fcb6bb927}
nats_port=$((pg_port + 500))
nats_pw=$(cat "$(secret nats-pw)")
(umask 077; printf 'listen: "127.0.0.1:%s"\njetstream { store_dir: "/data" }\nauthorization { user: "jc", password: "%s" }\n' \
  "$nats_port" "$nats_pw" > "$split/nats.conf")
mkdir -p "$split/nats-data"
if ! docker inspect -f '{{.State.Running}}' "rh-nats-$env_name" 2>/dev/null | grep -q true; then
  docker rm -f "rh-nats-$env_name" >/dev/null 2>&1 || true
  docker run -d --name "rh-nats-$env_name" --network host --user "$(id -u):$(id -g)" --read-only \
    -v "$split/nats.conf":/etc/nats/nats.conf:ro -v "$split/nats-data":/data \
    "$nats_image" -c /etc/nats/nats.conf >/dev/null
fi
for _ in $(seq 30); do (exec 3<>"/dev/tcp/127.0.0.1/$nats_port") 2>/dev/null && break; sleep 1; done
(exec 3<>"/dev/tcp/127.0.0.1/$nats_port") 2>/dev/null || { echo "split.sh: NATS did not listen on $nats_port" >&2; exit 1; }

# The realm of the cross-process calls, next to the deployment's in kaisatsu's import dir (written
# after apps.sh, which takes the first realm file there for the deployment's).
mkdir -p "$home/secrets/realm"
chmod 700 "$home/secrets/realm"
secret realm/portal-remote >/dev/null
python3 -I - "$home/state/realms/jc-split.json" "${door_modules[@]}" <<'PY'
import json, sys
out, mods = sys.argv[1], sys.argv[2:]
scope = lambda m: {"name": m, "protocol": "openid-connect",
    "attributes": {"include.in.token.scope": "true"},
    "protocolMappers": [{"name": f"{m}-audience", "protocol": "openid-connect",
        "protocolMapper": "oidc-audience-mapper", "consentRequired": False,
        "config": {"access.token.claim": "true", "id.token.claim": "false",
                   "included.custom.audience": m}}]}
realm = {"realm": "jc-split", "enabled": True, "clientScopes": [scope(m) for m in mods],
    "clients": [{"clientId": "portal-remote", "enabled": True, "protocol": "openid-connect",
        "publicClient": False, "clientAuthenticatorType": "client-secret",
        "secret": "$(file:portal-remote)", "standardFlowEnabled": False,
        "directAccessGrantsEnabled": False, "serviceAccountsEnabled": True,
        "defaultClientScopes": [], "optionalClientScopes": mods}]}
with open(out, "w") as f:
    json.dump(realm, f, indent=1)
PY
# kaisatsu answers at /login since T-6992; /id answers 308 there, which no token client follows.
split_realm="$origin/login/realms/jc-split"

door=$(printf '\n[door]\nissuer = "%s"\njwks_url = "%s/protocol/openid-connect/certs"\nclients = ["portal-remote"]\n' \
  "$split_realm" "$split_realm")
for m in "${remote_modules[@]}"; do
  {
    printf '%s\n%s\n%s\n' "$profile_line" "${part[$m]}" "$door"
    if [ "$m" = dataroom ]; then
      # Where the catalogue's live parts answer: in the one layout jc takes them from the gateway,
      # Portal and viz beside dataroom; apart, from here (the harvest reads Endpoints at api).
      printf '\n[dataroom.addresses]\napi = "%s"\nportal = "%s"\nviz = "%s/embed"\n' "$origin" "$origin" "$origin"
      dataroom_tail "$split"
    fi
  } > "$split/$m.toml"
done
{
  printf '%s\n\n'"$portal_mounts"'%s\n%s\n' "$profile_line" "${part[antares]}" "$realm_config"
  printf '\n[remote]\ntoken_url = "%s/protocol/openid-connect/token"\nclient_id = "portal-remote"\n' "$split_realm"
  printf 'client_secret_file = "%s"\n\n[remote.modules]\n' "$home/secrets/realm/portal-remote"
  for m in "${remote_modules[@]}"; do printf '%s = "%s/%s"\n' "$m" "$origin" "${slug[$m]}"; done
  if [ "$data_replicas" -gt 0 ]; then
    # The broker through the front's round robin; the gateway at replica 1 (a Service in a cluster).
    printf 'antares = "%s/broker"\ngateway = "http://127.0.0.1:%s"\n' "$origin" $((pg_port + 420))
  fi
} > "$split/core.toml"
if [ "$data_replicas" -gt 0 ]; then
  # The gateway's client secret both sides take, and the one OAuth signing key of every data replica.
  # An imported realm names the context-gateway client's secret itself (KC_CONTEXT_GATEWAY_SECRET in
  # the realm secrets dir): kaisatsu holds that one, so the data plane's token grant needs it too.
  gateway_secret=$home/secrets/realm/KC_CONTEXT_GATEWAY_SECRET
  [ -s "$gateway_secret" ] || gateway_secret=$(secret gateway-client-secret)
  {
    printf '%s\n%s\n%s\n%s\n' "$profile_line" "${part[antares]}" "$realm_config" "$door"
    printf '\n[gateway]\nportal_internal = "http://127.0.0.1:%s"\n' $((pg_port + 100))
    printf 'kaisatsu = "%s/login"\nkaisatsu_internal = "http://127.0.0.1:%s/login"\n' "$origin" $((pg_port + 400))
    printf 'client_secret_file = "%s"\noauth_key_file = "%s"\n' "$gateway_secret" "$(secret gateway-oauth-key)"
    printf '\n[remote]\ntoken_url = "%s/protocol/openid-connect/token"\nclient_id = "portal-remote"\n' "$split_realm"
    printf 'client_secret_file = "%s"\n\n[remote.modules]\nshako = "%s/git"\n' "$home/secrets/realm/portal-remote" "$origin"
  } > "$split/data.toml"
fi

start_jc() { # start_jc <name> <roles> <toml> <listen port> <admin port> <log> <pid file> [jc args]
  local name=$1 roles=$2 toml=$3 lport=$4 aport=$5 log=$6 pid_file=$7
  shift 7
  nohup "$jc_bin" --role "$roles" --scheme http --log-format text --config "$toml" \
    --listen "127.0.0.1:$lport" --base-host "${origin#http://}" \
    --admin-listen "127.0.0.1:$aport" --admin-token-file "$(secret metrics-token)" "$@" \
    > "$log" 2>&1 &
  echo $! > "$pid_file"
  echo "$env_name: $name ($roles) on 127.0.0.1:$lport, log $log"
}
i=0
for m in "${remote_modules[@]}"; do
  args=(--state-dir "$split/$m-state")
  case $m in
    shako) args+=(--shako-repo-root "$home/repos") ;;
    ekiben) args+=(--ekiben-file-root "$home/files" --ekiben-client-secrets-dir "$home/secrets/realm") ;;
  esac
  start_jc "$m" "$m" "$split/$m.toml" "${mport[$m]}" $((pg_port + 210 + i)) "$split/$m.log" "$split/$m.pid" "${args[@]}"
  i=$((i + 1))
done

# The front: the public origin, by path.
front_image=${REHEARSAL_FRONT_IMAGE:-nginx:1.27-alpine@sha256:65645c7bb6a0661892a8b03b89d0743208a18dd2f3f17a54ef4b76fb8e2f2a10}
{
  printf 'worker_processes 2;\npid /tmp/nginx.pid;\nerror_log stderr warn;\nevents { worker_connections 4096; }\nhttp {\n'
  printf '  access_log off;\n  client_max_body_size 0;\n  client_body_temp_path /tmp/body;\n  proxy_temp_path /tmp/proxy;\n'
  printf '  fastcgi_temp_path /tmp/fcgi;\n  uwsgi_temp_path /tmp/uwsgi;\n  scgi_temp_path /tmp/scgi;\n'
  printf '  proxy_http_version 1.1;\n  proxy_buffering off;\n  proxy_request_buffering off;\n  proxy_read_timeout 600s;\n'
  printf '  map $http_upgrade $connection_upgrade { default upgrade; "" close; }\n'
  printf '  upstream core {\n'
  for r in $(seq "$replicas"); do printf '    server 127.0.0.1:%s;\n' $((pg_port + 399 + r)); done
  printf '    keepalive 32;\n  }\n'
  if [ "$data_replicas" -gt 0 ]; then
    printf '  upstream data {\n'
    for r in $(seq "$data_replicas"); do printf '    server 127.0.0.1:%s;\n' $((pg_port + 419 + r)); done
    printf '    keepalive 32;\n  }\n'
  fi
  printf '  server {\n    listen %s:%s;\n    proxy_set_header Host $http_host;\n' "$bind" "$port"
  printf '    proxy_set_header Upgrade $http_upgrade;\n    proxy_set_header Connection $connection_upgrade;\n'
  for m in "${remote_modules[@]}"; do
    printf '    location = /%s { proxy_pass http://127.0.0.1:%s; }\n' "${slug[$m]}" "${mport[$m]}"
    printf '    location /%s/ { proxy_pass http://127.0.0.1:%s; }\n' "${slug[$m]}" "${mport[$m]}"
  done
  if [ "$data_replicas" -gt 0 ]; then
    for p in "${data_paths[@]}"; do
      printf '    location = %s { proxy_pass http://data; }\n    location %s/ { proxy_pass http://data; }\n' "$p" "$p"
    done
  fi
  printf '    location / { proxy_pass http://core; }\n  }\n}\n'
} > "$split/front.conf"
docker rm -f "rh-front-$env_name" >/dev/null 2>&1 || true
docker run -d --name "rh-front-$env_name" --network host --user 101:101 --read-only --tmpfs /tmp \
  -v "$split/front.conf":/etc/nginx/nginx.conf:ro "$front_image" >/dev/null

# core, replica 1 on jc.log/jc.pid as the one layout (import.sh, the smoke and memwatch read them).
nats_url="nats://jc:$nats_pw@127.0.0.1:$nats_port"
core_args=(--state-dir "$home/state" --kaisatsu-import-dir "$home/state/realms"
  --kaisatsu-import-secrets-dir "$home/secrets/realm"
  --kaisatsu-database-url-file "$home/secrets/kaisatsu-db-url"
  --kaisatsu-db-password-file "$(secret kaisatsu-app-pw)")
# kaisatsu's `context-gateway` client takes the secret the data plane holds (`[gateway]`).
[ "$data_replicas" = 0 ] || export JCBIN_GATEWAY_CLIENT_SECRET_FILE=$gateway_secret
for r in $(seq "$replicas"); do
  if [ "$r" = 1 ]; then log=$home/jc.log pid_file=$home/jc.pid; else log=$split/core-$r.log pid_file=$split/core-$r.pid; fi
  ANTARES_BUS=nats ANTARES_NATS_URL=$nats_url JC_INTERNAL_BIND=127.0.0.1:$((pg_port + 99 + r)) \
    start_jc "core-$r" "$core_roles" "$split/core.toml" $((pg_port + 399 + r)) $((pg_port + 199 + r)) \
    "$log" "$pid_file" "${core_args[@]}"
  # One replica at a time: the first start makes the state dir's generated secrets the next reads.
  "$here/wait-jc.sh" "$(cat "$pid_file")" "$log" || exit 1
done
unset JCBIN_GATEWAY_CLIENT_SECRET_FILE
for m in "${remote_modules[@]}"; do
  "$here/wait-jc.sh" "$(cat "$split/$m.pid")" "$split/$m.log" || exit 1
done
# The data plane after core: it takes its tokens from core's kaisatsu and its revocations from
# core's Portal. One replica at a time, as core: the first runs the broker's migrations.
for r in $(seq "$data_replicas"); do
  # The broker's admin token is the installation's, as one Secret gives every pod: core's Portal
  # keeps it in its state, the smoke and notify_once.py read it there. Without the Portal beside
  # it a broker holds a fresh one per boot in memory, which nobody else knows.
  JCBIN_ANTARES_ADMIN_TOKEN_FILE=$home/state/antares-admin-token ANTARES_BUS=nats ANTARES_NATS_URL=$nats_url start_jc "data-$r" gateway,antares "$split/data.toml" \
    $((pg_port + 419 + r)) $((pg_port + 219 + r)) "$split/data-$r.log" "$split/data-$r.pid" \
    --state-dir "$split/data-$r-state"
  "$here/wait-jc.sh" "$(cat "$split/data-$r.pid")" "$split/data-$r.log" || exit 1
done
unset nats_pw nats_url
echo "$env_name: split layout on $origin: core x$replicas, data x$data_replicas, ${remote_modules[*]} (front rh-front-$env_name, NATS rh-nats-$env_name)"
