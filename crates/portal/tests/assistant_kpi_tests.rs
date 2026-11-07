//! The assistant's indicator step, the parts that do not need a model (T-0583, PF-54,
//! PF-55): the aggregate over what the endpoint served, the entity with its provenance, and
//! the indicator space's endpoint found in the mirror. The step itself, from a model answer
//! to the card, runs in `kit_pass_tests.rs`.
use std::sync::Arc;

use joinedcontext_portal::agents::kpi::{self, Agg, ComputeKpi, Provenance};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::json;

fn params() -> ComputeKpi {
    serde_json::from_value(json!({
        "name": "closed-stations",
        "title": "Closed stations",
        "type": "BikeHireDockingStation",
        "attribute": "",
        "agg": "count",
        "unit": "C62",
        "q": "status==\"closed\""
    }))
    .expect("a call")
}

#[test]
fn a_count_with_a_filter_becomes_an_entity_the_platform_accepts() {
    let rows = vec![json!({ "id": "a" }), json!({ "id": "b" })];
    let (value, count) = kpi::compute(&rows, "", Agg::Count);
    assert_eq!((value, count), (Some(2.0), 2));
    let provenance = Provenance {
        org_domain: "hel.fi",
        project: "helsinki",
        endpoint_space: "helsinki",
        endpoint_name: "helsinki-bikes",
        run_id: "run-1",
        now: "2026-09-13T08:00:12Z",
    };
    let entity = kpi::entity(&params(), 2.0, &provenance).expect("an indicator");
    entity.validate().expect("PF-55 holds");
    let json = entity.to_json();
    assert_eq!(
        json["id"],
        "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:closed-stations"
    );
    assert_eq!(
        json["calculationFormula"]["value"],
        "count(BikeHireDockingStation) where status==\"closed\""
    );
    assert_eq!(json["currentValue"]["unitCode"], "C62");
    assert_eq!(
        json["computedBy"]["value"],
        "urn:ngsi-ld:AgentRun:hel.fi:helsinki:run-1"
    );
    assert!(json["@context"].is_array(), "the card posts it as JSON-LD");
}

#[test]
fn the_indicator_space_endpoint_is_the_one_whose_space_is_project_kpi() {
    let mirror = Arc::new(Mirror::new());
    for (name, space, slug) in [
        (
            "helsinki-bikes",
            "helsinki",
            "bikes00000000000000000000000000a",
        ),
        (
            "helsinki-indicators",
            "helsinki-kpi",
            "kpi000000000000000000000000000a",
        ),
    ] {
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: ObjectMeta::new(name, "helsinki"),
            spec: json!({ "slug": slug, "contextSpaceRef": { "kind": "ContextSpace", "name": space } }),
            status: None,
        });
    }
    let state = AppState::new(Config::for_tests(), None).with_mirror(mirror);
    assert_eq!(
        kpi::kpi_endpoint(&state, "helsinki"),
        Some((
            "kpi000000000000000000000000000a".to_owned(),
            "helsinki-indicators".to_owned()
        ))
    );
    assert_eq!(kpi::kpi_endpoint(&state, "espoo"), None);
    assert_eq!(kpi::kpi_endpoint(&state, "Not A Project"), None);
}
