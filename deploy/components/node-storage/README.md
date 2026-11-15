# node-storage

The node's local-path storage, for k3s clusters whose node runs SELinux enforcing (T-3431).

k3s's local-path-provisioner removes a released volume with a helper pod. Its stock helper is
unlabelled, so on an enforcing node it cannot delete files that a pod wrote under its own MCS
categories, and every Portal build cache leaked. The `local-path` part renders the provisioner's
`local-path-config` ConfigMap in `kube-system` with the stock data, the helper image pinned by
digest and `seLinuxOptions.type: spc_t` on the helper only.

k3s re-applies its `local-storage` addon on every server start. The dev node carries
`/var/lib/rancher/k3s/server/manifests/local-storage.yaml.skip` (written once, 2026-10-18), so k3s
leaves this ConfigMap alone and the provisioner Deployment, StorageClass and RBAC stay as k3s made
them. A new cluster needs that marker before this part takes effect.

The provisioner reads `helperPod.yaml` once at start. After a sync that changed the ConfigMap, a
postsync hook stamps the data's hash on the provisioner's pod template, which restarts it once.

Off by default; dev turns it on.
