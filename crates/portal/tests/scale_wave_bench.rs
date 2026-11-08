//! The reconciler wave over T-4443's generated organization (T-4458, CC-54): not a gate, a
//! measurement. `JC_SCALE_TREE` names a layout 1 render of `jcctl::bench_org` (the platform's
//! `bench_org::write`, assembled); a forge fake serves it file by file from disk, the way shako
//! does, and `sync_once` runs three times (the first one reads every blob, the next two only
//! the tree). The daemon logs its stage and wave times at info:
//!
//! ```text
//! JC_SCALE_TREE=/path/to/render RUST_LOG=joinedcontext_portal=info \
//!   cargo test --release --test scale_wave_bench -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::reconciler::Syncer;
use joinedcontext_portal::store::Mirror;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const REVISION: &str = "c0ffee1234567890";
const FILE: &str = "/odata/Repositories(Owner='bench',Name='org')/Shako.File(Path=@p,Ref=@r)";

/// One responder for every file: thirty thousand mocks would make each request a linear match.
struct FromDisk(PathBuf);

impl Respond for FromDisk {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let Some(rel) = request
            .url
            .query_pairs()
            .find(|(k, _)| k == "@p")
            .map(|(_, v)| v.trim_matches('\'').replace("''", "'"))
        else {
            return ResponseTemplate::new(404);
        };
        match std::fs::read(self.0.join(&rel)) {
            Ok(bytes) => ResponseTemplate::new(200).set_body_json(json!({
                "Sha": format!("blob-{rel}"),
                "Content": STANDARD.encode(bytes),
            })),
            Err(_) => ResponseTemplate::new(404),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: needs JC_SCALE_TREE"]
async fn a_wave_over_the_generated_organization() {
    let root = PathBuf::from(std::env::var("JC_SCALE_TREE").expect("JC_SCALE_TREE"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let mut tree = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("the tree").flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                stack.push(entry.path());
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(&root).expect("inside").display();
            tree.push(
                json!({ "Path": rel.to_string(), "Type": "blob", "Sha": format!("blob-{rel}") }),
            );
        }
    }
    let files = tree.len();

    let forge = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/odata/Repositories(Owner='bench',Name='org')"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='bench',Name='org')/Branches('main')",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Name": "main", "HeadSha": REVISION })),
        )
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/odata/Repositories(Owner='bench',Name='org')/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        ))).and(query_param("@r", format!("'{REVISION}'")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Sha": "tree", "Truncated": false, "Tree": tree })),
        )
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(FILE))
        .respond_with(FromDisk(root.clone()))
        .mount(&forge)
        .await;

    let shako = ShakoClient::new(forge.uri().parse().expect("url"), "bench", "org", "token")
        .expect("client");
    let mirror = Arc::new(Mirror::new());
    let syncer = Syncer::new(Arc::new(shako), Arc::clone(&mirror));
    for wave in 1..=3 {
        let started = std::time::Instant::now();
        let loaded = syncer.sync_once().await.expect("the wave runs");
        eprintln!(
            "wave={wave} files={files} manifests={loaded} wave_ms={:.1}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
}
