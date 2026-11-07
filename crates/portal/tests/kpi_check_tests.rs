//! Checking a KPI over several sources (API/01 §55, PL-102…PL-105, T-4935): jc-core's rules with
//! the field at fault, each source's readability for the pipeline's account, and the preview on
//! the sources' current pages read as the person.

mod common;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};
use joinedcontext_portal::api::kpi_check::records_verdict;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_validation::ModelSchema;
use joinedcontext_portal::reconciler::views::ViewSync;
use joinedcontext_portal::state::AppState;

const PARKING: &str = "p4y7pq2mzt6vhx3nbwrs5cjdaf";
const STOPS: &str = "s3nq7pzk2mtv6hx5brj4gcdsfa";
const SECRET: &str = "x7nq7pzk2mtv6hx5brj4gcdsfa";
const WRITER: &str = "w3nq7pzk2mtv6hx5brj4gcdsfa";
const TOKEN: &str = "an-admin-token-of-at-least-32-bytes!";
const ROUTE: &str = "/api/v1/projects/city/kpis/check";

/// The spaces `mobility` (parking and stops) and `secret` of `city`, as the reconciler reads them.
fn repository(dir: &Path) -> jcctl::loader::Repository {
    let write = |rel: &str, body: String| {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    };
    let head = |kind: &str, name: &str| {
        format!("apiVersion: joinedcontext.com/v1alpha1\nkind: {kind}\nmetadata:\n  name: {name}\n  namespace: city\nspec:\n")
    };
    for space in ["mobility", "secret", "city-kpi"] {
        write(
            &format!("projects/city/spaces/{space}/space.yaml"),
            format!("{}  isSandbox: false\n", head("ContextSpace", space)),
        );
    }
    for (space, name, slug) in [
        ("mobility", "parking", PARKING),
        ("mobility", "stops", STOPS),
        ("secret", "hidden", SECRET),
        ("city-kpi", "kpi-writer", WRITER),
    ] {
        write(
            &format!("projects/city/spaces/{space}/endpoints/{name}.yaml"),
            format!("{}  contextSpaceRef: {space}\n  slug: {slug}\n  audience: organization\n  enabledRepresentations: [ngsi-ld]\n", head("Endpoint", name)),
        );
    }
    write(
        "projects/city/spaces/mobility/policies/read.yaml",
        format!("{}  contextSpaceRef: mobility\n  assigner: did:web:city.example\n  assignee: {{ kind: role, id: public }}\n  operations: [retrieveOps]\n  information:\n    - entities: [{{ type: ParkingSite }}, {{ type: TransitStop }}]\n      propertyNames: [occupied, district]\n", head("Policy", "read")),
    );
    write(
        "projects/city/spaces/city-kpi/policies/write.yaml",
        format!("{}  contextSpaceRef: city-kpi\n  assigner: did:web:city.example\n  assignee: {{ kind: serviceAccount, id: pipelines }}\n  operations: [upsertBatch, createBatch, queryBatch]\n  information:\n    - entities:\n        - type: KeyPerformanceIndicator\n      propertyNames: [name, calculationFormula, currentValue, calculationPeriod, updatedAt, derivedFrom, computedBy]\n", head("Policy", "write")),
    );
    jcctl::loader::Repository::load(dir).expect("the repository loads")
}

