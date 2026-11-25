//! The stream deployer's refusals and failures (T-7807): every reason a pass gives a pipeline no
//! stream, each answer of the runner the happy-path tests never see, and the run queue's hints.

use super::*;
use std::collections::BTreeMap;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn put(mirror: &Mirror, kind: &str, ns: &str, name: &str, spec: Value) {
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: kind.to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some(ns.to_string()),
            ..Default::default()
        },
        spec,
        status: None,
    });
}

/// The outcome of `name` in one pass, as text (`Error("...")`, `Skipped("...")`, `Live`).
fn said(outcomes: &[(String, String, StreamOutcome)], name: &str) -> String {
    let (_, _, outcome) = outcomes
        .iter()
        .find(|(_, n, _)| n == name)
        .expect("an outcome for the pipeline");
    format!("{outcome:?}")
}

const TARGET: &str = "urn:ngsi-ld:Endpoint:example.org:helsinki:helsinki-all";

/// A Pipeline reading the resident `hsl-citybikes-free` socket into `helsinki-all`, with `extra`
/// merged over it.
fn pipeline(extra: Value) -> Value {
    let mut spec = serde_json::json!({
        "class": "resident",
        "source": { "dataSourceRef": { "kind": "DataSource", "name": "hsl-citybikes-free" } },
        "compute": { "kind": "bloblang", "bloblang": "root = this" },
        "targetEndpoint": TARGET
    });
    for (key, field) in extra.as_object().into_iter().flatten() {
        spec[key.as_str()] = field.clone();
    }
    spec
}

/// A runner that takes every budget and every PUT.
async fn taking_runner() -> MockServer {
    let server = runner().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn a_runner_token_goes_as_a_bearer_and_one_that_is_no_header_value_goes_not_at_all() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/metrics"))
        .and(header("authorization", "Bearer s3cret"))
        .respond_with(ResponseTemplate::new(200).set_body_string("up 1"))
        .mount(&server)
        .await;
    let deployer = StreamDeployer::new(server.uri()).with_runner_token(Some("s3cret".to_owned()));
    assert_eq!(deployer.metrics("helsinki").await.as_deref(), Some("up 1"));
    // No token, no header: the runner refuses the read.
    assert_eq!(
        StreamDeployer::new(server.uri()).metrics("helsinki").await,
        None
    );

    let (client, log) = crate::reconciler::logged(async {
        runner_client(crate::reach::builder(), Some("bad\ntoken"))
    })
    .await;
    assert!(log.contains("is no header value"), "{log}");
    let answer = client
        .get(format!("{}/metrics", server.uri()))
        .send()
        .await
        .expect("answered");
    assert_eq!(answer.status(), reqwest::StatusCode::NOT_FOUND);
}

/// A `contextSpaceRef` written as `{ name }` names the space as the bare string does.
#[test]
fn a_space_named_by_an_object_reference_is_the_streams_space() {
    let mirror = Mirror::new();
    put(
        &mirror,
        "Endpoint",
        "helsinki",
        "helsinki-all",
        serde_json::json!({ "contextSpaceRef": { "name": "air" }, "slug": "s" }),
    );
    let spec: PipelineSpec =
        serde_json::from_value(pipeline(serde_json::json!({}))).expect("a spec");
    assert_eq!(StreamSpaces::of(&mirror, "helsinki", &spec).spaces, ["air"]);
}

