//! An Endpoint that lists a domain-standard representation is checked against its space's model
//! on the write path (EP-125, T-4584, T-4900): the `ical` mapping, the `sdm-event` preset when
//! none is written, must fit the space's model, and the Portal refuses one that does not with
//! `jcctl validate`'s sentences before a Change exists, so the reconciler is never handed an
//! Endpoint jcctl would refuse. The form's dry run answers the same sentences.

mod common;

use axum::http::StatusCode;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const PROJECT: &str = "kultura";

/// The Smart Data Models Event, as a space would hold it: `startDate` and `name` required.
const EVENTS: &str = r#"id: https://example.org/models/events
name: events
classes:
  Event:
    slots:
      - name
      - startDate
      - endDate
slots:
  name:
    range: string
    required: true
  startDate:
    range: datetime
    required: true
  endDate:
    range: datetime
"#;

/// A model of the space without the Event type.
const PLACES: &str = r#"id: https://example.org/models/places
name: places
classes:
  PointOfInterest:
    slots:
      - name
slots:
  name:
    range: string
    required: true
"#;

fn admin() -> Identity {
    let mut identity = common::person("admin");
    identity.groups = vec!["portal-approver".to_owned()];
    identity
}

/// The Portal against a forge holding `linkml` as the model of space `podujatia`.
async fn portal(linkml: &str) -> (AppState, wiremock::MockServer) {
    // Deprecated backs a surface like published, without the four artifacts (DM-02, DM-26).
    portal_at(linkml, "deprecated").await
}

/// The Portal with the space's model in `lifecycle`.
async fn portal_at(linkml: &str, lifecycle: &str) -> (AppState, wiremock::MockServer) {
    let gitea = common::forge().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{}/contents/projects/{PROJECT}/spaces/podujatia/datamodels/model.linkml.yaml",
            common::REPO
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": common::encode(linkml)
        })))
        .mount(&gitea)
        .await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(common::envelope(
        "ContextSpace",
        "podujatia",
        PROJECT,
        json!({ "isSandbox": true }),
    ));
    state.mirror.upsert(common::envelope(
        "DataModel",
        "model",
        PROJECT,
        json!({
            "contextSpaceRef": "podujatia",
            "linkml": "./model.linkml.yaml",
            "version": "1.0.0",
            "lifecycle": lifecycle,
        }),
    ));
    (state, gitea)
}

fn endpoint(representations: Value) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Endpoint",
        "metadata": { "name": "kalendar", "namespace": PROJECT },
        "spec": {
            "contextSpaceRef": { "kind": "ContextSpace", "name": "podujatia" },
            "slug": "si6epqkx364lprho5uaigutk274r5grb",
            "audience": "project-list",
            "allowedProjects": [PROJECT],
            "enabledRepresentations": representations
        }
    })
}

async fn propose(state: &AppState, manifest: Value) -> common::Answer {
    common::checked_send(
        state,
        admin(),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
        Some(manifest),
    )
    .await
}

#[tokio::test]
async fn ical_on_a_space_without_the_event_type_is_refused_with_jcctl_s_sentence() {
    let (state, gitea) = portal(PLACES).await;
    let answer = propose(
        &state,
        with_outputs(json!({ "ical": [calendar("kalendar")] })),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains(
            "Endpoint kultura/kalendar, space podujatia: `ical` output `kalendar` maps the type \
             Event, which the data model does not have (EP-125)"
        ),
        "{}",
        answer.text
    );
    let requests = gitea.received_requests().await.expect("recorded");
    assert!(
        !requests.iter().any(|r| r.method.as_str() == "POST"),
        "no Change was opened"
    );
}

#[tokio::test]
async fn ical_without_a_required_start_names_the_attribute() {
    let optional_start = EVENTS.replacen(
        "  startDate:\n    range: datetime\n    required: true\n",
        "  startDate:\n    range: datetime\n",
        1,
    );
    let (state, _gitea) = portal(&optional_start).await;
    let answer = propose(
        &state,
        with_outputs(json!({ "ical": [calendar("kalendar")] })),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains(
            "`ical` output `kalendar` maps DTSTART to `startDate`, which is not a required attribute of Event"
        ),
        "{}",
        answer.text
    );
}