/// A broker whose parking page has districts A (10 + 5 occupied) and B (6), and whose stops
/// page has two stops in A and one in B.
async fn broker() -> MockServer {
    let broker = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/q/views"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&broker)
        .await;
    let row = |id: &str, district: &str, occupied: Option<u32>| {
        let mut row = json!({ "id": id, "type": "T",
            "district": { "type": "Property", "value": district } });
        if let Some(n) = occupied {
            row["occupied"] = json!({ "type": "Property", "value": n });
        }
        row
    };
    for (entity_type, rows) in [
        (
            "ParkingSite",
            vec![
                row("p1", "A", Some(10)),
                row("p2", "A", Some(5)),
                row("p3", "B", Some(6)),
            ],
        ),
        (
            "TransitStop",
            vec![
                row("s1", "A", None),
                row("s2", "A", None),
                row("s3", "B", None),
            ],
        ),
    ] {
        Mock::given(method("POST"))
            .and(path_regex("^/q/views/[a-z0-9-]+/preview$"))
            .and(query_param("type", entity_type))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "count": 3, "entities": rows })),
            )
            .mount(&broker)
            .await;
    }
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&broker)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&broker)
        .await;
    broker
}

struct Rig {
    state: AppState,
    broker: MockServer,
}

/// jana may propose pipelines in `city` and read its space `mobility`; eva only reads it. The
/// pipeline `kpi`'s account holds a read grant on TransitStop.
async fn rig() -> Rig {
    rig_on(Config::for_tests()).await
}

async fn rig_on(config: Config) -> Rig {
    let broker = broker().await;
    let mut state = AppState::new(config, None);
    for space in ["mobility", "secret", "city-kpi"] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, "city", json!({})));
    }
    for (space, name, slug) in [
        ("mobility", "parking", PARKING),
        ("mobility", "stops", STOPS),
        ("secret", "hidden", SECRET),
        ("city-kpi", "kpi-writer", WRITER),
    ] {
        state.mirror.upsert(envelope(
            "Endpoint",
            name,
            "city",
            json!({ "contextSpaceRef": space, "slug": slug, "audience": "organization" }),
        ));
    }
    state.mirror.upsert(envelope(
        "Policy",
        "pl-kpi-stops",
        "city",
        json!({ "contextSpaceRef": "mobility", "operations": ["retrieveOps"],
                "assignee": { "kind": "serviceAccount", "id": "city/pl-kpi" },
                "information": [{ "entities": [{ "type": "TransitStop" }] }] }),
    ));
    for (role, rules) in [
        (
            "pipeline-author",
            json!([{ "kinds": ["Project"], "verbs": ["read"] },
                   { "kinds": ["Pipeline"], "verbs": ["read", "propose"] }]),
        ),
        (
            "space-reader",
            json!([{ "kinds": ["ContextSpace", "Project"], "verbs": ["read"] }]),
        ),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": rules }),
        ));
    }
    // jana proposes pipelines anywhere in the project and reads only the space `mobility`.
    for (name, who, role, scope) in [
        (
            "jana-pipelines",
            "jana",
            "pipeline-author",
            json!({ "project": "city" }),
        ),
        (
            "jana-mobility",
            "jana",
            "space-reader",
            json!({ "contextSpace": "mobility" }),
        ),
        (
            "eva-mobility",
            "eva",
            "space-reader",
            json!({ "contextSpace": "mobility" }),
        ),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            name,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": role, "scope": scope }),
        ));
    }
    let dir = std::env::temp_dir().join(format!(
        "portal-kpi-check-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
    ));
    let token = dir.join("token");
    let repo = repository(&dir);
    std::fs::write(&token, format!("{TOKEN}\n")).expect("write the token");
    let views = ViewSync::new(broker.uri(), &token);
    views.converge(&repo).await;
    state.views = Some(Arc::new(views));
    Rig { state, broker }
}

fn draft(formula: &str, second: &str) -> Value {
    json!({ "pipeline": {
        "apiVersion": "joinedcontext.com/v1alpha2", "kind": "Pipeline",
        "metadata": { "name": "kpi" },
        "spec": {
            "class": "scheduled", "schedule": "0 * * * *",
            "sources": [
                { "name": "parking", "endpointRef": { "kind": "Endpoint", "name": "parking" },
                  "query": { "type": "ParkingSite", "attrs": ["occupied", "district"] } },
                { "name": "stops", "endpointRef": { "kind": "Endpoint", "name": second },
                  "query": { "type": "TransitStop", "attrs": ["district"] } }
            ],
            "steps": [{ "kind": "kpi", "kpi": {
                "name": "parking-per-stop", "formula": formula, "unitCode": "C62",
                "join": { "by": "area", "keys": { "parking": "district", "stops": "district" } } } }],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:city-kpi:kpi-writer" }]
        }
    }, "preview": true })
}