/// PL-16, PL-52: a read of the runner's environment is refused in a processor step, a route's
/// condition and the delete marker alike, and a datacube step whose parameters do not check
/// renders nothing.
#[test]
fn a_step_reading_the_runners_environment_is_refused_wherever_it_stands() {
    let refused = |extra: Value| {
        let spec: PipelineSpec = serde_json::from_value(pipeline(extra)).expect("a spec");
        reads_only_its_own(&spec).expect_err("refused")
    };
    let secret = r#"root = env("JC_CLIENT_SECRET")"#;
    let step = refused(serde_json::json!({
        "compute": null,
        "steps": [{ "processor": { "mapping": secret } }]
    }));
    assert!(step.contains("spec.steps.processor"), "{step}");
    let route = refused(serde_json::json!({
        "routes": [{ "name": "hot", "when": r#"env("JC_CLIENT_SECRET") == "x""#,
                     "outputs": [{ "targetEndpoint": TARGET }] }]
    }));
    assert!(route.contains("spec.routes.when"), "{route}");
    let marker = refused(serde_json::json!({
        "sourceDeletes": { "mode": "events", "deletedWhen": r#"env("JC_CLIENT_SECRET") != """# }
    }));
    assert!(
        marker.contains("spec.sourceDeletes.deletedWhen"),
        "{marker}"
    );
    let cube = refused(serde_json::json!({
        "compute": { "kind": "datacube", "datacube": {
            "wholeArea": "not a code!", "datasetPage": "https://example.org/{cube}", "cubes": {} } }
    }));
    assert!(cube.contains("wholeArea"), "{cube}");
}

/// The queue's hints reach it even after a panic poisoned its lock, and a pipeline it does not
/// hold cannot be run now.
#[test]
fn the_queue_takes_its_hints_through_a_poisoned_lock() {
    let deployer = StreamDeployer::new("http://runner").with_concurrency(2, 3);
    let poisoned = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let _held = deployer.queue.lock();
                panic!("poisons the queue");
            })
            .join()
    });
    assert!(poisoned.is_err() && deployer.queue.is_poisoned());
    deployer.ran(HashMap::new());
    deployer.streams(HashMap::new());
    deployer.failed(BTreeMap::new());
    deployer.unbound(HashSet::from(["helsinki/pipelines".to_owned()]));
    assert!(matches!(
        deployer.run_now("helsinki", "absent"),
        Err(run_queue::RunNow::NotQueued)
    ));
    assert!(lock(&deployer.rendered).is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_queue_ticks_until_it_is_stopped() {
    let deployer = Arc::new(StreamDeployer::new("http://runner"));
    let handle = Arc::clone(&deployer).spawn_queue();
    tokio::time::sleep(QUEUE_TICK * 3).await;
    assert!(!handle.is_finished(), "the queue loop ended by itself");
    handle.abort();
    assert!(handle.await.is_err_and(|err| err.is_cancelled()));
}

/// PF-73, PF-74: a resident pipeline beyond its project's quota is not scheduled, and says so.
#[tokio::test]
async fn a_resident_pipeline_beyond_the_quota_is_not_scheduled() {
    let server = taking_runner().await;
    let mirror = helsinki_test_mirror();
    put(
        &mirror,
        "Project",
        crate::permissions::ORG_NAMESPACE,
        "helsinki",
        serde_json::json!({ "quotas": { "residentPipelines": 0 } }),
    );
    let deployer = StreamDeployer::new(server.uri())
        .with_quota_bounds(jc_core::kinds::OrganizationBounds::default());
    let outcomes = deployer
        .converge(&mirror, &Bentos::new(), &Default::default())
        .await;
    let quota = said(&outcomes, "citybikes-free");
    assert!(
        quota.starts_with("Skipped(") && quota.contains("quota is used up"),
        "{quota}"
    );
    let sent = server.received_requests().await.expect("recorded");
    assert!(sent.iter().all(|r| r.method.as_str() != "PUT"), "{sent:?}");
}

/// Each reason a pipeline of the mirror gets no stream, one pipeline per reason, in one pass.
#[tokio::test]
async fn every_pipeline_the_pass_cannot_render_says_why() {
    let server = taking_runner().await;
    let mirror = helsinki_test_mirror();
    let ns = "helsinki";
    put(
        &mirror,
        "Endpoint",
        ns,
        "noslug",
        serde_json::json!({ "contextSpaceRef": "helsinki" }),
    );
    put(
        &mirror,
        "DataSource",
        ns,
        "broken",
        serde_json::json!({ "type": 7 }),
    );
    let endpoint_source = |query: Value| serde_json::json!({ "endpointRef": { "kind": "Endpoint", "name": "helsinki-all" }, "query": query });
    let cases = [
        ("bad-spec", serde_json::json!({ "class": 5 })),
        (
            "no-script-ref",
            pipeline(serde_json::json!({ "compute": null, "steps": [{ "kind": "script" }] })),
        ),
        (
            "no-source",
            serde_json::json!({ "class": "resident", "compute": { "kind": "bloblang", "bloblang": "root = this" }, "targetEndpoint": TARGET }),
        ),
        (
            "disabled",
            pipeline(serde_json::json!({ "enabled": false })),
        ),
        (
            "wasm",
            pipeline(serde_json::json!({ "compute": { "kind": "wasm" } })),
        ),
        (
            "empty-mapping",
            pipeline(serde_json::json!({
                "source": endpoint_source(serde_json::json!({ "type": "Bike" })),
                "compute": { "kind": "bloblang", "bloblang": " " }
            })),
        ),
        (
            "no-compute",
            pipeline(serde_json::json!({
                "source": endpoint_source(serde_json::json!({ "type": "Bike" })),
                "compute": null
            })),
        ),
        (
            "lost-target",
            pipeline(
                serde_json::json!({ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:missing" }),
            ),
        ),
        (
            "no-slug-target",
            pipeline(
                serde_json::json!({ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:noslug" }),
            ),
        ),
        (
            "no-target",
            pipeline(serde_json::json!({ "targetEndpoint": null })),
        ),
        (
            "lost-ds",
            pipeline(
                serde_json::json!({ "source": { "dataSourceRef": { "kind": "DataSource", "name": "gone" } } }),
            ),
        ),
        (
            "bad-ds",
            pipeline(
                serde_json::json!({ "source": { "dataSourceRef": { "kind": "DataSource", "name": "broken" } } }),
            ),
        ),
        (
            "bento-input",
            pipeline(serde_json::json!({ "compute": null })),
        ),
        (
            "src-noslug",
            pipeline(serde_json::json!({
                "source": { "endpointRef": { "kind": "Endpoint", "name": "noslug" }, "query": { "type": "Bike" } }
            })),
        ),
        (
            "comma-attr",
            pipeline(serde_json::json!({
                "source": endpoint_source(serde_json::json!({ "type": "Bike", "attrs": ["a,b"] }))
            })),
        ),
        (
            "merged-lost",
            serde_json::json!({
                "class": "resident",
                "sources": [
                    { "dataSourceRef": { "kind": "DataSource", "name": "hsl-citybikes-free" } },
                    { "dataSourceRef": { "kind": "DataSource", "name": "gone" } }
                ],
                "steps": [{ "kind": "bloblang", "bloblang": "root = this" }],
                "outputs": [{ "targetEndpoint": TARGET }]
            }),
        ),
    ];
    for (name, spec) in &cases {
        put(&mirror, "Pipeline", ns, name, spec.clone());
    }
    let bentos = Bentos::from([(
        (ns.to_owned(), "bento-input".to_owned()),
        "input:\n  stdin: {}\n".to_owned(),
    )]);
    let outcomes = deployer_for(&server)
        .converge(&mirror, &bentos, &Default::default())
        .await;
    let expect = [
        ("bad-spec", "Error(\"invalid pipeline spec"),
        ("no-script-ref", "without scriptRef"),
        ("no-source", "Skipped(\"not a supported stream pipeline\")"),
        ("disabled", "Skipped(\"pipeline is disabled\")"),
        ("wasm", "Skipped(\"compute is not bloblang\")"),
        (
            "empty-mapping",
            "Skipped(\"endpoint pipeline requires bloblang mapping\")",
        ),
        ("no-compute", "Skipped(\"not eligible for streams mode\")"),
        (
            "lost-target",
            "target endpoint missing is not in the mirror",
        ),
        ("no-slug-target", "target endpoint noslug has no slug"),
        ("no-target", "the pipeline names no target endpoint"),
        ("lost-ds", "data source gone is not in the mirror"),
        ("bad-ds", "invalid data source spec"),
        ("bento-input", "already declares an input"),
        ("src-noslug", "source endpoint noslug has no slug"),
        ("comma-attr", "Error("),
        ("merged-lost", "data source gone is not in the mirror"),
    ];
    for (name, words) in expect {
        let text = said(&outcomes, name);
        assert!(text.contains(words), "{name}: {text}");
    }
    // Only the one renderable pipeline reached the runner.
    let puts: Vec<String> = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .filter(|r| r.method.as_str() == "PUT")
        .map(|r| r.url.path().to_owned())
        .collect();
    assert_eq!(puts, ["/streams/helsinki.citybikes-free"]);
}

fn deployer_for(server: &MockServer) -> StreamDeployer {
    StreamDeployer::new(server.uri())
}

/// PL-53: a merged pipeline over a resident source is sent at once; a polled one with a period
/// that is no duration is not queued; an expiry whose target is gone sweeps nothing; a units rule
/// reads the space's model.
#[tokio::test]
async fn merged_scheduled_and_swept_pipelines_place_where_they_run() {
    let server = taking_runner().await;
    let mirror = helsinki_test_mirror();
    let ns = "helsinki";
    put(
        &mirror,
        "DataSource",
        ns,
        "feed",
        serde_json::json!({ "type": "http", "http": { "url": "https://example.org/feed.json" } }),
    );
    put(
        &mirror,
        "Pipeline",
        ns,
        "merged",
        serde_json::json!({
            "class": "resident",
            "sources": [
                { "dataSourceRef": { "kind": "DataSource", "name": "hsl-citybikes-free" } },
                { "dataSourceRef": { "kind": "DataSource", "name": "feed" } }
            ],
            "steps": [{ "kind": "bloblang", "bloblang": "root = this" }],
            "outputs": [{ "targetEndpoint": TARGET }]
        }),
    );
    put(
        &mirror,
        "Pipeline",
        ns,
        "bad-clock",
        pipeline(serde_json::json!({
            "class": "scheduled",
            "period": "soon",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "feed" } }
        })),
    );
    put(
        &mirror,
        "Pipeline",
        ns,
        "lost-sweep",
        pipeline(serde_json::json!({
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:missing",
            "expiry": { "after": "14d", "types": ["Vehicle"] }
        })),
    );
    put(
        &mirror,
        "Pipeline",
        ns,
        "units",
        pipeline(serde_json::json!({
            "attributes": [{ "type": "BikeHireDockingStation", "rules": [{ "names": ["name"], "unitCode": "model" }] }]
        })),
    );
    put(
        &mirror,
        "Endpoint",
        ns,
        "unmodelled",
        serde_json::json!({ "contextSpaceRef": "unmodelled", "slug": "unmodelledslug" }),
    );
    put(
        &mirror,
        "Pipeline",
        ns,
        "units-unmodelled",
        pipeline(serde_json::json!({
            "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:helsinki:unmodelled",
            "attributes": [{ "type": "BikeHireDockingStation", "rules": [{ "names": ["name"], "unitCode": "model" }] }]
        })),
    );
    // A kpi step off its schedule resolves its Endpoint source and is refused by the render.
    put(
        &mirror,
        "Pipeline",
        ns,
        "kpi-unscheduled",
        serde_json::json!({
            "class": "resident",
            "sources": [{ "name": "bikes", "endpointRef": { "kind": "Endpoint", "name": "helsinki-all" },
                          "query": { "type": "Bike", "attrs": ["n"] } }],
            "steps": [{ "kind": "kpi", "kpi": { "name": "bikes", "formula": "sum(bikes.n)",
                                                "unitCode": "C62", "join": { "by": "time" } } }],
            "outputs": [{ "targetEndpoint": TARGET }]
        }),
    );
    let schemas = Arc::new(crate::pipeline_validation::ModelSchemas::default());
    schemas.replace(HashMap::from([(
        (ns.to_owned(), "helsinki".to_owned()),
        air_model("1.0.0"),
    )]));
    let deployer = StreamDeployer::new(server.uri()).with_models(schemas);
    let outcomes = deployer
        .converge(&mirror, &Bentos::new(), &Default::default())
        .await;
    assert_eq!(said(&outcomes, "merged"), "Live");
    assert_eq!(said(&outcomes, "units"), "Live");
    assert_eq!(said(&outcomes, "units-unmodelled"), "Live");
    let kpi = said(&outcomes, "kpi-unscheduled");
    assert!(kpi.contains("runs on its schedule"), "{kpi}");
    let clock = said(&outcomes, "bad-clock");
    assert!(clock.contains("period `soon`"), "{clock}");
    let sweep = said(&outcomes, "lost-sweep.expiry");
    assert!(sweep.contains("nothing is swept"), "{sweep}");
}

/// A pass that raced another keeps its placeholder: the other pass dropped the queue's entry
/// while this one waited on the runner, so the queue has nothing to say for it.
#[tokio::test]
async fn a_pass_racing_another_keeps_its_placeholder() {
    let server = runner().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(300)))
        .mount(&server)
        .await;
    let mirror = helsinki_test_mirror();
    put(
        &mirror,
        "DataSource",
        "helsinki",
        "feed",
        serde_json::json!({ "type": "http", "http": { "url": "https://example.org/feed.json" } }),
    );
    put(
        &mirror,
        "Pipeline",
        "helsinki",
        "polled",
        pipeline(serde_json::json!({
            "class": "scheduled",
            "period": "1h",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "feed" } }
        })),
    );
    let deployer = StreamDeployer::new(server.uri());
    let first = deployer
        .converge(&mirror, &Bentos::new(), &Default::default())
        .await;
    assert!(
        said(&first, "polled").starts_with("Scheduled("),
        "{first:?}"
    );
    let (empty, bentos, refused) = (Mirror::new(), Bentos::new(), BTreeMap::new());
    let (raced, _) = tokio::join!(
        deployer.converge(&mirror, &bentos, &refused),
        deployer.converge(&empty, &bentos, &refused),
    );
    assert_eq!(said(&raced, "polled"), "Scheduled(\"Waiting\", \"\")");
}

