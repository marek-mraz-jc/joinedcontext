# The node disk like k3s's local-path, but the volume outlives its claim (T-6718, G-5): deleting
# a PVC by mistake left Postgres, jc-files and prod-final-backup gone with it. Stateful claims of
# jc-prod, jc-prod-mqtt, jc-dev and prod-final-backup name this class (policies/volumes.rego); a
# released volume is removed by hand, after the owner's yes.
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: local-path-retain
provisioner: rancher.io/local-path
reclaimPolicy: Retain
volumeBindingMode: WaitForFirstConsumer
allowVolumeExpansion: false
