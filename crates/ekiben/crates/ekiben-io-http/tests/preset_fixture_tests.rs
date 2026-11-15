//! T-7067: the `http_client` paginate option jc-manifest renders for each `spec.http.preset`
//! (tests/fixtures/presets/<kind>/render.json, checked against the render in jc-manifest's
//! bento_preset_fixture_tests.rs) reads every record of pages recorded from a live API.
//!
//! The fake server answers only the requests the walk must make, in order: the next page's
//! URL is the recorded URL plus the offset parameters, or the recorded page's next link. Any
//! other request answers 404 and fails the run, so a preset that asks for the wrong page fails.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{OriginalUri, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Router;
use ekiben_bloblang::Environment;
use ekiben_core::{ComponentConfig, CoreError, OrgId, OrgQuotas, Registry, ResourceManager};
use ekiben_core::{StreamContext, StreamMetrics};
use ekiben_io_http::paginate::lookup;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::watch;
use url::Url;

/// A request's path and its query as decoded pairs, sorted.
type RequestKey = (String, Vec<(String, String)>);

/// The query of `url` as decoded pairs, sorted, and its path.
fn request_key(url: &Url) -> RequestKey {
    let mut pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    pairs.sort();
    (url.path().to_string(), pairs)
}

struct Recorded {
    origin: String,
    pages: Vec<(RequestKey, String)>,
}

/// The requests the walk must make and the page each one answers.
fn recorded(dir: &Path, render: &Value) -> Recorded {
    let paginate = &render["paginate"];
    let records = paginate["records"].as_str().expect("records");
    let mut url = Url::parse(render["url"].as_str().expect("url")).expect("url");
    let origin = url.origin().ascii_serialization();
    let mut pages = Vec::new();
    let mut seen = 0usize;
    for n in 1.. {
        let Ok(text) = std::fs::read_to_string(dir.join(format!("page-{n}.json"))) else {
            break;
        };
        let body: Value = serde_json::from_str(&text).expect("page is JSON");
        let mut asked = url.clone();
        if paginate["mode"] == "offset" {
            let mut set: Vec<(String, String)> = vec![
                (
                    paginate["offset_param"].as_str().unwrap().into(),
                    seen.to_string(),
                ),
                (
                    paginate["limit_param"].as_str().unwrap().into(),
                    paginate["limit"].to_string(),
                ),
            ];
            if let Some(order) = paginate.get("order_by") {
                set.push((
                    order["param"].as_str().unwrap().into(),
                    order["value"].as_str().unwrap().into(),
                ));
            }
            let kept: Vec<(String, String)> = url
                .query_pairs()
                .into_owned()
                .filter(|(k, _)| !set.iter().any(|(s, _)| s == k))
                .collect();
            asked
                .query_pairs_mut()
                .clear()
                .extend_pairs(kept.iter().chain(set.iter()));
        }
        seen += lookup(&body, records)
            .and_then(Value::as_array)
            .expect("records")
            .len();
        pages.push((request_key(&asked), text));
        if paginate["mode"] == "next_url" {
            if let Some(next) =
                lookup(&body, paginate["next"].as_str().unwrap()).and_then(Value::as_str)
            {
                url = url.join(next).expect("next link");
            }
        }
    }
    Recorded { origin, pages }
}

async fn answer(
    State(rec): State<Arc<(Recorded, String)>>,
    OriginalUri(uri): OriginalUri,
) -> impl IntoResponse {
    let (rec, base) = &*rec;
    let url = Url::parse(&format!("{base}{uri}")).expect("request url");
    match rec.pages.iter().find(|(key, _)| *key == request_key(&url)) {
        // Absolute next links point at the recorded origin: serve them at this server.
        Some((_, body)) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            body.replace(&rec.origin, base),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, format!("no recorded page for {uri}")).into_response(),
    }
}

fn stream_context() -> (Arc<Registry>, StreamContext) {
    let reg = Arc::new(Registry::new());
    ekiben_io_http::register_all(&reg);
    let resources = ResourceManager::new();
    let org_id = OrgId::new("org-presets");
    resources.set_org_quotas(
        &org_id,
        OrgQuotas {
            allow_private_ips: true,
            ..Default::default()
        },
    );
    let org = resources.get_or_create_org(&org_id);
    let (_tx, rx) = watch::channel(false);
    let sctx = StreamContext {
        stream_id: "presets".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("presets", "org-presets"),
        label: "http_io".to_string(),
        path: "input".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };
    (reg, sctx)
}

#[tokio::test]
async fn each_preset_reads_every_record_of_its_recorded_pages() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/presets");
    let expected: BTreeMap<&str, usize> = [
        ("arcgis", 142),
        ("wfs", 453),
        ("ckan", 2),
        ("odata", 91),
        ("sensorthings", 368),
        ("ogc", 25),
    ]
    .into();
    for (name, total) in expected {
        let dir = fixtures.join(name);
        let render: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("render.json")).unwrap())
                .unwrap();
        let rec = recorded(&dir, &render);
        let page_count = rec.pages.len();
        assert!(
            page_count > 1,
            "{name}: a fixture of one page proves no paging"
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let origin = rec.origin.clone();
        let app = Router::new()
            .fallback(answer)
            .with_state(Arc::new((rec, base.clone())));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let url = render["url"].as_str().unwrap().replace(&origin, &base);
        let (reg, sctx) = stream_context();
        let conf = ComponentConfig::from_json_value(
            &serde_json::json!({ "http_client": {
                "url": url, "verb": "GET", "paginate": render["paginate"],
            }}),
            "noop",
            None,
        )
        .unwrap();
        let mut input = reg.build_input(&conf, &sctx).unwrap();
        input.connect(&sctx).await.unwrap();
        let records = render["paginate"]["records"].as_str().unwrap();
        let (mut read, mut pages) = (0usize, 0usize);
        let end = loop {
            match input.read_batch(&sctx).await {
                Ok((batch, ack)) => {
                    pages += 1;
                    let body: Value = serde_json::from_slice(&batch[0].as_bytes_ref()).unwrap();
                    read += lookup(&body, records)
                        .and_then(Value::as_array)
                        .unwrap()
                        .len();
                    ack.ack();
                }
                Err(e) => break e,
            }
        };
        assert!(matches!(end, CoreError::EndOfInput), "{name}: {end}");
        assert_eq!(
            (pages, read),
            (page_count, total),
            "{name}: pages and records read"
        );
    }
}
