//! `GET …/spaces/{space}/quality/checks` reads the gateway's counters by the space's tenant
//! segment, which is how the gateway labels `jc_quality_checks_total` (DM-94, T-3903).

mod common;

use std::collections::BTreeMap;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::quality_checks::Reading;
use joinedcontext_portal::state::AppState;

fn reading(at: &str, counts: &[(&str, &str, f64)]) -> Reading {
    Reading {
        at: at.parse().expect("an instant"),
        counts: counts
            .iter()
            .map(|(space, outcome, value)| {
                (
                    (
                        (*space).to_owned(),
                        "Car".to_owned(),
                        "speed-plausible".to_owned(),
                        (*outcome).to_owned(),
                    ),
                    *value,
                )
            })
            .collect::<BTreeMap<_, _>>(),
    }
}

/// The space `air` of `city` is tenant `city-air`; another project's space pinned to the
/// segment `air` writes its own counters under that label.
#[tokio::test]
async fn the_checks_are_the_spaces_own_by_its_segment_never_a_namesakes() {
    let state = AppState::new(Config::for_tests(), None);
    state.mirror.upsert(envelope(
        "ContextSpace",
        "air",
        "city",
        json!({ "quality": [{ "name": "speed-plausible", "entityType": "Car",
                              "kind": "range", "attribute": "speed", "max": 60 }] }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "space-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace", "Project"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "air-readers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "space-reader",
                "scope": { "contextSpace": "air" } }),
    ));
    state.quality_checks.record(reading(
        "2026-10-12T00:00:00Z",
        &[("city-air", "pass", 0.0), ("air", "reject", 0.0)],
    ));
    state.quality_checks.record(reading(
        "2026-10-12T06:00:00Z",
        &[("city-air", "pass", 40.0), ("air", "reject", 999.0)],
    ));
    let answer = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/city/spaces/air/quality/checks",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("JSON");
    let day = &body["rules"][0]["24h"];
    assert_eq!(body["rules"][0]["rule"], "speed-plausible", "{body}");
    assert_eq!(day["pass"], 40, "{body}");
    assert_eq!(
        day["reject"], 0,
        "a namesake's counters are not this space's: {body}"
    );
}
