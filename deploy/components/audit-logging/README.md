# audit-logging

A Vector daemonset that ships the audit trail of the Context Gateway, Keycloak and the forge
to append-only object storage (`docs/Deployment/05-monitoring-logging.md` section 3, OPS-42,
R42, AG-19).

One collector per node reads that node's container logs, parses the structured JSON each
component writes, annotates every record with the pod, namespace and node it came from, and
writes gzipped newline-delimited JSON to `audit/{component}/{date}/` in the bucket.

A record joins the trail because of the pod it came from, never because of what it says. The
membership rule is a label selector evaluated by the Kubernetes API, so a pod that does not
match is never read and no component can drop itself out of the trail by changing what it
logs.

Timestamps are parsed as RFC 3339. A record whose own timestamp does not parse keeps the
collector's ingest time and is tagged `jc_timestamp_source: ingest`; a line that is not JSON
at all keeps its text and is tagged `jc_structured: false`. Neither is dropped, because a
malformed audit record is still evidence.

## What the operator supplies

The bucket and the credential. The component is off until `bucket` is named: a collector
shipping into a bucket that does not exist looks healthy while the trail goes nowhere.

- The bucket carries S3 Object Lock in **compliance** mode with 90 day retention. Governance
  mode lets a sufficiently privileged account shorten a lock, and the render is refused if
  the values claim it.
- `audit-logging-s3` is a Secret holding `ACCESS_KEY_ID` and `SECRET_ACCESS_KEY` for a
  credential that may `PutObject` and nothing else. It is read through `envFrom`; no
  credential is ever rendered from values (CC-06).

The disk buffer blocks when full rather than dropping the newest records, so a long sink
outage backs the collector up instead of losing exactly the records the incident is about.

Deploy the component with:

```bash
just sync-component <environment> audit-logging
```

Tests: `tests/test_audit_logging.py`. They hand the rendered ConfigMap to the pinned Vector
image for `vector validate` and for `vector test`, so the transform under test is the one
that runs.
