#!/usr/bin/env bash
#
# This work is licensed under [EU PL 1.2]
# (https://gitlab.com/civitas-connect/civitas-core/civitas-core/-/blob/main/LICENSE)
# by Civitas Connect e. V., Hafenweg 7, 48155 Münster, Germany, and [other authors].
#
# You may not use this work except in compliance with the Licence.
#
# -----------------------------------------------------------------------------
# Deploy every deployment variant into its own fresh k3d dev cluster, one after
# another, and assert after each deployment that the smoke test passes:
#   * all pods are green (Running+Ready, or Completed), and
#   * every Deployment/StatefulSet/DaemonSet rolled out successfully.
#
# Variant axes (each ja/nein):
#   * multi-namespace  -> global.singleNamespace        (nein=true / ja=false)
#   * multi-instance   -> two-layer operators+instance  vs. all-in-one helmfile
#   * linkerd          -> global.serviceMesh.enable     (+ Linkerd control plane)
#
# A variant is encoded as a triplet  MNS,MI,LNK  of 0/1, e.g.  1,0,1
# (multi-namespace=yes, multi-instance=no, linkerd=yes). With no triplet given
# all 8 combinations run.
#
#   ./scripts/test-deployment-variants.sh                 # all 8 variants
#   ./scripts/test-deployment-variants.sh 0,0,0 1,1,1     # only these two
#   ./scripts/test-deployment-variants.sh --list          # show the matrix
# -----------------------------------------------------------------------------

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# --- defaults ---------------------------------------------------------------
# Scratch helmfile environment the harness OWNS: it generates this environment's
# global.yaml.gotmpl per variant with the toggle values. CLI --state-values-set is
# NOT forwarded to the components by the entrypoints, so the toggles must live in
# the environment's values file (which helmfile-root loads directly).
ENV="testing"
PROFILE="development"  # global.profile written into the scratch env (development|production)
CLUSTER_NAME="jc-local"
TIMEOUT="900"          # per-variant wait budget for pods+rollout (seconds)
MANAGE_CLUSTER=true    # recreate a fresh k3d cluster per variant
KEEP_CLUSTER=false     # keep the last cluster around after the run
CONTINUE_ON_FAILURE=false  # on failure, tear down and proceed to the next variant
USE_REGISTRY_CACHE=true    # start the local pull-through image caches first
LIST_ONLY=false
AFTER=""               # a command run on each variant after its smoke, then the smoke again
COMPONENTS="full"      # componentProfile written into the scratch env (full|minimal|broker-only, T-4856)
FOOTPRINT=""           # a file each passing variant appends its measured footprint row to
SAMPLER_PID=""
SELECTED=()

# --- colors -----------------------------------------------------------------
if [[ -t 1 ]]; then
  RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'; BLUE='\033[0;34m'; NC='\033[0m'
else
  RED=''; GREEN=''; YELLOW=''; BLUE=''; NC=''
fi
log()  { echo -e "${BLUE}==>${NC} $*"; }
ok()   { echo -e "${GREEN}✓${NC} $*"; }
warn() { echo -e "${YELLOW}!${NC} $*"; }
err()  { echo -e "${RED}✗${NC} $*" >&2; }

usage() {
  cat <<EOF
Deploy every deployment variant into its own fresh k3d dev cluster, one after
another, and smoke-test it (all pods green + all workloads rolled out).

Variant axes (each ja/nein):
  multi-namespace  -> global.singleNamespace        (true / false)
  multi-instance   -> two-layer operators+instance  vs. all-in-one helmfile
  linkerd          -> global.serviceMesh.enable     (+ Linkerd control plane)

A variant is a triplet  MNS,MI,LNK  of 0/1, e.g.  1,0,1
(multi-namespace=yes, multi-instance=no, linkerd=yes). No triplet => all 8.

Usage: $(basename "$0") [options] [MNS,MI,LNK ...]

Options:
  -e, --env <env>     Scratch helmfile environment to generate & deploy
                      (default: ${ENV}). Refuses committed envs (local/…).
      --profile <p>   global.profile written into the scratch env: development
                      or production (default: ${PROFILE}). production also
                      installs the Prometheus Operator CRDs the prod values need.
      --timeout <s>   Wait budget per variant in seconds (default: ${TIMEOUT})
      --keep          Do not delete the last k3d cluster after the run
      --continue-on-failure
                      Tear down and proceed to the next variant on failure.
                      Default: stop on first failure and keep the cluster up
                      for inspection (k3d uses host ports 80/443, so only one
                      cluster can exist at a time).
      --no-registry-cache
                      Do not start the local pull-through image caches
                      (dev-deployment/registry-cache.sh). On by default.
      --no-cluster    Do not create/delete clusters; deploy into the current
                      kubectl context and clean platform namespaces between runs
      --after <cmd>   After a variant's smoke passes, run <cmd> on it (JC_ENV names the
                      scratch environment, still in place) and then the smoke again: an
                      operation the deployment must survive, such as a rotation (T-2843)
      --components <p>
                      componentProfile written into the scratch env: full,
                      minimal or broker-only (default: ${COMPONENTS}). broker-only
                      installs no Kyverno and smokes the broker over NGSI-LD
                      instead of the edge (T-4856).
      --footprint <file>
                      Append each passing variant's measured footprint (peak and
                      settled memory and CPU of its pods, requests, limits, PVCs)
                      to <file> as a Markdown table row; GITHUB_STEP_SUMMARY too
      --list          Print the selected variant matrix and exit
  -h, --help          Show this help
EOF
}

