# ngsild-jsonld

The JSON-LD subset NGSI-LD uses (ETSI GS CIM 009 V1.9.1, clause 5.5.7, and JSON-LD 1.1), written
for NGSI-LD documents: `@context` processing, expansion with the structural validation of an
Entity, compaction back to the caller's terms, and a caching loader with the pinned core
`@context` (published as `ngsild-jsonld` once released).

| What | CIM 009 clause |
|---|---|
| `Context`: term definitions merged from the core and a user `@context`, `expand_key`, `compact_iri` | 5.5.7, 5.3.2.1 |
| `expand_entity`: an Entity or fragment expanded and checked (id, type, Property, Relationship, GeoProperty, LanguageProperty, VocabProperty, ListProperty, ListRelationship, JsonProperty) | 4.5, 5.2.4, 5.5.4 |
| `compact_entity`: the stored form back to the caller's `@context` | 5.5.7 |
| `core_context`, `CORE_CONTEXT`: the core `@context` V1.9, pinned in the crate | 5.3.2.1 |
| `Loader`: remote `@context` documents fetched, cached and bounded | 5.5.7, 6.3.5 |

```rust
use ngsild_jsonld::{compact_entity, core_context, expand_entity, ExpandOpts};
use serde_json::json;

let mut ctx = core_context();
ctx.merge_object(json!({"brandName": "https://smartdatamodels.org/brandName"})
    .as_object().unwrap())?;
ctx.freeze(); // builds the inverse map compaction reads; call once after the merges

let payload = json!({
    "id": "urn:ngsi-ld:Vehicle:A4567",
    "type": "Vehicle",
    "brandName": {"type": "Property", "value": "Mercedes"}
});
let stored = expand_entity(payload.as_object().unwrap(), &ctx, ExpandOpts::default())?;
assert!(stored.get("https://smartdatamodels.org/brandName").is_some());

// Back to the caller's terms.
let answer = compact_entity(&stored, &ctx);
assert_eq!(answer["brandName"]["value"], "Mercedes");

// A structural violation is a typed 400, never a panic.
let bad = json!({"id": "urn:ngsi-ld:Vehicle:B", "type": "Vehicle",
                 "speed": {"type": "Property"}}); // a Property without a value
let err = expand_entity(bad.as_object().unwrap(), &ctx, ExpandOpts::default()).unwrap_err();
assert_eq!(err.status(), 400);
# Ok::<(), antares_model::NgsiError>(())
```

Bounds: a request names at most `MAX_CONTEXT_URLS` (32) remote `@context` documents and the
loader follows at most `MAX_REDIRECTS` (3) redirects per fetch; the `EgressPolicy` refuses
loopback and private addresses unless the embedder allows them. Nesting is bounded by the JSON
parser: `serde_json` refuses a document nested deeper than 128 levels before expansion sees it.

The crate is not a general JSON-LD 1.1 processor: no `@graph`, framing, RDF conversion or
remote `@context` without the loader. It does not store or query Entities.

Licensed under the EUPL-1.2, as the rest of the Antares workspace.