#[tokio::test]
async fn ical_on_the_sdm_event_model_opens_a_change() {
    let (state, _gitea) = portal(EVENTS).await;
    let answer = propose(&state, endpoint(json!(["ngsi-ld", "ical"]))).await;
    assert!(
        answer.status.is_success(),
        "{} {}",
        answer.status,
        answer.text
    );
}

/// bbsk-podujatia's model keeps `startDate` a string with an ISO 8601 date or date-time pattern
/// (its sources mix whole-day and timed events): the default iCal preset opens a change over it
/// (T-5208: jc-core 207bf9b refused it, and the reconciler took the Endpoint off the broker).
#[tokio::test]
async fn ical_over_a_date_pattern_string_opens_a_change() {
    let dated = EVENTS.replace(
        "  startDate:\n    range: datetime\n",
        "  startDate:\n    range: string\n    pattern: '^[0-9]{4}-[0-9]{2}-[0-9]{2}(T[0-9]{2}:[0-9]{2}(:[0-9]{2})?(Z|[+-][0-9]{2}:[0-9]{2}))?$'\n",
    );
    assert_ne!(dated, EVENTS);
    let (state, _gitea) = portal(&dated).await;
    let answer = propose(&state, endpoint(json!(["ngsi-ld", "ical"]))).await;
    assert!(
        answer.status.is_success(),
        "{} {}",
        answer.status,
        answer.text
    );
}

#[tokio::test]
async fn an_endpoint_without_ical_is_not_held_to_the_event_type() {
    let (state, _gitea) = portal(PLACES).await;
    let answer = propose(&state, endpoint(json!(["ngsi-ld", "csv"]))).await;
    assert!(
        answer.status.is_success(),
        "{} {}",
        answer.status,
        answer.text
    );
}

fn mapped(mapping: Value) -> Value {
    let mut manifest = endpoint(json!(["ngsi-ld", "ical"]));
    manifest["spec"]["representations"] = json!({ "ical": { "mapping": mapping } });
    manifest
}

/// A mapping the Endpoint writes is checked field by field: every gap is one line, in the words
/// jc-core gives `jcctl validate`, and the form's dry run shows the same lines.
#[tokio::test]
async fn a_written_mapping_is_refused_per_gap_and_the_dry_run_says_the_same() {
    let (state, gitea) = portal(EVENTS).await;
    let broken = mapped(json!({ "SUMMARY": "name", "DTEND": "name" }));
    let answer = propose(&state, broken.clone()).await;
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        answer.text
    );
    let lines = [
        "Endpoint kultura/kalendar, space podujatia: `ical` output `events` maps no DTSTART, which every calendar entry needs (EP-125)",
        "Endpoint kultura/kalendar, space podujatia: `ical` output `events` maps DTEND to `name`, range string; DTEND takes date or datetime (EP-125)",
    ];
    for line in lines {
        assert!(answer.text.contains(line), "{line} in {}", answer.text);
    }
    let dry = common::checked_send(
        &state,
        admin(),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/endpoints?dryRun=All"),
        Some(broken),
    )
    .await;
    for line in lines {
        assert!(
            dry.text.contains(line),
            "{line} in the dry run {}",
            dry.text
        );
    }
    let requests = gitea.received_requests().await.expect("recorded");
    assert!(
        !requests.iter().any(|r| r.method.as_str() == "POST"),
        "no Change was opened"
    );

    let fitting = mapped(
        json!({ "DTSTART": "startDate", "SUMMARY": { "attribute": "name", "truncate": 120 } }),
    );
    let answer = propose(&state, fitting).await;
    assert!(
        answer.status.is_success(),
        "{} {}",
        answer.status,
        answer.text
    );
}