async fn previews(broker: &MockServer) -> usize {
    broker
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.url.path().ends_with("/preview"))
        .count()
}

/// PL-104, PL-105: one value per district both sources have, computed on the pages the person
/// reads; the stops source is readable by the pipeline's grant, parking is not yet.
#[tokio::test]
async fn a_valid_draft_previews_one_value_per_area() {
    let r = rig().await;
    let answer = send(
        &r.state,
        person("jana"),
        "POST",
        ROUTE,
        Some(draft("sum(parking.occupied) / count(stops)", "stops")),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("JSON");
    assert_eq!(body["problems"], json!([]));
    assert_eq!(
        body["sources"],
        json!([
            { "name": "parking", "endpoint": "parking", "readable": false },
            { "name": "stops", "endpoint": "stops", "readable": true }
        ])
    );
    assert_eq!(
        body["preview"],
        json!({
            "values": [
                { "key": "A", "name": "parking-per-stop-a", "value": 7.5 },
                { "key": "B", "name": "parking-per-stop-b", "value": 6.0 }
            ],
            "rows": { "parking": 3, "stops": 3 }
        })
    );
}

/// PL-103: a formula naming an attribute the source does not read is refused at its field, in
/// words, and nothing is read from the broker.
#[tokio::test]
async fn a_refused_formula_names_its_field_and_reads_nothing() {
    let r = rig().await;
    let answer = send(
        &r.state,
        person("jana"),
        "POST",
        ROUTE,
        Some(draft("sum(parking.free) / count(stops)", "stops")),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("JSON");
    let problems = body["problems"].as_array().expect("problems");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0]["field"]
            .as_str()
            .is_some_and(|f| f.starts_with("spec.")),
        "{problems:?}"
    );
    assert!(
        problems[0]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("free")),
        "{problems:?}"
    );
    assert!(body.get("preview").is_none());
    assert_eq!(previews(&r.broker).await, 0);
}

/// SP-06: a source in a space the person does not read is an unknown Endpoint, the same words
/// as one that is not there, and the broker is never asked.
#[tokio::test]
async fn a_source_in_an_unread_space_is_unknown() {
    let r = rig().await;
    let hidden = send(
        &r.state,
        person("jana"),
        "POST",
        ROUTE,
        Some(draft("sum(parking.occupied) / count(stops)", "hidden")),
    )
    .await;
    let absent = send(
        &r.state,
        person("jana"),
        "POST",
        ROUTE,
        Some(draft("sum(parking.occupied) / count(stops)", "absent")),
    )
    .await;
    let body = |text: &str| serde_json::from_str::<Value>(text).expect("JSON")["problems"].clone();
    assert_eq!(hidden.status, StatusCode::OK, "{}", hidden.text);
    assert_eq!(
        body(&hidden.text),
        json!([{ "field": "spec.sources[1].endpointRef",
                 "reason": "no Endpoint 'hidden' in project 'city' that you may read" }])
    );
    assert_eq!(
        body(&absent.text)[0]["reason"],
        "no Endpoint 'absent' in project 'city' that you may read"
    );
    assert_eq!(previews(&r.broker).await, 0);
}