/// PL-60: no space, spaces of different models, a space with no model and a stream with no
/// batch split each leave the stream unvalidated; a stream with no output gets no log sinks.
#[tokio::test]
async fn a_stream_the_stage_cannot_go_into_is_left_as_it_is() {
    let schemas = Arc::new(crate::pipeline_validation::ModelSchemas::default());
    schemas.replace(HashMap::from([(
        ("helsinki".to_owned(), "air".to_owned()),
        air_model("1.0.0"),
    )]));
    let deployer = StreamDeployer::new("http://runner").with_validation("http://portal/", schemas);
    let plain = serde_json::json!({ "pipeline": { "processors": [] } });
    let mut stream = plain.clone();
    deployer.validated(&mut stream, "helsinki", "p", &[]);
    deployer.validated(&mut stream, "helsinki", "p", &["unmodelled".to_owned()]);
    assert_eq!(stream, plain);
    let ((), log) = crate::reconciler::logged(async {
        deployer.validated(
            &mut stream,
            "helsinki",
            "p",
            &["air".to_owned(), "other".to_owned()],
        );
        deployer.validated(&mut stream, "helsinki", "p", &["air".to_owned()]);
    })
    .await;
    assert!(log.contains("several spaces"), "{log}");
    assert!(log.contains("no batch split"), "{log}");
    assert_eq!(stream, plain);
    deployer.logged(&mut stream, "helsinki", "p");
    assert_eq!(stream, plain, "a stream with no output got sinks");
}

