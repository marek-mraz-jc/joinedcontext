# ngsild-ql

The query languages of ETSI GS CIM 009 V1.9.1 parsed into one typed AST, with an in-memory
evaluator and a Postgres lowering (published as `ngsild-ql` once released).

| What | CIM 009 clause |
|---|---|
| `parse_q`, `QNode`: the NGSI-LD Query Language (`q=`), rendered back by `Display` | 4.9 |
| `eval::eval_q`: `q=` over an expanded Entity, linked-entity terms included | 4.9 |
| `geo::GeoQuery`: `georel`, `geometry`, `coordinates`, `geoproperty` | 4.10 |
| `scope::check_scope_q`, `scope::scope_matches`, `scope::intersect_scope_q`: the Scope Query Language | 4.19 |
| `type_selection_matches`: the Entity Type Selection Language | 4.17 |
| `geo::parse_ref_geometry`: the reference geometry of distance ordering | 4.23 |
| `sql`: `q=` lowered to a bind-parameter jsonpath predicate | (backend) |

```rust
use antares_jsonld::{core_context, expand_entity, ExpandOpts};
use ngsild_ql::eval::eval_q;
use ngsild_ql::{parse_q, MAX_Q_BYTES};
use serde_json::json;

let ctx = core_context();
let vehicle = json!({
    "id": "urn:ngsi-ld:Vehicle:A4567",
    "type": "Vehicle",
    "speed": {"type": "Property", "value": 80},
    "brandName": {"type": "Property", "value": "Mercedes"}
});
let entity = expand_entity(vehicle.as_object().unwrap(), &ctx, ExpandOpts::default())?;

let query = parse_q(r#"speed>50;brandName=="Mercedes""#)?;
assert!(eval_q(&query, &entity, &ctx, &|_| None));
assert!(!eval_q(&parse_q("speed<50")?, &entity, &ctx, &|_| None));

// Every refusal is a typed error with its HTTP status, never a panic.
assert_eq!(parse_q("speed>").unwrap_err().status(), 400);
let too_long = "a==1;".repeat(MAX_Q_BYTES);
assert_eq!(parse_q(&too_long).unwrap_err().status(), 403); // TooComplexQuery
let too_deep = format!("{}a==1{}", "(".repeat(100), ")".repeat(100));
assert_eq!(parse_q(&too_deep).unwrap_err().status(), 403);
# Ok::<(), antares_model::NgsiError>(())
```

Bounds, all public constants: a `q=` of at most `MAX_Q_BYTES` (4096) bytes, nested at most
`MAX_Q_DEPTH` (64) parentheses deep and at most `MAX_Q_NODES` (512) nodes; a linked-entity `q=`
resolves at most `eval::MAX_Q_LINK_LOOKUPS` (512) Entities; a request geometry has at most
`geo::MAX_GEO_VERTICES` (1024) positions; a `scopeQ` at most `scope::MAX_SCOPE_Q_BYTES` (4096)
bytes; a regular expression compiles to at most `regex::MAX_REGEX_PROGRAM_BYTES`. The parser
refuses before it recurses past its depth, so a hostile query cannot overflow the stack.

The crate does not fetch an `@context` (it takes an `antares_jsonld::Context`), run a query
against a store, or decide who may see what.

Licensed under the EUPL-1.2, as the rest of the Antares workspace.
