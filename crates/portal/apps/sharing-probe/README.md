# sharing-probe

The test page of the App sharing matrix (T-4911): one plain-HTML bundle with no build step, which
the Portal image ships five times, as `sharing-private`, `sharing-project`, `sharing-organization`,
`sharing-roles` and `sharing-public` (Dockerfile, stage `apps`). The five App manifests live in
the deployment's `components/context-gateway/seed/sharing/` (dev only) and differ only in
`spec.visibility`.

The page shows the App's name, the signed-in person from `#jc-config` and the count of city-bike
stations its endpoint answers to that person (`#count`, `data-entity-count`). The conformance
journey `e2e/journeys/39-app-sharing-matrix.spec.ts` opens it as six kinds of people.
