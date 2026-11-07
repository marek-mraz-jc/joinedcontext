//! What an assistant will know (API/05 §1.9, AG-121, AG-123, AG-124, T-4693): the editor's
//! preview of an unsaved AssistantDeployment and the context of a saved one, both resolved by
//! jc-core's one resolver over the mirrored repository.

mod common;

use std::sync::Arc;

use joinedcontext_portal::auth::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};

const PREVIEW: &str = "/api/v1/projects/bb/knowledge/deployments/context?dryRun=All&lang=sk";

fn admin() -> Identity {
    let mut person = common::person("anna");
    person.groups = vec!["portal-approver".into()];
    person
}

/// Project `bb` with a public parking Endpoint and an organization staff Endpoint, and project
/// `bbsk` with a public roads Endpoint and an organization budget Endpoint; both spaces serve the
/// `mobilita` model, whose classes are `OffStreetParking` and `Road`.
fn state() -> AppState {
    let mirror = Mirror::new();
    for project in ["bb", "bbsk"] {
        mirror.upsert(common::envelope(
            "ContextSpace",
            "mesto",
            project,
            json!({ "dataModelRef": "mobilita" }),
        ));
        mirror.upsert(common::envelope(
            "DataModel",
            "mobilita",
            project,
            json!({ "classes": ["OffStreetParking", "Road"] }),
        ));
    }
    for (name, project, slug, audience) in [
        ("parking", "bb", "aaaaaaaaaaaaaaaaaaaaaaaaaa", "public"),
        ("staff", "bb", "bbbbbbbbbbbbbbbbbbbbbbbbbb", "organization"),
        ("roads", "bbsk", "cccccccccccccccccccccccccc", "public"),
        (
            "budget",
            "bbsk",
            "dddddddddddddddddddddddddd",
            "organization",
        ),
    ] {
        mirror.upsert(common::envelope(
            "Endpoint",
            name,
            project,
            json!({ "contextSpaceRef": "mesto", "slug": slug, "audience": audience }),
        ));
    }
    mirror.upsert(common::envelope(
        "KnowledgeSource",
        "web",
        "bb",
        json!({ "source": "website", "startUrls": ["https://www.banskabystrica.sk/"], "visibility": "public", "categories": ["GOVE"] }),
    ));
    mirror.upsert(common::envelope(
        "AssistantDeployment",
        "doprava",
        "bb",
        deployment(json!({ "select": { "themes": ["TRAN"] }, "scope": { "projects": ["bbsk"] } })),
    ));
    AppState::new(Config::for_tests(), None).with_mirror(Arc::new(mirror))
}

/// A public deployment's spec with `extra` merged in.
fn deployment(extra: Value) -> Value {
    let mut spec = json!({
        "publicId": "bb-doprava",
        "channel": "public",
        "categories": ["TRAN"],
        "allowedOrigins": ["https://www.banskabystrica.sk"],
        "rateLimit": { "requestsPerMinute": 60, "perClientPerMinute": 10 },
        "budget": { "tokensPerDay": 2000000, "tokensPerConversation": 40000 },
        "theme": { "title": { "sk": "Doprava v Banskej Bystrici", "en": "Transport in Banská Bystrica" } },
        "languages": ["sk", "en"],
    });
    if let (Some(spec), Some(extra)) = (spec.as_object_mut(), extra.as_object()) {
        spec.extend(extra.clone());
    }
    spec
}

fn manifest(spec: Value) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "AssistantDeployment",
        "metadata": { "name": "doprava", "namespace": "bb" },
        "spec": spec,
    })
}

async fn preview(state: &AppState, spec: Value) -> Value {
    let answer = common::send(state, admin(), "POST", PREVIEW, Some(manifest(spec))).await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    serde_json::from_str(&answer.text).expect("json")
}

fn slugs(context: &Value) -> Vec<&str> {
    context["endpoints"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["slug"].as_str())
        .collect()
}

