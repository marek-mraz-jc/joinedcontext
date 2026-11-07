# ngsild-model

The NGSI-LD data model of ETSI GS CIM 009 V1.9.1 as Rust types and checks: no I/O, no clock, no
configuration. It is the crate every other NGSI-LD crate of Antares builds on (published as
`ngsild-model` once released).

| What | CIM 009 clause |
|---|---|
| `EntityId`: an Entity id is a URI or IRI, with no `.`/`..` path segment | 4.5.1, 5.2.1, Table 5.2.4-1 |
| `TenantId`: the `NGSILD-Tenant` name, `[A-Za-z0-9_-]{1,64}`, never a broker-reserved one | 4.14, 6.3.14 |
| `check_attr_name`: an Attribute name in a path is a term or IRI | 4.6.2 |
| `NgsiError` and `ProblemDetails`: the error types and their HTTP status | 5.5.2, 6.3.6, Table 6.3.2-1 |
| `OPERATION_NAMES`, `group_members`: the distributed operation names and groups | 4.20, Tables 4.20-1 and 4.20-2 |
| `parse_iso_duration`: the ISO 8601 durations of `throttling` and `timeInterval` | 5.2.12 |
| `API_ROOT`: `/ngsi-ld/v1` | 6.2 |

```rust
use ngsild_model::operations::group_members;
use ngsild_model::{check_attr_name, EntityId, NgsiError, TenantId};

let id = EntityId::new("urn:ngsi-ld:Vehicle:A4567")?;
assert_eq!(id.as_str(), "urn:ngsi-ld:Vehicle:A4567");

// No scheme, and a dot-segment that would climb out of /entities/{id}: both 400.
for bad in ["A4567", "urn:x/../attrs"] {
    let err = EntityId::new(bad).unwrap_err();
    assert_eq!(err.status(), 400);
    assert_eq!(err.to_problem_details().title, "BadRequestData");
}

assert!(TenantId::new("city-of-helsinki").is_ok());
assert!(TenantId::new("snap-index").is_err()); // a name the broker keeps for itself
assert!(check_attr_name("speed").is_ok());
assert!(check_attr_name("speed(km)").is_err());

assert!(group_members("federationOps").is_some_and(|ops| ops.contains(&"queryEntity")));
# Ok::<(), NgsiError>(())
```

The crate does not parse JSON-LD (`antares-jsonld`), queries (`antares-ql`) or match
subscriptions (`antares-matcher`). Its checks are the cheap, syntax-level ones every entry point
runs first; full IRI validation happens during JSON-LD expansion.

Bounds: a `TenantId` is at most `TenantId::MAX_LEN` (64) characters. No function recurses.

Licensed under the EUPL-1.2, as the rest of the Antares workspace.