/// PF-52: a reader who may not propose a pipeline is refused before anything is read; a workload
/// token's 403 is the authorization matrix's service-account cell (tests/authz_matrix.yaml).
/// The rig with `mobility`'s model: `occupied` in UCUM minutes, `capacity` a plain count.
fn with_model(rig: &Rig) {
    rig.state.model_schemas.replace(HashMap::from([(
        ("city".to_owned(), "mobility".to_owned()),
        Arc::new(ModelSchema::compile(
            "mobility",
            "1.0.0",
            &json!({ "definitions": {
                "ParkingSite": { "properties": {
                    "id": { "type": "string" }, "type": { "type": "string" },
                    "occupied": { "type": ["number", "null"], "x-ngsi-ld-kind": "Property", "x-unit": { "ucumCode": "min" } },
                    "district": { "type": ["string", "null"], "x-ngsi-ld-kind": "Property" }
                } },
                "TransitStop": { "properties": {
                    "id": { "type": "string" }, "type": { "type": "string" },
                    "district": { "type": ["string", "null"], "x-ngsi-ld-kind": "Property" }
                } }
            }}),
            &["ParkingSite".to_owned(), "TransitStop".to_owned()],
            false,
        )),
    )]));
}

/// The check's answer to jana.
async fn checked(rig: &Rig, body: Value) -> Value {
    let answer = send(&rig.state, person("jana"), "POST", ROUTE, Some(body)).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    serde_json::from_str(&answer.text).expect("JSON")
}

/// PL-103: the editor adds whatever the formula names to the source's attributes, so a typo is
/// caught against the model; else the broker answers no rows and the sum reads as a real 0.
#[tokio::test]
async fn an_attribute_the_model_lacks_is_refused_at_its_source() {
    let rig = rig().await;
    with_model(&rig);
    let mut body = draft("sum(parking.noSuchField) / count(stops)", "stops");
    body["pipeline"]["spec"]["sources"][0]["query"]["attrs"] = json!(["noSuchField", "district"]);
    let answer = checked(&rig, body).await;
    assert_eq!(
        answer["problems"][0]["field"], "spec.sources[0].query.attrs",
        "{answer}"
    );
    let reason = answer["problems"][0]["reason"].as_str().expect("a reason");
    assert!(
        reason.contains("`noSuchField`") && reason.contains("ParkingSite"),
        "{reason}"
    );
    assert_eq!(
        previews(&rig.broker).await,
        0,
        "a refused draft reads nothing"
    );
}

/// PL-103: a model names most units in UCUM; the check holds them to the declared UN/CEFACT code.
#[tokio::test]
async fn a_unit_the_model_names_in_ucum_is_held_to_the_declared_code() {
    let rig = rig().await;
    with_model(&rig);
    let mut body = draft("sum(parking.occupied) + sum(parking.occupied)", "stops");
    let answer = checked(&rig, body.clone()).await;
    assert_eq!(
        answer["problems"][0]["field"], "spec.steps.kpi.unitCode",
        "{answer}"
    );
    assert!(
        answer["problems"][0]["reason"]
            .as_str()
            .expect("a reason")
            .contains("MIN"),
        "{answer}"
    );
    body["pipeline"]["spec"]["steps"][0]["kpi"]["unitCode"] = json!("MIN");
    let answer = checked(&rig, body).await;
    assert_eq!(answer["problems"], json!([]), "{answer}");
}

/// PL-85: the one record a kpi step writes is judged where a write is, so its proposal passes the
/// gate; a target Endpoint the project lacks stays red.
#[tokio::test]
async fn a_kpi_draft_gets_its_record_check_from_the_target_it_writes() {
    let rig = rig().await;
    let mut body = draft("sum(parking.occupied) / count(stops)", "stops");
    let manifest = &mut body["pipeline"];
    manifest["spec"]["outputs"][0]["type"] = json!("KeyPerformanceIndicator");
    let green = records_verdict(&rig.state, "city", manifest).expect("a verdict");
    assert!(green.ok && green.checked == 1, "{green:?}");
    manifest["spec"]["outputs"][0]["targetEndpoint"] =
        json!("urn:ngsi-ld:Endpoint:hel.fi:city-kpi:gone");
    let red = records_verdict(&rig.state, "city", manifest).expect("a verdict");
    assert!(!red.ok, "no such Endpoint in the project: {red:?}");
}