/// AG-121: a data-only assistant (no source) picks its Endpoints by category inside its own
/// project and scope.projects, public ones alone on a public channel, with the labels in the
/// asked language.
#[tokio::test]
async fn a_data_only_assistant_previews_the_endpoints_its_selector_reaches() {
    let state = state();
    let context = preview(
        &state,
        deployment(json!({ "select": { "themes": ["TRAN"] }, "scope": { "projects": ["bbsk"] } })),
    )
    .await;
    assert_eq!(
        slugs(&context),
        ["aaaaaaaaaaaaaaaaaaaaaaaaaa", "cccccccccccccccccccccccccc"]
    );
    assert_eq!(context["sources"], json!([]));
    assert_eq!(
        context["categories"],
        json!([{ "code": "TRAN", "label": "Doprava" }])
    );
    assert_eq!(context["title"], "Doprava v Banskej Bystrici");
    let parking = &context["endpoints"][0];
    assert_eq!(parking["project"], "bb");
    assert_eq!(parking["types"], json!(["OffStreetParking", "Road"]));
    assert_eq!(parking["tools"], json!([]));
    assert_eq!(context["problems"], json!([]));
    assert_eq!(context["limits"]["perClientPerHour"], 60);
}

/// AG-121: a RAG-only assistant (no Endpoint) previews its sources and nothing else; on the
/// internal channel the organization Endpoint of its own project is reachable, another
/// project's never, and `exclude` wins.
#[tokio::test]
async fn a_rag_only_assistant_previews_its_sources_and_exclude_wins() {
    let state = state();
    let context = preview(&state, deployment(json!({ "sources": ["web"] }))).await;
    assert!(slugs(&context).is_empty());
    assert_eq!(context["sources"][0]["name"], "web");
    assert_eq!(
        context["sources"][0]["sites"],
        json!(["www.banskabystrica.sk"])
    );
    assert_eq!(
        context["sources"][0]["categories"],
        json!([{ "code": "GOVE", "label": "Vláda a verejný sektor" }])
    );

    let internal = preview(
        &state,
        deployment(json!({
            "channel": "internal",
            "select": {},
            "scope": { "projects": ["bbsk"] },
            "exclude": ["parking"],
        })),
    )
    .await;
    assert_eq!(
        slugs(&internal),
        ["bbbbbbbbbbbbbbbbbbbbbbbbbb", "cccccccccccccccccccccccccc"]
    );
}

/// The preview resolves what it can and names the manifest's own findings beside it.
#[tokio::test]
async fn an_unfinished_manifest_is_previewed_with_its_problems() {
    let state = state();
    let context = preview(
        &state,
        json!({ "publicId": "bb-doprava", "channel": "public", "select": {} }),
    )
    .await;
    assert_eq!(slugs(&context), ["aaaaaaaaaaaaaaaaaaaaaaaaaa"]);
    let problems = context["problems"].as_array().expect("problems");
    assert_eq!(problems.len(), 1, "{problems:?}");
}

/// A saved deployment's context is the same document.
#[tokio::test]
async fn a_saved_deployment_reads_its_context() {
    let state = state();
    let answer = common::send(
        &state,
        admin(),
        "GET",
        "/api/v1/projects/bb/knowledge/deployments/doprava/context?lang=en",
        None,
    )
    .await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    let context: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(context["categories"][0]["label"], "Transport");
    assert_eq!(context["endpoints"].as_array().map(Vec::len), Some(2));
    let missing = common::send(
        &state,
        admin(),
        "GET",
        "/api/v1/projects/bb/knowledge/deployments/nowhere/context",
        None,
    )
    .await;
    assert_eq!(missing.status, 404);
}

