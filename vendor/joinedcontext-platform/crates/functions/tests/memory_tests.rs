//! The memory a replica needs at its limits (T-7774): a binary of its own, so the process's
//! high-water mark is these tests' alone.
//!
//! The pod limit in deploy/components/jc/values/functions comes from
//! `sixteen_full_scripts_fit_the_pod_and_the_17th_is_429`: 16 scripts, each with a 4 MiB body and a
//! heap filled to about 60 MiB, at once.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use functions::sandbox::{run, Invocation, MEMORY_LIMIT};
use functions::{router, AppState, SCRIPT_BODY_LIMIT, SLOTS};
use jc_core::auth::token::Verifier;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tower::ServiceExt;

const ISSUER: &str = "https://portal.example/realms/joinedcontext";
/// The pod's memory limit (values/functions `resources.limits.memory`), in MiB.
const POD_LIMIT_MIB: u64 = 1536;

/// A script over `data`, as the assistant sends one (crates/assistant chat/script.rs `file`).
fn script(code: &str, data: Value) -> Value {
    json!({
        "files": { "script.js": format!("export default async (request) => {{ const data = request.body; const result = await (async () => {{ {code} }})(); return {{ body: result }}; }};") },
        "entry": "script.js",
        "request": { "method": "POST", "query": {}, "body": data, "user": null },
        "config": {},
        "via": "none",
    })
}

/// Bridges in the BBSK register's keyValues shape, `count` of them.
fn bridges(count: usize) -> Value {
    let classes = ["I", "II", "III", "IV", "V", "VI", "VII"];
    let districts = [
        "Banská Bystrica",
        "Brezno",
        "Detva",
        "Lučenec",
        "Revúca",
        "Zvolen",
    ];
    Value::Array(
        (0..count)
            .map(|i| {
                json!({
                    "id": format!("urn:ngsi-ld:Bridge:bbsk:{i:06}"),
                    "type": "Bridge",
                    "name": format!("Most cez potok {i} pri obci Šťavnička"),
                    "conditionClass": classes[i % classes.len()],
                    "length": 4.5 + (i % 97) as f64 * 1.25,
                    "district": districts[i % districts.len()],
                    "road": format!("III/{:05}", 50000 + i % 900),
                    "location": { "type": "Point", "coordinates": [19.1 + (i % 100) as f64 / 1000.0, 48.7 + (i % 77) as f64 / 1000.0] },
                })
            })
            .collect(),
    )
}

/// Group by condition class: the count and the average length of each.
const GROUP: &str = "const out = {}; for (const b of data) { const g = out[b.conditionClass] ??= { n: 0, sum: 0 }; g.n++; g.sum += b.length; } return Object.fromEntries(Object.entries(out).map(([k, g]) => [k, { n: g.n, avg: g.sum / g.n }]));";

fn invocation(code: &str, data: Value) -> Invocation {
    let call = script(code, data);
    Invocation {
        files: serde_json::from_value::<BTreeMap<String, String>>(call["files"].clone()).unwrap(),
        entry: "script.js".to_owned(),
        request: call["request"].clone(),
        config: json!({}),
        endpoint: None,
    }
}

/// T-7774: as many register entities as fit the 4 MiB body parse and group inside the 64 MiB
/// heap, which is why the body limit of a script is 4 MiB.
#[tokio::test]
async fn a_4_mib_body_parses_and_groups_inside_the_heap() {
    let mut count = 10_000;
    while serde_json::to_vec(&bridges(count)).unwrap().len() < SCRIPT_BODY_LIMIT {
        count += 1_000;
    }
    count -= 1_000;
    let data = bridges(count);
    assert!(serde_json::to_vec(&data).unwrap().len() <= SCRIPT_BODY_LIMIT);
    let outcome = run(invocation(GROUP, data)).await;
    assert_eq!(
        (outcome.status, outcome.error.clone()),
        (200, None),
        "{count} bridges"
    );
    let body = outcome.body.unwrap();
    let total: u64 = body
        .as_object()
        .unwrap()
        .values()
        .map(|g| g["n"].as_u64().unwrap())
        .sum();
    assert_eq!(total as usize, count);
    // A deep copy of all of it does not fit beside them: the memory problem, by name.
    let outcome = run(invocation(
        "const copy = JSON.parse(JSON.stringify(data)); return copy.length;",
        bridges(count),
    ))
    .await;
    assert_eq!(outcome.status, 500);
    assert!(
        outcome.error.unwrap().message.contains("out of memory"),
        "the memory problem"
    );
}

