# DataOffer

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `DataOffer`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/dataoffers`
- Repository path: `projects/{project}/spaces/{space}/dataspace/offers/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (DataOfferSpec)

Desired specification of a `DataOffer` resource (DS-07, DS-08, DS-14).

| Field | Type | Required | Description |
|---|---|---|---|
| `containsPersonalData` | boolean | no | Whether the offered data contains personal data (DS-14). |
| `contextSpaceRef` | string | yes | Target Context Space name offering context data (DS-07). |
| `endpointRefs` | list of [Ref](#ref) | yes | References to offered Endpoint resources (DS-07, DS-08). |
| `policy` | any | yes | Verbatim ODRL 2.2 offer in the `ngsi-ld:` profile (DS-07, R52). |
| `purpose` | string | no | Optional Data Privacy Vocabulary (DPV) purpose IRI (DS-14). |

## Ref

Reference to another resource, either bare name or typed reference (MF-07).

Type: string or [TypedRef](#typedref)

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |
