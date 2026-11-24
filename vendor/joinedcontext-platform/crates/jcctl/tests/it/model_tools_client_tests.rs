//! T-7793: the Model Tools client against a stub that answers each path with a raw HTTP answer,
//! so every way the service can answer badly is pinned to the error a person reads.

use crate::common;
use jcctl::model::http::{request, HttpError};
use jcctl::model::{import, Error, ModelTools};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// A Model Tools whose answer to a path is `answer(path)`, a whole raw HTTP response; returns
/// its URL and the request bodies it received.
fn stub(answer: fn(&str) -> String) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("an address").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut raw = Vec::new();
            let mut chunk = [0_u8; 4096];
            while let Ok(read) = stream.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&raw);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&raw).into_owned();
            let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
            let body = text
                .split_once("\r\n\r\n")
                .map(|(_, b)| b.to_owned())
                .unwrap_or_default();
            log.lock().expect("log").push(format!("{path} {body}"));
            let _ = stream.write_all(answer(&path).as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

fn ok(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
}

fn protocol(err: Error) -> String {
    match err {
        Error::Http(HttpError::Protocol(message)) => message,
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

#[test]
fn healthz_must_be_an_object_naming_the_generator() {
    let (url, _) = stub(|path| match path {
        "/a/healthz" => ok(200, r#"{"generatorVersion":"linkml-1.11.1"}"#),
        "/b/healthz" => ok(200, r#"{"status":"up"}"#),
        "/c/healthz" => ok(200, r#"["up"]"#),
        _ => ok(200, r#"{"generatorVersion":7}"#),
    });
    let at = |prefix: &str| ModelTools::new(format!("{url}/{prefix}"));
    assert_eq!(at("a").url(), format!("{url}/a"));
    assert_eq!(
        at("a").generator_version().expect("a version"),
        "linkml-1.11.1"
    );
    assert_eq!(
        protocol(at("b").generator_version().expect_err("no version")),
        "a /healthz answer with no generatorVersion"
    );
    assert!(
        protocol(at("c").generator_version().expect_err("not an object"))
            .starts_with("a /healthz answer that is not a JSON object: [\"up\"]")
    );
    assert_eq!(
        protocol(
            at("d")
                .generator_version()
                .expect_err("a number is no version")
        ),
        "a /healthz answer with no generatorVersion"
    );
}

#[test]
fn generate_sends_the_imports_by_name_and_shows_every_error_entry() {
    let (url, seen) = stub(|_| {
        ok(
            200,
            r#"{"generatorVersion":"v","jsonSchema":{},"errors":["slot x has no range",{"line":3}]}"#,
        )
    });
    let tools = ModelTools::new(&url);
    let imports = BTreeMap::from([("stations".to_owned(), "id: s\n".to_owned())]);
    let answer = tools.generate_with("id: m\n", &imports).expect("an answer");
    assert_eq!(answer.generator_version, "v");
    assert_eq!(answer.errors, ["slot x has no range", r#"{"line":3}"#]);
    assert!(answer.artifacts.contains_key("jsonSchema"));
    assert!(!answer.artifacts.contains_key("errors"));
    let plain = tools.generate("id: m\n").expect("an answer");
    assert_eq!(plain.errors.len(), 2);
    let bodies = seen.lock().expect("log").clone();
    let sent: serde_json::Value =
        serde_json::from_str(bodies[0].split_once(' ').expect("path and body").1).expect("json");
    assert_eq!(
        sent,
        json!({ "source": "id: m\n", "imports": { "stations": "id: s\n" } })
    );
    let sent: serde_json::Value =
        serde_json::from_str(bodies[1].split_once(' ').expect("path and body").1).expect("json");
    assert_eq!(
        sent,
        json!({ "source": "id: m\n" }),
        "no imports key when there are none"
    );
}

#[test]
fn an_answer_that_is_not_an_object_or_names_no_generator_is_refused() {
    let (url, _) = stub(|path| match path {
        "/a/generate" => ok(200, "<html>gateway</html>"),
        "/b/generate" => ok(200, r#"{"jsonSchema":{}}"#),
        _ => ok(200, r#"{"generatorVersion":"v","errors":"not a list"}"#),
    });
    let at = |prefix: &str| ModelTools::new(format!("{url}/{prefix}"));
    assert!(protocol(at("a").generate("x").expect_err("html")).contains("<html>gateway</html>"));
    assert!(protocol(at("b").generate("x").expect_err("no version")).contains("DM-19"));
    assert!(at("c")
        .generate("x")
        .expect("errors that are no list read as none")
        .errors
        .is_empty());
}

#[test]
fn a_status_that_is_not_a_result_carries_what_the_service_said() {
    let (url, _) = stub(|path| match path {
        "/a/generate" => ok(422, r#"{"errors":["source is empty",{"code":1}]}"#),
        "/b/generate" => ok(500, r#"{"detail":"boom"}"#),
        _ => ok(502, "bad gateway"),
    });
    let at = |prefix: &str| ModelTools::new(format!("{url}/{prefix}"));
    for (prefix, status, message) in [
        ("a", 422, r#"source is empty; {"code":1}"#),
        ("b", 500, r#"{"detail":"boom"}"#),
        ("c", 502, "bad gateway"),
    ] {
        match at(prefix).generate("x").expect_err("not a result") {
            Error::Service {
                status: s,
                message: m,
            } => {
                assert_eq!((s, m.as_str()), (status, message));
            }
            other => panic!("expected Service, got {other:?}"),
        }
    }
    let shown = at("c").generate("x").expect_err("502").to_string();
    assert_eq!(shown, "Model Tools answered 502: bad gateway");
}

#[test]
fn infer_sends_the_sample_base64_and_refuses_an_answer_that_is_not_an_object() {
    let (url, seen) = stub(|path| match path {
        "/a/infer-schema" => ok(200, r#"{"linkml":"id: draft\n","operations":[]}"#),
        _ => ok(200, "[]"),
    });
    let draft = ModelTools::new(format!("{url}/a"))
        .infer("s.csv", b"a,b\n1,2\n")
        .expect("a draft");
    assert_eq!(draft["linkml"], "id: draft\n");
    let body = seen.lock().expect("log")[0].clone();
    assert!(body.contains(r#""content":"YSxiCjEsMgo=""#), "{body}");
    assert!(body.contains(r#""name":"s.csv""#), "{body}");
    assert!(protocol(
        ModelTools::new(format!("{url}/b"))
            .infer("s.csv", b"")
            .expect_err("a list")
    )
    .starts_with("an infer-schema answer that is not a JSON object"));
}

#[test]
fn import_writes_nothing_when_the_catalogue_answers_errors_and_no_source() {
    let (url, _) = stub(|path| match path {
        "/a/import-sdm" => ok(
            200,
            r#"{"generatorVersion":"v","errors":["no such model"]}"#,
        ),
        "/b/import-sdm" => ok(200, r#"{"generatorVersion":"v","errors":[]}"#),
        _ => ok(
            200,
            r#"{"generatorVersion":"v","linkml":"id: air\n","errors":[]}"#,
        ),
    });
    let dir = common::temp_dir("model-import-edges");
    let out = dir.join("nested/deeper/air.linkml.yaml");
    let report = import(&ModelTools::new(format!("{url}/a")), "sdm/Nope", &out).expect("a report");
    assert_eq!(report["written"], json!([]));
    assert_eq!(report["errors"], json!(["no such model"]));
    assert!(!out.exists());

    let refused =
        import(&ModelTools::new(format!("{url}/b")), "sdm/Air", &out).expect_err("no linkml");
    assert!(protocol(refused).contains("no `linkml`"));

    let report = import(&ModelTools::new(format!("{url}/c")), "sdm/Air", &out).expect("written");
    assert_eq!(std::fs::read_to_string(&out).expect("source"), "id: air\n");
    assert_eq!(report["generatorVersion"], "v");

    std::fs::write(dir.join("file"), "x").expect("a file");
    let under_a_file = dir.join("file/air.linkml.yaml");
    match import(
        &ModelTools::new(format!("{url}/c")),
        "sdm/Air",
        &under_a_file,
    ) {
        Err(Error::Io { path, .. }) => assert!(path.ends_with("file"), "{}", path.display()),
        other => panic!("expected Io, got {other:?}"),
    }
}

#[test]
fn the_client_refuses_urls_it_cannot_use_and_answers_it_will_not_guess_at() {
    for (url, says) in [
        ("https://mt:8080", "not an http:// URL"),
        ("http://user:pw@mt:8080", "carries no credentials"),
        ("http://mt:eighty", "'eighty' is not a port"),
        ("http://:8080", "names no host"),
    ] {
        match request(url, "/healthz", None) {
            Err(HttpError::Url(message)) => assert!(message.contains(says), "{url}: {message}"),
            other => panic!("{url}: expected a Url error, got {other:?}"),
        }
    }
    let (url, _) = stub(|path| match path {
        "/chunked" => {
            "HTTP/1.1 200 X\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n".into()
        }
        "/headless" => "HTTP/1.1 200 X".into(),
        "/status" => "SMTP ready\r\n\r\n{}".into(),
        _ => "HTTP/1.1 200 X\n\n{\"lf\":true}".into(),
    });
    for (path, says) in [
        ("/chunked", "chunked"),
        ("/headless", "no header block"),
        ("/status", "'SMTP ready' is not a status line"),
    ] {
        match request(&url, path, Some("{}")) {
            Err(HttpError::Protocol(message)) => {
                assert!(message.contains(says), "{path}: {message}")
            }
            other => panic!("{path}: expected a Protocol error, got {other:?}"),
        }
    }
    let bare = request(&url, "/lf", None).expect("a bare-LF answer still reads");
    assert_eq!((bare.status, bare.body.as_str()), (200, "{\"lf\":true}"));
    assert!(HttpError::Protocol("x".into())
        .to_string()
        .starts_with("Model Tools answered something unusable"));
    assert!(HttpError::Io("x".into())
        .to_string()
        .starts_with("the connection to Model Tools failed"));
}