/// The runner's list says how an unchanged render stands: an endless stream it holds but does
/// not run is sent again with a warning, one whose input ends by itself is left alone, and an
/// adoption check the runner answers with no JSON sends the stream.
#[tokio::test]
async fn an_unchanged_render_is_sent_again_only_when_it_died() {
    let server = taking_runner().await;
    let deployer = StreamDeployer::new(server.uri());
    let endless = serde_json::json!({ "input": { "generate": { "interval": "1s", "mapping": "root = {}" } } });
    let finite =
        serde_json::json!({ "input": { "generate": { "count": 1, "mapping": "root = {}" } } });
    Mock::given(method("GET"))
        .and(path("/streams/helsinki.endless"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/streams"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "helsinki.endless": { "active": false },
            "helsinki.finite": { "active": false }
        })))
        .mount(&server)
        .await;
    let puts = || async {
        server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .filter(|r| r.method.as_str() == "PUT")
            .count()
    };
    for (name, stream) in [("endless", &endless), ("finite", &finite)] {
        let first = deployer
            .apply("helsinki", name, stream.clone(), &Running::new())
            .await;
        assert_eq!(first, StreamOutcome::Live, "{name}");
    }
    assert_eq!(puts().await, 2);
    let (outcome, log) = crate::reconciler::logged(deployer.apply(
        "helsinki",
        "endless",
        endless.clone(),
        &Running::new(),
    ))
    .await;
    assert_eq!(outcome, StreamOutcome::Live);
    assert!(log.contains("not running; sending it again"), "{log}");
    assert_eq!(puts().await, 3);
    let finished = deployer
        .apply("helsinki", "finite", finite.clone(), &Running::new())
        .await;
    assert_eq!(finished, StreamOutcome::Live);
    assert_eq!(
        puts().await,
        3,
        "a stream that ended by itself was sent again"
    );
}

