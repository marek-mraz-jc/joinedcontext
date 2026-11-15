# sandbox-reaper

A CronJob that deletes sandbox namespaces once they are older than their Time-To-Live
(`docs/Architecture/06-configuration-as-code.md` section 4, OPS-44, PF-19, CC-67).

It does one thing: it lists the namespaces carrying
`sandbox.joinedcontext.com/lifecycle` with the value `unmanaged` or `preview`, computes each
one's age from `metadata.creationTimestamp`, and deletes the ones past the limit with a
foreground cascade. The ceiling is 14 calendar days. A `sandbox.joinedcontext.com/ttl`
annotation may shorten one sandbox's life and can never extend it: a longer value is clamped
back to the ceiling, because an occupant of a sandbox can write that annotation.

A namespace without the label is never a candidate. It is not filtered out further down, it
is never fetched: the label selector goes to the Kubernetes API. Sandboxes are labelled at
creation by the green lane, so an unlabelled namespace is a permanent one and the reaper has
to be wrong in the direction that keeps data.

Its ClusterRole carries `list` and `delete` on namespaces and nothing else. It cannot read a
Secret and cannot touch a workload directly; the workloads go because the namespace around
them does.

`dryRun: true` logs what it would delete and deletes nothing. Worth one cycle on an
installation whose sandboxes are not labelled yet.

## What it does not do

The sandbox Context Space itself is a manifest, and removing it is a repository change, not
a Kubernetes deletion. `jcctl` is what would make it, and `jcctl` cannot write to a running
platform yet (board task T-0421). Until it can, this component reaps the namespace and the
manifest is reaped by whoever created it.

Deploy the component with:

```bash
just sync-component <environment> sandbox-reaper
```

Tests: `tests/test_sandbox_reaper.py`. They run the script out of the rendered ConfigMap
against a stub `kubectl`, so what is tested is the text that reaches the cluster.
