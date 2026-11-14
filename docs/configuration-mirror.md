# Configuration off the pod: the push mirror

Task T-6892. Every configuration write, an MCP write included, ends as a commit on shako's `main` (a Change
merged on approval). shako keeps those repositories on the one node's volume. The configuration mirror pushes
them to a private remote the operator names, so a lost volume does not lose the configuration.

## 1. Setting

The mirror is off by default. A deployment turns it on with two Portal variables, set together or not at all:

| Variable | Value | Example |
|---|---|---|
| `JC_CONFIG_MIRROR_REMOTE` | an `https` address with `{repo}`, the forge repository's name; no credential, query or fragment in it | `https://github.com/acme/jc-{repo}.git` |
| `JC_CONFIG_MIRROR_TOKEN_FILE` | a file on the secrets mount holding the token, nothing else | `/var/run/secrets/jc/config-mirror-token` |

The Portal refuses to start when only one variable is set, when the address is not `https`, has no `{repo}`
or carries a credential, or when the token file cannot be read. The Portal reads the token file again for
every mirror it sets up, so a rotated token is used without a restart.

The token needs contents write on the remote repositories and nothing more. The operator creates one private
repository per forge repository: `jc-configuration` for the installation repository and `jc-<project
repository>` for each project. On github.com the Portal asks GitHub whether the repository is private and
refuses a public one or a missing one; on another host the operator vouches for privacy.

## 2. What is mirrored

- The installation repository (`joinedcontext/configuration`) and every project repository.
- `main` and new tags only. An open Change's branch never leaves the pod.
- Fast-forward only. A remote whose `main` holds commits that are not on the forge's `main` is reported, never
  overwritten; a tag that points elsewhere on the remote is reported the same way.
- The installation repository holds every Organization, so it goes to the operator's remote only: the Portal
  removes any other push mirror on it. A project repository keeps other mirrors beside the operator's one.

Committed manifests carry `secretRef` only (the secret scan in `ops::call` refuses a value), so the remote
receives what `main` holds and no secret.

## 3. When a push fails

shako pushes on every commit to `main`, inside the merge, and records the outcome; a failed push never fails
the merge. The reconciler's leader looks at every mirror once a minute: a repository without its mirror gets
one, and the first push catches up on everything merged before. A mirror whose last push failed is set up again
with the current token and pushed, after a backoff that starts at one minute and doubles up to 30 minutes. The
next push that goes through sends everything missed.

Each failed push counts in `jc_forge_mirror_failures_total{repository}`. The organization health page
(`GET /api/v1/organization/health`, check `configuration-mirror`) lists each repository, green or with the
reason its push failed, as the leader replica last saw it.

A per-Organization remote would need a remote and a token per Organization, each in its own Secret, and the
rule that a project repository follows its Organization's remote. This version has the operator's remote only.

## Related

- `crates/portal/src/forge/config_mirror.rs`, `crates/shako/git/src/send_pack.rs`
- Tests: `shako-server` `tests/config_mirror.rs`, `shako-git` `send_pack::tests`, Portal `forge::config_mirror::tests`
