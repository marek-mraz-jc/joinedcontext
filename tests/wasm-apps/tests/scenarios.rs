//! Every server WASM App that ships a scenario, `apps/<name>/server/host-test.json` (T-3351..T-3355),
//! played on the real host: the App placed twice by [`Harness`], the gateway a mock that answers
//! below the App's own Endpoint alone, so a call anywhere else is never answered.
//!
//! ```json
//! {
//!   "gateway": [{"path": "/ngsi-ld/v1/entities", "query": {"type": "Alert"}, "file": "host-test/alerts.json"}],
//!   "steps": [
//!     {"call": "POST /api/reports", "body": {"title": "t"}, "status": 201, "save": {"report": "/id"}},
//!     {"call": "GET /api/reports/{report}", "status": 200, "expect": {"/title": "t"}, "length": {"/places": 0}},
//!     {"call": "POST /api/reports/{report}/snapshot", "status": 200, "upload": "/url"},
//!     {"call": "GET /api/reports/{report}/snapshot", "status": 200, "download": "/url"},
//!     {"call": "GET /api/reports", "as": "second", "status": 200, "length": {"": 0}}
//!   ]
//! }
//! ```
//!
//! `gateway[].path` is below the Endpoint (`/api/endpoint/<slug>`), a `file` relative to
//! `server/`; it answers a caller's token only, or with `anonymous: true` also a call with none,
//! as an Endpoint's public role does; `method` (default `GET`) mocks a write such as `PATCH`, its
//! `file` optional, and `times` checks how often it was called by the end of the scenario, so a
//! refused write is shown never to have reached the context data. A step's `{name}`, in its call and in a text it expects, is what an earlier step's
//! `save` took; `upload` puts bytes to the
//! URL at that pointer and `download` reads them back, or with `downloaded` checks that what the
//! App wrote holds that text; `as: "second"` calls as the second App, which every scenario must
//! do at least once. `caller` names who calls ([`wasm_apps_harness::token`]): `anonymous`, a role
//! such as `viewer`, roles such as `steward+viewer`, or `viewer@u-2` for another person with that
//! role; without it the step runs as one person holding every role of the App's `app.yaml`.
//! `rows` counts a table of the called App after the step (`{"plans": 1}`), so a refused write is
//! shown to have changed nothing. A step `{"job": "inspect-daily"}` runs that job export of the
//! App (`as` picks which) under a job principal's token, as the host's scheduler does (AP-154), and
//! the run must succeed within the scheduler's wall time.
//! Needs the variables of src/lib.rs.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use wasm_apps_harness::{fetch, Harness};
use wiremock::matchers::{header_regex, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BYTES: &str = "jc-host-test";

fn apps() -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("apps/")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("server/host-test.json").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

fn text(value: &Value, at: &str) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("{at}: not a string: {value}"))
        .to_owned()
}

