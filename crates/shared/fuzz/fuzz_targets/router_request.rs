//! Any method, Host, path, query and Accept through a router with a shared host, a bound host, an
//! Organization with an id, live id addresses and a host-less module: every request ends in an
//! answer, and a redirect never names another site (`//x` or `/\x`) or a host outside the table
//! (T-7805).
#![no_main]

use axum::body::Body;
use axum::http::{Method, Request, Uri};
use axum::Router;
use jc_router::{HostTable, JcRouter, JcRouterBuilder, MountSpec, RegistryProject};
use libfuzzer_sys::fuzz_target;
use std::sync::LazyLock;

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
const BRANCH: &str = "daaaaaaaaaaaaaaaaaaaaaaaaa";

static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime")
});

static JC: LazyLock<JcRouter> = LazyLock::new(|| {
    let table = HostTable::new()
        .shared("city.example")
        .and_then(|t| t.shared("*.apps.city.example"))
        .and_then(|t| t.organization("bratislava"))
        .and_then(|t| t.organization_id("zilina", ID))
        .and_then(|t| t.bound("opendata.example.com", "bratislava", "/data"))
        .expect("rows");
    let module = |name: &str, prefix: Option<&str>| {
        (
            MountSpec {
                module: name.into(),
                host: None,
                path_prefix: prefix.map(str::to_owned),
            },
            Router::new().fallback(|uri: Uri| async move { uri.to_string() }),
        )
    };
    let mut b = JcRouterBuilder::new("https", "city.example").hosts(table);
    for (spec, router) in [module("site", None), module("data", Some("/data")), module("portal", Some("/api"))] {
        b = b.mount(spec, router);
    }
    let jc = b.build().expect("router");
    jc.host_swap().expect("a host table").replace_projects(&[
        RegistryProject { organization: "bratislava", id: ID, name: "traffic", expires_at: None },
        RegistryProject {
            organization: "bratislava",
            id: BRANCH,
            name: "b-daaaaaaaaaaaaaaaaaaaaaaaaa",
            expires_at: Some(i64::MAX),
        },
    ]);
    jc
});

fuzz_target!(|data: &[u8]| {
    // Lines: method, Host, path and query, Accept.
    let text = String::from_utf8_lossy(data);
    let mut lines = text.splitn(4, '\n');
    let (Some(method), Some(host), Some(path)) = (lines.next(), lines.next(), lines.next()) else {
        return;
    };
    let accept = lines.next().unwrap_or("");
    let Ok(method) = Method::from_bytes(method.as_bytes()) else {
        return;
    };
    let Ok(uri) = Uri::try_from(path) else {
        return;
    };
    let Ok(req) = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("accept", accept)
        .body(Body::empty())
    else {
        return;
    };
    let resp = RT.block_on(JC.dispatch(req, None));
    if let Some(location) = resp.headers().get("location").and_then(|l| l.to_str().ok()) {
        assert!(!location.starts_with("//") && !location.starts_with("/\\"), "{location}");
        if let Some(rest) = location.strip_prefix("https://") {
            let to = rest.split('/').next().unwrap_or_default();
            assert!(
                to == "city.example" || to == "opendata.example.com" || to.ends_with(".apps.city.example"),
                "{location}"
            );
        } else {
            assert!(location.starts_with('/'), "{location}");
        }
    }
});
