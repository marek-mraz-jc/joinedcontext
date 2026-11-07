# ngsild-matcher

Subscription matching of ETSI GS CIM 009 V1.9.1 against one in-memory Entity: the predicates
the Antares broker applies on its notification path, usable without a broker. A gateway or an
edge component can answer "would this change notify Subscription X" with the broker's own
semantics (published as `ngsild-matcher` once released).

| What | CIM 009 clause |
|---|---|
| `would_notify`: every predicate below at once | 5.8.6 |
| `is_active`: `isActive` and `expiresAt` | 5.8.1.4, 5.2.12 |
| `selector_match`: the `entities` selector, `id`, `idPattern`, `type` and its selection language | 5.2.33, 4.17 |
| `conditions_match`: `q`, `scopeQ` and `geoQ` together | 4.9, 4.19, 4.10 |
| `geo_params`: a Subscription's `geoQ` in the 4.10 parameter shape | Table 5.2.13-1 |
| `throttled`: the `throttling` window since the last notification | 5.2.12 |

```rust
use antares_jsonld::{core_context, expand_entity, ExpandOpts};
use ngsild_matcher::would_notify;
use serde_json::json;

let ctx = core_context();
// The Subscription as stored: selector types are expanded IRIs.
let sub = json!({
    "id": "urn:ngsi-ld:Subscription:speeding",
    "type": "Subscription",
    "entities": [{"type": "https://uri.etsi.org/ngsi-ld/default-context/Vehicle"}],
    "q": "speed>25",
    "geoQ": {"georel": "near;maxDistance==2000", "geometry": "Point",
             "coordinates": "[-3.7,40.4]"},
    "notification": {"endpoint": {"uri": "http://gateway/notify"}}
});
let vehicle = |speed: f64, lon: f64| {
    let payload = json!({
        "id": "urn:ngsi-ld:Vehicle:A1", "type": "Vehicle",
        "speed": {"type": "Property", "value": speed},
        "location": {"type": "GeoProperty",
                     "value": {"type": "Point", "coordinates": [lon, 40.4]}}
    });
    expand_entity(payload.as_object().unwrap(), &ctx, ExpandOpts::default()).unwrap()
};
// No store at hand: a 4.9 linked-entity term resolves to nothing.
assert!(would_notify(&sub, &vehicle(30.0, -3.7), &ctx, &|_| None));
assert!(!would_notify(&sub, &vehicle(20.0, -3.7), &ctx, &|_| None)); // too slow
assert!(!would_notify(&sub, &vehicle(30.0, -4.7), &ctx, &|_| None)); // too far
```

Inputs are the stored forms: the Subscription document as created (selector types expanded) and
the Entity as `antares_jsonld::expand_entity` returns it. A Subscription field the matcher cannot
read (an unparsable `q`, a malformed `geoQ`) matches nothing; nothing here panics. Bounds are the
ones `antares-ql` documents for `q`, `scopeQ`, `geoQ` and `idPattern`.

The crate does not store Subscriptions, send notifications or validate a Subscription on
create; the broker does those.

Licensed under the EUPL-1.2, as the rest of the Antares workspace.
