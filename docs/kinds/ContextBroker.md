# ContextBroker

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `ContextBroker`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/contextbrokers`
- Repository path: `brokers/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (ContextBrokerSpec)

One broker instance (PF-110).

| Field | Type | Required | Description |
|---|---|---|---|
| `default` | boolean | no | The broker spaces without a `brokerRef` live on; exactly one per organization. |
| `features` | list of [BrokerFeature](#brokerfeature) | no | The surfaces this instance serves; an Endpoint needing one its broker lacks is refused. |
| `image` | string | yes | The Antares image, pinned by digest: `{repository}@sha256:{64 hex}`. |
| `size` | [BrokerSize](#brokersize) | no | A resource preset of the chart; a manifest never asks the node for raw resources. |
| `storageClass` | string | no | The storage class of the broker's database; the cluster default when absent. |

## BrokerFeature

A surface of a broker instance.

Type: one of `"temporal"`, `"ogc"`, `"odata"`, `"mcp"`, `"csr"`

## BrokerSize

A resource preset of the broker chart (ADR-N-047 §2.7).

Type: one of `"small"`, `"medium"`, `"large"`