/// T-2298: a name that is not a project reaches no runner, whichever way it comes.
#[tokio::test]
async fn a_name_that_is_not_a_project_reaches_no_runner() {
    let server = taking_runner().await;
    let deployer = StreamDeployer::new(server.uri());
    let key = ("Not_A_Project".to_owned(), "feed".to_owned());
    let outcome = deployer
        .apply(
            &key.0,
            &key.1,
            serde_json::json!({ "input": {} }),
            &Running::new(),
        )
        .await;
    assert_eq!(
        outcome,
        StreamOutcome::Error("`Not_A_Project` is not a project name".to_owned())
    );
    deployer.retire(&key.0, std::slice::from_ref(&key.1)).await;
    assert_eq!(Runner::state(&deployer, &key).await, RunState::Gone);
    assert!(Runner::delete(&deployer, &key).await);
    assert!(server
        .received_requests()
        .await
        .expect("recorded")
        .is_empty());
}

/// T-2891: the wait after an unanswered PUT doubles while the same render misses, up to the
/// ceiling, and starts over for a new render; a changed render is not held by the old wait.
#[test]
fn the_wait_doubles_to_its_ceiling_and_a_new_render_starts_over() {
    let deployer = StreamDeployer::new("http://runner");
    let key = ("helsinki".to_owned(), "feed".to_owned());
    let waits: Vec<String> = (0..6)
        .map(|_| {
            format!(
                "{:?}",
                deployer.back_off(key.clone(), 1, "refused".to_owned())
            )
        })
        .collect();
    for (said, secs) in waits.iter().zip([60, 120, 240, 480, 900, 900]) {
        assert!(said.contains(&format!("again in {secs} s")), "{said}");
    }
    let held = format!("{:?}", deployer.waiting(&key, 1));
    assert!(
        held.contains("did not answer the last change") && held.contains("(refused)"),
        "{held}"
    );
    assert_eq!(deployer.waiting(&key, 2), None);
    let fresh = format!("{:?}", deployer.back_off(key, 2, "refused".to_owned()));
    assert!(fresh.contains("again in 60 s"), "{fresh}");
}