/// AG-113: a caller who may not read the project learns nothing; the preview is a dry run of an
/// AssistantDeployment alone, and a field the kind does not know is refused.
#[tokio::test]
async fn the_preview_refuses_before_it_resolves() {
    let state = state();
    let stranger = common::person("nobody");
    for (http, uri, body) in [
        ("POST", PREVIEW, Some(manifest(deployment(json!({}))))),
        (
            "GET",
            "/api/v1/projects/bb/knowledge/deployments/doprava/context",
            None,
        ),
    ] {
        let answer = common::send(&state, stranger.clone(), http, uri, body).await;
        assert_eq!(answer.status, 404, "{uri}: {}", answer.text);
    }
    let wet = common::send(
        &state,
        admin(),
        "POST",
        "/api/v1/projects/bb/knowledge/deployments/context",
        Some(manifest(deployment(json!({})))),
    )
    .await;
    assert_eq!(wet.status, 400, "{}", wet.text);
    assert!(wet.text.contains("dryRun=All"), "{}", wet.text);
    let mut other = manifest(deployment(json!({})));
    other["kind"] = json!("KnowledgeSource");
    let answer = common::send(&state, admin(), "POST", PREVIEW, Some(other)).await;
    assert_eq!(answer.status, 400, "{}", answer.text);
    let unknown = common::send(
        &state,
        admin(),
        "POST",
        PREVIEW,
        Some(manifest(deployment(json!({ "themes": ["TRAN"] })))),
    )
    .await;
    assert_eq!(unknown.status, 400, "{}", unknown.text);
    assert!(unknown.text.contains("themes"), "{}", unknown.text);
}

/// AG-124: the Portal's category labels are jc-core's, in every language the Portal speaks.
#[test]
fn the_category_labels_are_the_jc_core_table() {
    use jc_core::kinds::catalog::DataTheme;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/src/locales");
    for lang in ["sk", "en", "cs", "de"] {
        let text = std::fs::read_to_string(root.join(format!("{lang}.json"))).expect("locale");
        let locale: Value = serde_json::from_str(&text).expect("json");
        for theme in DataTheme::ALL {
            assert_eq!(
                locale["knowledge"]["category"][theme.code()].as_str(),
                Some(theme.label_in(lang)),
                "{lang} {}",
                theme.code()
            );
        }
    }
}

/// AG-124: the context reads the same from the resolver whatever door asks; a language the
/// labels do not hold answers in English.
#[test]
fn an_unknown_language_reads_the_english_labels() {
    use joinedcontext_portal::api::assistant_context::context_of;
    let state = state();
    let spec = serde_json::from_value(deployment(
        json!({ "select": { "themes": ["TRAN"] }, "scope": { "projects": ["bbsk"] } }),
    ))
    .expect("a deployment");
    let context = context_of(&state.mirror, "bb", &spec, "xx", &|_, _| true);
    assert_eq!(
        context["categories"],
        json!([{ "code": "TRAN", "label": "Transport" }])
    );
    assert_eq!(context["title"], "Transport in Banská Bystrica");
    assert_eq!(
        slugs(&context),
        ["aaaaaaaaaaaaaaaaaaaaaaaaaa", "cccccccccccccccccccccccccc"]
    );
}

/// F-R2POR2-003 (T-6226): the preview tells of another project's KnowledgeSource only when the
/// caller reads that project's sources; one it may not read reads as absent, its own always shows.
#[test]
fn a_source_of_a_project_the_caller_cannot_read_is_absent_from_the_preview() {
    use joinedcontext_portal::api::assistant_context::context_of;
    let state = state();
    state.mirror.upsert(common::envelope(
        "KnowledgeSource",
        "roads-web",
        "bbsk",
        json!({ "source": "website", "startUrls": ["https://intranet.bbsk.sk/"], "visibility": "internal", "categories": ["TRAN"] }),
    ));
    let spec = serde_json::from_value(deployment(json!({
        "sources": ["web", "bbsk/roads-web"],
        "scope": { "projects": ["bbsk"] },
    })))
    .expect("a deployment");
    let names = |context: &Value| -> Vec<String> {
        context["sources"]
            .as_array()
            .expect("sources")
            .iter()
            .map(|source| {
                format!(
                    "{}/{}",
                    source["project"].as_str().unwrap_or_default(),
                    source["name"].as_str().unwrap_or_default()
                )
            })
            .collect()
    };
    let own_only = |namespace: &str, _: &str| namespace == "bb";
    assert_eq!(
        names(&context_of(&state.mirror, "bb", &spec, "en", &own_only)),
        ["bb/web"]
    );
    assert_eq!(
        names(&context_of(&state.mirror, "bb", &spec, "en", &|_, _| true)),
        ["bb/web", "bbsk/roads-web"]
    );
}
