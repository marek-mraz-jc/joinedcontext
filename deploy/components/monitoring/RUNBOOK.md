# monitoring runbook

What to do when an alert of the monitoring component fires (T-6727). Each rule's `runbook_url`
points at its section here. Every command reads the cluster with the kubeconfig of the instance
the alert names (`KUBECONFIG=.secrets/kubeconfig-dev.yaml` for jc-prod); a fix goes through Git
and the deployer's apply path, never a hand edit of a live object.

## JcProdWorkloadDown

A Deployment in `jc-prod` or `jc-prod-mqtt` has had fewer available replicas than it asks for
for 10 minutes. `jc` down means every smartcity host answers 502 or 503.

1. `kubectl -n <namespace> get deploy,pods -l app.kubernetes.io/instance` and
   `kubectl -n <namespace> describe pod <pod>`: Pending (node memory), ImagePullBackOff (pin) or
   CrashLoopBackOff (read `kubectl logs --previous`).
2. A bad image pin: pin the previous digest in `components/<component>/images.yaml` and apply
   that one release.
3. Node memory: `kubectl top nodes`, then the node rules below.

## JcProdMetricsMissing

kube-state-metrics reports no `jc` Deployment in `jc-prod` for 15 minutes: the Deployment is gone
or kube-state-metrics (namespace `dev`) is down, and every rule here is blind.

1. `kubectl -n dev get pods -l app.kubernetes.io/name=kube-state-metrics`.
2. `kubectl -n jc-prod get deploy jc`; a missing Deployment is a jc-server release to apply.

## JcProdContainerRestarting

A container in `jc-prod` or `jc-prod-mqtt` restarted more than 3 times in an hour.

1. `kubectl -n <namespace> get pod <pod> -o jsonpath='{.status.containerStatuses[*].lastState}'`:
   OOMKilled raises the memory limit in Git; an exit code means `kubectl logs --previous`.
2. A liveness probe that kills a slow but healthy container: compare the probe with the startup
   time in the log before changing it.

## JcProdPostgresDown

No `postgres-cluster` instance in `jc-prod` has been Ready for 5 minutes: jc answers 5xx on
every write and most reads.

1. `kubectl -n jc-prod get cluster.postgresql.cnpg.io postgres-cluster` and its pods.
2. `kubectl -n jc-prod logs postgres-cluster-1 -c postgres | tail -50`: a full volume
   (see VolumeAlmostFull), a WAL error, or the CNPG operator (namespace `dev`) down.
3. Never delete the PVC or the cluster object: restore from the newest dump only with the owner's
   yes (components/postgres/README.md).

## JcProdCertificateExpiresSoon

A cert-manager Certificate in `jc-prod` expires within 14 days or is not Ready for an hour.
cert-manager renews 30 days ahead, so renewal has failed for two weeks.

1. `kubectl -n jc-prod describe certificate <name>`, then its CertificateRequest, Order and
   Challenge: an HTTP-01 challenge needs the host's DNS on this node and the edge to pass
   `/.well-known/acme-challenge/`.
2. `kubectl -n cert-manager logs deploy/cert-manager | tail -50`.

## NodeDiskAlmostFull

A node filesystem is more than 85 % full. Postgres and every local-path volume stop at 100 %.

1. `kubectl get --raw /api/v1/nodes/<node>/proxy/stats/summary` or `df -h` on the node.
2. Free space by removing what the platform no longer uses (old images: `crictl rmi --prune`),
   never a volume or a dump without the owner's yes.

## VolumeAlmostFull

A PersistentVolumeClaim is more than 85 % full. For Postgres this is the database filling up.

1. `kubectl -n <namespace> exec <pod> -- df -h <mount>`.
2. Postgres: find the largest tables read-only (`pg_total_relation_size`) and grow the volume in
   Git (`storage.size`); local-path cannot grow in place, so a bigger volume is a planned restore.

## NodeMemoryHigh

The node has used more than 90 % of its memory for 10 minutes; the next pod to grow is
OOM-killed.

1. `kubectl top pods -A --sort-by=memory | head -20`.
2. Lower the largest consumer in Git, or stop an optional workload (the owner orders which).

## PostgresBackupStale

The newest Postgres dump (`<cluster>-local-backup` CronJob) or CNPG backup in the named namespace
is older than 26 hours: a night was missed.

1. `kubectl -n <namespace> get cronjob,jobs | grep backup` and the last Job's log.
2. Start one: `kubectl -n <namespace> create job --from=cronjob/postgres-cluster-local-backup
   backup-manual-$(date +%H%M)`, then check the new file on the backup volume.