/// T-6593: the indicator is judged against the KPI contract its space declares (seed
/// key-performance-indicator models): `updatedAt` an NGSI-LD DateTime, `derivedFrom` one source
/// Endpoint or the list of them (PL-104). The model check read the typed literal as an object and
/// the seed took one string, so every KPI editor proposal was refused `records_failed` on dev.
#[tokio::test]
async fn a_kpi_draft_meets_the_indicator_model_of_its_space() {
    let rig = rig().await;
    let string = json!({ "type": "string", "x-ngsi-ld-kind": "Property" });
    let model = |derived_from: Value| {
        Arc::new(ModelSchema::compile(
            "city-kpi",
            "1.0.0",
            &json!({ "definitions": {
                "Any": { "additionalProperties": true, "type": ["null", "boolean", "object", "number", "string"] },
                "KeyPerformanceIndicator": {
                    "additionalProperties": false,
                    "properties": {
                        "id": { "type": "string" },
                        "name": string, "calculationFormula": string, "computedBy": string,
                        "currentValue": { "$ref": "#/definitions/Any", "anyOf": [{ "type": "number" }, { "type": "string" }], "x-ngsi-ld-kind": "Property" },
                        "calculationPeriod": { "type": "object", "x-ngsi-ld-kind": "Property" },
                        "updatedAt": { "type": "string", "format": "date-time", "x-ngsi-ld-kind": "Property" },
                        "derivedFrom": derived_from
                    },
                    "required": ["id", "name", "currentValue", "calculationPeriod", "calculationFormula", "derivedFrom", "computedBy", "updatedAt"]
                }
            }}),
            &["KeyPerformanceIndicator".to_owned()],
            false,
        ))
    };
    let held = |derived_from: Value| {
        rig.state.model_schemas.replace(HashMap::from([(
            ("city".to_owned(), "city-kpi".to_owned()),
            model(derived_from),
        )]));
    };
    let mut body = draft("sum(parking.occupied) / count(stops)", "stops");
    let manifest = &mut body["pipeline"];
    manifest["spec"]["outputs"][0]["type"] = json!("KeyPerformanceIndicator");
    held(
        json!({ "items": { "type": "string" }, "minItems": 1, "type": ["string", "array"], "x-ngsi-ld-kind": "Property" }),
    );
    let green = records_verdict(&rig.state, "city", manifest).expect("a verdict");
    assert!(green.ok && green.checked == 1, "{green:?}");
    held(string.clone());
    let red = records_verdict(&rig.state, "city", manifest).expect("a verdict");
    assert!(
        !red.ok,
        "a model taking one string still refuses the list: {red:?}"
    );
}

/// Any other Pipeline keeps its test on a sample.
#[tokio::test]
async fn a_pipeline_without_a_kpi_step_gets_no_record_check_here() {
    let rig = rig().await;
    let manifest = json!({ "metadata": { "name": "plain" }, "spec": {
        "class": "scheduled", "schedule": "0 * * * *",
        "source": { "endpointRef": { "kind": "Endpoint", "name": "parking" } },
        "compute": { "kind": "bloblang", "bloblang": "root = this" },
        "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:mobility:parking" } });
    assert!(records_verdict(&rig.state, "city", &manifest).is_none());
}

/// A source with no rows gives no value and says so; a sum over nothing must not read as 0.
#[tokio::test]
async fn an_empty_source_gives_no_value_and_says_why() {
    let rig = rig().await;
    rig.broker.reset().await;
    Mock::given(method("POST"))
        .and(path_regex("^/q/views/[a-z0-9-]+/preview$"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "count": 0, "entities": [] })),
        )
        .mount(&rig.broker)
        .await;
    let answer = checked(&rig, draft("sum(parking.occupied) / count(stops)", "stops")).await;
    assert_eq!(answer["preview"]["values"], json!([]), "{answer}");
    assert_eq!(answer["preview"]["rows"]["parking"], 0, "{answer}");
    let problem = answer["preview"]["problem"].as_str().expect("a problem");
    assert!(
        problem.contains("input parking") && problem.contains("no rows"),
        "{problem}"
    );
}

