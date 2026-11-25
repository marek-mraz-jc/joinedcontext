//! T-7793: the Portal client library against a raw stub, for the answers the binary-level tests
//! in portal_client_tests do not reach: unusable addresses and tokens, answers that are not JSON
//! or not a List, a list that never ends, problems with every error listed and the token
//! redacted, and the per-manifest reports of delete and plan.

use jcctl::portal::{self, Output, Portal, PortalError};
use jcctl::SecretValue;
use std::io::{Read, Write};
use std::net::TcpListener;

const TOKEN: &str = "eyJhbGciOiJFUzI1NiJ9.portal-lib-token";

/// A Portal whose answer to `METHOD /path?query` is `answer(line)`, a whole raw response.
fn stub(answer: fn(&str) -> String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("an address").port();
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
            let line: String = text
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ");
            let _ = stream.write_all(answer(&line).as_bytes());
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn http(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
}

fn client(url: &str) -> Portal {
    Portal::new(url, SecretValue::minted(TOKEN.to_owned())).expect("a usable Portal")
}

fn manifest(name: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: {name}\n  \
         namespace: p\nspec:\n  audience: public\n"
    )
}

#[test]
fn output_names_the_three_formats_and_nothing_else() {
    assert!(matches!(Output::parse("name"), Some(Output::Name)));
    assert!(matches!(Output::parse("yaml"), Some(Output::Yaml)));
    assert!(matches!(Output::parse("json"), Some(Output::Json)));
    assert!(Output::parse("wide").is_none());
    assert!(Output::parse("").is_none());
}

