# CIVITAS references clusterIssuer "letsencrypt-prod" (global.ingress.clusterIssuer).
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-prod
spec:
  acme:
    server: https://acme-v02.api.letsencrypt.org/directory
    email: ${acme_email}
    privateKeySecretRef:
      name: letsencrypt-account-key
    solvers:
      - http01:
          ingress:
            class: traefik
            # Solver pods land in the CIVITAS namespace (linkerd.io/inject:
            # enabled). Upstream v2.0.0's cert-manager-http01 NetworkPolicy
            # denies ALL their egress, so a meshed solver's proxy can't reach
            # the Linkerd control plane, fails its own startup probe (403) and
            # no certificate ever issues. The solver only serves a public
            # token over plain HTTP — keep it out of the mesh.
            podTemplate:
              metadata:
                annotations:
                  linkerd.io/inject: disabled
