# app-pods

The `ui-rust` Apps an installation without a build lane runs as pods. On prod the Portal's
reconciler ran each one as Deployment and Service `app-{name}` from the image the forge's build
lane made. jc has no such lane, so the monorepo's `.github/workflows/image-apps.yml` builds the
image from `crates/portal/apps/{name}` (the `ui-rust-app` target of
`crates/portal/builder/Dockerfile`, the fullstack branch of `build-app.sh`), smoke-tests it,
pushes `ghcr.io/marek-mraz-jc/jc-app-{name}` by digest, signs it with cosign and scans it.

The component is opt-in. An environment turns it on by listing `app-pods` in `components:`
after `jc`, its one caller. `addons` renders it with an example image pin
(`environments/addons/app-pods.yaml.gotmpl`).

## 1. What it deploys

One release of `charts/workload` per App, today `app-pods-hsl-transport`:

- **Deployment `app-hsl-transport`**: one replica, image pinned by digest in `images.yaml`, uid
  65532 (the image's user), read-only root filesystem, every capability dropped. Its environment
  is what prod's reconciler set: `JC_BIND_ADDRESS`, `JC_BASE_PATH=/`, `JC_APP_CONFIG`, and
  `JC_ENDPOINT_URL` on jc's apps listener, `http://jc.{jc namespace}.svc.cluster.local:8083/api/endpoint/{slug}/`.
- **Service `app-hsl-transport`** on 8080.

## 2. How a request reaches it

1. The edge sends `hsl-transport.apps.{domain}` to jc, like every App host.
2. jc's Portal finds a `ui-rust` App named in `JC_PORTAL_APP_PODS` and proxies the request to
   the Service. It drops cookies both ways and sets the App's Content-Security-Policy. The
   jc values set the variable from this component's parts.
3. The App reads its Endpoint through jc's gateway on jc's apps listener (Service port 8083,
   `JCBIN_APPS_LISTEN`): the gateway's `GET`/`HEAD` `/api/endpoint/...` with the same policy as
   the public read path, and no other route or method. jc's public listener (8080) stays the
   edge's alone. The listener needs a jc image that has it: list `app-pods` only once jc's pin
   carries `JCBIN_APPS_LISTEN`. An older jc ignores the variable and opens nothing on 8083.

An App host whose App is not listed here keeps the Portal's designed 503 page.

## 3. Values

| key | meaning |
|---|---|
| `app-pods.hsl-transport.endpointSlug` | the slug of the public Endpoint the App reads; the render refuses an empty one |
| `app-pods.hsl-transport.project` | the App's project, for its Portal link and basemap route (`helsinki`) |

Resources (development and production alike): requests 20m CPU / 32Mi, limits 500m / 128Mi.
On the prod rehearsal (2026-11-10) the pod held 3.4 MiB idle and 4.6 MiB under 120 parallel page
and API reads, with 8304 stops and 25 alerts loaded and an empty fleet (the rehearsal has no live
Vehicles). A live HSL fleet of a few thousand buses adds a few MiB; recheck `kubectl top pod`
after the first day on prod.

## 4. Network

`networkpolicies.yaml`: the App pod takes 8080 from jc's pod alone, and calls jc's pod on
8083 alone, plus DNS. jc's halves are `jc-egress-to-app-pods` and `jc-from-app-pods`.
`networkpolicies-linkerd.yaml` adds the proxy port 4143 to each rule and the control plane.
Meshed, the pod's Linkerd `Server` admits jc's identity alone (`meshAuthorization`), and jc's
Server `jc-apps` on 8083 admits the App pods' ServiceAccounts alone (the jc values'
`meshAuthorization.extraPorts`); jc's Server on 8080 admits APISIX alone, so the pods' way to
jc's inbound proxy (4143) opens nothing but 8083. Linkerd gives the pod its identity from a
projected token, so `automountServiceAccountToken: false` stays.