# --- arg parsing ------------------------------------------------------------
while [[ $# -gt 0 ]]; do
  case "$1" in
    -e|--env)     ENV="$2"; shift 2 ;;
    --profile)    PROFILE="$2"; shift 2 ;;
    --timeout)    TIMEOUT="${2%s}"; shift 2 ;;
    --keep)       KEEP_CLUSTER=true; shift ;;
    --continue-on-failure) CONTINUE_ON_FAILURE=true; shift ;;
    --no-registry-cache) USE_REGISTRY_CACHE=false; shift ;;
    --no-cluster) MANAGE_CLUSTER=false; shift ;;
    --after)      AFTER="${2:?--after needs a command}"; shift 2 ;;
    --components) COMPONENTS="${2:?--components needs a profile}"; shift 2 ;;
    --footprint)  FOOTPRINT="${2:?--footprint needs a file}"; shift 2 ;;
    --list)       LIST_ONLY=true; shift ;;
    -h|--help)    usage; exit 0 ;;
    all)          SELECTED=(); shift ;;
    [01],[01],[01]) SELECTED+=("$1"); shift ;;
    *) err "unknown argument: $1"; echo; usage; exit 1 ;;
  esac
done

case "$PROFILE" in
  development|production) ;;
  *) err "--profile must be 'development' or 'production' (got '$PROFILE')"; exit 1 ;;
esac
case "$COMPONENTS" in
  full|minimal|broker-only) ;;
  *) err "--components must be 'full', 'minimal' or 'broker-only' (got '$COMPONENTS')"; exit 1 ;;
esac
BROKER_ONLY=false; [[ "$COMPONENTS" == "broker-only" ]] && BROKER_ONLY=true

