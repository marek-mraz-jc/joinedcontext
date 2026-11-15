# Gitea

The in-cluster Git forge holding the configuration repositories: the single source of
truth the reconciler applies from (ADR-N-004, CC-02).

Deploy the component with:
```bash
helmfile apply -i --selector component=gitea
```

## What it deploys

The upstream chart (`gitea-charts/gitea`), which since version 10 runs a Deployment over
one ReadWriteOnce volume rather than a StatefulSet. That volume holds the bare
repositories; PostgreSQL holds everything else. One replica is therefore the ceiling in
every profile — two pods cannot share the claim — which is why the production values pick
`Recreate` over a rolling update and carry no PodDisruptionBudget.

Every bundled dependency is off: PostgreSQL comes from CloudNativePG (`databases.yaml`),
and the queue, cache and session stores that would otherwise pull in a three-node Valkey
cluster are Gitea's own — level queue on disk, sessions in the database.

## Served under /git/

The forge shares the primary host with the portal (`docs/Deployment/10-edge-routing-apisix.md`
section 2), so there is no `git.<domain>` name and no certificate for it. Gitea listens at
the root and builds every URL it renders from `ROOT_URL`, which carries the prefix; APISIX
strips it on the way in with `proxy-rewrite`. Change one of those two and clone URLs, OIDC
callbacks and asset paths all break together.

## Identity

Human login is Keycloak's, through the `gitea` client in `keycloak-clients.yaml`. The chart
passes `--key`/`--secret` to `gitea admin auth add-oauth` and falls back to
`${GITEA_OAUTH_KEY_0}`/`${GITEA_OAUTH_SECRET_0}` when the values omit them — which is how
the generated client secret reaches the process without ever being rendered into a
manifest. The chart's own `oauth[].existingSecret` is not usable here: it reads the keys
`key` and `secret`, and the platform generates `keycloak-client-<id>` with `client-secret`.

`gitea-admin-credentials` is the forge's local administrator. It exists because the chart's
init container needs an admin to create on an empty database, and because someone has to
reach the site-administration pages when the identity provider is the broken thing. Its
password is written once (`passwordMode: initialOnlyNoReset`), not on every apply.

## Webhooks

Gitea refuses to deliver a webhook to a private address unless the host is allow-listed, so
`webhook.ALLOWED_HOST_LIST` names the portal Service, which hosts the reconciler loop, and
`networkpolicies.yaml` opens the matching egress and ingress path. The hooks themselves are
per-repository and are created by whoever creates the repository.

## Image

`docker.gitea.com/gitea`, rootless variant, pinned by digest. The chart appends
`-rootless` to the tag, so the digest in `images.yaml` is the one of `<tag>-rootless`; the
file carries the one-line command that resolves it. The root-based image cannot run under
the restricted Pod Security Standards this deployment enforces — it starts as uid 0 to drop
privileges itself.

## Gaps

- A person in the Keycloak group `forge-admins` (`gitea.forge.adminGroup`, T-1421)
  administers the forge; nobody is in it by default. Every other signed-in person lands in
  the read-only `readers` team through `platform-readers`. No team writes: a push to `main`
  is refused, and a Change approved in the Portal stays the one way in (CC-41).

- Gitea Actions is off: it needs a runner, and this component deploys none.
- Git over SSH is off: the edge terminates HTTPS only, and an SSH port would need a
  LoadBalancer of its own. The chart renders a `gitea-ssh` Service regardless; nothing
  listens behind it.
- The `gitea` Keycloak client does not require PKCE (T-1420). Gitea 1.27 sends no
  `code_challenge` as an OAuth2 client, and Keycloak's realm default of S256 refused every
  forge login. Only this client carries the exception; it stays confidential with one exact
  redirect URI. Remove the `attributes` block in `keycloak-clients.yaml` once a pinned Gitea
  release ships go-gitea/gitea#38202.
