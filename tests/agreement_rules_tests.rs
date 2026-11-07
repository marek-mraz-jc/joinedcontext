//! An accepted data-space agreement reaches the broker's engine as rules for the partner's DID
//! alone, never wider than the offer (DS-10, DS-03, R26, T-3927).
//!
//! The repository offers a public Endpoint whose space holds a public read Policy and a
//! pipeline ServiceAccount's write Policy. The rule document the Portal writes to the broker is
//! what the engine enforces for a request carrying `x-jc-did` (T-3800), so asserting it is
//! asserting what the participant reads through the view.

use std::path::Path;

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::reconciler::views::ViewSync;

const DID: &str = "did:web:partner.example";
const SLUG: &str = "k4y7pq2mzt6vhx3nbwrs5cjdaf";

fn repository(dir: &Path, agreement_state: &str) -> jcctl::loader::Repository {
    let write = |rel: &str, body: String| {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    };
    let head = |kind: &str, name: &str| {
        format!("apiVersion: joinedcontext.com/v1alpha1\nkind: {kind}\nmetadata:\n  name: {name}\n  namespace: city\nspec:\n")
    };
    write(
        "projects/city/spaces/air/space.yaml",
        format!("{}  isSandbox: false\n", head("ContextSpace", "air")),
    );
    write(
        "projects/city/spaces/air/endpoints/air-public.yaml",
        format!("{}  contextSpaceRef: air\n  slug: {SLUG}\n  audience: public\n  enabledRepresentations: [ngsi-ld]\n", head("Endpoint", "air-public")),
    );
    write(
        "projects/city/spaces/air/policies/cars.yaml",
        format!("{}  contextSpaceRef: air\n  assigner: did:web:city.example\n  assignee: {{ kind: role, id: public }}\n  operations: [retrieveOps]\n  information:\n    - entities: [{{ type: Car }}]\n      propertyNames: [speed]\n", head("Policy", "cars")),
    );
    write(
        "projects/city/spaces/air/policies/pipeline-writes.yaml",
        format!("{}  contextSpaceRef: air\n  assigner: did:web:city.example\n  assignee: {{ kind: serviceAccount, id: air-pipeline }}\n  operations: [updateOps, createEntity]\n  information:\n    - entities: [{{ type: Car }}]\n", head("Policy", "pipeline-writes")),
    );
    write(
        "projects/city/spaces/air/dataspace/offers/air-offer.yaml",
        format!("{}  contextSpaceRef: air\n  endpointRefs: [{{ kind: Endpoint, name: air-public }}]\n  policy:\n    \"@type\": Offer\n    permission:\n      - action: odrl:read\n", head("DataOffer", "air-offer")),
    );
    write(
        "projects/city/dataspace/agreements/partner.yaml",
        format!("{}  role: provider\n  offerRef: {{ kind: DataOffer, name: air-offer }}\n  remoteParticipant: {DID}\n  agreementId: urn:uuid:7c1e\n  state: {agreement_state}\n  validity: {{ from: \"2026-01-01T00:00:00Z\", to: \"2099-01-01T00:00:00Z\" }}\n  constraints:\n    q: speed<130\n", head("DataAgreement", "partner")),
    );
    jcctl::loader::Repository::load(dir).expect("the repository loads")
}

/// Every rule of every document the broker was given, as JSON.
async fn rules_written(state: &str) -> Vec<Value> {
    let broker = MockServer::start().await;
    for list in ["/q/views", "/q/policy", "/q/schemas"] {
        Mock::given(method("GET"))
            .and(path(list))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
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
    let dir = std::env::temp_dir().join(format!(
        "portal-agreement-{state}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
    ));
    let repo = repository(&dir, state);
    let token = dir.join("token");
    std::fs::write(&token, "an-admin-token-of-at-least-32-bytes!\n").expect("the token");
    ViewSync::new(broker.uri(), &token).converge(&repo).await;
    let mut rules = Vec::new();
    for request in broker.received_requests().await.unwrap_or_default() {
        if request.method.as_str() != "PUT" {
            continue;
        }
        let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
            continue;
        };
        collect(&body, &mut rules);
    }
    rules
}

/// The objects anywhere in `value` that carry an `assignee`.
fn collect(value: &Value, out: &mut Vec<Value>) {
    match value {
        Value::Object(fields) => {
            if fields.contains_key("assignee") {
                out.push(value.clone());
            }
            fields.values().for_each(|v| collect(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| collect(v, out)),
        _ => {}
    }
}

fn for_did(rules: &[Value]) -> Vec<&Value> {
    rules
        .iter()
        .filter(|rule| rule.to_string().contains(DID))
        .collect()
}

#[tokio::test]
async fn the_partner_reads_what_its_agreement_grants_and_writes_nothing() {
    let rules = rules_written("finalized").await;
    assert!(
        rules.iter().any(|r| r.to_string().contains("air-pipeline")),
        "the space's own rules are written too: {rules:#?}"
    );
    let partner = for_did(&rules);
    assert_eq!(partner.len(), 1, "one rule for the DID: {rules:#?}");
    let by_id = |id: &str| {
        rules
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("{id} in {rules:#?}"))
    };
    // The public read, as the space grants it, and none of the pipeline's writes.
    assert_eq!(partner[0]["operations"], by_id("city/cars")["operations"]);
    let writes = by_id("city/pipeline-writes")["operations"]
        .as_array()
        .expect("operations")
        .clone();
    // Every grant carries the schema reads (T-3645); none of the pipeline's own clauses.
    let schema = [
        json!("5.7.5"),
        json!("5.7.7"),
        json!("5.7.8"),
        json!("5.7.10"),
    ];
    for granted in partner[0]["operations"].as_array().expect("operations") {
        assert!(
            schema.contains(granted) || !writes.contains(granted),
            "{granted} is the pipeline's"
        );
    }
    let rule = partner[0].to_string();
    assert!(rule.contains("speed<130"), "the agreement's filter: {rule}");
    assert!(rule.contains("Car"), "{rule}");
}

#[tokio::test]
async fn an_agreement_not_yet_accepted_grants_the_partner_nothing() {
    let rules = rules_written("requested").await;
    assert!(!rules.is_empty(), "the space's own rules are written");
    assert!(for_did(&rules).is_empty(), "{rules:#?}");
}