/// T-4886: a draft or retired model backs no domain standard; the fitting Event model is refused
/// until it is published, with the lifecycle named and nothing of the data shown.
#[tokio::test]
async fn ical_on_a_draft_model_is_refused_until_it_is_published() {
    for lifecycle in ["draft", "retired"] {
        let (state, gitea) = portal_at(EVENTS, lifecycle).await;
        let answer = propose(&state, endpoint(json!(["ngsi-ld", "ical"]))).await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
        assert!(
            answer.text.contains(&format!(
                "Endpoint kultura/kalendar: `ical` needs a published data model in space \
                 podujatia; its model is {lifecycle} (EP-125, DM-26)"
            )),
            "{}",
            answer.text
        );
        let requests = gitea.received_requests().await.expect("recorded");
        assert!(!requests.iter().any(|r| r.method.as_str() == "POST"));
    }
}

#[tokio::test]
async fn a_draft_model_does_not_hold_back_an_endpoint_without_a_domain_standard() {
    let (state, _gitea) = portal_at(PLACES, "draft").await;
    let answer = propose(&state, endpoint(json!(["ngsi-ld", "csv"]))).await;
    assert!(
        answer.status.is_success(),
        "{} {}",
        answer.status,
        answer.text
    );
}

/// API/11 EP-159 (T-5218): the named outputs the Endpoint form edits.
fn with_outputs(representations: Value) -> Value {
    let mut manifest = endpoint(json!(["ngsi-ld", "ical"]));
    manifest["spec"]["representations"] = representations;
    manifest
}

fn calendar(name: &str) -> Value {
    json!({ "name": name, "title": { "sk": "Kalendár" }, "sources": [{ "type": "Event", "preset": "sdm-event" }] })
}

fn feed(name: &str) -> Value {
    json!({ "name": name, "title": { "sk": "Novinky" }, "sources": [{ "type": "Event", "preset": "generic" }] })
}

/// Each refusal is a 422 whose `errors` name the output and the member, before a Change exists.
async fn refused(state: &AppState, gitea: &wiremock::MockServer, manifest: Value, member: &str) {
    let answer = propose(state, manifest).await;
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{member}: {}",
        answer.text
    );
    let body: Value = serde_json::from_str(&answer.text).expect("a problem document");
    let errors = body["errors"].as_array().expect("one entry per member");
    assert!(
        errors.iter().any(|e| e.to_string().contains(member)),
        "{member} named in {}",
        answer.text
    );
    let requests = gitea.received_requests().await.expect("recorded");
    assert!(
        !requests.iter().any(|r| r.method.as_str() == "POST"),
        "no Change was opened"
    );
}

#[tokio::test]
async fn three_feeds_and_two_calendars_open_a_change() {
    let (state, _gitea) = portal(EVENTS).await;
    let manifest = with_outputs(json!({
        "ical": [calendar("kalendar"), calendar("sport")],
        "feed": [feed("novinky"), feed("kultura"), feed("deti")],
    }));
    let answer = propose(&state, manifest).await;
    assert!(
        answer.status.is_success(),
        "{} {}",
        answer.status,
        answer.text
    );
}

#[tokio::test]
async fn a_duplicate_output_name_is_a_422_naming_it() {
    let (state, gitea) = portal(EVENTS).await;
    let manifest = with_outputs(json!({ "ical": [calendar("kalendar"), calendar("kalendar")] }));
    refused(&state, &gitea, manifest, "kalendar").await;
}

#[tokio::test]
async fn a_21st_output_is_a_422() {
    let (state, gitea) = portal(EVENTS).await;
    let feeds: Vec<Value> = (0..21).map(|n| feed(&format!("f{n}"))).collect();
    refused(
        &state,
        &gitea,
        with_outputs(json!({ "feed": feeds })),
        "spec.representations.feed",
    )
    .await;
}

#[tokio::test]
async fn a_name_that_is_no_slug_is_a_422_naming_it() {
    let (state, gitea) = portal(EVENTS).await;
    refused(
        &state,
        &gitea,
        with_outputs(json!({ "feed": [feed("Kultúra")] })),
        "Kultúra",
    )
    .await;
}

