//! T-7793: what `jcctl publish ckan` reads from the gateway and how a refused read reads on the
//! Portal's CKAN page (T-5748), and the printed line of one Endpoint.

use jcctl::commands::publish_ckan::{fetch, Error, Line};
use jcctl::publish::ckan::{Outcome, Views};
use jcctl::ResourceId;
use std::io::{Read, Write};
use std::net::TcpListener;

/// A gateway whose answer to a path is `answer(path)`, a whole raw response.
fn stub(answer: fn(&str) -> String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("an address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut raw = Vec::new();
            let mut chunk = [0_u8; 4096];
            while let Ok(read) = stream.read(&mut chunk) {
                raw.extend_from_slice(&chunk[..read]);
                if read == 0 || raw.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&raw).into_owned();
            let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
            let _ = stream.write_all(answer(&path).as_bytes());
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn http(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn refused(url: &str) -> String {
    match fetch(url, "application/ld+json") {
        Err(Error::Gateway { url: at, message }) => {
            assert_eq!(at, url);
            message
        }
        other => panic!("{url}: expected a Gateway error, got {other:?}"),
    }
}

#[test]
fn a_refused_read_says_the_status_and_the_problem_in_words() {
    let url = stub(|path| match path {
        "/ok" => http(200, r#"{"@id":"x"}"#),
        "/detail" => http(404, r#"{"title":"Not Found","detail":"no Endpoint air"}"#),
        "/title" => http(403, r#"{"title":"Forbidden here"}"#),
        "/json" => http(429, r#"{"retry":5}"#),
        "/text" => http(503, "  upstream\n\n  connect   error  "),
        "/long" => http(500, &"x".repeat(500)),
        _ => http(502, ""),
    });
    assert_eq!(
        fetch(&format!("{url}/ok"), "application/ld+json").expect("read"),
        r#"{"@id":"x"}"#
    );
    assert_eq!(refused(&format!("{url}/detail")), "404 no Endpoint air");
    assert_eq!(refused(&format!("{url}/title")), "403 Forbidden here");
    assert_eq!(refused(&format!("{url}/json")), "429 Too Many Requests");
    assert_eq!(
        refused(&format!("{url}/text")),
        "503 upstream connect error"
    );
    assert_eq!(
        refused(&format!("{url}/long")),
        format!("500 {}", "x".repeat(200))
    );
    assert_eq!(refused(&format!("{url}/empty")), "502 Bad Gateway");
}

#[test]
fn an_unreachable_gateway_is_an_error_naming_the_url() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let url = format!("http://{}/record", listener.local_addr().expect("address"));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            drop(stream);
        }
    });
    let message = refused(&url);
    assert!(!message.is_empty());
    let shown = fetch(&url, "text/yaml").expect_err("closed").to_string();
    assert!(shown.starts_with(&format!("GET {url}: ")), "{shown}");
}

#[test]
fn a_line_names_the_views_and_a_missing_linkml_only_when_there_are_any() {
    let endpoint = ResourceId {
        group: "joinedcontext.com".into(),
        kind: "Endpoint".into(),
        namespace: Some("helsinki".into()),
        name: "air".into(),
    };
    let mut line = Line {
        endpoint,
        dataset: "helsinki-air".into(),
        outcome: Outcome::Unchanged,
        no_linkml: false,
        views: Views::default(),
    };
    assert_eq!(
        line.to_string(),
        "Endpoint/helsinki/air: dataset helsinki-air unchanged"
    );
    line.outcome = Outcome::Updated;
    line.views = Views {
        created: 1,
        updated: 2,
        deleted: 3,
    };
    line.no_linkml = true;
    assert_eq!(
        line.to_string(),
        "Endpoint/helsinki/air: dataset helsinki-air updated, views 1 created, 2 updated, 3 \
         deleted; the record lists no model.linkml.yaml, the dataset links /schema/index.json only"
    );
    for (outcome, word) in [
        (Outcome::NotPublished, "not published"),
        (Outcome::Created, "created"),
        (Outcome::Withdrawn, "withdrawn"),
    ] {
        line.outcome = outcome;
        line.views = Views::default();
        line.no_linkml = false;
        assert!(line.to_string().ends_with(word), "{line}");
    }
}