/// One source, `by: time`, read back a week: the draft of the editor's "Change since last week".
fn change(formula: &str) -> Value {
    let mut body = draft(formula, "stops");
    let spec = &mut body["pipeline"]["spec"];
    spec["sources"].as_array_mut().expect("sources").truncate(1);
    spec["steps"][0]["kpi"]["join"] = json!({ "by": "time" });
    body
}

/// `parking` as a public Endpoint and a gateway whose Temporal API answers `rows`: the rig of the
/// preview's read back, which goes through the gateway and not the broker's view preview.
async fn rig_with_history(rows: Value) -> (Rig, MockServer) {
    let gateway = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/endpoint/{PARKING}/ngsi-ld/v1/temporal/entities"
        )))
        .and(query_param("type", "ParkingSite"))
        .and(query_param("attrs", "occupied"))
        .and(query_param("timerel", "before"))
        .and(query_param("lastN", "1"))
        .and(query_param("limit", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(rows))
        .mount(&gateway)
        .await;
    let mut config = Config::for_tests();
    config.gateway_url = Some(gateway.uri());
    let rig = rig_on(config).await;
    rig.state.mirror.upsert(envelope(
        "Endpoint",
        "parking",
        "city",
        json!({ "contextSpaceRef": "mobility", "slug": PARKING, "audience": "public" }),
    ));
    (rig, gateway)
}

/// PL-117, PL-105: a change over one source previews the sum now (21) minus the rows as they
/// stood a week ago (the broker's shape: a list of instances per attribute).
#[tokio::test]
async fn a_change_since_last_week_previews_a_value_from_history() {
    let instance = |id: &str, n: u32| {
        json!({ "id": id, "type": "ParkingSite",
        "occupied": [{ "type": "Property", "value": n }] })
    };
    let (rig, _gateway) = rig_with_history(json!([
        instance("p1", 7),
        instance("p2", 8),
        instance("p3", 2)
    ]))
    .await;
    let answer = checked(
        &rig,
        change("sum(parking.occupied) - prev(sum(parking.occupied), P7D)"),
    )
    .await;
    assert_eq!(answer["problems"], json!([]), "{answer}");
    assert_eq!(
        answer["preview"],
        json!({
            "values": [{ "key": null, "name": "parking-per-stop", "value": 4.0 }],
            "rows": { "parking": 3, "parking@P7D": 3 }
        })
    );
}

/// PL-117: no history gives no value with the reason, never the whole sum as the change.
#[tokio::test]
async fn no_history_gives_no_value_and_says_why() {
    let (rig, _gateway) = rig_with_history(json!([])).await;
    let answer = checked(
        &rig,
        change("sum(parking.occupied) - prev(sum(parking.occupied), P7D)"),
    )
    .await;
    assert_eq!(answer["preview"]["values"], json!([]), "{answer}");
    let problem = answer["preview"]["problem"].as_str().expect("a problem");
    assert!(
        problem.contains("input parking") && problem.contains("no history 7 days back"),
        "{problem}"
    );
}

/// PL-117: the history is the person's to read: without their token nothing is asked of the
/// gateway on an Endpoint that is not public, and a refused read gives no value.
#[tokio::test]
async fn a_person_without_a_token_reads_no_history() {
    let gateway = MockServer::start().await;
    let mut config = Config::for_tests();
    config.gateway_url = Some(gateway.uri());
    let rig = rig_on(config).await;
    let answer = checked(
        &rig,
        change("sum(parking.occupied) - prev(sum(parking.occupied), P7D)"),
    )
    .await;
    let problem = answer["preview"]["problem"].as_str().expect("a problem");
    assert!(problem.contains("your own access token"), "{problem}");
    assert_eq!(answer["preview"]["values"], json!([]), "{answer}");
    assert_eq!(
        gateway.received_requests().await.unwrap_or_default().len(),
        0
    );
}

