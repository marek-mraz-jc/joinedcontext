# shellcheck shell=bash
# shellcheck disable=SC2154  # env_name, home, pg_port and here are up.sh's
# Sourced by up.sh: the rehearsal's Prometheus, read the way jc-prod reads dev's (deployment
# components/jc metricsProxy). Prometheus scrapes jc's admin listener (its token from the 0600
# metrics-token file) under namespace="jc-prod" and itself under namespace="monitoring";
# prom-label-proxy pins every query to namespace="jc-prod"; daiya's datasource `prometheus`
# (organization 1) points at the proxy, the one address `[daiya] prometheus_reach` opens.
# REHEARSAL_PROMETHEUS=off leaves all of it out (the dashboards' Prometheus panels then skip).
# Ports: Prometheus on <postgres port>+300, the proxy on +301, both on 127.0.0.1 only.
# Reads: env_name, home, pg_port, here, secret(); appends to daiya_lines.

if [ "${REHEARSAL_PROMETHEUS:-on}" != off ]; then
  # The pins of the deployment (components/prometheus/images.yaml, components/jc/images.yaml).
  prom_image=${REHEARSAL_PROM_IMAGE:-quay.io/prometheus/prometheus:v3.15.0-distroless@sha256:b2a413d5a03ea6a76782a508d1c7947440bba3b973931a25676e278431891b01}
  proxy_image=${REHEARSAL_PROM_PROXY_IMAGE:-quay.io/prometheuscommunity/prom-label-proxy:v0.16.2@sha256:9812cce38fa78c39250c9c59c78bbe75895ef9126952a098cd44adef2e21dbc5}
  prom_port=$((pg_port + 300))
  proxy_port=$((pg_port + 301))
  mkdir -p "$home/prometheus" "$home/daiya-provisioning/datasources"
  python3 -I "$here/rehearsal.py" prometheus-config --admin "127.0.0.1:$((pg_port + 200))" \
    --self "127.0.0.1:$prom_port" --proxy "127.0.0.1:$proxy_port" \
    --config "$home/prometheus/prometheus.yml" \
    --datasource "$home/daiya-provisioning/datasources/prometheus.yaml"
  for c in "rh-prom-$env_name" "rh-promproxy-$env_name"; do docker rm -f "$c" >/dev/null 2>&1 || true; done
  # --network host as rh-pg (the per-task dind bridge gives no reachable address); the TSDB on a
  # tmpfs (history starts at each up.sh); the user of this shell, so the 0600 token file reads.
  docker run -d --name "rh-prom-$env_name" --network host --user "$(id -u):$(id -g)" \
    --read-only --tmpfs /prometheus:size=512m --memory 512m \
    -v "$home/prometheus/prometheus.yml":/etc/rh/prometheus.yml:ro \
    -v "$(secret metrics-token)":/etc/rh/token:ro \
    "$prom_image" --config.file=/etc/rh/prometheus.yml --storage.tsdb.path=/prometheus \
    --storage.tsdb.retention.time=2d --web.listen-address="127.0.0.1:$prom_port" >/dev/null
  docker run -d --name "rh-promproxy-$env_name" --network host --read-only --memory 64m \
    "$proxy_image" --insecure-listen-address="127.0.0.1:$proxy_port" \
    --upstream="http://127.0.0.1:$prom_port" --label=namespace --label-value=jc-prod \
    --enable-label-apis >/dev/null
  case $daiya_lines in
    *provisioning_dir*) ;;
    *) daiya_lines+="provisioning_dir = \"$home/daiya-provisioning\""$'\n' ;;
  esac
  daiya_lines+="prometheus_reach = \"http://127.0.0.1:$proxy_port\""$'\n'
  echo "$env_name: Prometheus on 127.0.0.1:$prom_port, its label proxy (namespace=jc-prod) on 127.0.0.1:$proxy_port"
fi