/// T-5370: a DELETE the runner refuses still forgets the stream; one it never answers in time
/// reads as unanswered, one whose connection is refused as answered.
#[tokio::test]
async fn a_delete_refused_or_unanswered_is_logged_and_forgotten() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/streams/helsinki.refused"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/streams/helsinki.slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;
    let deployer = StreamDeployer::new(server.uri())
        .with_timeouts(Duration::from_secs(5), Duration::from_millis(200));
    lock(&deployer.deployed).insert(("helsinki".to_owned(), "refused".to_owned()));
    let (answered, log) =
        crate::reconciler::logged(deployer.remove(&server.uri(), "helsinki", "refused", "a test"))
            .await;
    assert!(answered);
    assert!(
        log.contains("failed to delete stream from runner") && log.contains("500"),
        "{log}"
    );
    assert!(lock(&deployer.deployed).is_empty());
    let (answered, log) =
        crate::reconciler::logged(deployer.remove(&server.uri(), "helsinki", "slow", "a test"))
            .await;
    assert!(!answered, "a DELETE past its timeout read as answered");
    assert!(log.contains("failed to delete stream from runner"), "{log}");
    let closed = closed_port();
    let address = format!("http://{}", closed.local_addr().expect("an address"));
    assert!(
        deployer
            .remove(&address, "helsinki", "gone", "a test")
            .await
    );
}

