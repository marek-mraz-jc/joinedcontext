# ngsild-residual

The residual of a standard query over an NGSI-LD view (Antares ADR-0028 rules 2 and 7): the part
of an `OData`, CQL2 or `SensorThings` filter that the NGSI-LD Query Language (ETSI GS CIM 009
V1.9.1, clause 4.9) cannot say, as one intermediate form, and one three-valued evaluator over a
normalized NGSI-LD Entity (clause 4.5). A surface pushes what `q=` can say down to the broker
and evaluates the rest here, Entity by Entity (published as `ngsild-residual` once released).

| What | Where it comes from |
|---|---|
| `Expr`, `Path`, `Lit`, `Cmp`, `Func`, `Spatial`: the intermediate form | `OData` URL Conventions 5.1.1, CQL2 7 |
| `compile`: the bounds checked, `now` folded, each pattern built once | ADR-0028 rule 7 |
| `eval`, `Truth`: `True`, `False` or `Unknown` for one Entity | `OData` URL Conventions 5.1.1.1, CQL2 7.4 |
| `DateTime`, `Date`, `Time`: the instants of an NGSI-LD `DateTime` | CIM 009 4.6.3 |

A missing value, a hidden one and a value of the wrong type are all unknown, and so are an
integer overflow, a division by zero and a string past `MAX_STRING_BYTES`: never a wrapped or
truncated value. The caller keeps an Entity only on `Truth::True`.

```rust
use ngsild_residual::{compile, eval, Cmp, DateTime, Expr, Func, Lit, Path, Truth};
use serde_json::json;

// contains(tolower(name), 'sta') and not (pm10 gt 50)
let filter = Expr::And(vec![
    Expr::Call(Func::Contains, vec![
        Expr::Call(Func::ToLower, vec![Expr::Path(Path::value("name"))]),
        Expr::Lit(Lit::Str("sta".into())),
    ]),
    Expr::Not(Box::new(Expr::Cmp(
        Cmp::Gt,
        Box::new(Expr::Path(Path::value("pm10"))),
        Box::new(Expr::Lit(Lit::Int(50))),
    ))),
]);
let now = DateTime::parse("2026-10-11T12:00:00Z").ok_or("now")?;
let program = compile(&filter, now)?;
let station = json!({
    "id": "urn:ngsi-ld:AirQuality:1", "type": "AirQuality",
    "name": {"type": "Property", "value": "Station North"},
    "pm10": {"type": "Property", "value": 12},
});
assert_eq!(eval(&program, &station)?, Truth::True);
// no pm10: `pm10 gt 50` is unknown, and so is its negation
let silent = json!({"id": "urn:ngsi-ld:AirQuality:2", "type": "AirQuality",
    "name": {"type": "Property", "value": "Station South"}});
assert_eq!(eval(&program, &silent)?, Truth::Unknown);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Bounds, all public constants: an expression nests at most `MAX_DEPTH` (64) deep and holds at most
`MAX_NODES` (4096) nodes; one evaluation visits at most `MAX_STEPS` (2^20) nodes, past which it
answers `Error::TooComplex`; a pattern is at most `MAX_PATTERN_BYTES` (4096) bytes and compiles to
at most `MAX_PROGRAM_BYTES`; a geometry holds at most `MAX_GEO_VERTICES` (1024) positions.

The crate does not parse `OData`, CQL2 or `SensorThings` text (each surface does, into `Expr`) and
does not decide what a view hides: it sees the Entity after the view removed it.

Licensed under the EUPL-1.2, as the rest of the Antares workspace.
