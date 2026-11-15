# Hetzner clusters

One kube-hetzner terraform module, one `clusters/<name>.tfvars` per cluster: `dev`, `smartcity`
(production), `stage`. The GitLab kit `civitas-core-v2-terraform-hetzner-deployment-guide` stays
the upstream this was taken from; changes land here.

| Path | Holds |
|---|---|
| `module/` | The root terraform module: kube-hetzner and providers pinned, `.terraform.lock.hcl`, `extra-manifests/` |
| `clusters/*.tfvars` | What differs per cluster: server type, location, firewall CIDRs, cluster name, ssh key name, k3s line and server flags |
| `cluster.sh` | What `just cluster-plan`, `cluster-apply` and `cluster-destroy` run |

`module/` stays the root module, so its resource addresses equal the kit's and a kit state moves
in by copying the file.

## What lives outside Git

Each cluster has one state directory, `${CLUSTER_STATE_DIR:-/workspace/.cluster-state}/<name>/`,
mode 700. It holds `terraform.tfstate`, which carries private keys and the token, plus the saved
plan, `ssh/<ssh_key_name>`, `kubeconfig.yaml` and `.terraform/`. Back it up; losing it means a
cluster terraform can no longer change.

`cluster.sh` reads the token from `.secrets/hcloud-token-<project>` into the environment only. The
project equals the cluster name, except `smartcity`, which uses `prod`.

A person's address never goes in Git: put it in `<state dir>/private.tfvars`, for example
`firewall_extra_ips = ["192.0.2.1/32"]`. `cluster.sh` passes that file when it exists. The module
refuses a `/0` in either firewall list.

## Create a cluster

1. Add `clusters/<name>.tfvars` and store the Hetzner project's token as
   `.secrets/hcloud-token-<name>`.
2. Build the Leap Micro snapshot once per Hetzner project. The module finds it by label and the
   plan fails with `Resource not found` without it. The build is the Packer step of the kit's
   `scripts/01-hetzner-setup.sh`, with the kube-hetzner 3.2.1 template (T-4677). Packer is not in
   the sandbox image.
3. `just cluster-plan <name>` makes an ssh key on the first run and saves the plan. Read
   `<state dir>/<name>.plan.txt`.
4. `just cluster-apply <name>` applies that saved plan and nothing else, then writes
   `<state dir>/kubeconfig.yaml`.

## Change a cluster

Edit the tfvars or the module, then plan and apply as above. Terraform refuses a plan that went
stale. A plan that destroys or replaces a server stops both recipes unless
`ALLOW_REPLACE=<name>` is set, and that deletes every PVC on the node disk.

`enable_secrets_encryption` and `cluster_name` are fixed at creation.

## Adopt an existing cluster

Copy the kit's `terraform.tfstate` and its ssh key pair into the state directory, mode 600:
`<state dir>/terraform.tfstate`, `<state dir>/ssh/<ssh_key_name>` and `.pub`. Then
`just cluster-plan <name>` and read the plan. It has to show no destroy or replace of the server;
list any drift in the task before applying. With no state to copy, `terraform import` each
resource with the same `TF_DATA_DIR` and token the script sets.

Expect these changes on the first plan of a kit state:

- the kustomization resources re-run, because `extra-manifests/` moved the Linkerd CNI chart into
  its own file and merged the dev and prod Traefik values;
- on `dev`, whatever differs from the cluster taken over in T-3040, and the k3s config write of
  `k3s_server_config`.

## Production

`cluster.sh` runs `smartcity` only as `AGENT_NAME=prod-deployer`, and never destroys it. A prod
change needs a plan read by prod-deployer first. Its state stays in prod-deployer's home (T-4684).

## Destroy a cluster

`just cluster-destroy <name>` asks for the name typed twice. Announce it in `AI_shared_folder.md`
first.

## Module notes

- `k3s_server_config` merges into the node's `config.yaml`. A key given there replaces the
  module's: `kube-controller-manager-arg` keeps `flex-volume-plugin-dir`, which is why it lists it
  beside `leader-elect=false` (T-5892). `smartcity` leaves it empty, because the flag restarts k3s.
- `mqtt_ports_open` opens 8883 and 18830-18839 and adds their Traefik entrypoints (T-3996, T-5856).
- Keep the `linkerd2-cni` chart version equal to `linkerd-crds` and `linkerd-control-plane`. Order
  on a live cluster: the `linkerd-cni` DaemonSet Ready, then the control plane with `cniEnabled`,
  then a rollout restart of every meshed workload. Compare with the live chart before an apply:
  `kubectl -n kube-system get helmchart linkerd2-cni -o jsonpath='{.spec.valuesContent}'`.
