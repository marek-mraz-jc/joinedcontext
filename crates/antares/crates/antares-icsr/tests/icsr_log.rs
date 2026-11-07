// SPDX-License-Identifier: EUPL-1.2
//! Threat C9 for what the grants and the transport log beside the forward
//! line: a grant written and removed, a stored document that grants
//! nothing, and each forward that is not made, all under the broker's
//! global subscriber, none of them naming the subject.

mod common;

use antares_ops::test_kit::TestBroker;
use common::{broker, car, cars, grant, ids, register, send, warned, SELF_URL};
use serde_json::json;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// A `tracing` writer into a shared buffer.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The binary's global subscriber (a thread's own misses the callsites
/// another test registered first), and what it has written.
fn logged() -> String {
    static LOG: OnceLock<Log> = OnceLock::new();
    let log = LOG.get_or_init(|| {
        let log = Log::default();
        let writer = log.clone();
        let _ = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .try_init();
        log
    });
    String::from_utf8_lossy(&log.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
}

/// Whether a line of `text` holds `message` and every one of `fields`.
fn has(text: &str, message: &str, fields: &[&str]) -> bool {
    text.lines()
        .any(|l| l.contains(message) && fields.iter().all(|f| l.contains(f)))
}

/// A forward below the self URL, a write under a read grant and one the
/// grant's view does not allow are not made and are logged with the
/// registration and the Tenants; grants written and removed are logged.
#[tokio::test]
async fn icsr_grants_and_unmade_forwards_are_logged_without_the_subject() {
    logged();
    let (_b, app) = broker(None, Vec::new()).await;
    car(&app, "b", 1).await;
    grant(&app, "b", "a", json!({})).await;
    let below = format!("{SELF_URL}/other");
    let r = register(&app, "a", "below", Some("b"), json!({"endpoint": below})).await;
    assert_eq!(r.status, 201, "{}", r.body);
    let read = cars(&app, "a", Some("alice")).await;
    assert!(warned(&read.warnings, "299"), "{:?}", read.warnings);

    let view = json!({"id": "b-read", "tenant": "b", "selector": ["Car"],
                      "rules": [{"types": ["Car"], "attributes": ["speed"]}],
                      "operations": ["query_entities", "retrieve_entity"]});
    let put = send(&app, "PUT", "/q/views/b-read", None, None, Some(view)).await;
    assert_eq!(put.status, 201, "{}", put.body);
    grant(&app, "b", "e", json!({})).await;
    grant(
        &app,
        "b",
        "d",
        json!({"view": "b-read", "access": "read-write"}),
    )
    .await;
    for source in ["e", "d"] {
        let w = register(
            &app,
            source,
            "w",
            Some("b"),
            json!({"operations": ["createEntity"]}),
        )
        .await;
        assert_eq!(w.status, 201, "{}", w.body);
        let new_car = json!({"id": format!("urn:ngsi-ld:Car:{source}1"), "type": "Car"});
        let a = send(
            &app,
            "POST",
            "/ngsi-ld/v1/entities",
            Some(source),
            Some("alice"),
            Some(new_car),
        )
        .await;
        assert_eq!(a.status, 207, "{}", a.body);
    }
    assert_eq!(
        ids(&cars(&app, "b", None).await.body),
        ["urn:ngsi-ld:Car:b-1"]
    );
    assert_eq!(
        send(&app, "DELETE", "/q/grants/b/e", None, None, None)
            .await
            .status,
        204
    );

    let text = logged();
    for (message, fields) in [
        (
            "export grant written",
            &[
                r#"target_tenant="b""#,
                r#"source_tenant="d""#,
                "access=ReadWrite",
                r#"view="b-read""#,
            ][..],
        ),
        (
            "export grant removed",
            &[r#"target_tenant="b""#, r#"source_tenant="e""#][..],
        ),
        (
            "no NGSI-LD operation is not made",
            &[
                r#"registration="urn:ngsi-ld:ContextSourceRegistration:below""#,
                r#"source_tenant="a""#,
                r#"method="GET""#,
            ][..],
        ),
        (
            "without a grant for it is not made",
            &[
                r#"source_tenant="e""#,
                r#"target_tenant="b""#,
                "granted=true",
                r#"operation="create_entity""#,
            ][..],
        ),
        (
            "does not allow the forward",
            &[
                r#"registration="urn:ngsi-ld:ContextSourceRegistration:w""#,
                r#"view="b-read""#,
                r#"operation="create_entity""#,
            ][..],
        ),
    ] {
        assert!(has(&text, message, fields), "{message}: {text}");
    }
    assert!(!text.contains("alice"), "{text}");
}

/// A stored document of another pair grants nothing, and says so in the
/// log with the pair it was read for.
#[tokio::test]
async fn icsr_a_stored_document_of_another_pair_is_logged() {
    logged();
    let kept = Arc::new(Mutex::new(None));
    let keep = kept.clone();
    let selfs = antares_icsr::SelfUrls::parse(SELF_URL).unwrap_or_default();
    let app = TestBroker::new()
        .await
        .and_then(|b| {
            b.with_forward_transport(move |ops| {
                *keep.lock().unwrap_or_else(PoisonError::into_inner) = Some(ops.clone());
                antares_icsr::transport(selfs)(ops)
            })
        })
        .and_then(|b| b.serve_mounted(Vec::new()))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let ops = kept
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .unwrap_or_else(|| unreachable!("the transport was made"));
    let doc = json!({"target": "b", "source": "a"});
    assert_eq!(ops.put_grant_doc("b~f", doc).await, Ok(false));
    assert_eq!(
        send(&app, "GET", "/q/grants/b/f", None, None, None)
            .await
            .status,
        404
    );
    let text = logged();
    let fields = [r#"target_tenant="b""#, r#"source_tenant="f""#];
    assert!(
        has(&text, "a stored export grant does not read as one", &fields),
        "{text}"
    );
}
