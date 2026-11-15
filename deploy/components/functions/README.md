# Functions

`jc-functions` from the platform image (docs Architecture/20 §3, SDK-21…SDK-23): the QuickJS
runtime of the functions a generated application ships. One Deployment and a ClusterIP Service
on 8080; the Portal sends every invocation, the function's code included, so the runtime holds
no application, no database connection and no credential of its own.

Deploy the component with:
```bash
helmfile apply -i --selector component=functions
```

## What it wires

- **Portal**: `JC_FUNCTIONS_URL`, rendered by the portal component when this component is in
  the environment's list; the Portal calls the runtime with its own client-credentials token.
- **Keycloak**: the `jc-functions` audience mapper on the Portal's client `portal-api`
  (components/portal/keycloak-clients.yaml); the runtime accepts that audience with `azp`
  `portal-api` and nothing else, and reads the realm's keys over the in-cluster Service.
- **Network**: ingress from the Portal only; egress to the Context Gateway (each function's
  requests, with its caller's token), Keycloak (the JWKS) and CoreDNS.

## Sizing

Each invocation runs in its own runtime capped at 64 MiB and 5 s, at most 16 at once, so the
limit is 1Gi; the request is what an idle pod uses.
