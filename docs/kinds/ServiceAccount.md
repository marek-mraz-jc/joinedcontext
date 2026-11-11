# ServiceAccount

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `ServiceAccount`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/serviceaccounts`
- Repository path: `projects/{project}/access/serviceaccounts/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (ServiceAccountSpec)

Desired specification of a `ServiceAccount` resource (PF-34, PF-35, PF-47).

| Field | Type | Required | Description |
|---|---|---|---|
| `credentials` | list of [Credential](#credential) | yes | Authentication credentials registered for this identity. |
| `delegation` | [Delegation](#delegation) | no | Whether tokens this account's client obtains by exchanging a person's own act for that person (ADR-N-038, AG-95). Absent, a token of this client whose subject is a person is refused. |
| `limits` | [ServiceAccountLimits](#serviceaccountlimits) | no | Optional rate limiting configuration. |
| `owner` | [Owner](#owner) | yes | Accountable human owner of this service identity (PF-34). |
| `purpose` | string | yes | Reason why this identity exists, shown in audit reviews. |
| `roles` | list of [RoleBinding](#rolebinding) | yes | Scoped platform role bindings granted to this identity. |
| `workload` | [Workload](#workload) | no | Optional Kubernetes workload binding (PF-47). |

## Credential

Declared credential for a service account (PF-36, PF-37).

**Security note**: The security property of this kind is the complete absence of secret fields. `Credential` has nowhere to put a secret and carries `deny_unknown_fields`, so `secret:`, `value:`, `password:`, `clientSecret:` and `apiKey:` are all rejected at parse time (PF-36). The key id and the Argon2id hash are stored in the database / Keycloak / secret store, never in manifest state or Git.

| Field | Type | Required | Description |
|---|---|---|---|
| `expiresAt` | string (date-time) | no | Optional expiration timestamp (UTC). |
| `ipAllowList` | list of string | no | Whitelist of allowed client IP CIDR blocks (PF-36). |
| `jwks` | [Jwks](#jwks) | no | The public keys a device outside the cluster signs its client assertion with (PF-47, T-4015); only on an `oauth-client` credential, never beside `workload`. |
| `kind` | [CredentialKind](#credentialkind) | yes | Type of credential. |
| `name` | string | yes | Credential identifier name (DNS-1123 label). |

## Delegation

How an account's client may act for a person (ADR-N-038, AG-95).

Type: `"token-exchange"`

## ServiceAccountLimits

Rate limiting bounds for a service account.

| Field | Type | Required | Description |
|---|---|---|---|
| `requestsPerMinute` | integer (uint32) | yes | Maximum allowed requests per minute. |

## Owner

Accountable human owner reference (PF-34).

| Field | Type | Required | Description |
|---|---|---|---|
| `user` | string | yes | Identifier or username of the accountable human user. |

## RoleBinding

Scoped role grant assignment.

| Field | Type | Required | Description |
|---|---|---|---|
| `operations` | list of [OperationRef](#operationref) | no | CIM 009 operations or operation groups granted (R8). |
| `role` | string | yes | Role template name (DNS-1123 label). |
| `scope` | [RoleScope](#rolescope) | yes | Target hierarchical scope of this grant. |
| `types` | list of string | no | Optional entity types whitelist. |
| `validity` | [BindingValidity](#bindingvalidity) | no | When the grant applies; outside the window it grants nothing, and the Portal removes it once `notAfter` has passed (AG-131). |

## Workload

Cluster workload binding container (PF-47).

| Field | Type | Required | Description |
|---|---|---|---|
| `kubernetes` | [KubernetesBinding](#kubernetesbinding) | yes | Kubernetes pod workload binding. |

## Jwks

A JSON Web Key Set of public keys (RFC 7517), the device's half of its key pair.

| Field | Type | Required | Description |
|---|---|---|---|
| `keys` | list of [PublicJwk](#publicjwk) | yes | One to five public keys, each with its own `kid`. |

## CredentialKind

Category of service account credential.

Type: `"oauth-client"`

## OperationRef

Reference to a CIM 009 operation or operation group (R8).

Closed enum ensures no proprietary verbs (`read`, `write`, `upsertEntity`) are permitted, satisfying the strict R8 CIM 009 vocabulary guarantee.

Type: [Operation](#operation) or [OperationGroup](#operationgroup)

## RoleScope

Target scope of a role grant: exactly one of `contextSpace`, `project`, or `organization`.

| Field | Type | Required | Description |
|---|---|---|---|
| `contextSpace` | string | no | Context Space scope target. |
| `organization` | string | no | Organization scope target. |
| `project` | string | no | Project scope target. |

## BindingValidity

When a binding applies; absent bounds are open.

| Field | Type | Required | Description |
|---|---|---|---|
| `notAfter` | string (date-time) | no | Not in force after this instant (UTC). |
| `notBefore` | string (date-time) | no | Not in force before this instant (UTC). |

## KubernetesBinding

Kubernetes ServiceAccount binding coordinates (PF-47).

| Field | Type | Required | Description |
|---|---|---|---|
| `namespace` | string | yes | Kubernetes namespace name. |
| `serviceAccount` | string | yes | Kubernetes ServiceAccount name. |

## PublicJwk

One public key (RFC 7517/7518). It has no member for a private or symmetric key, so a JWK carrying `d`, `p`, `q`, `dp`, `dq`, `qi` or `k` does not read at all.

| Field | Type | Required | Description |
|---|---|---|---|
| `alg` | string | no | Intended algorithm: `ES256` for an `EC` key, `RS256` for an `RSA` key. |
| `crv` | string | no | Curve of an `EC` key: `P-256`. |
| `e` | string | no | Public exponent of an `RSA` key, base64url. |
| `kid` | string | yes | Key id, unique within the set; the device names it in its assertion header. |
| `kty` | string | yes | Key type: `EC` (P-256) or `RSA` (2048 bits or more). |
| `n` | string | no | Modulus of an `RSA` key, base64url. |
| `use` | string | no | Intended use: `sig`. |
| `x` | string | no | X coordinate of an `EC` key, base64url. |
| `y` | string | no | Y coordinate of an `EC` key, base64url. |

## Operation

Table 4.20-1: every named distributed API operation in ETSI GS CIM 009 (R8).

Closed enum ensures no proprietary verbs (`upsertEntity`, `read`, `write`) are accepted, satisfying the strict R8 CIM 009 vocabulary guarantee.

Type: one of `"createEntity"`, `"updateEntity"`, `"appendAttrs"`, `"updateAttrs"`, `"deleteAttrs"`, `"deleteEntity"`, `"createBatch"`, `"upsertBatch"`, `"updateBatch"`, `"deleteBatch"`, `"upsertTemporal"`, `"appendAttrsTemporal"`, `"deleteAttrsTemporal"`, `"updateAttrInstanceTemporal"`, `"deleteAttrInstanceTemporal"`, `"deleteTemporal"`, `"mergeEntity"`, `"replaceEntity"`, `"replaceAttrs"`, `"mergeBatch"`, `"purgeEntity"`, `"retrieveEntity"`, `"queryEntity"`, `"queryBatch"`, `"retrieveTemporal"`, `"queryTemporal"`, `"retrieveEntityTypes"`, `"retrieveEntityTypeDetails"`, `"retrieveEntityTypeInfo"`, `"retrieveAttrTypes"`, `"retrieveAttrTypeDetails"`, `"retrieveAttrTypeInfo"`, `"createSubscription"`, `"updateSubscription"`, `"retrieveSubscription"`, `"querySubscription"`, `"deleteSubscription"`, `"retrieveEntityMap"`, `"updateEntityMap"`, `"deleteEntityMap"`, `"createEntityMapQueryEntity"`, `"createEntityMapQueryTemporal"`, `"retrieveContextSourceIdentity"`

## OperationGroup

Table 4.20-2: the five distributed operation group names (R8).

Type: one of `"federationOps"`, `"associationOps"`, `"updateOps"`, `"retrieveOps"`, `"redirectionOps"`