/// A bound socket that never listens: a connection to it is refused, and no other test takes
/// its port while it lives.
fn closed_port() -> tokio::net::TcpSocket {
    let socket = tokio::net::TcpSocket::new_v4().expect("a socket");
    socket
        .bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("a port");
    socket
}

/// T-3002: a stream under its bare name that is gone already (404) counts as removed; one the
/// runner keeps, by its answer or by its silence, is asked about again on the next pass.
#[tokio::test]
async fn a_bare_name_stream_is_retired_once_and_asked_again_when_it_stays() {
    let held = serde_json::json!({ "feed": { "active": true } });
    let pass = |status: u16, delay: u64| {
        let held = held.clone();
        async move {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/streams"))
                .respond_with(ResponseTemplate::new(200).set_body_json(held))
                .mount(&server)
                .await;
            Mock::given(method("DELETE"))
                .and(path("/streams/feed"))
                .respond_with(ResponseTemplate::new(status).set_delay(Duration::from_millis(delay)))
                .mount(&server)
                .await;
            let deployer = StreamDeployer::new(server.uri())
                .with_timeouts(Duration::from_secs(5), Duration::from_millis(200));
            let (_, log) = crate::reconciler::logged(async {
                deployer
                    .retire_unqualified("helsinki", ["feed"].into_iter())
                    .await;
                deployer
                    .retire_unqualified("helsinki", ["feed"].into_iter())
                    .await;
            })
            .await;
            let lists = server
                .received_requests()
                .await
                .expect("recorded")
                .iter()
                .filter(|r| r.method.as_str() == "GET")
                .count();
            (lists, log)
        }
    };
    let (lists, log) = pass(404, 0).await;
    assert_eq!(lists, 1, "{log}");
    assert!(
        log.contains("removed the stream kept under its bare pipeline name"),
        "{log}"
    );
    let (lists, log) = pass(500, 0).await;
    assert_eq!(lists, 2, "{log}");
    assert!(
        log.contains("the runner kept the stream") && log.contains("500"),
        "{log}"
    );
    let (lists, log) = pass(200, 2000).await;
    assert_eq!(lists, 2, "{log}");
    assert!(log.contains("the runner kept the stream"), "{log}");
}