/// T-7774: a script allocating past the 64 MiB heap stops with the memory problem.
#[tokio::test]
async fn a_script_past_its_heap_stops_out_of_memory() {
    let past = MEMORY_LIMIT / (1024 * 1024) + 1;
    let outcome = run(invocation(
        &format!("const a = []; for (let i = 0; i < {past}; i++) a.push('x'.repeat(1024 * 1024) + i); return a.length;"),
        json!(null),
    ))
    .await;
    assert_eq!(outcome.status, 500);
    assert!(
        outcome.error.unwrap().message.contains("out of memory"),
        "the memory problem"
    );
}

struct Realm {
    signing: EncodingKey,
    verifier: Arc<Verifier>,
}

impl Realm {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let point = pair.public_key().as_ref();
        let jwks = serde_json::from_value(json!({ "keys": [{
            "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": "k1",
            "x": URL_SAFE_NO_PAD.encode(&point[1..33]), "y": URL_SAFE_NO_PAD.encode(&point[33..]),
        }]}))
        .unwrap();
        let verifier = Verifier::new(ISSUER);
        assert_eq!(verifier.replace_keys(&jwks), 1);
        Self {
            signing: EncodingKey::from_ec_der(pkcs8.as_ref()),
            verifier: Arc::new(verifier),
        }
    }

    fn assistant_token(&self) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("k1".to_owned());
        let claims = json!({ "iss": ISSUER, "sub": "service-account-jc-assistant", "aud": "jc-functions", "azp": "jc-assistant", "exp": now + 300, "iat": now - 10 });
        encode(&header, &claims, &self.signing).unwrap()
    }
}

async fn send(app: axum::Router, token: &str, body: Vec<u8>) -> (StatusCode, Value) {
    let request = Request::post("/invoke")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The process's resident high-water mark, in MiB (`VmHWM`).
fn peak_mib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmHWM:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        })
        .map_or(0, |kib| kib / 1024)
}

/// T-7774 item I: 16 scripts at once, each handed a 4 MiB body and holding about 60 MiB of heap,
/// all run; the 17th is 429; and the replica's peak resident memory stays under the pod limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sixteen_full_scripts_fit_the_pod_and_the_17th_is_429() {
    let realm = Realm::new();
    let app = router(Arc::new(AppState {
        verifier: Arc::clone(&realm.verifier),
        audience: "jc-functions".to_owned(),
        caller: "joinedcontext-portal".to_owned(),
        assistant_caller: Some("jc-assistant".to_owned()),
        gateway: "http://127.0.0.1:9".to_owned(),
        proxy: None,
        http: reqwest::Client::new(),
        slots: Arc::new(Semaphore::new(SLOTS)),
    }));
    let token = realm.assistant_token();
    // The body string and its parsed copy are 8 MiB of the heap; 50 strings of 1 MiB fill it to
    // about 60 MiB, then the script waits until its 10 s are up.
    let full = script(
        "const a = []; for (let i = 0; i < 50; i++) a.push('y'.repeat(1024 * 1024) + i); globalThis.keep = a; await new Promise(() => {});",
        json!("x".repeat(SCRIPT_BODY_LIMIT - 2)),
    )
    .to_string()
    .into_bytes();
    let before = peak_mib();
    let running: Vec<_> = (0..SLOTS)
        .map(|_| {
            let (app, token, body) = (app.clone(), token.clone(), full.clone());
            tokio::spawn(async move { send(app, &token, body).await })
        })
        .collect();
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let (status, answer) = send(app.clone(), &token, full.clone()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{answer}");
    for call in running {
        let (status, answer) = call.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        // Each filled its heap and was then stopped by the clock, not by memory.
        assert!(
            answer["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("longer than 10 s"),
            "{answer}"
        );
    }
    let peak = peak_mib();
    println!("peak resident {peak} MiB (before the scripts {before} MiB)");
    assert!(
        peak < POD_LIMIT_MIB,
        "peak {peak} MiB over the pod limit {POD_LIMIT_MIB} MiB"
    );
}
