//! Public dashboards in the open-data catalogue (T-6790): what the reconciler writes for a
//! `visibility: public` Dashboard, that a second pass writes nothing, that a package somebody else
//! made is never written over. The organization sweep withdraws a dashboard by the same
//! `generated_by` mark as a dataset (ckan_reconciler_tests), so the package carries it.

use std::sync::atomic::{AtomicUsize, Ordering};

use jcctl::commands::publish_ckan as publisher;
use jcctl::publish::ckan::{CkanApi, InMemoryCkan, Outcome, Settings, GENERATOR};
use joinedcontext_portal::reconciler::ckan_dashboards::{
    desired, publish, targets, DashboardTarget, DASHBOARD_TYPE, ENDPOINTS_EXTRA, LAYOUT_EXTRA,
};
use serde_json::{json, Value};

const PROJECT: &str = "bbsk";
const SLUG: &str = "scsd2eehkx42n53z2zyd6vshfh7s7irf";

fn manifest(kind: &str, name: &str, spec: Value) -> String {
    serde_norway::to_string(&json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": kind,
        "metadata": { "name": name, "namespace": PROJECT },
        "spec": spec,
    }))
    .expect("yaml")
}

fn chart() -> Value {
    json!({
        "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
        "title": "Docks per district",
        "data": { "name": "source" },
        "mark": "bar",
        "encoding": {
            "x": { "field": "district", "type": "nominal" },
            "y": { "aggregate": "count", "type": "quantitative" }
        }
    })
}

fn dashboard(visibility: &str) -> Value {
    json!({
        "title": "Ukazovatele",
        "visibility": visibility,
        "pages": [{
            "title": "Overview",
            "widgets": [
                { "widgetType": "vega-lite", "endpointRef": "bikes",
                  "data": { "source": "csv", "type": "BikeHireDockingStation" }, "spec": chart() },
                { "widgetType": "grid", "endpointRef": "bikes", "entityType": "BikeHireDockingStation" },
                { "widgetType": "temporal-chart", "endpointRef": "bikes",
                  "entityId": "urn:ngsi-ld:BikeHireDockingStation:1", "property": "availableBikeNumber" }
            ]
        }]
    })
}

/// The repository of one project: its catalogue, a public Endpoint and the dashboard.
fn targets_of(visibility: &str) -> Vec<DashboardTarget> {
    static RUN: AtomicUsize = AtomicUsize::new(0);
    let repository = std::env::temp_dir().join(format!(
        "jc-ckan-dashboards-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&repository).expect("a scratch directory");
    let instance = json!({
        "url": "https://data.example.sk",
        "organizationDefault": "bbsk",
        "apiTokenRef": { "name": "ckan-api-token", "key": "token" },
    });
    let endpoint = json!({
        "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" },
        "slug": SLUG,
        "audience": "public",
        "enabledRepresentations": ["ngsi-ld", "csv"],
    });
    std::fs::write(
        repository.join("a.yaml"),
        [
            manifest("CkanInstance", "bbsk", instance),
            manifest("Endpoint", "bikes", endpoint),
            manifest("Dashboard", "ukazovatele", dashboard(visibility)),
        ]
        .join("---\n"),
    )
    .expect("the repository");
    let repo = jcctl::loader::Repository::load(&repository).expect("the repository loads");
    let (wanted, _) = publisher::targets_with(&repo, PROJECT, None).expect("targets");
    let out = targets(&repo, &wanted);
    std::fs::remove_dir_all(&repository).expect("cleaned up");
    out
}

fn settings() -> Settings {
    Settings::new("api.example.sk")
}

fn extra<'a>(package: &'a Value, key: &str) -> &'a str {
    package["extras"]
        .as_array()
        .and_then(|e| e.iter().find(|e| e["key"] == key))
        .and_then(|e| e["value"].as_str())
        .unwrap_or_default()
}

#[test]
fn a_public_dashboard_is_a_dashboard_package_with_a_chart_view_per_vega_lite_widget() {
    let found = targets_of("public");
    assert_eq!(found.len(), 1, "{found:?}");
    let want = desired(&found[0], "bbsk", &settings()).expect("desired");
    let package = &want.package;
    assert_eq!(package["name"], "bbsk-ukazovatele");
    assert_eq!(package["type"], DASHBOARD_TYPE);
    assert_eq!(package["owner_org"], "bbsk");
    assert_eq!(extra(package, "generated_by"), GENERATOR);
    let url = format!("https://api.example.sk/api/endpoint/{SLUG}");
    assert_eq!(extra(package, ENDPOINTS_EXTRA), url);
    assert_eq!(package["resources"][0]["url"], url.as_str());
    // The Portal-only temporal chart is not drawn by the catalogue; the chart and the grid are.
    let layout: Value = serde_json::from_str(extra(package, LAYOUT_EXTRA)).expect("layout");
    assert_eq!(
        layout,
        json!({ "pages": [{ "title": "Overview", "widgets": [
            { "view": "p0w0" },
            { "grid": SLUG, "type": "BikeHireDockingStation" }
        ]}]})
    );
    assert_eq!(want.views.len(), 1);
    let (slug, view) = &want.views[0];
    assert_eq!(slug, SLUG);
    assert_eq!(view["view_type"], "jc_visualization");
    assert_eq!(view["title"], "Docks per district");
    assert_eq!(view["endpoint"], SLUG);
    let source: Value = serde_json::from_str(view["source"].as_str().unwrap_or_default())
        .expect("source is JSON text");
    assert_eq!(
        source,
        json!({ "source": "csv", "type": "BikeHireDockingStation" })
    );
}

#[test]
fn a_dashboard_that_is_not_public_is_not_published() {
    for visibility in ["private", "project", "organization"] {
        assert!(targets_of(visibility).is_empty(), "{visibility}");
    }
}

#[test]
fn a_second_pass_over_an_unchanged_dashboard_writes_nothing_and_an_edit_updates_its_view() {
    let found = targets_of("public");
    let want = desired(&found[0], "bbsk", &settings()).expect("desired");
    let mut api = InMemoryCkan::new().with_organization("bbsk");
    assert_eq!(publish(&mut api, &want), Ok(Outcome::Created));
    let resource = api.package("bbsk-ukazovatele").expect("created")["resources"][0]["id"]
        .as_str()
        .expect("a resource id")
        .to_owned();
    assert_eq!(api.views(&resource).len(), 1);

    let writes = |api: &InMemoryCkan| {
        api.actions()
            .into_iter()
            .filter(|a| !a.ends_with("_show") && !a.ends_with("_list"))
            .count()
    };
    let before = writes(&api);
    assert_eq!(publish(&mut api, &want), Ok(Outcome::Unchanged));
    assert_eq!(writes(&api), before, "{:?}", api.actions());

    let mut edited = found[0].clone();
    edited.spec.pages[0].widgets[0].spec = Some({
        let mut spec = chart();
        spec["mark"] = json!("line");
        spec
    });
    let want = desired(&edited, "bbsk", &settings()).expect("desired");
    assert_eq!(publish(&mut api, &want), Ok(Outcome::Updated));
    assert!(
        api.actions().contains(&"resource_view_update"),
        "{:?}",
        api.actions()
    );
    assert_eq!(api.views(&resource).len(), 1);
    assert!(api.views(&resource)[0]["spec"]
        .as_str()
        .is_some_and(|s| s.contains("\"line\"")));
}

#[test]
fn a_package_of_that_name_somebody_else_made_is_never_written_over() {
    let found = targets_of("public");
    let want = desired(&found[0], "bbsk", &settings()).expect("desired");
    let mut api = InMemoryCkan::new().with_organization("bbsk");
    api.action(
        "package_create",
        &json!({ "name": "bbsk-ukazovatele", "title": "Hand made", "owner_org": "bbsk" }),
    )
    .expect("a hand-made dataset");
    let refused = publish(&mut api, &want).expect_err("refused");
    assert!(refused.contains("rename the dashboard"), "{refused}");
    assert_eq!(
        api.package("bbsk-ukazovatele").expect("kept")["title"],
        "Hand made"
    );
}