#[tokio::test]
async fn a_filter_the_broker_would_refuse_is_a_422_naming_the_member() {
    let (state, gitea) = portal(EVENTS).await;
    let mut output = calendar("sport");
    output["filter"] = json!({ "q": "eventCategory==" });
    refused(
        &state,
        &gitea,
        with_outputs(json!({ "ical": [output] })),
        "filter.q",
    )
    .await;
}

#[tokio::test]
async fn an_unknown_member_of_an_output_is_a_422_naming_it() {
    let (state, gitea) = portal(EVENTS).await;
    let mut output = feed("novinky");
    output["colour"] = json!("red");
    refused(
        &state,
        &gitea,
        with_outputs(json!({ "feed": [output] })),
        "colour",
    )
    .await;
}

#[tokio::test]
async fn a_role_naming_a_slot_the_model_lacks_is_a_422_naming_it() {
    let (state, gitea) = portal(EVENTS).await;
    let output = json!({ "name": "novinky", "sources": [{ "type": "Event", "preset": "generic", "mapping": { "title": "headline" } }] });
    refused(
        &state,
        &gitea,
        with_outputs(json!({ "feed": [output] })),
        "headline",
    )
    .await;
}

#[tokio::test]
async fn a_role_of_the_wrong_range_is_a_422_naming_it() {
    let (state, gitea) = portal(EVENTS).await;
    let output = json!({ "name": "kalendar", "sources": [{ "type": "Event", "mapping": { "DTSTART": "name", "SUMMARY": "name" } }] });
    refused(
        &state,
        &gitea,
        with_outputs(json!({ "ical": [output] })),
        "DTSTART",
    )
    .await;
}

#[tokio::test]
async fn a_source_type_the_model_lacks_is_a_422_naming_it() {
    let (state, gitea) = portal(EVENTS).await;
    let output = json!({ "name": "kalendar", "sources": [{ "type": "CouncilMeeting", "preset": "sdm-event" }] });
    refused(
        &state,
        &gitea,
        with_outputs(json!({ "ical": [output] })),
        "CouncilMeeting",
    )
    .await;
}

