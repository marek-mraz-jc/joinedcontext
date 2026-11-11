# DataSpaceParticipant

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `DataSpaceParticipant`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/dataspaceparticipants`
- Repository path: `dataspace/participant.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (DataSpaceParticipantSpec)

Desired specification of a `DataSpaceParticipant` resource (DS-04, DS-06, DS-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `connectorUrl` | string | yes | HTTPS base URL of the local connector protocol endpoint (DS-07). |
| `credentialIssuer` | string | yes | HTTPS base URL of the Verifiable Credential issuer (DS-04, DS-07). |
| `did` | [Did](#did) | yes | Participant decentralized identifier `did:web:{orgDomain}` (DS-04, DS-07). |
| `engine` | [ConnectorEngine](#connectorengine) | no | Connector protocol engine implementation (DS-06, DS-07). |
| `trustAnchors` | list of [Did](#did) | no | Trusted credential issuer and authority DIDs (DS-07). |

## Did

Decentralized Identifier in the did:web method bound to an organization domain (DS-04, PF-41)

Type: string

## ConnectorEngine

Supported data space connector runtime engine (DS-06).

Type: one of `"rainbow"`, `"edc"`