#[test]
fn an_address_or_token_the_token_must_not_travel_with_is_refused() {
    let token = || SecretValue::minted(TOKEN.to_owned());
    for (url, says) in [
        ("not a url", "it is not a URL"),
        ("https://u:p@portal.example", "carries credentials"),
        ("ftp://portal.example", "not an http(s) URL"),
        ("http://portal.example", "over https alone"),
    ] {
        match Portal::new(url, token()) {
            Err(PortalError::Unavailable { url: at, message }) => {
                assert_eq!(at, url);
                assert!(message.contains(says), "{url}: {message}");
            }
            other => panic!("{url}: expected Unavailable, got {other:?}"),
        }
    }
    match Portal::new("https://portal.example", SecretValue::minted(String::new())) {
        Err(PortalError::Unavailable { message, .. }) => {
            assert_eq!(message, "the token file is empty")
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

#[test]
fn a_list_that_is_not_json_or_not_a_list_or_refused_says_so_with_the_token_redacted() {
    let url = stub(|line| {
        if line.contains("/projects/a/") {
            http(200, "<html>login</html>")
        } else if line.contains("/projects/b/") {
            http(200, r#"{"kind":"Status"}"#)
        } else {
            http(
                403,
                &format!(
                    r#"{{"detail":"token {TOKEN} may not list","errors":["needs reader","needs scope",7]}}"#
                ),
            )
        }
    });
    let portal = client(&url);
    let shown = |project: &str| {
        portal
            .list(project, "endpoints", None)
            .expect_err("refused")
            .to_string()
    };
    assert!(
        shown("a").contains("the answer is not JSON: <html>login</html>"),
        "{}",
        shown("a")
    );
    assert!(
        shown("b").ends_with("the answer is not a List"),
        "{}",
        shown("b")
    );
    let refused = shown("c");
    assert_eq!(
        refused,
        "endpoints: the Portal refused (403): token [redacted] may not list; needs reader; needs scope"
    );
}

#[test]
fn a_list_whose_continue_never_ends_is_refused_rather_than_followed_forever() {
    let url = stub(|_| http(200, r#"{"items":[],"metadata":{"continue":"again"}}"#));
    let refused = client(&url)
        .list("p", "endpoints", Some("a=b"))
        .expect_err("endless");
    assert!(
        refused
            .to_string()
            .ends_with("the list did not end after 1000 pages"),
        "{refused}"
    );
}

#[test]
fn get_and_describe_of_a_name_the_portal_does_not_hold_name_it_and_the_project() {
    let url = stub(|line| {
        if line.contains("/broken") {
            http(500, "upstream down")
        } else {
            http(404, r#"{"detail":"not found"}"#)
        }
    });
    let portal = client(&url);
    let missing = portal::get(&portal, "p", "endpoints", Some("air"), Output::Json, None)
        .expect_err("not there");
    assert_eq!(missing.to_string(), "endpoints/air: not found in project p");
    let missing = portal::describe(&portal, "p", "endpoints", "air").expect_err("not there");
    assert_eq!(missing.to_string(), "endpoints/air: not found in project p");
    let broken = portal.get("p", "endpoints", "broken").expect_err("500");
    assert_eq!(
        broken.to_string(),
        "endpoints/broken: the Portal refused (500): upstream down"
    );
}

#[test]
fn describe_leaves_out_empty_title_description_and_status() {
    let url = stub(|_| {
        http(
            200,
            r#"{"kind":"Endpoint","metadata":{"name":"air","title":"Air"},"spec":{"audience":"public"},"status":null}"#,
        )
    });
    let page = portal::describe(&client(&url), "p", "endpoints", "air").expect("described");
    assert!(page.contains("Title:    Air\n"), "{page}");
    assert!(!page.contains("Description:"), "{page}");
    assert!(page.contains("Spec:\n  audience: public\n"), "{page}");
    assert!(!page.contains("Status:"), "{page}");
}

#[test]
fn delete_reports_each_change_or_refusal_and_goes_on() {
    let url = stub(|line| {
        if line.contains("/gone") {
            http(409, r#"{"detail":"an open Change holds it"}"#)
        } else {
            http(
                202,
                r#"{"metadata":{"name":"chg-1"},"status":{"phase":"Proposed"}}"#,
            )
        }
    });
    let mut text = manifest("gone");
    text.push_str("---\n");
    text.push_str(&manifest("air"));
    let targets = portal::targets(&text, None).expect("two manifests");
    let report = portal::delete(&client(&url), &targets);
    assert!(report.failed);
    assert_eq!(
        report.lines,
        [
            "error: endpoints/gone: the Portal refused (409): an open Change holds it",
            "endpoints/air: deleted, chg-1 Proposed (? lane)",
        ]
    );
}

#[test]
fn apply_and_plan_report_a_live_manifest_or_an_answer_that_does_not_read() {
    let url = stub(|line| {
        if line.starts_with("GET") {
            http(200, r#"{"kind":7}"#)
        } else {
            http(200, "not json")
        }
    });
    let targets = portal::targets(&manifest("air"), None).expect("one manifest");
    let applied = portal::apply(&client(&url), &targets);
    assert!(applied.failed);
    assert!(
        applied.lines[0].starts_with("error: the live manifest does not read"),
        "{:?}",
        applied.lines
    );
    let planned = portal::plan(&client(&url), &targets);
    assert!(planned.failed);
    assert!(
        planned.lines[0].contains("the answer is not JSON: not json"),
        "{:?}",
        planned.lines
    );
}

#[test]
fn ops_that_answer_something_other_than_a_list_are_refused() {
    let url = stub(|line| {
        if line.contains("/projects/a/") {
            http(200, r#"{"ops":[]}"#)
        } else {
            http(401, "")
        }
    });
    let refused = portal::operations(&client(&url), "a").expect_err("an object");
    assert!(
        refused
            .to_string()
            .ends_with("the answer is not a list of operations"),
        "{refused}"
    );
    let refused = portal::operations(&client(&url), "b").expect_err("401");
    assert!(
        refused
            .to_string()
            .starts_with("ops: the Portal refused (401)"),
        "{refused}"
    );
}

/// A document that is no manifest, a kind jcctl does not know and a manifest naming no project
/// without --project are refused with the document they came from.
#[test]
fn a_document_that_cannot_be_sent_is_named() {
    for (text, words) in [
        ("kind: [unclosed\n", "not a manifest"),
        (
            "apiVersion: joinedcontext.com/v1alpha1\nkind: Teapot\nmetadata:\n  name: x\n  namespace: p\nspec: {}\n",
            "no kind 'Teapot'",
        ),
        (
            "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: x\nspec: {}\n",
            "names no project",
        ),
    ] {
        let refused = portal::targets(text, None).expect_err(words);
        assert!(refused.to_string().contains(words), "{refused}");
        assert!(refused.to_string().contains("document 1"), "{refused}");
    }
}

#[test]
fn a_file_with_only_empty_documents_holds_no_manifest() {
    for text in ["", "---\n---\n", "# a comment\n"] {
        let refused = portal::targets(text, Some("p")).expect_err("nothing to send");
        assert_eq!(refused.to_string(), "the file holds no manifest");
    }
}

#[test]
fn an_unreachable_portal_names_its_address() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            drop(stream);
        }
    });
    match client(&url).operations("p") {
        Err(PortalError::Unavailable { url: at, .. }) => assert!(at.starts_with(&url), "{at}"),
        other => panic!("expected Unavailable, got {other:?}"),
    }
}
