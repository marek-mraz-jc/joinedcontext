# Portal

Joinedcontext Portal: Axum API, embedded React UI, and in-process reconciler.

## APISIX Route Mapping

The Portal is served on `portal.{domain}` behind the edge login (docs ADR-N-019): APISIX's
`openid-connect` plugin logs the person in with the `edge` client and hands the Portal
`X-Userinfo` and `X-Access-Token`; `JC_TRUST_EDGE_TOKEN: "true"` makes the Portal accept the
token header as it accepts a bearer, and `JC_PORTAL_PUBLIC_URL` names the Portal host. The
apex keeps `/apps/*` (`apps-surface`, static apps served by the Portal behind the same login)
and redirects everything else to the Portal host (`portal-redirect`).

APISIX standalone mode derives upstream, route, and plugin_config IDs from the same map key.
While architecture docs describe shared `pc-*` plugin configs, each route in `apisix-plugins.yaml`
defines its dedicated plugin configuration under the matching route ID (`portal-ui`,
`portal-api`, `portal-metrics`, `apps-surface`, `portal-redirect`).

Deploy the component with:
```bash
helmfile apply -i --selector component=portal
```

## Image

`ghcr.io/marek-mraz-jc/joinedcontext-portal`, pinned by digest. Distroless, entrypoint
`/usr/local/bin/joinedcontext-portal`, listening on 8080 — the component sets no `command`
and lets the entrypoint run.

The part `ui` runs `ghcr.io/marek-mraz-jc/joinedcontext-portal-ui`, the UI bundle (nginx, uid
101, Service `portal-ui:8080`). The Portal reads every page from it through `JC_PORTAL_UI_URL`,
and nothing else may reach it (T-2485, T-5297). `scripts/pin-portal.sh <commit>` pins both
images together. When a commit built no bundle, the script keeps the pinned one only if ui/ and
sdk/ did not change since; when a ui/-only commit built no binary (T-6158), it keeps the pinned
binary, so the pin moves the bundle alone and `just dev-apply-pins` rolls out `portal-ui` alone. `GET /api/v1/version` says `contract: match` when the bundle was
built against the binary's API, and dev-smoke warns otherwise.

## Each project's apps namespace

A pod-backed App runs in its project's own namespace, `{instanceSlug}-{project}-apps` (AP-116).
The Portal creates it on the project's first pod-backed App, with Pod Security `restricted`, a
default-deny NetworkPolicy that admits APISIX only, the registry pull Secret and a RoleBinding
of `{instanceSlug}-portal-apps` to itself, and deletes it when the last one is retired.

The `namespaces` part (`charts/namespaces`) grants that. The ClusterRole
`{instanceSlug}-portal-namespaces` lets the Portal create, patch and delete namespaces and
RoleBindings and bind that one role. RBAC cannot name a prefix, so a ValidatingAdmissionPolicy
bounds those rights (AP-117). The API server evaluates it for every request, and it always
denies. The Portal may write a namespace only if the namespace is named
`{instanceSlug}-{project}-apps` and carries `joinedcontext.com/managed-by=joinedcontext-portal`
and `joinedcontext.com/project={project}`. It may write a RoleBinding only in such a namespace,
binding `{instanceSlug}-portal-apps` to itself alone. `tests/test_portal_namespaces_policy.py`
runs both rules through the kyverno CLI.

`portal.namespaces.enabled: false` removes the grant and the Portal's `JC_PORTAL_RELEASE`, and
the Portal then refuses every pod-backed App and says why.

## The secret backend a pipeline's `secretRef` resolves through

A `Pipeline` names its credentials, it never carries them (CC-06, PL-15). The reconciler
resolves every `secretRef` of every pipeline on each sync and writes one `pipeline-secrets`
Secret into the runner's namespace; the runner takes it as environment, so a stream reads
`${MQTT_PASSWORD}` and nothing in Git, in a ConfigMap or in a log line holds the value.

An installation configures **one** backend, never both:

- **SOPS** (`portal.portal.sopsAgeSecret`, what `dev` uses): the reconciler decrypts the
  `*.enc.yaml` files of the configuration repository it staged, with an age identity mounted
  read-only at `/etc/jc/age/keys.txt`. One tenant, one node, nothing else to run.
- **OpenBao** (`JC_PORTAL_OPENBAO_ADDR`, `JC_PORTAL_OPENBAO_ROLE`): a server of its own and the
  Kubernetes auth method bound to this ServiceAccount. The multi-tenant answer (ADR-N-012).

With neither, every reference is refused with `no secret backend is configured`, the pipeline
carries the reason on its `StreamDeployed` condition, and nothing runs half-credentialed.

### Handing an installation its age identity

The private half is operator-supplied: generated once, kept out of Git, and copied into the
Portal's namespace by hand. The public half is a recipient anyone may read.

```bash
age-keygen -o .secrets/age-dev.key            # mode 600, never committed
age-keygen -y .secrets/age-dev.key            # the recipient, for encrypting
kubectl -n dev create secret generic portal-age-identity \
  --from-file=keys.txt=.secrets/age-dev.key
```

Then encrypt what a pipeline needs to the same recipient and commit the result — ciphertext —
to the configuration repository, beside the manifests that name it:

```bash
sops --encrypt --age "$recipient" plain.yaml \
  > projects/{project}/secrets/{name}.enc.yaml
```

The file's top-level keys are the `secretRef.name`s, one level of keys below them the
`secretRef.key`s. The mount is `optional`: an installation that names the Secret and has not
created it yet runs and refuses every reference with a reason, rather than never starting.