async fn play(app: &str) {
    let server = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps")
        .join(app)
        .join("server");
    let scenario: Value = serde_json::from_str(
        &std::fs::read_to_string(server.join("host-test.json")).expect("host-test.json"),
    )
    .unwrap_or_else(|err| panic!("{app}/server/host-test.json: {err}"));
    let slug = wasm_apps_harness::slug(app);

    let gateway = MockServer::start().await;
    for answer in scenario["gateway"].as_array().into_iter().flatten() {
        let body = match answer["file"].as_str() {
            Some(file) => std::fs::read(server.join(file))
                .unwrap_or_else(|err| panic!("{app}: {file}: {err}")),
            None => Vec::new(),
        };
        let verb = answer["method"].as_str().unwrap_or("GET");
        let mut mock = Mock::given(method(verb)).and(path(format!(
            "/api/endpoint/{slug}{}",
            text(&answer["path"], "gateway[].path")
        )));
        if answer["anonymous"] != Value::Bool(true) {
            mock = mock.and(header_regex("authorization", "^Bearer tok-"));
        }
        for (key, value) in answer["query"].as_object().into_iter().flatten() {
            mock = mock.and(query_param(key.as_str(), text(value, key).as_str()));
        }
        let status = answer["status"].as_u64().unwrap_or(200) as u16;
        let mut mounted =
            mock.respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"));
        if let Some(times) = answer["times"].as_u64() {
            mounted = mounted.expect(times);
        }
        mounted.mount(&gateway).await;
    }

    let harness = Harness::new(app, gateway).await;
    let everyone = wasm_apps_harness::token(&format!(
        "{}@u-scenario",
        wasm_apps_harness::roles(app).join("+")
    ));
    let http = reqwest::Client::new();
    let mut saved: BTreeMap<String, String> = BTreeMap::new();
    let mut second_acted = false;
    for (n, step) in scenario["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .enumerate()
    {
        if let Some(export) = step["job"].as_str() {
            let app = match step["as"].as_str() {
                Some("second") => &harness.second,
                _ => &harness.first,
            };
            let token = wasm_apps_harness::token("job").expect("a job token");
            let ran = harness
                .host
                .run_job(app, export, token, wasm_host::jobs::WALL_TIME)
                .await;
            assert_eq!(ran, Ok(()), "{app} step {n}: the job {export}", app = app.name);
            continue;
        }
        let call = text(&step["call"], "call");
        let (verb, target) = call
            .split_once(' ')
            .unwrap_or_else(|| panic!("{app} step {n}: call is `VERB /path`"));
        let mut target = target.to_owned();
        for (key, value) in &saved {
            target = target.replace(&format!("{{{key}}}"), value);
        }
        let caller = match step["as"].as_str() {
            None => &harness.first,
            Some("second") => {
                second_acted = true;
                &harness.second
            }
            Some(other) => panic!("{app} step {n}: unknown caller {other}"),
        };
        let body = step.get("body").map(Value::to_string).unwrap_or_default();
        let token = match step["caller"].as_str() {
            None => everyone.clone(),
            Some(named) => wasm_apps_harness::token(named),
        };
        let (status, answer) = harness
            .call(caller, verb, &target, &body, token.as_deref())
            .await;
        let at = format!("{app} step {n} ({verb} {target})");
        assert_eq!(
            Some(u64::from(status)),
            step["status"].as_u64(),
            "{at}: {answer}"
        );
        for (pointer, want) in step["expect"].as_object().into_iter().flatten() {
            let want = match want {
                Value::String(text) => Value::String(
                    saved
                        .iter()
                        .fold(text.clone(), |t, (k, v)| t.replace(&format!("{{{k}}}"), v)),
                ),
                other => other.clone(),
            };
            assert_eq!(
                answer.pointer(pointer),
                Some(&want),
                "{at} {pointer}: {answer}"
            );
        }
        for (pointer, want) in step["length"].as_object().into_iter().flatten() {
            let got = answer
                .pointer(pointer)
                .and_then(Value::as_array)
                .map(Vec::len);
            assert_eq!(
                got.map(|n| n as u64),
                want.as_u64(),
                "{at} length of {pointer}: {answer}"
            );
        }
        for (table, want) in step["rows"].as_object().into_iter().flatten() {
            assert_eq!(
                Some(harness.count(caller, table).await as u64),
                want.as_u64(),
                "{at}: rows of {table}"
            );
        }
        for (key, pointer) in step["save"].as_object().into_iter().flatten() {
            let value = match answer.pointer(&text(pointer, key)) {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Number(number)) => number.to_string(),
                other => panic!("{at}: nothing to save at {pointer}: {other:?}"),
            };
            saved.insert(key.clone(), value);
        }
        if let Some(pointer) = step["upload"].as_str() {
            let url = answer
                .pointer(pointer)
                .and_then(Value::as_str)
                .expect("an upload URL");
            let put = http.put(url).body(BYTES).send().await.expect("upload");
            assert_eq!(put.status(), 200, "{at}: the upload");
        }
        if let Some(pointer) = step["download"].as_str() {
            let url = answer
                .pointer(pointer)
                .and_then(Value::as_str)
                .expect("a download URL");
            let (status, got) = fetch(url).await;
            assert_eq!(status, 200, "{at}: the download");
            match step["downloaded"].as_str() {
                Some(part) => assert!(got.contains(part), "{at}: the download holds {got}"),
                None => assert_eq!(got, BYTES, "{at}: the download"),
            }
        }
    }
    assert!(
        second_acted,
        "{app}: no step runs `as: \"second\"`, so nothing shows the App's data is its own"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn every_scenario_plays_on_the_host() {
    let apps = apps();
    assert!(!apps.is_empty(), "no apps/*/server/host-test.json");
    for app in apps {
        play(&app).await;
    }
}
