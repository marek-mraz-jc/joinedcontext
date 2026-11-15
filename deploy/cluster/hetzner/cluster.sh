#!/usr/bin/env bash
# just cluster-plan|cluster-apply|cluster-destroy <name>: one terraform module, $JC_ENVIRONMENTS/clusters/<name>.tfvars
# (private: docs/Deployment/01-repository-layout.md),
# and one state directory per cluster outside the repository (T-6444, README.md).
set -euo pipefail
umask 077

action=${1:-}
name=${2:-}
here=$(cd "$(dirname "$0")" && pwd)
die() { echo "cluster: $*" >&2; exit 1; }
[ -n "${JC_ENVIRONMENTS:-}" ] || die "set JC_ENVIRONMENTS to the private environments/ checkout: the clusters' tfvars live there"
clusters=$JC_ENVIRONMENTS/clusters

[[ $action =~ ^(plan|apply|destroy)$ ]] || die "usage: cluster.sh plan|apply|destroy <name>"
[[ $name =~ ^[a-z0-9-]+$ && -f $clusters/$name.tfvars ]] || die "no $clusters/$name.tfvars"

if [ "$name" = smartcity ]; then
  [ "$action" != destroy ] || die "smartcity is production: this recipe never destroys it"
  [ "${AGENT_NAME:-}" = prod-deployer ] || die "only prod-deployer runs terraform for smartcity (README.md)"
  project=prod
else
  project=$name
fi

state=${CLUSTER_STATE_DIR:-/workspace/.cluster-state}/$name
mkdir -p "$state/ssh"
chmod 700 "${state%/*}" "$state" "$state/ssh"
plan=$state/$name.tfplan
vars=(-var-file="$clusters/$name.tfvars")
[ ! -f "$state/private.tfvars" ] || vars+=(-var-file="$state/private.tfvars")

token_file=${SECRETS_DIR:-/workspace/.secrets}/hcloud-token-$project
[ -r "$token_file" ] || die "no token at $token_file"
TF_VAR_hcloud_token=$(<"$token_file")
export TF_VAR_hcloud_token HCLOUD_TOKEN=$TF_VAR_hcloud_token
export TF_VAR_ssh_key_dir=$state/ssh TF_DATA_DIR=$state/.terraform TF_IN_AUTOMATION=1

tf() { terraform -chdir="$here/module" "$@"; }

# Addresses of the existing servers the saved plan deletes (a replace deletes too).
servers_lost() {
  tf show -json "$plan" | jq -r '.resource_changes[]? | select(.type == "hcloud_server")
    | select(.change.actions | index("delete")) | .address'
}

refuse_a_lost_server() {
  local lost
  lost=$(servers_lost)
  [ -n "$lost" ] || return 0
  [ "${ALLOW_REPLACE:-}" = "$name" ] && return 0
  rm -f "$plan"
  die "the plan destroys or replaces: $lost. Read it, then rerun with ALLOW_REPLACE=$name"
}

tf init -input=false -lockfile=readonly -reconfigure -backend-config="path=$state/terraform.tfstate" >/dev/null

case $action in
  plan)
    key=$(sed -nE 's/^ssh_key_name *= *"([^"]+)".*/\1/p' "$clusters/$name.tfvars")
    if [ ! -f "$state/ssh/$key" ]; then
      [ ! -s "$state/terraform.tfstate" ] || die "state exists but $state/ssh/$key does not: copy the cluster's key"
      ssh-keygen -q -t ed25519 -N "" -C "$key" -f "$state/ssh/$key"
    fi
    tf plan -input=false "${vars[@]}" -out="$plan"
    refuse_a_lost_server
    tf show -no-color "$plan" >"$state/$name.plan.txt"
    echo "cluster: plan saved, read $state/$name.plan.txt, then just cluster-apply $name"
    ;;
  apply)
    [ -f "$plan" ] || die "no saved plan: run just cluster-plan $name first"
    refuse_a_lost_server
    tf apply -input=false "$plan"
    rm -f "$plan"
    tf output -raw kubeconfig >"$state/kubeconfig.yaml"
    echo "cluster: kubeconfig at $state/kubeconfig.yaml"
    ;;
  destroy)
    echo "This deletes cluster $name and every disk on it. Type its name twice, one line each."
    read -r first || die "not confirmed"
    read -r second || die "not confirmed"
    [[ $first == "$name" && $second == "$name" ]] || die "names differ from $name, nothing destroyed"
    rm -f "$plan"
    tf destroy -input=false -auto-approve "${vars[@]}"
    ;;
esac