/// A stream the runner does not hold is POSTed; a POST it never answers is a transport failure.
#[tokio::test]
async fn a_post_the_runner_never_answers_is_a_transport_failure() {
    let server = runner().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/streams/helsinki.feed"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;
    let deployer = StreamDeployer::new(server.uri())
        .with_timeouts(Duration::from_secs(5), Duration::from_millis(200));
    let sent = deployer
        .deploy_stream("helsinki", "feed", &serde_json::json!({ "input": {} }))
        .await;
    assert!(sent.is_err(), "{sent:?}");
}

/// PL-78, F-POR3-006: a run whose start count the runner's metrics do not give keeps no base,
/// and a run the runner refuses fails with the runner's words.
#[tokio::test]
async fn a_refused_run_fails_with_the_runners_words_and_keeps_no_base() {
    let server = runner().await;
    Mock::given(method("GET"))
        .and(path("/metrics"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(serde_json::json!({ "lint_errors": ["bad"] })),
        )
        .mount(&server)
        .await;
    let (ends, _ended) = tokio::sync::mpsc::unbounded_channel();
    let deployer = StreamDeployer::new(server.uri()).with_run_ends(ends);
    let key = ("helsinki".to_owned(), "feed".to_owned());
    lock(&deployer.report_drops).insert(key.clone(), 7);
    let started = Runner::start(&deployer, &key, &serde_json::json!({ "input": {} })).await;
    assert_eq!(
        started,
        Err("the runner found errors in the pipeline: bad".to_owned())
    );
    assert!(lock(&deployer.report_drops).is_empty());
}

/// A run's state when the runner answers neither 404 nor success: still running, until its cap.
#[tokio::test]
async fn a_run_the_runner_answers_with_an_error_is_still_running() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let deployer = StreamDeployer::new(server.uri());
    let key = ("helsinki".to_owned(), "feed".to_owned());
    assert_eq!(Runner::state(&deployer, &key).await, RunState::Running);
}

/// Without a listener for run ends, an ended run touches no start count.
#[tokio::test]
async fn an_ended_run_with_nobody_listening_keeps_the_counts() {
    let deployer = StreamDeployer::new("http://runner");
    let key = ("helsinki".to_owned(), "feed".to_owned());
    lock(&deployer.report_drops).insert(key.clone(), 3);
    Runner::ended(&deployer, &key, "run-1", true, "abc").await;
    assert_eq!(lock(&deployer.report_drops).get(&key), Some(&3));
}

/// A poisoned map is read as it stands: a panic in one pass does not stop the next.
#[test]
fn a_poisoned_lock_is_read_as_it_stands() {
    let map = Mutex::new(HashSet::from(["kept".to_owned()]));
    let _ = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let _held = map.lock();
                panic!("poisons the map");
            })
            .join()
    });
    assert!(map.is_poisoned());
    assert!(lock(&map).contains("kept"));
}

/// PL-73: a processor step is a step a stream runs.
#[test]
fn a_processor_step_is_eligible_for_a_stream() {
    let spec = second_shape(serde_json::json!({
        "class": "resident",
        "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "hsl-citybikes-free" } }],
        "steps": [{ "processor": { "unarchive": { "format": "json_array" } } }],
        "outputs": [{ "targetEndpoint": TARGET }]
    }));
    assert!(eligible(&spec));
}

/// A long refusal is cut at a character boundary, never inside one.
#[test]
fn a_long_refusal_is_cut_at_a_character_boundary() {
    assert_eq!(truncate_body("ééé", 3), "é...");
    assert_eq!(truncate_body("short", 500), "short");
    let said = runner_refusal(reqwest::StatusCode::BAD_REQUEST, &"x".repeat(600));
    assert!(said.ends_with("...") && said.len() < 540, "{said}");
    let lints = serde_json::json!({ "lint_errors": ["y".repeat(600)] }).to_string();
    let said = runner_refusal(reqwest::StatusCode::BAD_REQUEST, &lints);
    assert_eq!(said.len(), 503, "{said}");
}
