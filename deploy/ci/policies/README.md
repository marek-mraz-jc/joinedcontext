# Kyverno policies

Policies applied to the rendered helmfile output in CI (`scan-kyverno-policies`)
and locally via `just verify-policies`.

## Layout

| Path          | Applied to                                           | Content                                                                      |
| ------------- | ---------------------------------------------------- | ---------------------------------------------------------------------------- |
| `base/`       | every environment (`local`, `production`)            | full PSS baseline + restricted plus hygiene checks (rootfs, probes, ingress) |
| `production/` | `production` rendering only (in addition to `base/`) | HA hardening (replicas, PDB, resources, rolling update)                      |
| `tests/`      | `kyverno test ci/policies` / `just test-policies`   | good/bad fixtures and expected results for every rule                        |

## Pod Security Standards coverage

`base/` covers the complete
[PSS **baseline** profile](https://kubernetes.io/docs/concepts/security/pod-security-standards/#baseline)
(privileged containers, host namespaces, hostPath volumes, hostPorts,
HostProcess, capabilities, proc mount, SELinux, AppArmor, seccomp, sysctls)
and the complete
[PSS **restricted** profile](https://kubernetes.io/docs/concepts/security/pod-security-standards/#restricted)
(capabilities strict, privilege escalation, runAsNonRoot, runAsUser,
seccomp strict, volume types) as vendored Kyverno policies.

Where restricted strictly supersets a baseline control, only the strict
variant is vendored (`disallow-capabilities-strict` instead of
`disallow-capabilities` and the best-practices `require-drop-all`,
`restrict-seccomp-strict` instead of `restrict-seccomp`) so a violation is
reported once, not twice.

Unlike baseline, restricted requires explicitly set good values: a pod
without `allowPrivilegeEscalation: false`, `runAsNonRoot: true` and a
`seccompProfile` fails even though it sets nothing bad.

Note that this is a shift-left check on rendered manifests in CI, not
admission control - PSS enforcement inside the cluster (Kyverno admission
webhooks or Pod Security Admission namespace labels) is a separate concern.

## Vendored upstream policies

`base/require-ro-rootfs.yaml` and the PSS policies (`base/disallow-*.yaml`,
`base/require-run-as-*.yaml`, `base/restrict-*.yaml`) are vendored verbatim
from [kyverno/policies](https://github.com/kyverno/policies) at the commit
pinned in `vendor-upstream-policies.sh`. Do not edit them by hand; to update,
bump `UPSTREAM_REF` in the script and run `just vendor-policies`, then review
the diff and run `just test-policies`.

Known difference: the upstream `require-ro-rootfs` only validates
`spec.containers`, not init containers. The gap is pinned down by an explicit
fixture in `tests/` so an upstream change shows up as a test diff.

All other policies are project-specific and maintained here. Every rule must
have at least one passing and one failing fixture in `tests/` - a broken
JMESPath expression silently passes everything, which is exactly what the
test suite exists to catch.

## Accepting a known finding

If a component can't satisfy a policy (e.g. an upstream chart doesn't expose
the required field), don't silence it centrally. Add a `PolicyException`
scoped to that resource in `components/<name>/policy-exceptions.yaml`, with
a `reason` annotation explaining why it's accepted - see
`components/frost/policy-exceptions.yaml` for an example.

`just verify-policies` and the `scan-kyverno-policies` CI job all pick up every `components/*/policy-exceptions.yaml`
automatically and pass it to `kyverno apply --exception`.
Excepted rules show up as `skip` in the policy report and gitlab testreport instead of `fail`.
