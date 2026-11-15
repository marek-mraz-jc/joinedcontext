# preview

One code-branch preview on the dev node: the namespace `pv-<slug>` with its quota, limits,
default-deny NetworkPolicy, the Portal and its UI by digest, and the Ingress
`preview-<slug>.<domain>` (docs `Deployment/17-branch-previews.md`, ADR-N-062).

No environment installs this component: its `parts` list is empty and it has no helmfile.
`scripts/preview.sh`, behind `just dev-preview <slug>`, `just dev-preview-down <slug>` and
`just dev-preview-isolation <slug> <image>`, renders the chart once per preview and applies it
after `just _dev-guard`. The dev side (role `pv_previews`, its pg_hba lines, the Keycloak and
Postgres NetworkPolicies) sits behind `global.branchPreviews.enabled`, which is off on dev.

The sandbox reaper (`components/sandbox-reaper`) deletes each `pv-<slug>` namespace after its
`sandbox.joinedcontext.com/ttl` annotation (48 h by default).