# Build the variant matrix (default: all 8 combinations).
COMBOS=()
if [[ ${#SELECTED[@]} -gt 0 ]]; then
  COMBOS=("${SELECTED[@]}")
else
  for mns in 0 1; do for mi in 0 1; do for lnk in 0 1; do
    COMBOS+=("${mns},${mi},${lnk}")
  done; done; done
fi

yn() { [[ "$1" == "1" ]] && echo "ja " || echo "nein"; }
describe() { # MNS MI LNK -> human readable
  printf "multi-ns=%s  multi-instance=%s  linkerd=%s  components=%s" "$(yn "$1")" "$(yn "$2")" "$(yn "$3")" "$COMPONENTS"
}

if $LIST_ONLY; then
  log "Variant matrix (env=${ENV}):"
  for c in "${COMBOS[@]}"; do IFS=',' read -r mns mi lnk <<<"$c"; echo "  ${c}   $(describe "$mns" "$mi" "$lnk")"; done
  exit 0
fi

# --- environment overlay (where the toggles actually take effect) -----------
# Refuse to overwrite a committed environment's config.
case "$ENV" in
  local|production|nl-dev)
    err "--env '$ENV' is a committed environment; the harness generates the env file and would overwrite it."
    err "Use a scratch env (default: testing)."
    exit 1 ;;
esac
ENV_FILE="deployment/environments/${ENV}/global.yaml.gotmpl"
if [[ -e "$ENV_FILE" ]] && git ls-files --error-unmatch "$ENV_FILE" >/dev/null 2>&1; then
  err "$ENV_FILE is tracked by git; refusing to overwrite. Pick a different scratch env."
  exit 1
fi
# Remove the generated scratch env file on exit so the repo is left clean.
cleanup_env() { rm -f "$ENV_FILE"; rmdir "deployment/environments/${ENV}" 2>/dev/null || true; }
trap '[[ -n "$SAMPLER_PID" ]] && kill "$SAMPLER_PID" 2>/dev/null; cleanup_env' EXIT

# --- tooling pre-flight -----------------------------------------------------
need=(helmfile kubectl helm yq just)
# curl only needed for the managed-cluster HTTP ingress smoke (known 80/443 mapping).
$MANAGE_CLUSTER && need+=(k3d curl)
# linkerd CLI only required if any selected variant enables the mesh
if grep -q ',1$' < <(printf '%s\n' "${COMBOS[@]}"); then need+=(linkerd); fi
missing=()
for t in "${need[@]}"; do command -v "$t" >/dev/null 2>&1 || missing+=("$t"); done
if [[ ${#missing[@]} -gt 0 ]]; then err "missing required tools: ${missing[*]}"; exit 1; fi

KCTX=""   # active kube context, set per variant

# --- instance slug (namespace base) -----------------------------------------
get_slug() {
  local f="deployment/environments/${ENV}/global.yaml.gotmpl" s=""
  [[ -f "$f" ]] && s="$(yq -r '.global.instanceSlug // ""' "$f" 2>/dev/null || true)"
  [[ -z "$s" || "$s" == "null" ]] && s="$(yq -r '.global.instanceSlug' defaults/environment/global.yaml)"
  echo "$s"
}

# --- cluster lifecycle ------------------------------------------------------
# Start the local pull-through image caches once. They persist across cluster
# recreations and are wired into k3d-jc-local.yaml, so fresh clusters pull
# from the local cache instead of re-pulling every image from the internet.
ensure_registry_cache() {
  local script="dev-deployment/registry-cache.sh"
  if [[ ! -x "$script" ]]; then
    warn "registry cache script not found ($script) — continuing without it"
    return 0
  fi
  if ! command -v docker >/dev/null 2>&1; then
    warn "docker not available — skipping registry cache"
    return 0
  fi
  log "Starting local pull-through image caches"
  "$script" up || warn "registry cache failed to start — clusters will pull from upstream"
}

create_cluster() {
  log "Recreating fresh k3d cluster '${CLUSTER_NAME}' (+ ingress/metallb/cert-manager)"
  k3d cluster delete "$CLUSTER_NAME" >/dev/null 2>&1 || true
  local certarg=""
  [[ -f dev-deployment/.ssl/jc.crt && -f dev-deployment/.ssl/jc.key ]] || certarg="cert"
  # Checked explicitly, because `set -e` does NOT apply inside a function invoked as
  # `create_cluster || return 1` -- bash suppresses it for the whole body, so a failing
  # startup.sh would fall through to the two lines below and the function would return the
  # status of `kubectl config use-context`, i.e. success. That is how a half-built cluster
  # (MetalLB webhook race, no cert-manager) got six minutes further and then failed on a
  # missing CRD instead of failing here in one (T-0350).
  if ! ./dev-deployment/startup.sh -k $certarg; then
    err "cluster bootstrap failed — see startup.sh output above"
    return 1
  fi
  KCTX="k3d-${CLUSTER_NAME}"
  kubectl config use-context "$KCTX" >/dev/null
}

# The production profile turns on CNPG's PodMonitor (and other ServiceMonitors),
# which need the Prometheus Operator CRDs to exist or the apply/reconcile fails.
# Install just the CRDs (server-side; the bundle is large) into the fresh cluster.
PROM_OPERATOR_VERSION="v0.78.2"
install_monitoring_crds() {
  log "Installing Prometheus Operator CRDs (${PROM_OPERATOR_VERSION}) for production profile"
  kubectl apply --server-side -f \
    "https://github.com/prometheus-operator/prometheus-operator/releases/download/${PROM_OPERATOR_VERSION}/stripped-down-crds.yaml"
}

install_linkerd() {
  log "Installing Linkerd control plane"
  linkerd check --pre
  kubectl apply --server-side -f \
    https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.4.0/standard-install.yaml
  linkerd install --crds | kubectl apply -f -
  linkerd install --set proxy.nativeSidecar=true | kubectl apply -f -
  linkerd check
}

# Mesh the nginx ingress controller so ingress->APISIX is mTLS — required
# whenever the ingress edge enforces mTLS (serviceMesh.allowUnauthenticatedIngress
# =false), which is the default in BOTH the development and production profiles.
# dev-deployment/startup.sh installs an UNMESHED ingress-nginx; without this, the APISIX
# data-plane Server (all-authenticated) rejects the plaintext ingress with a
# Linkerd 403, breaking every OIDC call routed through it (portal->keycloak).
#
# The ingress-nginx namespace does NOT carry the cluster-authenticated
# default-inbound-policy (the prepare hook only annotates platform namespaces),
# so the meshed controller keeps an all-unauthenticated inbound policy and still
# accepts external plaintext traffic from the metallb LoadBalancer on 80/443.
# service-upstream is enabled so nginx dials APISIX via its ClusterIP, letting
# Linkerd attach the destination's mesh identity for mTLS.
mesh_ingress_controller() {
  local ns="ingress-nginx"
  if ! kubectl get ns "$ns" >/dev/null 2>&1; then
    warn "namespace '$ns' not found — skipping ingress meshing"
    return 0
  fi
  log "Meshing the nginx ingress controller (ingress->APISIX mTLS)"
  kubectl annotate namespace "$ns" linkerd.io/inject=enabled --overwrite
  # 443 carries TLS the mesh must not try to parse. The Portal resolves the PUBLIC issuer
  # (idm.<domain>) through CoreDNS onto this very controller, so a meshed Portal sends its own
  # HTTPS to a meshed destination on 443; without this, Linkerd runs HTTP protocol detection on
  # a TLS stream and the connection is reset (`Connection reset by peer (os error 104)`), which
  # is what failed variant 0,0,1 while the unmeshed 0,0,0 passed. Opaque is not a weakening:
  # proxy-to-proxy mTLS still applies, only L7 detection and per-request routing are skipped.
  #
  # Marking the Service opaque as well did not keep the client hop alive: run 34784223904 had
  # Linkerd report protocol Opaque for the Service on 443 and still reset Gitea's OIDC discovery.
  # The client side is settled on the clients instead: every meshed pod that dials the public
  # host carries config.linkerd.io/skip-outbound-ports: "443" (T-0418).
  kubectl annotate namespace "$ns" config.linkerd.io/opaque-ports=443 --overwrite
  # Route to the backend ClusterIP so Linkerd can resolve the upstream identity.
  kubectl -n "$ns" patch configmap ingress-nginx-controller --type merge \
    -p '{"data":{"service-upstream":"true"}}' >/dev/null 2>&1 || true
  kubectl -n "$ns" rollout restart deploy
  kubectl -n "$ns" rollout status deploy --timeout=300s
  # Confirm the running controller pod actually came back meshed. The harness
  # installs Linkerd with proxy.nativeSidecar=true, so the linkerd-proxy is
  # injected as a native-sidecar *initContainer* (restartPolicy: Always), NOT a
  # regular container — scan BOTH lists. Scope to Running pods and scan every
  # listed pod so a lingering pre-restart pod can't mask the freshly injected one.
  local containers
  containers="$(kubectl -n "$ns" get pods \
    -l app.kubernetes.io/component=controller \
    --field-selector=status.phase=Running \
    -o jsonpath='{range .items[*]}{.spec.initContainers[*].name} {.spec.containers[*].name}{"\n"}{end}' 2>/dev/null)"
  if grep -q linkerd-proxy <<<"$containers"; then
    ok "ingress controller is meshed (containers: $(echo "$containers" | tr '\n' '|'))"
  else
    err "ingress controller is NOT meshed (containers: $(echo "$containers" | tr '\n' '|'))"
    return 1
  fi
}

# Kyverno must be running before the platform deploys, because the
# runtime-policies component (components/runtime-policies) applies cluster-scoped
# ClusterPolicy objects that need Kyverno's CRDs + admission webhook to exist.
# This is the same idempotent bootstrap the CI smoke test and `just kyverno` use.
install_kyverno() {
  log "Installing Kyverno (runtime ClusterPolicies prerequisite)"
  ./scripts/ci/install-kyverno-if-missing.sh || return 1
  # Wait for the webhook to be serving so the policy applies don't race it.
  kubectl -n kyverno rollout status deploy --timeout=300s
}

# Keycloak needs its keycloak-smtp secret to exist before its pod starts,
# otherwise the pod never becomes ready. Mirror `just deploy`.
create_smtp_secret() { # MNS
  local slug ns
  slug="$(get_slug)"
  if [[ "$1" == "1" ]]; then ns="${slug}-keycloak"; else ns="${slug}"; fi
  log "Pre-creating keycloak-smtp secret in namespace '${ns}'"
  kubectl create namespace "$ns" --dry-run=client -o yaml | kubectl apply -f -
  kubectl create secret generic keycloak-smtp -n "$ns" \
    --from-literal=host='smtp.example.com' \
    --from-literal=port='587' \
    --from-literal=from='noreply@example.com' \
    --from-literal=user='noreply@example.com' \
    --from-literal=password='YOUR_SMTP_PASSWORD' \
    --dry-run=client -o yaml | kubectl apply -f -
}

# --- deployment -------------------------------------------------------------
# Write the variant's toggles into the scratch environment's global override.
# This is the highest-precedence values file helmfile-root loads, so the toggles
# reliably reach every component (unlike --state-values-set, which the entrypoints
# do not forward). multi-instance is NOT set here — it is a deploy-time choice of
# entrypoint (operators+instance vs. all-in-one), not a global value.
# `deployment/` is gitignored: it is assembled from defaults/deployment plus the example
# environments, and a fresh checkout has no state file to sync at all. Every variant builds
# it here rather than assuming somebody ran `just dev-apply` first — which is exactly what a
# CI runner never did, and why this job spent a whole run reporting
# "specified state file ./deployment/helmfile.yaml is not found".
assemble_deployment() {
  log "Assembling deployment/ from defaults and the example environments"
  just _dev-assemble
}

write_env_overlay() { # MNS LNK
  local mns="$1" lnk="$2" single mesh
  single=$([[ "$mns" == "1" ]] && echo false || echo true)   # multi-ns => singleNamespace=false
  mesh=$([[ "$lnk" == "1" ]] && echo true || echo false)
  mkdir -p "$(dirname "$ENV_FILE")"
  cat > "$ENV_FILE" <<EOF
---
# Generated by scripts/test-deployment-variants.sh — DO NOT COMMIT.
componentProfile: ${COMPONENTS}
global:
  profile: ${PROFILE}
  domain: joinedcontext.test
  instanceSlug: dev
  initialUserEmail: admin@joinedcontext.test
  singleNamespace: ${single}
  # The layout dev and smartcity run (CC-85). In layout 1 the whole development seed is one
  # repository of more than a thousand tree entries, past the one page of its tree the Portal
  # reads, and the Portal never became ready on any variant (ci-full 39197981231, T-4709).
  configRepo:
    layout: 2
  ingress:
    # No backticks in this heredoc: EOF is unquoted so the variant toggles interpolate, which
    # means a backtick would run as a command substitution and land its output in the file.
    #
    # This flag gates exactly one thing in the repository: the apisix-edge Certificate that
    # fills the Secret the APISIX Ingress terminates on. The Ingress renders either way, so
    # with the flag off nothing errors -- ingress-nginx serves its built-in fake certificate,
    # the probes below still pass because curl --resolve never verifies a chain, and the first
    # component that does verify one dies with UnknownIssuer five releases later (T-0350).
    enabled: true
    # startup.sh installs cert-manager with the self-signed CA of dev-deployment/ca-template.yaml
    # (ClusterIssuer selfsigned-ca, its root in cert-manager/ca-secret). Naming it here gives
    # idm.joinedcontext.test a certificate that CA signed and lets the prepare component hand
    # the CA to every namespace, which is what Gitea's OIDC registration verifies the issuer
    # against; without it ingress-nginx serves its fake certificate and configure-gitea
    # crashloops on x509 (T-0651).
    clusterIssuer: selfsigned-ca
  serviceMesh:
    enable: ${mesh}
    type: linkerd
    patchNamespaces: ${mesh}
EOF
}

deploy_variant() { # MI
  local mi="$1"
  if [[ "$mi" == "1" ]]; then
    log "Deploying shared operators (multi-instance layer)"
    helmfile -f ./deployment/helmfile-operators.yaml sync -e "$ENV"
    log "Deploying instance"
    helmfile -f ./deployment/helmfile-instance.yaml.gotmpl sync -e "$ENV"
  else
    log "Deploying all-in-one"
    helmfile -f ./deployment/helmfile.yaml sync -e "$ENV"
  fi
}

# --- smoke test: namespaces, pods green, rollout ----------------------------
# Platform namespaces = whatever appeared since the snapshot taken before deploy.
snapshot_namespaces() {
  kubectl get ns -o name | sed 's#namespace/##' | sort
}

# Print every pod in the given namespaces that is NOT green.
not_green_pods() { # ns...
  local ns
  for ns in "$@"; do
    kubectl get pods -n "$ns" --no-headers 2>/dev/null | awk -v ns="$ns" '
      {
        ready=$2; status=$3
        if (status=="Completed" || status=="Succeeded") next
        split(ready, a, "/")
        if (status=="Running" && a[2]>0 && a[1]==a[2]) next
        printf "  %s/%s  %s  %s\n", ns, $1, ready, status
      }'
  done
}

wait_pods_green() { # deadline ns...
  local deadline="$1"; shift
  local ng
  while :; do
    ng="$(not_green_pods "$@")"
    [[ -z "$ng" ]] && { ok "all pods green"; return 0; }
    if (( SECONDS >= deadline )); then
      err "pods still not green after timeout:"; echo "$ng" >&2; return 1
    fi
    sleep 10
  done
}

check_rollouts() { # remaining_seconds ns...
  local remaining="$1"; shift
  local ns kind obj
  for ns in "$@"; do
    for kind in deployment statefulset daemonset; do
      while IFS= read -r obj; do
        [[ -z "$obj" ]] && continue
        kubectl rollout status "$obj" -n "$ns" --timeout="${remaining}s" || return 1
      done < <(kubectl get "$kind" -n "$ns" -o name 2>/dev/null)
    done
  done
  ok "all rollouts complete"
}

# HTTP status of one ingress request (prints "000" on connect/DNS/TLS failure).
ingress_code() { # host url
  local c
  c="$(curl -sk -o /dev/null -w '%{http_code}' --max-time 10 \
    --resolve "${1}:443:${CLUSTER_IP:-127.0.0.1}" "$2" 2>/dev/null)"
  echo "${c:-000}"
}

# End-to-end HTTP checks through the ingress edge. Pod-readiness + rollout alone
# stay green even when the ingress->APISIX hop is broken (e.g. an unmeshed ingress
# rejected by APISIX's Linkerd `Server` with a 403, or an APISIX upstream that
# can't resolve a service cross-namespace -> 502), so assert real requests
# traverse ingress -> APISIX -> backend. Two routes are probed:
#   * Keycloak OIDC discovery (idm) -> must be a clean 200 (public, no auth).
#   * The Portal on the apex host -> must return ANY real HTTP status, not a
#     gateway/upstream error; the Portal does its own OIDC, so 2xx/3xx/401/403 all
#     prove the ingress->APISIX->portal TLS path works, whereas 000/502/503/504 is
#     exactly the DM1 broken-upstream / SNI-mismatch symptom. This probe used to
#     target `nifi.<domain>`, which no component has served since Bento replaced
#     NiFi: APISIX matched no route and answered 404, so the check could never pass
#     and asserted nothing. The Portal is the same shape of backend on a host that
#     exists, so the property being tested is unchanged.
# Only meaningful with our managed k3d cluster, whose loadbalancer maps host
# 80/443 (k3d-jc-local.yaml); skipped in --no-cluster mode.
smoke_ingress_http() { # deadline
  local deadline="$1" domain code
  domain="$(yq -r '.global.domain // ""' "$ENV_FILE" 2>/dev/null || true)"
  [[ -z "$domain" || "$domain" == "null" ]] && domain="joinedcontext.test"

  # 1) Keycloak OIDC discovery must be a clean 200.
  local kc_host="idm.${domain}" kc_url
  kc_url="https://${kc_host}/realms/master/.well-known/openid-configuration"
  log "Smoke-testing ingress HTTP: ${kc_url}"
  while :; do
    code="$(ingress_code "$kc_host" "$kc_url")"
    [[ "$code" == "200" ]] && { ok "ingress HTTP 200 (${kc_host} -> APISIX -> keycloak)"; break; }
    if (( SECONDS >= deadline )); then
      err "ingress HTTP check failed: last status ${code} for ${kc_url}"; return 1
    fi
    sleep 10
  done

  # 2) The Portal host must return a real HTTP response (not a gateway/upstream error).
  # The Portal lives on portal.{domain} behind the edge login (ADR-N-019), so the healthy
  # answer for an anonymous probe is the 302 to Keycloak.
  local portal_host="portal.${domain}" portal_url
  portal_url="https://${portal_host}/"
  log "Smoke-testing ingress HTTP: ${portal_url}"
  # Require a positive response from the edge in front of the Portal. A 000 (unreachable),
  # 404 (APISIX matched no route) or 5xx (502/503/504 = the DM1 broken-upstream /
  # SNI-mismatch symptom, or the plugin unable to reach the issuer) is retried and, at the
  # deadline, fails.
  while :; do
    code="$(ingress_code "$portal_host" "$portal_url")"
    case "$code" in
      2[0-9][0-9]|3[0-9][0-9]|401|403)
        ok "ingress HTTP ${code} (${portal_host} -> APISIX -> portal)"; return 0 ;;
    esac
    if (( SECONDS >= deadline )); then
      err "ingress HTTP check failed: last status ${code} for ${portal_url} (no healthy response from the portal upstream)"; return 1
    fi
    sleep 10
  done
}

# Cluster-wide diagnostics — usable even when the deploy died before any
# platform namespace was detected. Prints the not-ready pods, describes them
# (events: ImagePullBackOff, scheduling, probes…) and tails their logs.
dump_diagnostics() {
  warn "diagnostics for context '${KCTX:-current}':"
  echo "--- pods not Running/Completed (cluster-wide) ---"
  kubectl get pods -A -o wide 2>/dev/null | awk 'NR==1 || $4!="Running" && $4!="Completed"' || true
  # describe + logs for every not-ready pod (ns/name pairs)
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    local ns name
    ns="${line%% *}"; name="${line##* }"
    echo "--- describe ${ns}/${name} (events) ---"
    kubectl describe pod -n "$ns" "$name" 2>/dev/null | sed -n '/Events:/,$p' | head -n 20 || true
    echo "--- logs ${ns}/${name} (tail) ---"
    kubectl logs -n "$ns" "$name" --all-containers --tail=20 2>/dev/null || true
  done < <(kubectl get pods -A --no-headers 2>/dev/null | awk '
      { ready=$3; status=$4
        if (status=="Completed" || status=="Succeeded") next
        split(ready, a, "/"); if (status=="Running" && a[2]>0 && a[1]==a[2]) next
        print $1, $2 }')
}

# --- run a single variant (returns non-zero on any failure) -----------------
run_variant() { # MNS MI LNK
  local mns="$1" mi="$2" lnk="$3"

  if $MANAGE_CLUSTER; then
    create_cluster || return 1
  else
    KCTX="$(kubectl config current-context)"
    log "Using existing context '${KCTX}'"
  fi

  # Before the overlay, never after: the recipe starts by removing deployment/, which would
  # take the scratch environment this writes with it.
  assemble_deployment || return 1
  write_env_overlay "$mns" "$lnk"
  [[ "$PROFILE" == "production" ]] && { install_monitoring_crds || return 1; }
  [[ "$lnk" == "1" ]] && { install_linkerd || return 1; mesh_ingress_controller || return 1; }
  # broker-only deploys neither the runtime policies Kyverno enforces nor Keycloak.
  if ! $BROKER_ONLY; then
    install_kyverno || return 1
    create_smtp_secret "$mns" || return 1
  fi

  local before after
  before="$(snapshot_namespaces)"
  [[ -n "$FOOTPRINT" ]] && start_sampler
  if ! deploy_variant "$mi"; then
    stop_sampler; err "helmfile sync failed"; dump_diagnostics; return 1
  fi
  after="$(snapshot_namespaces)"

  # Namespaces the platform created during this deploy.
  local PLATFORM_NS=()
  while IFS= read -r line; do [[ -n "$line" ]] && PLATFORM_NS+=("$line"); done \
    < <(comm -13 <(echo "$before") <(echo "$after"))
  # The before/after diff misses any namespace that already existed at snapshot
  # time: in single-namespace mode the whole platform lands in the slug namespace
  # (pre-created above for the keycloak-smtp secret), so it is never "new"; in
  # multi-namespace mode the pre-created <slug>-keycloak is likewise skipped.
  # Always include this instance's namespaces (slug exact, or "<slug>-*").
  local slug; slug="$(get_slug)"
  while IFS= read -r ns; do
    [[ "$ns" == "$slug" || "$ns" == "$slug"-* ]] && PLATFORM_NS+=("$ns")
  done < <(echo "$after")
  # De-duplicate (the slug namespace may be both new and slug-matched).
  if [[ ${#PLATFORM_NS[@]} -gt 0 ]]; then
    local _uniq=(); while IFS= read -r line; do [[ -n "$line" ]] && _uniq+=("$line"); done \
      < <(printf '%s\n' "${PLATFORM_NS[@]}" | sort -u)
    PLATFORM_NS=("${_uniq[@]}")
  fi
  if [[ ${#PLATFORM_NS[@]} -eq 0 ]]; then
    err "no new namespaces detected after deploy — nothing to smoke test"; return 1
  fi
  log "Smoke-testing namespaces: ${PLATFORM_NS[*]}"
  smoke_platform || { stop_sampler; return 1; }
  if [[ -n "$FOOTPRINT" ]]; then
    # One more minute of samples once everything is green: the settled figure.
    sleep 60
    stop_sampler
    report_footprint "$1,$2,$3" || return 1
  fi

  # An operation the deployment must survive, then the same smoke: it is only survived when
  # every pod is green again and the edge still answers.
  if [[ -n "$AFTER" ]]; then
    log "After the smoke: ${AFTER}"
    if ! JC_ENV="$ENV" bash -c "$AFTER"; then
      err "the step after the smoke failed: ${AFTER}"; dump_diagnostics; return 1
    fi
    log "Smoke-testing again after: ${AFTER}"
    smoke_platform || return 1
  fi

  # In no-cluster mode clean up the platform namespaces for the next run.
  if ! $MANAGE_CLUSTER; then
    warn "cleaning platform namespaces (no-cluster mode)"
    kubectl delete ns "${PLATFORM_NS[@]}" --ignore-not-found --wait=true >/dev/null 2>&1 || true
  fi
  return 0
}

# Pods green, rollouts complete and, on our own cluster, the edge answering, in the namespaces
# run_variant found (PLATFORM_NS, its local, which bash hands to the functions it calls).
smoke_platform() {
  local deadline=$((SECONDS + TIMEOUT))
  if ! wait_pods_green "$deadline" "${PLATFORM_NS[@]}"; then
    dump_diagnostics; return 1
  fi
  local remaining=$((deadline - SECONDS)); (( remaining < 30 )) && remaining=30
  if ! check_rollouts "$remaining" "${PLATFORM_NS[@]}"; then
    dump_diagnostics; return 1
  fi

  # Assert the ingress edge actually serves traffic (see smoke_ingress_http).
  # Only with our managed k3d cluster, whose loadbalancer maps host 80/443.
  if $BROKER_ONLY; then
    # No edge in front of the broker: its smoke is NGSI-LD over a port-forward.
    smoke_broker_ngsild || { dump_diagnostics; return 1; }
  elif $MANAGE_CLUSTER; then
    if ! smoke_ingress_http "$((SECONDS + 180))"; then
      dump_diagnostics; return 1
    fi
  else
    warn "--no-cluster: skipping ingress HTTP smoke (ingress endpoint unknown)"
  fi
}

# --- broker-only smoke: NGSI-LD over a port-forward (T-4856) ----------------
# The profile has no edge and no Keycloak, and the broker's NetworkPolicies admit no client
# namespace; a port-forward reaches the pod through the kubelet. Create, read, query and delete
# one entity, and a never-created id is a 404 with a problem body (ETSI GS CIM 009 6.3.2,
# 6.4.3.1, 6.5.3.1, 6.8.3.1). With JC_ETSI_SMOKE_DIR naming the conformance repository's
# tests/etsi folder, its smoke.robot runs against the same URL afterwards.
# The broker answers a request without a subject only with its admin bearer token (T-3642), so
# the smoke sends the profile's generated one, read from its Secret into a header file curl
# reads, never onto a command line.
smoke_broker_ngsild() {
  local ns port=18080 pf base id code body auth
  ns="$(kubectl get svc -A --field-selector metadata.name=context-broker -o jsonpath='{.items[0].metadata.namespace}')"
  [[ -n "$ns" ]] || { err "no context-broker Service found"; return 1; }
  auth="$(mktemp)"
  chmod 600 "$auth"
  printf 'Authorization: Bearer %s\n' \
    "$(kubectl -n "$ns" get secret context-broker-admin-token -o jsonpath='{.data.token}' | base64 -d)" > "$auth"
  grep -q 'Bearer .' "$auth" || { err "no admin token in Secret ${ns}/context-broker-admin-token"; rm -f "$auth"; return 1; }
  kubectl -n "$ns" port-forward svc/context-broker "${port}:8080" >/dev/null 2>&1 &
  pf=$!
  base="http://127.0.0.1:${port}/ngsi-ld/v1"
  local rc=0
  for _ in $(seq 30); do curl -sf -o /dev/null --max-time 2 "http://127.0.0.1:${port}/q/ready" && break; sleep 2; done
  id="urn:ngsi-ld:SmokeProbe:$(date +%s)$RANDOM"
  expect() { # want got what
    if [[ "$2" == "$1" ]]; then ok "NGSI-LD $3: $2"; else err "NGSI-LD $3: want $1, got $2"; rc=1; fi
  }
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 -H "@$auth" -X POST "$base/entities" \
    -H 'Content-Type: application/json' \
    -d "{\"id\":\"$id\",\"type\":\"SmokeProbe\",\"reading\":{\"type\":\"Property\",\"value\":21.5}}")"
  expect 201 "$code" "create entity"
  body="$(curl -s --max-time 10 -H "@$auth" "$base/entities/$id")"
  [[ "$body" == *"\"$id\""* && "$body" == *21.5* ]] && ok "NGSI-LD retrieve entity: body names it" \
    || { err "NGSI-LD retrieve entity: ${body:0:300}"; rc=1; }
  body="$(curl -s --max-time 10 -H "@$auth" "$base/entities?type=SmokeProbe")"
  [[ "$body" == *"\"$id\""* ]] && ok "NGSI-LD query by type: finds it" \
    || { err "NGSI-LD query by type: ${body:0:300}"; rc=1; }
  body="$(curl -s -w ' %{http_code}' --max-time 10 -H "@$auth" "$base/entities/urn:ngsi-ld:SmokeProbe:never-created")"
  [[ "$body" == *ResourceNotFound*" 404" ]] && ok "NGSI-LD unknown id: 404 ResourceNotFound" \
    || { err "NGSI-LD unknown id: ${body:0:300}"; rc=1; }
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 -H "@$auth" -X DELETE "$base/entities/$id")"
  expect 204 "$code" "delete entity"
  if [[ $rc -eq 0 && -n "${JC_ETSI_SMOKE_DIR:-}" ]]; then
    log "ETSI smoke.robot against ${base}"
    NGSILD_URL="$base" JC_REPORTS_DIR="${JC_ETSI_REPORTS:-/tmp/etsi-broker-only}" \
      "${JC_ETSI_SMOKE_DIR}/run.sh" --include smoke || rc=1
  fi
  rm -f "$auth"
  kill "$pf" 2>/dev/null; wait "$pf" 2>/dev/null || true
  return $rc
}

# --- footprint (T-4856) ------------------------------------------------------
# Samples `kubectl top pods` every 15 s from the deploy to a minute after the smoke, then sums
# each sample over the variant's namespaces (Kyverno's too when the profile needs it): the peak
# and the last sample are the two memory and CPU figures the profiles table in docs reports.
SAMPLES=""
start_sampler() {
  SAMPLES="$(mktemp)"
  ( while :; do
      kubectl top pods -A --no-headers 2>/dev/null | sed "s/^/$(date +%s) /"
      sleep 15
    done ) >"$SAMPLES" &
  SAMPLER_PID=$!
}
stop_sampler() {
  [[ -n "$SAMPLER_PID" ]] || return 0
  kill "$SAMPLER_PID" 2>/dev/null; wait "$SAMPLER_PID" 2>/dev/null || true
  SAMPLER_PID=""
}

# Quantities to MiB (memory) or millicores (cpu); reads one per line.
to_mib() { awk '{v=$1; n=v+0; u=v; sub(/^[0-9.]+/,"",u)
  f=(u=="Ki")?1/1024:(u=="Mi")?1:(u=="Gi")?1024:(u=="Ti")?1048576:(u=="k")?1000/1048576:(u=="M")?1e6/1048576:(u=="G")?1e9/1048576:1/1048576
  s+=n*f} END {printf "%d", s+0.5}'; }
to_milli() { awk '{v=$1; n=v+0; u=v; sub(/^[0-9.]+/,"",u)
  f=(u=="m")?1:(u=="u")?0.001:(u=="n")?0.000001:1000; s+=n*f} END {printf "%d", s+0.5}'; }

report_footprint() { # variant
  local nss=("${PLATFORM_NS[@]}") peak peak_mem peak_cpu settled_mem settled_cpu pods req lim pvc
  [[ "$COMPONENTS" != "broker-only" ]] && kubectl get ns kyverno >/dev/null 2>&1 && nss+=(kyverno)
  local list; list="$(printf '%s ' "${nss[@]}")"
  # Per sample time: summed MiB and millicores over the namespaces; then max and last.
  peak="$(awk -v list="$list" 'BEGIN{n=split(list,a," "); for(i=1;i<=n;i++) want[a[i]]=1}
    want[$2] {
      m=$5; mn=m+0; mu=m; sub(/^[0-9.]+/,"",mu); mf=(mu=="Ki")?1/1024:(mu=="Gi")?1024:1
      c=$4; cn=c+0; cu=c; sub(/^[0-9.]+/,"",cu); cf=(cu=="m")?1:1000
      mem[$1]+=mn*mf; cpu[$1]+=cn*cf; if ($1>last) last=$1 }
    END { for (t in mem) { if (mem[t]>pm) pm=mem[t]; if (cpu[t]>pc) pc=cpu[t] }
          printf "%d %d %d %d", pm+0.5, pc+0.5, mem[last]+0.5, cpu[last]+0.5 }' "$SAMPLES")"
  read -r peak_mem peak_cpu settled_mem settled_cpu <<<"$peak"
  [[ "${peak_mem:-0}" -gt 0 ]] || { err "no metrics samples for ${list}(is metrics-server running?)"; return 1; }
  pods=0; req=""; lim=""; pvc=""
  local ns
  for ns in "${nss[@]}"; do
    pods=$((pods + $(kubectl get pods -n "$ns" --field-selector=status.phase=Running --no-headers 2>/dev/null | wc -l)))
    req+="$(kubectl get pods -n "$ns" --field-selector=status.phase=Running -o jsonpath='{range .items[*].spec.containers[*]}{.resources.requests.memory}{"\n"}{end}')"$'\n'
    lim+="$(kubectl get pods -n "$ns" --field-selector=status.phase=Running -o jsonpath='{range .items[*].spec.containers[*]}{.resources.limits.memory}{"\n"}{end}')"$'\n'
    pvc+="$(kubectl get pvc -n "$ns" -o jsonpath='{range .items[*]}{.status.capacity.storage}{"\n"}{end}')"$'\n'
  done
  local req_mib lim_mib pvc_mib
  req_mib="$(grep . <<<"$req" | to_mib)"; lim_mib="$(grep . <<<"$lim" | to_mib)"; pvc_mib="$(grep . <<<"$pvc" | to_mib)"
  local row
  row="| ${COMPONENTS} | ${1} | ${pods} | ${peak_mem} | ${settled_mem} | ${peak_cpu} | ${settled_cpu} | ${req_mib} | ${lim_mib} | ${pvc_mib} | ${GITHUB_RUN_ID:-local} |"
  if [[ ! -s "$FOOTPRINT" ]]; then
    {
      echo "| Profile | Variant | Pods | Peak memory MiB | Settled memory MiB | Peak CPU m | Settled CPU m | Memory requests MiB | Memory limits MiB | PVC MiB | Run |"
      echo "|---|---|---|---|---|---|---|---|---|---|---|"
    } >"$FOOTPRINT"
  fi
  echo "$row" >>"$FOOTPRINT"
  [[ -n "${GITHUB_STEP_SUMMARY:-}" ]] && cat "$FOOTPRINT" >>"$GITHUB_STEP_SUMMARY"
  ok "footprint (${list% }): ${row}"
  rm -f "$SAMPLES"
}

# --- main loop --------------------------------------------------------------
# Pull-through caches only help the k3d cluster we configure ourselves.
if $USE_REGISTRY_CACHE; then
  if $MANAGE_CLUSTER; then
    ensure_registry_cache
  else
    warn "--no-cluster: skipping registry cache (your cluster's registry config is not managed here)"
  fi
fi

RESULTS=()
total=${#COMBOS[@]}
idx=0
overall_rc=0

for c in "${COMBOS[@]}"; do
  idx=$((idx + 1))
  IFS=',' read -r mns mi lnk <<<"$c"
  echo
  echo -e "${YELLOW}════════════════════════════════════════════════════════════════════${NC}"
  echo -e "${YELLOW}Variant ${idx}/${total}: ${c}   $(describe "$mns" "$mi" "$lnk")${NC}"
  echo -e "${YELLOW}════════════════════════════════════════════════════════════════════${NC}"

  start=$SECONDS
  if run_variant "$mns" "$mi" "$lnk"; then
    dur=$((SECONDS - start))
    ok "variant ${c} PASSED in ${dur}s"
    RESULTS+=("PASS  ${c}  $(describe "$mns" "$mi" "$lnk")  (${dur}s)")
  else
    dur=$((SECONDS - start))
    err "variant ${c} FAILED after ${dur}s"
    RESULTS+=("FAIL  ${c}  $(describe "$mns" "$mi" "$lnk")  (${dur}s)")
    overall_rc=1
    if ! $CONTINUE_ON_FAILURE; then
      if $MANAGE_CLUSTER; then
        warn "Stopping. Cluster '${CLUSTER_NAME}' (context ${KCTX}) is kept for inspection."
        warn "Inspect with: kubectl --context ${KCTX} get pods -A"
        warn "Re-run remaining variants with --continue-on-failure, or delete with: k3d cluster delete ${CLUSTER_NAME}"
      fi
      KEEP_CLUSTER=true
      break
    fi
  fi
done

# Tear down the last cluster unless asked (or forced by a kept failure) to keep it. A GitHub
# runner is thrown away with the job, and the delete cost up to a minute of its 30 (T-5846).
if $MANAGE_CLUSTER && ! $KEEP_CLUSTER && [ "${GITHUB_ACTIONS:-}" != true ]; then
  log "Removing k3d cluster '${CLUSTER_NAME}'"
  k3d cluster delete "$CLUSTER_NAME" >/dev/null 2>&1 || true
fi

echo
echo -e "${YELLOW}════════════════════════════ SUMMARY ════════════════════════════════${NC}"
for r in "${RESULTS[@]}"; do
  if [[ "$r" == PASS* ]]; then echo -e "${GREEN}${r}${NC}"; else echo -e "${RED}${r}${NC}"; fi
done
echo -e "${YELLOW}══════════════════════════════════════════════════════════════════════${NC}"

exit $overall_rc