#[tokio::test]
async fn a_refused_history_read_gives_no_value() {
    let gateway = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&gateway)
        .await;
    let mut config = Config::for_tests();
    config.gateway_url = Some(gateway.uri());
    let rig = rig_on(config).await;
    rig.state.mirror.upsert(envelope(
        "Endpoint",
        "parking",
        "city",
        json!({ "contextSpaceRef": "mobility", "slug": PARKING, "audience": "public" }),
    ));
    let answer = checked(
        &rig,
        change("sum(parking.occupied) - prev(sum(parking.occupied), P7D)"),
    )
    .await;
    let problem = answer["preview"]["problem"].as_str().expect("a problem");
    assert!(problem.contains("may not read its history"), "{problem}");
}

/// PL-117: a source whose grant holds no `queryTemporal` is not readable for a `prev` term, though
/// the same grant reads its current page.
#[tokio::test]
async fn a_read_back_needs_the_history_grant() {
    let rig = rig().await;
    let grant = |operations: Value, name: &str| {
        envelope(
            "Policy",
            name,
            "city",
            json!({
                "contextSpaceRef": "mobility",
                "assignee": { "kind": "serviceAccount", "id": "city/pl-kpi" },
                "operations": operations,
                "information": [{ "entities": [{ "type": "ParkingSite" }] }],
            }),
        )
    };
    let read = |rig_body: Value| rig_body["sources"][0]["readable"].clone();
    let formula = "sum(parking.occupied) - prev(sum(parking.occupied), P7D)";
    rig.state
        .mirror
        .upsert(grant(json!(["retrieveOps"]), "kpi-read"));
    let mut plain = change("sum(parking.occupied) + count(parking)");
    plain["preview"] = json!(false);
    plain["pipeline"]["spec"]["sources"]
        .as_array_mut()
        .expect("sources")
        .push(draft("", "stops")["pipeline"]["spec"]["sources"][1].clone());
    assert_eq!(read(checked(&rig, plain).await), json!(true));
    let mut body = change(formula);
    body["preview"] = json!(false);
    assert_eq!(read(checked(&rig, body.clone()).await), json!(false));
    rig.state.mirror.upsert(grant(
        json!(["retrieveOps", "queryTemporal"]),
        "kpi-history",
    ));
    assert_eq!(read(checked(&rig, body).await), json!(true));
}

/// PL-117: the join and the shape of a read back are refused at their fields.
#[tokio::test]
async fn a_read_back_refusal_names_its_field() {
    let rig = rig().await;
    let mut by_area = change("sum(parking.occupied) - prev(sum(parking.occupied), P7D)");
    by_area["pipeline"]["spec"]["steps"][0]["kpi"]["join"] =
        json!({ "by": "area", "keys": { "parking": "district" } });
    let answer = checked(&rig, by_area).await;
    assert_eq!(
        answer["problems"][0]["field"], "spec.steps.kpi.join",
        "{answer}"
    );
    let answer = checked(
        &rig,
        change("sum(parking.occupied) - prev(count(parking), P7D)"),
    )
    .await;
    assert_eq!(
        answer["problems"][0]["field"], "spec.steps.kpi.formula",
        "{answer}"
    );
    assert!(answer.get("preview").is_none(), "{answer}");
}

#[tokio::test]
async fn only_a_person_who_may_propose_checks() {
    let r = rig().await;
    let reader = send(
        &r.state,
        person("eva"),
        "POST",
        ROUTE,
        Some(draft("count(stops)", "stops")),
    )
    .await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);
    assert_eq!(previews(&r.broker).await, 0);
}
