# DataAgreement

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `DataAgreement`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/dataagreements`
- Repository path: `projects/{project}/dataspace/agreements/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (DataAgreementSpec)

Desired specification of a `DataAgreement` resource (DS-09, DS-10, DS-15).

| Field | Type | Required | Description |
|---|---|---|---|
| `agreementId` | string | yes | Unique Dataspace Protocol agreement identifier (DS-09, DS-13). |
| `constraints` | [AgreementConstraints](#agreementconstraints) | no | Lossless query constraints mapped from contract agreement (DS-10). |
| `offerRef` | [Ref](#ref) | no | Reference to local DataOffer for provider role (DS-07). |
| `remoteParticipant` | [Did](#did) | yes | Decentralized identifier of the remote participant (DS-04, DS-10). |
| `role` | [AgreementRole](#agreementrole) | yes | Operational role in this agreement (provider or consumer) (DS-07, DS-15). |
| `state` | [AgreementState](#agreementstate) | yes | Negotiation lifecycle state of this agreement (DS-09, DS-12). |
| `tokenSecretRef` | [SecretRef](#secretref) | no | Reference to transfer token in secret store for consumer role (DS-15). |
| `validity` | [Validity](#validity) | yes | Temporal validity window for this agreement (DS-10). |

## AgreementConstraints

Lossless query filter constraints mapped from an ODRL agreement (DS-10).

| Field | Type | Required | Description |
|---|---|---|---|
| `geoQ` | string | no | Residual geographic query filter string (DS-10). |
| `q` | string | no | Residual NGSI-LD query filter string (DS-10). |
| `scopeQ` | string | no | Residual scope query filter string (DS-10). |
| `temporalQ` | string | no | Residual temporal query filter string (DS-10). |

## Ref

Reference to another resource, either bare name or typed reference (MF-07).

Type: string or [TypedRef](#typedref)

## Did

Decentralized Identifier in the did:web method bound to an organization domain (DS-04, PF-41)

Type: string

## AgreementRole

Role of this organization within a data space agreement (DS-07, DS-15).

Type: one of `"provider"`, `"consumer"`

## AgreementState

Negotiation state of a Dataspace Protocol contract agreement (DS-09, DS-12).

Type: one of `"requested"`, `"offered"`, `"accepted"`, `"finalized"`, `"terminated"`

## SecretRef

Reference to a credential stored in SOPS or OpenBao (PF-36).

`deny_unknown_fields` is a security control: it rejects inline `value:` or `password:` fields at parse time, enforcing that secrets never appear in Git or manifest files.

| Field | Type | Required | Description |
|---|---|---|---|
| `envVar` | string | no | Optional environment variable name for injection into runners. |
| `key` | string | no | Optional key within secret. |
| `name` | string | yes | Secret name in secret store. |

## Validity

Temporal validity window for a policy grant.

| Field | Type | Required | Description |
|---|---|---|---|
| `from` | string (date-time) | no | Effective start timestamp (UTC). |
| `to` | string (date-time) | no | Effective expiration timestamp (UTC). |

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |
