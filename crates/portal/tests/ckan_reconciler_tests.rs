//! What a reconcile run does to the open-data catalogue (T-2405, EP-62…EP-67, CC-19).
//!
//! The publisher has been complete for a while and nothing drove it: a dataset appeared when
//! somebody ran `jcctl publish ckan` from a shell, and an Endpoint that stopped publishing left
//! its dataset in the catalogue. These tests are about the wave that drives it: what it publishes,
//! what it withdraws, what it refuses to touch, and that the API token is in none of it.
//!
//! The catalogue is the publisher's own in-memory double, which carries a token the way the real
//! transport does, so a payload that contained it would be visible here.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jcctl::commands::publish_ckan::{Models, Target};
use jcctl::loader::{RawManifest, RawMetadata, ResourceId};
use jcctl::publish::ckan::{InMemoryCkan, Outcome, Settings};
use joinedcontext_portal::reconciler::ckan::{
    catalogue_gap, claimed_twice, fingerprint, publish_with, publish_without_models, read_gap,
    sweep_with, wave, withdrawn, CkanSync, Organizations, Paced, Pacing, Publication, Report,
    CALL_GAP, MODELS_READS, MOST_BACKOFF, PUBLISH_CONCURRENCY, RECORD_READS, REFRESH,
};
use joinedcontext_portal::resource::ResourceEnvelope;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};

const PROJECT: &str = "helsinki";
const HOST: &str = "portal.example.fi";
/// The token the catalogue holds. It may not appear in anything a run writes or reports.
const TOKEN: &str = "ckan-api-token-2ZQ8vN";

fn settings() -> Settings {
    Settings::new(HOST).titled("City of Helsinki")
}

fn envelope(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    serde_json::from_value(json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": kind,
        "metadata": { "name": name, "namespace": PROJECT },
        "spec": spec,
    }))
    .expect("an envelope")
}

/// The catalogue manifest of the seed, without the token (EP-67).
fn instance() -> Value {
    json!({
        "url": "https://data.example.fi",
        "organizationDefault": "hel-fi",
        "apiTokenRef": { "name": "ckan-api-token", "key": "token", "envVar": "CKAN_API_TOKEN" },
    })
}

/// One Endpoint, publishing or not.
fn endpoint(name: &str, audience: &str, publish: Option<Value>) -> Value {
    let mut spec = json!({
        "contextSpaceRef": { "kind": "ContextSpace", "name": "bikes" },
        "slug": "scsd2eehkx42n53z2zyd6vshfh7s7irf",
        "audience": audience,
        "enabledRepresentations": ["ngsi-ld", "csv"],
    });
    if let Some(publication) = publish {
        spec["publish"] = json!({ "ckan": publication });
    }
    let _ = name;
    spec
}

/// The `publish.ckan` block of an Endpoint that also names its licence.
fn licensed() -> Value {
    json!({
        "instanceRef": { "kind": "CkanInstance", "name": "hel-fi" },
        "license": "cc-by",
    })
}

/// The `publish.ckan` block of an Endpoint that publishes the dataset alone.
fn dataset_only() -> Value {
    json!({ "instanceRef": { "kind": "CkanInstance", "name": "hel-fi" } })
}

/// A target the way `publisher::targets` builds one, for the calls that need no repository.
fn target(name: &str, audience: &str, publication: Value) -> Target {
    let spec = endpoint(name, audience, Some(publication.clone()));
    Target {
        id: ResourceId {
            group: "joinedcontext.com".to_owned(),
            kind: "Endpoint".to_owned(),
            namespace: Some(PROJECT.to_owned()),
            name: name.to_owned(),
        },
        manifest: RawManifest {
            api_version: "joinedcontext.com/v1alpha1".to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: RawMetadata {
                name: name.to_owned(),
                namespace: Some(PROJECT.to_owned()),
                rest: serde_json::Map::new(),
            },
            spec: spec.clone(),
            status: None,
        },
        slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf".to_owned(),
        publication: serde_json::from_value(publication).expect("a publication"),
        slice: None,
        instance_name: "hel-fi".to_owned(),
        instance: serde_json::from_value(instance()).expect("an instance"),
        language: None,
        instance_title: None,
        history: Default::default(),
        granted: Vec::new(),
    }
}

/// The DCAT-AP record the gateway answers with, as the publisher reads it (EP-27).
fn record() -> Value {
    json!({
        "dct:title": "Helsinki city bike stations",
        "dct:description": "Every station, its capacity and how many bikes are free.",
        "dct:license": "cc-by",
        "dct:publisher": "City of Helsinki",
        "dcat:keyword": ["bikes", "mobility"],
        "dct:accrualPeriodicity": "http://publications.europa.eu/resource/authority/frequency/UPDATE_CONT",
    })
}

/// A tabular answer of two stations, as the CSV representation serves it.
/// The models the gateway answers: a schema without classes and no LinkML (T-4672).
fn models() -> Models {
    Models {
        schema: json!({}),
        linkml: None,
    }
}

#[test]
fn an_endpoint_becomes_a_dataset() {
    let mut api = InMemoryCkan::new().with_token(TOKEN);
    let target = target("helsinki-bikes", "public", licensed());

    let published = publish_with(&mut api, &target, &record(), Some(&models()), &settings())
        .expect("the endpoint publishes");
    let Publication::Published {
        dataset, outcome, ..
    } = published
    else {
        panic!("a publication, got {published:?}");
    };
    assert_eq!(dataset, "helsinki-bikes");
    assert_eq!(outcome, Outcome::Created);

    let package = api.package("helsinki-bikes").expect("the dataset");
    assert_eq!(package["title"], "Helsinki city bike stations");
    assert_eq!(package["owner_org"], "hel-fi");
    // A public endpoint is a public dataset; the rest of EP-69 is the case below.
    assert_eq!(package["private"], false);
    // The organization the catalogue did not have is created with the installation's own name.
    assert!(api.actions().contains(&"organization_create"));
}

#[test]
fn a_narrower_endpoint_publishes_a_private_dataset() {
    // EP-69, closed by default: only a `public` endpoint may produce a public dataset, and the
    // form offers no visibility of its own that could disagree with it.
    for audience in ["organization", "project-list"] {
        let mut api = InMemoryCkan::new().with_token(TOKEN);
        let target = target("helsinki-bikes", audience, dataset_only());
        publish_with(&mut api, &target, &record(), None, &settings()).expect("it publishes");
        assert_eq!(
            api.package("helsinki-bikes").expect("the dataset")["private"],
            true,
            "{audience} is not public"
        );
    }
}

#[test]
fn a_second_run_over_an_unchanged_endpoint_writes_nothing() {
    // CC-18: the wave runs every tick, so a converged catalogue has to make no writing call at
    // all. Without this a dataset would be rewritten once a minute for its whole life.
    let mut api = InMemoryCkan::new()
        .with_organization("hel-fi")
        .with_token(TOKEN);
    let target = target("helsinki-bikes", "public", dataset_only());
    publish_with(&mut api, &target, &record(), None, &settings()).expect("the first run");
    let calls = api.actions().len();

    let again = publish_with(&mut api, &target, &record(), None, &settings()).expect("the second");
    assert_eq!(
        again,
        Publication::Published {
            dataset: "helsinki-bikes".to_owned(),
            outcome: Outcome::Unchanged,
            no_linkml: false,
        }
    );
    assert_eq!(api.actions().len(), calls, "the second run wrote something");
}

#[test]
fn the_token_is_in_nothing_a_run_writes_or_reports() {
    // EP-67: the token travels in the Authorization header of the transport and nowhere else.
    let mut api = InMemoryCkan::new().with_token(TOKEN);
    let target = target("helsinki-bikes", "public", licensed());
    let published = publish_with(&mut api, &target, &record(), Some(&models()), &settings())
        .expect("it publishes");

    let written = api
        .calls()
        .map(|(action, payload)| format!("{action} {payload}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!written.contains(TOKEN), "a payload carried the token");
    assert!(
        !written.contains("ckan-api-token-"),
        "a payload carried the token"
    );
    assert!(
        !format!("{published:?}").contains(TOKEN),
        "the report carried the token"
    );
    // The reference's own name is fine and useful; the value is not.
    assert_eq!(api.token(), TOKEN);
}

#[test]
fn a_withdrawn_publication_and_a_deleted_endpoint_both_lose_their_dataset() {
    // CC-19: a dataset is withdrawn when the Endpoint that declared it stops declaring one, and
    // when the Endpoint is gone altogether. Both are read from the previous run's mirror, so a
    // dataset this reconciler never published is never deleted by it.
    let previous = Mirror::new();
    previous.upsert(envelope("CkanInstance", "hel-fi", instance()));
    previous.upsert(envelope(
        "Endpoint",
        "still-published",
        endpoint("still-published", "public", Some(dataset_only())),
    ));
    previous.upsert(envelope(
        "Endpoint",
        "block-withdrawn",
        endpoint("block-withdrawn", "public", Some(licensed())),
    ));
    previous.upsert(envelope(
        "Endpoint",
        "endpoint-deleted",
        endpoint("endpoint-deleted", "public", Some(dataset_only())),
    ));
    // A closed Endpoint without a block is never a dataset (EP-174).
    previous.upsert(envelope(
        "Endpoint",
        "never-published",
        endpoint("never-published", "organization", None),
    ));

    // What this run publishes: the first endpoint's dataset only. The second dropped its block,
    // the third is gone from the repository, the fourth never published.
    let published: BTreeSet<(String, String)> = [(
        "https://data.example.fi".to_owned(),
        "still-published".to_owned(),
    )]
    .into_iter()
    .collect();

    let gone = withdrawn(&previous, &published, None);
    let names: BTreeSet<String> = gone.iter().map(|t| t.id.name.clone()).collect();
    assert_eq!(
        names,
        ["block-withdrawn".to_owned(), "endpoint-deleted".to_owned()]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    // Each carries the catalogue it published to, which is the only way its dataset is reachable.
    for target in &gone {
        assert_eq!(target.instance_name, "hel-fi");
        assert_eq!(target.instance.base_url(), "https://data.example.fi");
        assert_eq!(target.id.namespace.as_deref(), Some(PROJECT));
    }
}

#[test]
fn an_endpoint_whose_catalogue_is_also_gone_is_left_alone() {
    // Nothing can be reached without the instance's URL and token reference. Inventing either
    // would be worse than leaving the dataset to a person: the run says so in the log and touches
    // nothing.
    let previous = Mirror::new();
    previous.upsert(envelope(
        "Endpoint",
        "orphaned",
        endpoint("orphaned", "public", Some(dataset_only())),
    ));
    assert!(withdrawn(&previous, &BTreeSet::new(), None).is_empty());
}

#[test]
fn withdrawing_removes_the_dataset_and_can_be_repeated() {
    // CC-19 through the publisher's explicit-deletion path: the wave's withdrawal removes the
    // record, and a repeat after a partial failure is not an error.
    let mut api = InMemoryCkan::new()
        .with_organization("hel-fi")
        .with_token(TOKEN);
    let target = target("helsinki-bikes", "public", licensed());
    publish_with(&mut api, &target, &record(), Some(&models()), &settings()).expect("it publishes");
    assert!(api.package("helsinki-bikes").is_some());

    let gone = joinedcontext_portal::reconciler::ckan::withdraw_one(&mut api, &target)
        .expect("it withdraws");
    assert_eq!(
        gone,
        Publication::Withdrawn {
            dataset: "helsinki-bikes".to_owned()
        }
    );
    assert!(
        api.actions().contains(&"package_delete"),
        "{:?}",
        api.actions()
    );

    // A run repeated after a failure withdraws what is already gone without an error.
    joinedcontext_portal::reconciler::ckan::withdraw_one(&mut api, &target)
        .expect("withdrawing twice is not an error");
}

#[tokio::test]
async fn without_a_way_to_read_the_token_the_run_says_so_and_publishes_nothing() {
    // A Portal with no age identity and no variable for the reference: the publication is one
    // failed report naming the reference, and no socket is opened towards the catalogue. The
    // reason is the publisher's own sentence, which names what to pass and never a value.
    let previous = Mirror::new();
    previous.upsert(envelope("CkanInstance", "hel-fi", instance()));
    previous.upsert(envelope(
        "Endpoint",
        "helsinki-bikes",
        endpoint("helsinki-bikes", "public", Some(dataset_only())),
    ));
    let sync = CkanSync::new(HOST);
    let repository = std::env::temp_dir().join("jc-ckan-reconciler-no-secrets");
    std::fs::create_dir_all(&repository).expect("a scratch directory");

    // `converge` over a repository with no manifests publishes nothing and withdraws what the
    // previous run had; both need the token, so the report is the refusal.
    let empty = jcctl::loader::Repository::load(&repository).expect("an empty repository");
    let reports = sync.converge(&empty, &previous, &repository).await;
    assert_eq!(reports.len(), 1, "{reports:?}");
    let Report {
        project,
        endpoint,
        publication,
        ..
    } = &reports[0];
    assert_eq!(project, PROJECT);
    assert_eq!(endpoint, "helsinki-bikes");
    let Publication::Failed(reason) = publication else {
        panic!("a refusal, got {publication:?}");
    };
    assert!(
        reason.contains("api-token-env") || reason.contains("age-key-file"),
        "the reason says what is missing: {reason}"
    );
    assert!(!reason.contains(TOKEN), "the reason carried a credential");
}

/// T-2465, EP-62, EP-63: the reconciler's run carries what the walk resolved — the space's
/// language picks the Slovak title and notes, the publication's licence fills the one term the
/// gateway's record lacks, and an organization the catalogue lacks takes the instance's title
/// rather than the installation's branding (which on dev names every organization after Helsinki).
#[test]
fn a_slovak_endpoint_publishes_under_its_slovak_title_licence_and_organization() {
    let mut api = InMemoryCkan::new().with_token(TOKEN);
    let mut target = target(
        "public-air",
        "public",
        json!({ "instanceRef": { "kind": "CkanInstance", "name": "hel-fi" }, "license": "cc-by" }),
    );
    target.language = Some("sk".to_owned());
    target.instance_title = Some("Mesto Banská Bystrica".to_owned());
    let record = json!({
        "dct:title": [
            { "@value": "Air quality in Banská Bystrica", "@language": "en" },
            { "@value": "Kvalita ovzdušia v Banskej Bystrici", "@language": "sk" }
        ],
    });

    publish_with(&mut api, &target, &record, None, &settings()).expect("the endpoint publishes");

    assert_eq!(api.actions(), vec!["organization_create", "package_create"]);
    let (_, organization) = api.calls().next().expect("the organization call");
    assert_eq!(organization["title"], json!("Mesto Banská Bystrica"));
    let dataset = api.package("public-air").expect("the dataset");
    assert_eq!(
        dataset["title"],
        json!("Kvalita ovzdušia v Banskej Bystrici")
    );
    assert_eq!(dataset["license_id"], json!("cc-by"));

    // CC-18: the next reconcile finds the catalogue as it left it.
    let again = publish_with(&mut api, &target, &record, None, &settings()).expect("second run");
    assert!(
        matches!(
            again,
            Publication::Published {
                outcome: Outcome::Unchanged,
                ..
            }
        ),
        "{again:?}"
    );
}

/// T-3903, DM-95: a dataset's measurements are its space's 7-day pass rate, to a hundredth, and
/// the age of the newest data when the daily run read it; nothing measured is no measurement.
#[test]
fn the_measurements_are_the_spaces_pass_rate_and_freshness() {
    use joinedcontext_portal::quality::{SpaceQuality, TypeQuality};
    use joinedcontext_portal::quality_checks::{Reading, Readings};
    use joinedcontext_portal::reconciler::ckan::measurements;

    let at = |text: &str| {
        text.parse::<chrono::DateTime<chrono::Utc>>()
            .expect("a time")
    };
    let checks = Readings::new(None);
    assert_eq!(
        measurements(None, &checks.report("air", &[])),
        jcctl::publish::ckan::Measurements::default()
    );

    let key = |rule: &str, outcome: &str| {
        (
            "air".to_owned(),
            "AirQualityObserved".to_owned(),
            rule.to_owned(),
            outcome.to_owned(),
        )
    };
    checks.record(Reading {
        at: at("2026-10-12T00:00:00Z"),
        counts: [
            (key("pm10", "pass"), 2.0),
            (key("pm10", "reject"), 1.0),
            (key("no2", "pass"), 0.0),
        ]
        .into(),
    });
    let daily = SpaceQuality {
        observed_at: at("2026-10-12T02:00:00Z"),
        checked: 3,
        invalid: 0,
        truncated: false,
        rules: Vec::new(),
        freshness: Vec::new(),
        types: ["2026-10-12T01:56:00Z", "2026-10-12T01:00:00Z"]
            .iter()
            .map(|newest| TypeQuality {
                entity_type: "AirQualityObserved".into(),
                count: 1,
                newest: Some(at(newest)),
                attributes: Vec::new(),
            })
            .collect(),
    };
    let measured = measurements(Some(&daily), &checks.report("air", &[]));
    // The first reading is its own base, so the week counts nothing yet: no rate, never 100 %.
    assert_eq!(measured.pass_rate, None);
    assert_eq!(measured.freshness_seconds, Some(240));

    checks.record(Reading {
        at: at("2026-10-12T01:00:00Z"),
        counts: [(key("pm10", "pass"), 4.0), (key("pm10", "reject"), 2.0)].into(),
    });
    let measured = measurements(Some(&daily), &checks.report("air", &[]));
    assert_eq!(measured.pass_rate, Some(0.67));
}

/// DM-95, T-3903: a dataset's pass rate is its space's, read by the tenant segment the gateway
/// labels its counters with (`helsinki-bikes` here), never by a namesake labelled `bikes`.
#[test]
fn a_datasets_measurements_are_read_by_its_spaces_segment() {
    use std::sync::Arc;

    use joinedcontext_portal::quality::Store;
    use joinedcontext_portal::quality_checks::{Reading, Readings};

    let checks = Arc::new(Readings::new(None));
    for (at, ours, namesake) in [
        ("2026-10-12T00:00:00Z", 0.0, 0.0),
        ("2026-10-12T01:00:00Z", 3.0, 50.0),
    ] {
        let key = |space: &str, outcome: &str| {
            (
                space.to_owned(),
                "Bike".to_owned(),
                "speed".to_owned(),
                outcome.to_owned(),
            )
        };
        checks.record(Reading {
            at: at.parse().expect("a time"),
            counts: [
                (key("helsinki-bikes", "pass"), ours),
                (key("helsinki-bikes", "reject"), ours / 3.0),
                (key("bikes", "reject"), namesake),
            ]
            .into(),
        });
    }
    let sync = CkanSync::new(HOST).with_quality(Arc::new(Store::default()), checks);
    let repository = std::env::temp_dir().join("jc-ckan-reconciler-measured");
    std::fs::create_dir_all(&repository).expect("a scratch directory");
    let repo = jcctl::loader::Repository::load(&repository).expect("an empty repository");
    let measured = sync.measured(&repo, &target("helsinki-bikes", "public", dataset_only()));
    assert_eq!(measured.pass_rate, Some(0.75));
}

/// T-4065: an Endpoint that starts serving its WebSocket brings the dataset a resource for it,
/// whose URL ends `/ws` and whose description links the guide.
#[test]
fn an_endpoint_gaining_websocket_updates_its_dataset_with_the_ws_resource() {
    let mut api = InMemoryCkan::new()
        .with_organization("hel-fi")
        .with_token(TOKEN);
    let settings = settings().documented_at("https://docs.example.fi");
    let before = target("helsinki-transport", "public", dataset_only());
    publish_with(&mut api, &before, &record(), None, &settings).expect("the first run");

    let mut after = before.clone();
    after.manifest.spec["enabledRepresentations"] = json!(["ngsi-ld", "csv", "websocket"]);
    let published =
        publish_with(&mut api, &after, &record(), None, &settings).expect("it publishes");
    assert!(
        matches!(
            published,
            Publication::Published {
                outcome: Outcome::Updated,
                ..
            }
        ),
        "{published:?}"
    );
    assert_eq!(api.actions().last(), Some(&"package_update"));
    let package = api.package("helsinki-transport").expect("the dataset");
    let ws = package["resources"]
        .as_array()
        .expect("resources")
        .iter()
        .find(|resource| {
            resource["url"]
                .as_str()
                .is_some_and(|url| url.ends_with("/ws"))
        })
        .expect("a WebSocket resource");
    assert_eq!(ws["format"], "WebSocket");
    let description = ws["description"].as_str().unwrap_or_default();
    assert!(
        description.contains("https://docs.example.fi/API/07-websocket"),
        "{description}"
    );
}

/// T-4065: models that do not read (the gateway timing out) never keep the dataset at its old
/// resources: it is updated, and the run fails naming the reason and never the token.
#[test]
fn models_that_do_not_read_still_bring_the_dataset_to_its_record() {
    let mut api = InMemoryCkan::new()
        .with_organization("hel-fi")
        .with_token(TOKEN);
    let mut target = target("helsinki-transport", "public", licensed());
    publish_with(&mut api, &target, &record(), Some(&models()), &settings()).expect("first run");

    target.manifest.spec["enabledRepresentations"] = json!(["ngsi-ld", "csv", "websocket"]);
    let unread = "the gateway did not answer model.schema.json: operation timed out";
    let failed = publish_without_models(&mut api, &target, &record(), unread, &settings())
        .expect_err("the charts were not refreshed");
    assert_eq!(
        failed,
        format!("the dataset is updated, its charts were not refreshed: {unread}")
    );
    let package = api.package("helsinki-transport").expect("the dataset");
    let resources = package["resources"].as_array().expect("resources");
    assert!(resources
        .iter()
        .any(|r| r["url"].as_str().is_some_and(|url| url.ends_with("/ws"))));
    assert!(!failed.contains(TOKEN));
}

/// T-4065: the log says each Endpoint's outcome when it changes, every write, and a failure
/// again only when its reason changes.
#[test]
fn the_log_says_an_outcome_once_per_change_and_every_write() {
    let sync = CkanSync::new(HOST);
    let report = |publication: Publication| Report {
        project: PROJECT.to_owned(),
        endpoint: "helsinki-transport".to_owned(),
        dataset: "helsinki-transport".to_owned(),
        publication,
    };
    let published = |outcome| {
        report(Publication::Published {
            dataset: "helsinki-transport".to_owned(),
            outcome,
            no_linkml: false,
        })
    };
    let failed = report(Publication::Failed("the gateway answered 429".to_owned()));
    assert_eq!(
        sync.log(std::slice::from_ref(&failed)),
        ["helsinki/helsinki-transport: failed: the gateway answered 429"]
    );
    assert!(
        sync.log(&[failed]).is_empty(),
        "the same failure is said once"
    );
    assert_eq!(sync.log(&[published(Outcome::Updated)]).len(), 1);
    assert_eq!(
        sync.log(&[published(Outcome::Updated)]).len(),
        1,
        "every write is said"
    );
    assert_eq!(
        sync.log(&[published(Outcome::Unchanged)]),
        ["helsinki/helsinki-transport: unchanged"]
    );
    assert!(sync.log(&[published(Outcome::Unchanged)]).is_empty());
}

/// A catalogue that answers `package_search` over one organization's datasets, which the
/// in-memory double does not; every other call is the publisher's own shape.
struct Searchable {
    datasets: Vec<Value>,
    deleted: Vec<String>,
}

impl jcctl::publish::ckan::CkanApi for Searchable {
    fn show(
        &self,
        action: &str,
        name: &str,
    ) -> Result<Option<Value>, jcctl::publish::ckan::CkanError> {
        assert_eq!(action, "package_show");
        Ok(self
            .datasets
            .iter()
            .find(|d| d["name"] == name && !self.deleted.iter().any(|gone| gone == name))
            .cloned())
    }

    fn action(
        &mut self,
        action: &str,
        payload: &Value,
    ) -> Result<Value, jcctl::publish::ckan::CkanError> {
        match action {
            "package_search" => {
                assert_eq!(payload["fq"], "organization:\"susr\"", "{payload}");
                assert_eq!(payload["include_private"], true, "{payload}");
                let live: Vec<Value> = self
                    .datasets
                    .iter()
                    .filter(|d| !self.deleted.iter().any(|gone| *gone == d["name"]))
                    .cloned()
                    .collect();
                Ok(json!({ "count": live.len(), "results": live }))
            }
            "package_delete" => {
                self.deleted
                    .push(payload["id"].as_str().unwrap_or_default().to_owned());
                Ok(Value::Null)
            }
            other => panic!("no {other} in a sweep"),
        }
    }
}

fn dataset(name: &str, generated_by: Option<&str>) -> Value {
    let extras: Vec<Value> = generated_by
        .map(|value| json!({ "key": "generated_by", "value": value }))
        .into_iter()
        .collect();
    json!({ "name": name, "organization": { "name": "susr" }, "extras": extras })
}

#[test]
fn a_dataset_the_platform_published_for_an_endpoint_that_is_gone_is_withdrawn_and_a_harvests_is_not(
) {
    // T-4461: susr-zo-krajiny's Endpoint left the seed in the apply that restarted the Portal, so
    // no mirror of it ever held the Endpoint; the catalogue's own mark is what says it was ours.
    let mut api = Searchable {
        datasets: vec![
            dataset("susr-om", Some(jcctl::publish::ckan::GENERATOR)),
            dataset("susr-zo-krajiny", Some(jcctl::publish::ckan::GENERATOR)),
            dataset("harvested-from-nkod", None),
            dataset("someone-elses", Some("ckanext-harvest")),
            // The platform's own dataset of another organization the text search let through
            // is not this sweep's (R2POR5-006).
            {
                let mut other = dataset("other-org", Some(jcctl::publish::ckan::GENERATOR));
                other["organization"]["name"] = json!("bratislava");
                other
            },
        ],
        deleted: Vec::new(),
    };
    let kept: BTreeSet<String> = ["susr-om".to_owned()].into();
    let swept = sweep_with(&mut api, "susr", &kept).expect("the sweep runs");
    assert_eq!(
        swept,
        vec![(
            "susr-zo-krajiny".to_owned(),
            Publication::Withdrawn {
                dataset: "susr-zo-krajiny".to_owned()
            }
        )]
    );
    assert_eq!(api.deleted, vec!["susr-zo-krajiny".to_owned()]);

    // The next run finds nothing more to withdraw.
    assert!(sweep_with(&mut api, "susr", &kept)
        .expect("it runs again")
        .is_empty());
}

#[tokio::test]
async fn a_pass_with_a_project_whose_publication_does_not_read_sweeps_nothing() {
    // T-4461 review: project b's broken spec leaves its datasets out of what the pass keeps, so a
    // sweep of the shared default organization would withdraw them. No sweep in such a pass.
    let repository =
        std::env::temp_dir().join(format!("jc-ckan-sweep-guard-{}", std::process::id()));
    std::fs::create_dir_all(&repository).expect("a scratch directory");
    let manifest = |project: &str, kind: &str, name: &str, spec: Value| {
        serde_norway::to_string(&json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": kind,
            "metadata": { "name": name, "namespace": project },
            "spec": spec,
        }))
        .expect("yaml")
    };
    std::fs::write(
        repository.join("a.yaml"),
        format!(
            "{}---\n{}",
            manifest("a", "CkanInstance", "hel-fi", instance()),
            manifest(
                "a",
                "Endpoint",
                "bikes",
                endpoint("bikes", "public", Some(dataset_only()))
            )
        ),
    )
    .expect("project a");
    std::fs::write(
        repository.join("b.yaml"),
        manifest(
            "b",
            "Endpoint",
            "parking",
            endpoint(
                "parking",
                "public",
                Some(json!({ "instanceRef": { "kind": "CkanInstance", "name": "nowhere" } })),
            ),
        ),
    )
    .expect("project b");
    let repo = jcctl::loader::Repository::load(&repository).expect("the repository loads");
    let reports = CkanSync::new(HOST)
        .converge(&repo, &Mirror::new(), &repository)
        .await;
    // b's spec is reported, a's dataset is tried (and refused for want of a token), and no
    // sweep line of project a's organization follows.
    assert!(
        reports
            .iter()
            .any(|r| r.project == "b" && r.endpoint.is_empty()),
        "{reports:?}"
    );
    assert!(
        !reports
            .iter()
            .any(|r| r.project == "a" && r.endpoint.is_empty()),
        "{reports:?}"
    );
    std::fs::remove_dir_all(&repository).expect("cleaned up");
}

// T-4717: the prod Portal republished every dataset on every sync, through the public edge, at
// about 1,000 requests a minute; the edge answered 429 and each refusal was retried a tick later.

fn key() -> (String, String) {
    (PROJECT.to_owned(), "air".to_owned())
}

fn published_now() -> Publication {
    Publication::Published {
        dataset: "air".to_owned(),
        outcome: Outcome::Unchanged,
        no_linkml: false,
    }
}

#[test]
fn an_unchanged_endpoint_is_published_again_only_after_the_refresh() {
    let print = fingerprint(&target("air", "public", dataset_only()), &settings());
    let mut pacing = Pacing::default();
    let now = Instant::now();
    assert!(pacing.due(&key(), print, now), "never published, so due");
    pacing.record(key(), print, &published_now(), now);
    assert!(!pacing.due(&key(), print, now + Duration::from_secs(60)));
    assert!(!pacing.due(&key(), print, now + REFRESH - Duration::from_secs(1)));
    assert!(pacing.due(&key(), print, now + REFRESH));
}

#[test]
fn a_changed_endpoint_is_due_at_once() {
    let before = fingerprint(&target("air", "public", dataset_only()), &settings());
    let after = fingerprint(&target("air", "public", licensed()), &settings());
    assert_ne!(before, after);
    assert_eq!(
        before,
        fingerprint(&target("air", "public", dataset_only()), &settings())
    );
    let mut pacing = Pacing::default();
    let now = Instant::now();
    pacing.record(key(), before, &published_now(), now);
    assert!(pacing.due(&key(), after, now + Duration::from_secs(1)));
    // A failing Endpoint the user then fixes is not left waiting out its backoff.
    pacing.record(key(), before, &Publication::Failed("refused".into()), now);
    assert!(pacing.due(&key(), after, now));
}

#[test]
fn a_429_backs_off_from_five_minutes_and_doubles_up_to_an_hour() {
    let print = 7;
    let refused =
        Publication::Failed("CKAN rejected organization_show: 429 Too Many Requests".into());
    let mut pacing = Pacing::default();
    let mut now = Instant::now();
    pacing.record(key(), print, &refused, now);
    assert!(!pacing.due(&key(), print, now + Duration::from_secs(299)));
    assert!(pacing.due(&key(), print, now + Duration::from_secs(300)));
    now += Duration::from_secs(300);
    pacing.record(key(), print, &refused, now);
    assert!(!pacing.due(&key(), print, now + Duration::from_secs(599)));
    assert!(pacing.due(&key(), print, now + Duration::from_secs(600)));
    for _ in 0..10 {
        pacing.record(key(), print, &refused, now);
    }
    assert!(!pacing.due(&key(), print, now + MOST_BACKOFF - Duration::from_secs(1)));
    assert!(pacing.due(&key(), print, now + MOST_BACKOFF));
    // Once it goes through, the next is the ordinary refresh.
    pacing.record(key(), print, &published_now(), now);
    assert!(!pacing.due(&key(), print, now + REFRESH - Duration::from_secs(1)));
    assert!(pacing.due(&key(), print, now + REFRESH));
}

#[test]
fn another_failure_is_tried_again_after_a_minute_and_a_number_containing_429_is_not_a_429() {
    let mut pacing = Pacing::default();
    let now = Instant::now();
    let failed = Publication::Failed("the gateway did not answer: 4290 rows, 502".into());
    pacing.record(key(), 1, &failed, now);
    assert!(!pacing.due(&key(), 1, now + Duration::from_secs(59)));
    assert!(pacing.due(&key(), 1, now + Duration::from_secs(60)));
}

/// A catalogue that only notes when it was called.
struct Clocked(Arc<Mutex<Vec<Instant>>>);

impl jcctl::publish::ckan::CkanApi for Clocked {
    fn show(&self, _: &str, _: &str) -> Result<Option<Value>, jcctl::publish::ckan::CkanError> {
        self.0.lock().expect("calls").push(Instant::now());
        Ok(None)
    }

    fn action(&mut self, _: &str, _: &Value) -> Result<Value, jcctl::publish::ckan::CkanError> {
        self.0.lock().expect("calls").push(Instant::now());
        Ok(Value::Null)
    }
}

#[test]
fn calls_to_the_catalogue_keep_the_gap_across_every_client_of_one_portal() {
    use jcctl::publish::ckan::CkanApi;
    let gap = Duration::from_millis(40);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let last = Arc::new(Mutex::new(None));
    // Each call's turn as Paced took it under its lock, read back from the gate it left. The
    // double notes its own time only after that lock is let go, so a thread held up there longer
    // on one call than on the next shrinks a gap measured between the notes (39.96 ms on ci).
    let turn = || turn_of(&last, gap);
    let mut first = Paced::new(Clocked(calls.clone()), last.clone(), gap);
    first.show("package_show", "a").expect("show");
    let mut turns = vec![turn()];
    first.action("package_patch", &json!({})).expect("action");
    turns.push(turn());
    // The next Endpoint's client is a new one; the gap is the Portal's, not the client's.
    let second = Paced::new(Clocked(calls.clone()), last.clone(), gap);
    second.show("package_show", "b").expect("show");
    turns.push(turn());
    for pair in turns.windows(2) {
        assert!(pair[1] - pair[0] >= gap, "{:?}", pair[1] - pair[0]);
    }
    let at = calls.lock().expect("calls").clone();
    assert_eq!(at.len(), 3);
    for (call, turn) in at.iter().zip(&turns) {
        assert!(call >= turn, "a call goes no sooner than its turn");
    }
    assert_eq!(
        CALL_GAP,
        Duration::from_millis(200),
        "at most five calls a second"
    );
}

#[test]
fn the_gateway_reads_of_each_publication_keep_the_catalogues_pace() {
    // T-4717: prod's edge saw the publisher at 25 requests a second, all from the record, `/ws`
    // and schema reads, which went out beside the paced catalogue calls.
    use jcctl::publish::ckan::CkanApi;
    let gap = Duration::from_millis(30);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let last = Arc::new(Mutex::new(None));
    let api = Paced::new(Clocked(calls.clone()), last.clone(), gap);
    let read = || calls.lock().expect("calls").push(Instant::now());
    let endpoints = 3;
    // The turns as Paced took them, not as the reads noted them (see the test above).
    let mut first = None;
    for _ in 0..endpoints {
        api.reads(RECORD_READS, read);
        first.get_or_insert(turn_of(&last, gap * RECORD_READS));
        api.reads(MODELS_READS, read);
    }
    // The next Endpoint's catalogue call, on a client of its own, waits for every read before it.
    Paced::new(Clocked(calls.clone()), last.clone(), gap)
        .show("package_show", "next")
        .expect("show");
    let (first, next) = (first.expect("a read"), turn_of(&last, gap));
    let at = calls.lock().expect("calls").clone();
    assert_eq!(at.len(), 2 * endpoints + 1);
    let held = gap * (RECORD_READS + MODELS_READS) * endpoints as u32;
    assert!(next - first >= held, "{:?} < {held:?}", next - first);
    assert!(at[0] >= first && at[at.len() - 1] >= next);
    assert_eq!(
        (RECORD_READS, MODELS_READS),
        (3, 2),
        "/, /ws, /ngsi-ld/v1/types; the two schemas"
    );

    // A read of nothing waits for nothing and takes no turn, even behind a gate held 10 s out:
    // waiting would cost the whole 10 s, which no load on a runner stalls a thread for.
    let far = Duration::from_secs(10);
    let gate = Some(Instant::now() + far);
    *last.lock().expect("gate") = gate;
    let before = Instant::now();
    api.reads(0, || ());
    assert!(before.elapsed() < far);
    assert_eq!(*last.lock().expect("gate"), gate);
}

/// The turn the last paced call took: Paced leaves its gate at that turn plus `held`.
fn turn_of(last: &Mutex<Option<Instant>>, held: Duration) -> Instant {
    last.lock().expect("gate").expect("a turn taken") - held
}

/// The publisher's catalogue double behind a shared handle, so each Endpoint's client is a new
/// one as in a pass, noting every `*_show` it is asked.
struct Shared(Arc<Mutex<(InMemoryCkan, Vec<String>)>>);

impl jcctl::publish::ckan::CkanApi for Shared {
    fn show(
        &self,
        action: &str,
        name: &str,
    ) -> Result<Option<Value>, jcctl::publish::ckan::CkanError> {
        let mut held = self.0.lock().expect("catalogue");
        held.1.push(format!("{action} {name}"));
        held.0.show(action, name)
    }

    fn action(
        &mut self,
        action: &str,
        payload: &Value,
    ) -> Result<Value, jcctl::publish::ckan::CkanError> {
        let answered = self
            .0
            .lock()
            .expect("catalogue")
            .0
            .action(action, payload)?;
        // CKAN answers organization_create with the organization; the double answers `{}`.
        Ok(match action {
            "organization_create" => payload.clone(),
            _ => answered,
        })
    }
}

#[test]
fn a_pass_over_many_endpoints_of_one_organization_asks_for_it_once() {
    // T-5314: prod's edge saw 184 organization_show calls in two minutes for one organization,
    // one per published Endpoint, all through the public limit.
    let catalogue = Arc::new(Mutex::new((
        InMemoryCkan::new().with_token(TOKEN),
        Vec::new(),
    )));
    let shows = |catalogue: &Arc<Mutex<(InMemoryCkan, Vec<String>)>>| {
        let held = catalogue.lock().expect("catalogue");
        held.1
            .iter()
            .filter(|call| call.starts_with("organization_show"))
            .count()
    };
    let last = Arc::new(Mutex::new(None));
    let known = Organizations::default();
    let api_url = "http://ckan.ckan.svc.cluster.local:5000";
    for n in 0..5 {
        let mut api = Paced::new(Shared(catalogue.clone()), last.clone(), Duration::ZERO)
            .remembering(api_url, known.clone());
        let target = target(&format!("helsinki-bikes-{n}"), "public", dataset_only());
        let published = publish_with(&mut api, &target, &record(), None, &settings())
            .expect("the endpoint publishes");
        assert!(
            matches!(published, Publication::Published { .. }),
            "{published:?}"
        );
    }
    // The first asks, finds none and creates it; the other four take what the creation answered.
    assert_eq!(shows(&catalogue), 1);
    assert_eq!(
        catalogue
            .lock()
            .expect("catalogue")
            .0
            .actions()
            .iter()
            .filter(|a| **a == "organization_create")
            .count(),
        1
    );
    for n in 0..5 {
        assert!(catalogue
            .lock()
            .expect("catalogue")
            .0
            .package(&format!("helsinki-bikes-{n}"))
            .is_some());
    }

    // Another catalogue, or the next pass, asks again: nothing outlives one pass of one catalogue.
    let elsewhere = Paced::new(Shared(catalogue.clone()), last.clone(), Duration::ZERO)
        .remembering("https://data.example.fi", known.clone());
    jcctl::publish::ckan::CkanApi::show(&elsewhere, "organization_show", "hel-fi").expect("show");
    let next_pass = Paced::new(Shared(catalogue.clone()), last, Duration::ZERO)
        .remembering(api_url, Organizations::default());
    jcctl::publish::ckan::CkanApi::show(&next_pass, "organization_show", "hel-fi").expect("show");
    assert_eq!(shows(&catalogue), 3);
}

#[test]
fn an_organization_the_catalogue_answers_with_nothing_usable_is_asked_again() {
    // The double answers organization_patch with `{}`: that is no organization to answer from.
    use jcctl::publish::ckan::CkanApi;
    let catalogue = Arc::new(Mutex::new((
        InMemoryCkan::new()
            .with_token(TOKEN)
            .with_organization("hel-fi"),
        Vec::new(),
    )));
    let known = Organizations::default();
    let mut api = Paced::new(Shared(catalogue.clone()), Arc::default(), Duration::ZERO)
        .remembering("https://data.example.fi", known.clone());
    api.show("organization_show", "hel-fi").expect("show");
    api.show("organization_show", "hel-fi").expect("show");
    api.action(
        "organization_patch",
        &json!({ "id": "hel-fi", "title": "Helsinki" }),
    )
    .expect("patch");
    api.show("organization_show", "hel-fi").expect("show");
    let calls = catalogue.lock().expect("catalogue").1.clone();
    assert_eq!(
        calls,
        ["organization_show hel-fi", "organization_show hel-fi"]
    );
}

/// [`Shared`] taking a little time per call, as a catalogue does, so publications overlap.
struct Slow(Shared);

impl jcctl::publish::ckan::CkanApi for Slow {
    fn show(
        &self,
        action: &str,
        name: &str,
    ) -> Result<Option<Value>, jcctl::publish::ckan::CkanError> {
        std::thread::sleep(Duration::from_millis(15));
        self.0.show(action, name)
    }

    fn action(
        &mut self,
        action: &str,
        payload: &Value,
    ) -> Result<Value, jcctl::publish::ckan::CkanError> {
        std::thread::sleep(Duration::from_millis(15));
        self.0.action(action, payload)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wave_publishes_side_by_side_and_creates_each_organization_once() {
    // C1: 609 datasets one at a time took about 50 minutes. Side by side, the first datasets of
    // one organization would each find it missing and each create it: the organization's first
    // goes alone, the pass's `Organizations` holds it, the rest follow at once.
    use std::sync::atomic::{AtomicUsize, Ordering};
    let catalogue = Arc::new(Mutex::new((
        InMemoryCkan::new().with_token(TOKEN),
        Vec::new(),
    )));
    let known = Organizations::default();
    let (now, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let targets: Vec<Target> = (0..24)
        .map(|n| target(&format!("helsinki-bikes-{n}"), "public", dataset_only()))
        .collect();
    let names: Vec<String> = targets.iter().map(|t| t.id.name.clone()).collect();
    let results = wave(targets, PUBLISH_CONCURRENCY, |target| {
        let (catalogue, known, now, most) =
            (catalogue.clone(), known.clone(), now.clone(), most.clone());
        async move {
            tokio::task::spawn_blocking(move || {
                let running = now.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(running, Ordering::SeqCst);
                let mut api = Paced::new(Slow(Shared(catalogue)), Arc::default(), Duration::ZERO)
                    .remembering("http://127.0.0.1:42301", known);
                let out = publish_with(&mut api, &target, &record(), None, &settings());
                now.fetch_sub(1, Ordering::SeqCst);
                (target.id.name.clone(), out)
            })
            .await
            .expect("the publication finishes")
        }
    })
    .await;
    // Each result in its target's place, each its own.
    assert_eq!(
        results.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
        names
    );
    assert!(results
        .iter()
        .all(|(_, r)| matches!(r, Ok(Publication::Published { .. }))));
    let held = catalogue.lock().expect("catalogue");
    let creates = held
        .0
        .actions()
        .iter()
        .filter(|a| **a == "organization_create")
        .count();
    assert_eq!(creates, 1, "{:?}", held.0.actions());
    assert!(
        most.load(Ordering::SeqCst) > 1,
        "the wave ran one at a time"
    );
    assert!(most.load(Ordering::SeqCst) <= PUBLISH_CONCURRENCY);
}

/// A target whose Endpoint serves several representations and declares one chart, so its
/// dataset has several resources and one view.
fn charted(name: &str) -> Target {
    let mut target = target(name, "public", dataset_only());
    let spec = &mut target.manifest.spec;
    spec["enabledRepresentations"] = json!(["ngsi-ld", "csv", "ogc-features"]);
    spec["catalog"] = json!({ "visualizations": [{
        "name": "bikes-now",
        "title": { "en": "Bikes now" },
        "description": { "en": "Free bikes per station." },
        "data": { "source": "csv", "type": "BikeHireDockingStation" },
        "spec": {
            "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
            "data": { "name": "source" },
            "mark": "bar",
            "encoding": { "x": { "field": "name", "type": "nominal" } },
        },
    }]});
    target
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wave_makes_a_fixed_number_of_catalogue_calls_per_dataset() {
    // C1: on the prod rehearsal every hourly republish read the view list of every resource of
    // every dataset, about 15 a dataset: 25,004 resource_view_list calls in 2 h 35 min over
    // about 609 datasets. Each dataset's calls are bounded whatever its resource count.
    const N: usize = 12;
    let catalogue = Arc::new(Mutex::new((
        InMemoryCkan::new().with_token(TOKEN),
        Vec::new(),
    )));
    let targets: Vec<Target> = (0..N)
        .map(|n| charted(&format!("helsinki-bikes-{n}")))
        .collect();
    let run = |targets: Vec<Target>| {
        let catalogue = catalogue.clone();
        let known = Organizations::default();
        wave(targets, PUBLISH_CONCURRENCY, move |target| {
            let (catalogue, known) = (catalogue.clone(), known.clone());
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut api = Paced::new(Shared(catalogue), Arc::default(), Duration::ZERO)
                        .remembering("http://127.0.0.1:42301", known);
                    publish_with(&mut api, &target, &record(), None, &settings())
                })
                .await
                .expect("the publication finishes")
            }
        })
    };
    // (shows, writes) the catalogue saw so far, and forget them.
    let calls = || {
        let mut held = catalogue.lock().expect("catalogue");
        let shows = std::mem::take(&mut held.1);
        let writes = held.0.actions().len();
        (shows, writes)
    };

    let first = run(targets.clone()).await;
    assert!(
        first
            .iter()
            .all(|r| matches!(r, Ok(Publication::Published { .. }))),
        "{first:?}"
    );
    let (shows, writes_first) = calls();
    let resources = catalogue
        .lock()
        .expect("catalogue")
        .0
        .package("helsinki-bikes-0")
        .expect("dataset")["resources"]
        .as_array()
        .expect("resources")
        .len();
    assert!(resources > 2, "the dataset has {resources} resources");
    // One organization_show for the pass; then per dataset a package_show before the write and
    // one before the views; a new dataset holds no view, so no list is read.
    assert!(shows.len() <= 1 + 2 * N, "{} shows: {shows:?}", shows.len());
    // organization_create once; per dataset package_create and resource_view_create.
    assert!(writes_first <= 1 + 2 * N, "{writes_first} writes");

    // The hourly refresh of the same datasets: two package_show and the one resource that holds
    // the view, whatever the number of resources; nothing written.
    let again = run(targets).await;
    assert!(
        again
            .iter()
            .all(|r| matches!(r, Ok(Publication::Published { .. }))),
        "{again:?}"
    );
    let (shows, writes) = calls();
    assert_eq!(writes, writes_first, "an unchanged dataset is written");
    let lists = shows
        .iter()
        .filter(|s| s.starts_with("resource_view_list"))
        .count();
    assert_eq!(lists, N, "{shows:?}");
    assert!(shows.len() <= 1 + 3 * N, "{} shows: {shows:?}", shows.len());
}

#[tokio::test]
async fn one_failing_target_of_a_wave_is_its_own_result() {
    let targets: Vec<Target> = ["a", "b", "c", "d"]
        .iter()
        .map(|n| target(n, "public", dataset_only()))
        .collect();
    let results = wave(targets, 2, |target| async move {
        if target.id.name == "b" {
            Err(format!("{} failed", target.id.name))
        } else {
            Ok(target.id.name)
        }
    })
    .await;
    assert_eq!(
        results,
        vec![
            Ok("a".to_owned()),
            Err("b failed".to_owned()),
            Ok("c".to_owned()),
            Ok("d".to_owned())
        ]
    );
}

#[test]
fn the_gap_holds_on_the_public_route_and_not_on_the_in_cluster_or_loopback_one() {
    use jcctl::publish::ckan::CkanApi;
    let public = "https://data.example.fi";
    assert_eq!(catalogue_gap(public, public), CALL_GAP);
    assert_eq!(catalogue_gap("https://data.example.fi/", public), CALL_GAP);
    assert_eq!(
        catalogue_gap("http://ckan.ckan.svc.cluster.local:5000", public),
        Duration::ZERO
    );
    assert_eq!(
        catalogue_gap(
            "http://127.0.0.1:42301/opendata",
            "http://127.0.0.1:42301/opendata"
        ),
        Duration::ZERO
    );
    assert_eq!(read_gap("api.example.fi"), CALL_GAP);
    assert_eq!(read_gap("https://api.example.fi"), CALL_GAP);
    assert_eq!(read_gap("http://127.0.0.1:42301"), Duration::ZERO);
    assert_eq!(read_gap("http://api.hel.localhost:42301"), Duration::ZERO);
    assert_eq!(read_gap("[::1]:8080"), Duration::ZERO);

    // An unspaced call never moves the public route's reservation earlier.
    let gap = Duration::from_millis(60);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let last = Arc::new(Mutex::new(None));
    let mut public_api = Paced::new(Clocked(calls.clone()), last.clone(), gap);
    let loopback = Paced::new(Clocked(calls.clone()), last.clone(), Duration::ZERO);
    public_api
        .action("package_patch", &json!({}))
        .expect("call");
    let (first, reserved) = (turn_of(&last, gap), *last.lock().expect("gate"));
    // The loopback call neither waits nor takes a turn, even behind a gate held 10 s out.
    let far = Duration::from_secs(10);
    let gate = Some(Instant::now() + far);
    *last.lock().expect("gate") = gate;
    let before = Instant::now();
    loopback.show("package_show", "x").expect("call");
    assert!(before.elapsed() < far, "the loopback call waited");
    assert_eq!(
        *last.lock().expect("gate"),
        gate,
        "the loopback call took a turn"
    );
    *last.lock().expect("gate") = reserved;
    public_api.show("package_show", "y").expect("call");
    let next = turn_of(&last, gap);
    assert!(next - first >= gap, "{:?}", next - first);
    let at = calls.lock().expect("calls").clone();
    assert!(at[0] >= first && at[2] >= next);

    // Gateway reads on a public data host keep the gap even when the catalogue is in-cluster.
    let reads = Paced::new(Clocked(calls.clone()), last, Duration::ZERO).with_read_gap(gap);
    let before = Instant::now();
    reads.reads(1, || ());
    reads.reads(1, || ());
    assert!(before.elapsed() >= gap);
}

#[test]
fn only_the_deployments_own_catalogue_is_called_on_its_service() {
    let own: jc_core::kinds::ckan::CkanInstanceSpec =
        serde_json::from_value(instance()).expect("an instance");
    let mut other_spec = instance();
    other_spec["url"] = json!("https://data.other.example.fi/");
    let other: jc_core::kinds::ckan::CkanInstanceSpec =
        serde_json::from_value(other_spec).expect("an instance");

    let sync = CkanSync::new(HOST).reaching(
        "https://data.example.fi/",
        "http://ckan.ckan.svc.cluster.local:5000/",
    );
    assert_eq!(
        sync.api_url(&own),
        "http://ckan.ckan.svc.cluster.local:5000"
    );
    assert_eq!(sync.api_url(&other), "https://data.other.example.fi");
    // Without the operator's route every catalogue is called where its manifest says.
    assert_eq!(CkanSync::new(HOST).api_url(&own), "https://data.example.fi");
}

// T-6079: every seeded CkanInstance on dev points at one CKAN, and five org catalogues were all
// called `catalog`: each pass published package `catalog` five times and kept the last.

/// `name`'s target in `project`, publishing to the catalogue at `url`.
fn target_in(project: &str, name: &str, url: &str) -> Target {
    let mut target = target(name, "public", dataset_only());
    target.id.namespace = Some(project.to_owned());
    target.manifest.metadata.namespace = Some(project.to_owned());
    target.instance.url = url.to_owned();
    target
}

#[test]
fn two_endpoints_of_two_projects_publishing_one_dataset_name_to_one_catalogue_are_both_refused() {
    let zilina = target_in("zilina", "catalog", "https://opendata.example.sk");
    // The other project's instance writes the URL with a trailing slash and capitals: one CKAN.
    let praha = target_in("praha", "catalog", "https://OpenData.example.sk/");
    let elsewhere = target_in("praha", "bikes", "https://opendata.example.sk");
    let refused = claimed_twice(&[zilina, praha, elsewhere]);

    assert_eq!(refused.len(), 2, "{refused:?}");
    let key = |project: &str| {
        (
            project.to_owned(),
            "catalog".to_owned(),
            "catalog".to_owned(),
        )
    };
    let reason = &refused[&key("zilina")];
    assert!(reason.contains("Endpoint praha/catalog"), "{reason}");
    assert!(reason.contains("`zilina-catalog`"), "{reason}");
    let reason = &refused[&key("praha")];
    assert!(reason.contains("Endpoint zilina/catalog"), "{reason}");
    assert!(reason.contains("spec.publish.ckan.name"), "{reason}");
}

#[test]
fn a_dataset_name_set_in_the_block_collides_with_another_endpoints_own_name() {
    let own = target_in(PROJECT, "bikes", "https://data.example.fi");
    let mut named = target_in(PROJECT, "stations", "https://data.example.fi");
    named.publication.name = Some("bikes".to_owned());
    assert_eq!(claimed_twice(&[own, named]).len(), 2);
}

#[test]
fn two_endpoints_on_two_catalogues_may_share_a_dataset_name() {
    let zilina = target_in("zilina", "catalog", "https://opendata.zilina.sk");
    let praha = target_in("praha", "catalog", "https://opendata.praha.eu");
    assert!(claimed_twice(&[zilina, praha]).is_empty());
}

fn manifest_in(project: &str, kind: &str, name: &str, spec: Value) -> String {
    serde_norway::to_string(&json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": kind,
        "metadata": { "name": name, "namespace": project },
        "spec": spec,
    }))
    .expect("yaml")
}

#[tokio::test]
async fn a_pass_refuses_a_colliding_pair_before_any_catalogue_call_and_withdraws_nothing_still_published(
) {
    let repository = std::env::temp_dir().join(format!("jc-ckan-collision-{}", std::process::id()));
    std::fs::create_dir_all(&repository).expect("a scratch directory");
    for project in ["a", "b"] {
        std::fs::write(
            repository.join(format!("{project}.yaml")),
            format!(
                "{}---\n{}",
                manifest_in(project, "CkanInstance", "hel-fi", instance()),
                manifest_in(
                    project,
                    "Endpoint",
                    "catalog",
                    endpoint("catalog", "public", Some(dataset_only()))
                )
            ),
        )
        .expect("a project");
    }
    // Project c published `catalog` to the same CKAN a run ago and is gone: its withdrawal would
    // delete the dataset a and b still name.
    let previous = Mirror::new();
    for (kind, name, spec) in [
        ("CkanInstance", "hel-fi", instance()),
        (
            "Endpoint",
            "catalog",
            endpoint("catalog", "public", Some(dataset_only())),
        ),
    ] {
        previous.upsert(
            serde_json::from_value(json!({
                "apiVersion": "joinedcontext.com/v1alpha1",
                "kind": kind,
                "metadata": { "name": name, "namespace": "c" },
                "spec": spec,
            }))
            .expect("an envelope"),
        );
    }
    let repo = jcctl::loader::Repository::load(&repository).expect("the repository loads");
    let reports = CkanSync::new(HOST)
        .converge(&repo, &previous, &repository)
        .await;
    std::fs::remove_dir_all(&repository).expect("cleaned up");

    // Two Endpoint lines, each the collision: no token was resolved for either (that refusal
    // reads "api-token-env"), so neither reached the catalogue; no line for c's withdrawal. The
    // organization's sweep (a read, and it keeps `catalog`) is the line without an Endpoint.
    let lines: Vec<(&str, &str)> = reports
        .iter()
        .filter(|r| !r.endpoint.is_empty())
        .filter_map(|r| match &r.publication {
            Publication::Failed(reason) => Some((r.project.as_str(), reason.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(lines.len(), 2, "{reports:?}");
    assert!(lines[0].1.contains("Endpoint b/catalog"), "{lines:?}");
    assert!(lines[1].1.contains("Endpoint a/catalog"), "{lines:?}");
    assert!(reports.iter().all(|r| r.project != "c"), "{reports:?}");
}

/// One Endpoint listing two datasets in `spec.catalog.datasets` (EP-175).
fn listing(names: &[&str]) -> Value {
    let mut spec = endpoint("air", "public", None);
    spec["catalog"] = json!({
        "datasets": names
            .iter()
            .map(|name| json!({
                "name": name,
                "title": { "en": format!("Dataset {name}") },
                "description": { "en": "A slice of the air" },
                "types": ["AirQualityObserved"],
            }))
            .collect::<Vec<_>>(),
    });
    spec
}

/// EP-178: removing one entry of an Endpoint's list withdraws exactly that dataset: never its
/// sibling, never another Endpoint's, and a public Endpoint without a block is a dataset by its
/// own name, withdrawn when it stops being public (EP-174).
#[test]
fn removing_one_entry_withdraws_only_that_dataset() {
    let previous = Mirror::new();
    previous.upsert(envelope("CkanInstance", "hel-fi", instance()));
    previous.upsert(envelope(
        "Endpoint",
        "air",
        listing(&["hel-air-pm", "hel-air-no2"]),
    ));
    previous.upsert(envelope(
        "Endpoint",
        "bikes",
        endpoint("bikes", "public", None),
    ));
    previous.upsert(envelope(
        "Endpoint",
        "closed-bikes",
        endpoint("closed-bikes", "public", None),
    ));

    // This run: `air` keeps `hel-air-pm` alone, `bikes` is still public, `closed-bikes` closed.
    let catalogue = "https://data.example.fi".to_owned();
    let kept: BTreeSet<(String, String)> = ["hel-air-pm", "bikes"]
        .into_iter()
        .map(|name| (catalogue.clone(), name.to_owned()))
        .collect();
    let gone = withdrawn(&previous, &kept, None);
    let names: BTreeSet<(String, String)> = gone
        .iter()
        .map(|t| (t.id.name.clone(), t.dataset_name().to_owned()))
        .collect();
    assert_eq!(
        names,
        [
            ("air".to_owned(), "hel-air-no2".to_owned()),
            ("closed-bikes".to_owned(), "closed-bikes".to_owned()),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
    );
    for target in &gone {
        assert_eq!(target.instance_name, "hel-fi");
    }
}

/// EP-174: a closed Endpoint without a block was never a dataset, so it is never withdrawn,
/// and an App's own door (`app-{name}` beside App `{name}`) is not one either.
#[test]
fn a_closed_endpoint_and_an_apps_door_are_never_datasets_to_withdraw() {
    let previous = Mirror::new();
    previous.upsert(envelope("CkanInstance", "hel-fi", instance()));
    previous.upsert(envelope(
        "Endpoint",
        "internal",
        endpoint("internal", "organization", None),
    ));
    previous.upsert(envelope("App", "bikes", json!({})));
    previous.upsert(envelope(
        "Endpoint",
        "app-bikes",
        endpoint("app-bikes", "public", None),
    ));
    assert!(withdrawn(&previous, &BTreeSet::new(), None).is_empty());
}

/// EP-177: two entries of two Endpoints with one name on one catalogue are refused; the other
/// dataset of the same Endpoint is still published.
#[test]
fn a_colliding_entry_is_refused_and_its_sibling_is_not() {
    let entry = |name: &str| -> jc_core::kinds::catalog_datasets::CatalogDataset {
        serde_json::from_value(json!({
            "name": name,
            "title": { "en": name },
            "description": { "en": name },
            "types": ["AirQualityObserved"],
        }))
        .expect("an entry")
    };
    let mut pm = target_in(PROJECT, "air", "https://data.example.fi");
    pm.slice = Some(entry("hel-air-pm"));
    let mut no2 = target_in(PROJECT, "air", "https://data.example.fi");
    no2.slice = Some(entry("hel-air-no2"));
    let other = target_in(PROJECT, "hel-air-pm", "https://data.example.fi");
    let refused = claimed_twice(&[pm, no2, other]);
    assert_eq!(refused.len(), 2, "{refused:?}");
    let reason = &refused[&(
        PROJECT.to_owned(),
        "air".to_owned(),
        "hel-air-pm".to_owned(),
    )];
    assert!(reason.contains("Endpoint helsinki/hel-air-pm"), "{reason}");
    assert!(reason.contains("spec.catalog.datasets entry"), "{reason}");
    assert!(!refused.contains_key(&(
        PROJECT.to_owned(),
        "air".to_owned(),
        "hel-air-no2".to_owned()
    )));
}

/// EP-174, EP-178: an Endpoint the planner cannot place this pass (a second CkanInstance made
/// its catalogue ambiguous) is said to be unpublished and keeps the dataset it had: neither a
/// withdrawal nor a sweep deletes it while its author fixes the block.
#[tokio::test]
async fn an_organizations_catalogue_keeps_the_dataset_its_endpoint_still_publishes() {
    // Host mode (jc [[organizations]]): the dataset goes to hel's own catalogue, not the
    // instance's data.example.fi. The previous run published it; this run publishes it again.
    let repository = std::env::temp_dir().join(format!("jc-ckan-org-kept-{}", std::process::id()));
    std::fs::create_dir_all(&repository).expect("a scratch directory");
    let token_file = repository.join("hel.token");
    std::fs::write(&token_file, "hel-token\n").expect("the token");
    let org = |kind: &str, name: &str, spec: Value| {
        serde_norway::to_string(&json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": kind,
            "metadata": { "name": name },
            "spec": spec,
        }))
        .expect("yaml")
    };
    let bikes = endpoint("bikes", "public", Some(dataset_only()));
    std::fs::write(
        repository.join("helsinki.yaml"),
        [
            org("Organization", "hel", json!({ "installation": true })),
            org("Project", PROJECT, json!({ "organizationRef": "hel" })),
            manifest_in(PROJECT, "CkanInstance", "hel-fi", instance()),
            manifest_in(PROJECT, "Endpoint", "bikes", bikes.clone()),
        ]
        .join("---\n"),
    )
    .expect("the project");
    let previous = Mirror::new();
    previous.upsert(envelope("CkanInstance", "hel-fi", instance()));
    previous.upsert(envelope("Endpoint", "bikes", bikes));
    let repo = jcctl::loader::Repository::load(&repository).expect("the repository loads");
    let catalogue = joinedcontext_portal::organization_host::OrganizationCatalogue {
        organization: "hel".to_owned(),
        url: "http://opendata.hel.localhost:9".parse().expect("a url"),
        api: None,
        // Nothing listens there: each catalogue call is one failed report, never a hang.
        reach: Some("127.0.0.1:9".parse().expect("an address")),
        data_host: None,
        token_file,
    };
    let reports = CkanSync::new(HOST)
        .for_organizations(vec![catalogue])
        .converge(&repo, &previous, &repository)
        .await;
    std::fs::remove_dir_all(&repository).expect("cleaned up");

    // One line for `bikes`, its publication. Without the fix a second one: its withdrawal from
    // hel's catalogue, compared against the instance's catalogue it no longer goes to.
    let bikes: Vec<_> = reports.iter().filter(|r| r.dataset == "bikes").collect();
    assert_eq!(bikes.len(), 1, "{reports:?}");
}

/// T-6853, ADR-N-058 2.6: a project's datasets go to its own Organization's catalogue, so the
/// pass after its `organizationRef` moved from `hel` to `bbsk` publishes them in `bbsk`'s, and
/// the pass after the move back in `hel`'s again. Neither catalogue has its token here, so each
/// publication fails naming the catalogue it went to, and never the other.
#[tokio::test]
async fn a_moved_projects_datasets_follow_its_organization() {
    let bikes = endpoint("bikes", "public", Some(dataset_only()));
    let previous = Mirror::new();
    previous.upsert(envelope("CkanInstance", "hel-fi", instance()));
    previous.upsert(envelope("Endpoint", "bikes", bikes.clone()));
    let org = |kind: &str, name: &str, spec: Value| {
        serde_norway::to_string(&json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": kind,
            "metadata": { "name": name },
            "spec": spec,
        }))
        .expect("yaml")
    };
    for (owner, other) in [("bbsk", "hel"), ("hel", "bbsk")] {
        let repository = std::env::temp_dir().join(format!(
            "jc-ckan-moved-{owner}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&repository).expect("a scratch directory");
        std::fs::write(
            repository.join("helsinki.yaml"),
            [
                org("Organization", "hel", json!({ "domain": "hel.fi" })),
                org("Organization", "bbsk", json!({ "domain": "bbsk.sk" })),
                org("Project", PROJECT, json!({ "organizationRef": owner })),
                manifest_in(PROJECT, "CkanInstance", "hel-fi", instance()),
                manifest_in(PROJECT, "Endpoint", "bikes", bikes.clone()),
            ]
            .join("---\n"),
        )
        .expect("the project");
        let catalogue = |organization: &str| {
            joinedcontext_portal::organization_host::OrganizationCatalogue {
                organization: organization.to_owned(),
                url: format!("http://opendata.{organization}.localhost:9")
                    .parse()
                    .expect("a url"),
                api: None,
                reach: Some("127.0.0.1:9".parse().expect("an address")),
                data_host: None,
                token_file: repository.join(format!("{organization}.token")),
            }
        };
        let repo = jcctl::loader::Repository::load(&repository).expect("the repository loads");
        let reports = CkanSync::new(HOST)
            .for_organizations(vec![catalogue("hel"), catalogue("bbsk")])
            .converge(&repo, &previous, &repository)
            .await;
        std::fs::remove_dir_all(&repository).expect("cleaned up");
        let published: Vec<_> = reports.iter().filter(|r| r.dataset == "bikes").collect();
        assert!(!published.is_empty(), "{reports:?}");
        for report in published {
            let Publication::Failed(reason) = &report.publication else {
                panic!("{owner}: published without a token: {report:?}");
            };
            assert!(reason.contains(&format!("{owner}'s catalogue")), "{owner}: {reason}");
            assert!(!reason.contains(&format!("{other}'s")), "{owner}: {reason}");
        }
    }
}

#[tokio::test]
async fn an_endpoint_the_pass_cannot_place_keeps_its_dataset() {
    let previous = Mirror::new();
    previous.upsert(envelope("CkanInstance", "hel-fi", instance()));
    previous.upsert(envelope(
        "Endpoint",
        "bikes",
        endpoint("bikes", "public", None),
    ));

    let repository = std::env::temp_dir().join(format!("jc-ckan-held-{}", std::process::id()));
    std::fs::create_dir_all(&repository).expect("a scratch directory");
    let mut second = instance();
    second["url"] = json!("https://open.example.fi");
    std::fs::write(
        repository.join("helsinki.yaml"),
        [
            manifest_in(PROJECT, "CkanInstance", "hel-fi", instance()),
            manifest_in(PROJECT, "CkanInstance", "open-fi", second),
            manifest_in(
                PROJECT,
                "Endpoint",
                "bikes",
                endpoint("bikes", "public", None),
            ),
        ]
        .join("---\n"),
    )
    .expect("the project");
    let repo = jcctl::loader::Repository::load(&repository).expect("the repository loads");
    let reports = CkanSync::new(HOST)
        .converge(&repo, &previous, &repository)
        .await;
    std::fs::remove_dir_all(&repository).expect("cleaned up");

    assert!(
        reports.iter().any(|r| r.endpoint == "bikes"
            && matches!(&r.publication, Publication::Failed(reason) if reason.contains("open-fi"))),
        "{reports:?}"
    );
    // Without the guard the pass withdraws `bikes` from hel-fi: a report of that dataset.
    assert!(!reports.iter().any(|r| r.dataset == "bikes"), "{reports:?}");
}