/// A person without the Endpoint right is refused before any output is looked at.
#[tokio::test]
async fn a_person_without_the_endpoint_right_gets_403() {
    let (state, gitea) = portal(EVENTS).await;
    let manifest =
        with_outputs(json!({ "ical": [calendar("kalendar")], "feed": [feed("novinky")] }));
    let answer = common::checked_send(
        &state,
        common::person("visitor"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
        Some(manifest),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    let requests = gitea.received_requests().await.expect("recorded");
    assert!(
        !requests.iter().any(|r| r.method.as_str() == "POST"),
        "no Change was opened"
    );
}

/// The Event with a category enum and a free-entry flag, the fields a map legend colours by.
const MAP_EVENTS: &str = r#"id: https://example.org/models/events
name: events
classes:
  Event:
    slots: [name, startDate, category, free]
slots:
  name: { range: string, required: true }
  startDate: { range: datetime, required: true }
  category: { range: EventCategory }
  free: { range: boolean }
enums:
  EventCategory:
    permissible_values: { sport: {}, music: {} }
"#;

fn map(mapping: Value) -> Value {
    with_outputs(
        json!({ "widget": [{ "name": "mapa", "sources": [{ "type": "Event", "mapping": mapping, "cluster": false }] }] }),
    )
}

/// API/11 EP-163 (T-5239): the publisher's map defaults save when every role fits the model.
#[tokio::test]
async fn map_defaults_that_fit_the_model_open_a_change() {
    let (state, _gitea) = portal(MAP_EVENTS).await;
    for mapping in [
        json!({ "geometry": "location", "colour": "category", "popup": ["name", "startDate"] }),
        json!({ "colour": "free" }),
    ] {
        let answer = propose(&state, map(mapping.clone())).await;
        assert!(
            answer.status.is_success(),
            "{mapping}: {} {}",
            answer.status,
            answer.text
        );
    }
}

/// One 422 per map role, unknown slot and wrong range alike, naming the member.
#[tokio::test]
async fn a_map_role_the_model_cannot_fill_is_a_422_naming_the_member() {
    let (state, gitea) = portal(MAP_EVENTS).await;
    for (mapping, member) in [
        (
            json!({ "geometry": "startDate" }),
            "`mapping.geometry` reads `startDate`",
        ),
        (
            json!({ "geometry": "place" }),
            "`mapping.geometry` names `place`",
        ),
        (json!({ "colour": "name" }), "`mapping.colour` reads `name`"),
        (
            json!({ "colour": "colour" }),
            "`mapping.colour` names `colour`",
        ),
        (
            json!({ "popup": ["name", "nowhere"] }),
            "`mapping.popup` names `nowhere`",
        ),
        (
            json!({ "popup": ["location"] }),
            "`mapping.popup` names `location`, a GeoProperty",
        ),
    ] {
        refused(&state, &gitea, map(mapping), member).await;
    }
    let mut not_a_flag = map(json!({}));
    not_a_flag["spec"]["representations"]["widget"][0]["sources"][0]["cluster"] = json!("often");
    refused(&state, &gitea, not_a_flag, "spec.representations").await;
}

fn table(start: Value) -> Value {
    with_outputs(
        json!({ "widget": [{ "name": "tabulka", "sources": [{ "type": "Event", "table": start }] }] }),
    )
}

/// API/11 EP-164 (T-5197): a table start whose filter parses and whose attributes the class has saves.
#[tokio::test]
async fn a_table_start_that_fits_the_model_opens_a_change() {
    let (state, _gitea) = portal(MAP_EVENTS).await;
    let start = json!({ "filter": "startDate>=today;category==\"music\"", "orderBy": "startDate;desc",
        "pageLength": 50, "columns": ["name", "startDate"] });
    let answer = propose(&state, table(start)).await;
    assert!(
        answer.status.is_success(),
        "{} {}",
        answer.status,
        answer.text
    );
}

/// One 422 per table member the widget cannot use, naming the member.
#[tokio::test]
async fn a_table_start_the_widget_cannot_use_is_a_422_naming_the_member() {
    let (state, gitea) = portal(MAP_EVENTS).await;
    for (start, member) in [
        (
            json!({ "filter": "startDate>=>today" }),
            "`table.filter` is not an NGSI-LD q",
        ),
        (
            json!({ "orderBy": "nowhere" }),
            "`table.orderBy` names `nowhere`",
        ),
        (
            json!({ "orderBy": "location" }),
            "`table.orderBy` names `location`, a GeoProperty",
        ),
        (
            json!({ "pageLength": 25 }),
            "`table.pageLength: 25` is not one of 10, 20, 50 or 100",
        ),
        (
            json!({ "columns": ["name", "nowhere"] }),
            "`table.columns` names `nowhere`",
        ),
        (
            json!({ "columns": ["name", "name"] }),
            "`table.columns` names `name` twice",
        ),
    ] {
        refused(&state, &gitea, table(start), member).await;
    }
    refused(
        &state,
        &gitea,
        table(json!({ "sort": "name" })),
        "spec.representations",
    )
    .await;
}

/// A table start is an Endpoint change: a person without the Endpoint right gets 403.
#[tokio::test]
async fn a_person_without_the_endpoint_right_cannot_set_a_table_start() {
    let (state, gitea) = portal(MAP_EVENTS).await;
    let answer = common::checked_send(
        &state,
        common::person("visitor"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
        Some(table(json!({ "pageLength": 50 }))),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    let requests = gitea.received_requests().await.expect("recorded");
    assert!(
        !requests.iter().any(|r| r.method.as_str() == "POST"),
        "no Change was opened"
    );
}

/// The map defaults are an Endpoint change: a person without the Endpoint right gets 403.
#[tokio::test]
async fn a_person_without_the_endpoint_right_cannot_set_map_defaults() {
    let (state, gitea) = portal(MAP_EVENTS).await;
    let answer = common::checked_send(
        &state,
        common::person("visitor"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
        Some(map(json!({ "colour": "category" }))),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    let requests = gitea.received_requests().await.expect("recorded");
    assert!(
        !requests.iter().any(|r| r.method.as_str() == "POST"),
        "no Change was opened"
    );
}
