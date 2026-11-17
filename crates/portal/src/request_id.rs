//! One reference per request, the same in the browser, the edge and the Portal's log (UI-16,
//! OPS-15, T-2747).
//!
//! The edge's `request-id` plugin names every request and returns the name in `X-Request-Id`;
//! a failure page shows it, so a person can quote it and whoever reads the log finds the line.
//! The Portal takes the edge's id when it is a plain token, mints one otherwise (a Portal run
//! without the edge, or a header nobody should trust into a log), puts it on the span every log
//! line of the request is written in, and answers with it.
//!
//! The same span carries the tenancy key of the project the address names, by id (ARCH/TEN-04,
//! T-7192 PK-2a): `org` and `project` are recorded the moment the request resolves that
//! project's key (`project_ids::key_of`), so a log line names the project the way its rows are
//! keyed, never by a name that a rename or a branch would make ambiguous.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

pub const HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// A UUID or a similar token: letters, digits and dashes, 8 to 64 of them. Anything else could
/// forge a log line or carry more than a reference.
fn plain(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn fresh() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

tokio::task_local! {
    /// The id of the request the current task answers, for the calls it makes to the Portal's
    /// neighbours (the forge, the gateway): theirs log the same reference.
    static CURRENT: String;
    /// The request's span and the project its address names, whose key [`record_key`] puts on it.
    static ADDRESSED: (tracing::Span, Option<String>);
    /// The project id an id address placed the request under (RTE-05): jc-router's
    /// `IdAddress` extension, which a client cannot send.
    static PLACED: Option<String>;
}

/// The project id, in path form, of the id address the current request came in on; `None` on a
/// name address and outside a request (a spawned task, a sweep), where no branch resolves.
pub fn placed_id() -> Option<String> {
    PLACED.try_with(Clone::clone).ok().flatten()
}

/// Runs `f` as a request the router placed under the id address `id` (a test's stand-in for
/// [`middleware`] behind jc-router).
pub async fn placed<F: std::future::Future>(id: Option<String>, f: F) -> F::Output {
    PLACED.scope(id, f).await
}

/// The project an address names: `/api/v1/projects/{project}/...` or the OData form
/// `/api/v1/projects('{project}')...`. A name that is not a DNS-1123 label names none.
fn addressed_project(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/api/v1/projects")?;
    let name = if let Some(rest) = rest.strip_prefix('/') {
        rest.split('/').next()?
    } else {
        rest.strip_prefix("('")?.split("')").next()?
    };
    crate::resource::is_dns1123(name).then_some(name)
}

/// Records `key` as the request's `org` and `project` span fields when `project` is the project
/// the request's address names; any other key (a fan-out over projects, a sweep outside a
/// request) leaves the span as it is.
pub fn record_key(project: &str, key: jc_types::ProjectKey) {
    let _ = ADDRESSED.try_with(|(span, addressed)| {
        if addressed.as_deref() == Some(project) {
            span.record("org", tracing::field::display(key.org));
            span.record("project", tracing::field::display(key.project));
        }
    });
}

/// `builder` with the current request's id, when the task answers a request.
pub fn forward(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match CURRENT.try_with(Clone::clone) {
        Ok(id) => builder.header(HEADER, id),
        Err(_) => builder,
    }
}

/// Runs `f` as if it answered the request `id` (a test's stand-in for [`middleware`]).
#[cfg(test)]
pub async fn answering<F: std::future::Future>(id: &str, f: F) -> F::Output {
    CURRENT.scope(id.to_owned(), f).await
}

pub async fn middleware(request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get(&HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|id| plain(id))
        .map_or_else(fresh, str::to_owned);
    let span = tracing::info_span!(
        "request",
        request_id = %id,
        org = tracing::field::Empty,
        project = tracing::field::Empty
    );
    let addressed = addressed_project(request.uri().path()).map(str::to_owned);
    let placed = request
        .extensions()
        .get::<jc_router::IdAddress>()
        .map(|placed| placed.0.clone());
    let mut response = CURRENT
        .scope(
            id.clone(),
            ADDRESSED.scope(
                (span.clone(), addressed),
                PLACED.scope(placed, next.run(request).instrument(span.clone())),
            ),
        )
        .await;
    if response.status().is_server_error() {
        span.in_scope(|| tracing::warn!(status = response.status().as_u16(), "request failed"));
    }
    // `plain` or hex: always a valid header value.
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(HEADER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_token_is_taken_from_the_edge() {
        assert!(plain("3f2c1a9e-6b1d-4c3e-9a8f-0d1e2f3a4b5c"));
        assert!(!plain("short"));
        assert!(!plain("a\nlevel=error msg=forged"));
        assert!(!plain(&"a".repeat(65)));
        assert!(!plain("id with spaces"));
    }

    #[tokio::test]
    async fn a_call_made_while_answering_a_request_carries_its_id() {
        let client = reqwest::Client::new();
        let outside = forward(client.get("http://127.0.0.1:9/"))
            .build()
            .expect("request");
        assert!(outside.headers().get(&HEADER).is_none());
        let inside = CURRENT
            .scope("3f2c1a9e-6b1d-4c3e".to_owned(), async {
                forward(client.get("http://127.0.0.1:9/"))
                    .build()
                    .expect("request")
            })
            .await;
        assert_eq!(
            inside.headers().get(&HEADER).map(|v| v.to_str().ok()),
            Some(Some("3f2c1a9e-6b1d-4c3e"))
        );
    }

    #[test]
    fn the_addressed_project_is_the_label_after_projects() {
        assert_eq!(
            addressed_project("/api/v1/projects/helsinki/endpoints"),
            Some("helsinki")
        );
        assert_eq!(
            addressed_project("/api/v1/projects/helsinki"),
            Some("helsinki")
        );
        assert_eq!(
            addressed_project("/api/v1/projects('helsinki')/groups"),
            Some("helsinki")
        );
        assert_eq!(addressed_project("/api/v1/projects"), None);
        assert_eq!(
            addressed_project("/api/v1/projects/Not%20a%20label/x"),
            None
        );
        assert_eq!(addressed_project("/api/v1/organizations/helsinki"), None);
    }

    /// T-7192 PK-2a: a log line of a request names the addressed project's key by id once the
    /// request resolves it; another project's key, resolved in the same request, never.
    #[tokio::test]
    async fn the_request_span_carries_the_addressed_projects_key_by_id() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Clone, Default)]
        struct Lines(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Lines {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("lines").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let lines = Lines::default();
        let writer = lines.clone();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone()),
        );
        let _guard = tracing::subscriber::set_default(subscriber);
        let mine = jc_types::ProjectKey::new(jc_types::OrgId::mint(), jc_types::ProjectId::mint());
        let other = jc_types::ProjectKey::new(jc_types::OrgId::mint(), jc_types::ProjectId::mint());
        // A fallback, not a route: openapi_tests reads every mounted route of the crate.
        let app = axum::Router::new()
            .fallback(move || async move {
                record_key("tampere", other);
                record_key("helsinki", mine);
                tracing::info!("listed");
            })
            .layer(axum::middleware::from_fn(middleware));
        let request = axum::http::Request::builder()
            .uri("/api/v1/projects/helsinki/endpoints")
            .body(axum::body::Body::empty())
            .expect("request");
        let response = tower::ServiceExt::oneshot(app, request)
            .await
            .expect("answer");
        assert!(response.status().is_success());
        let text = String::from_utf8(lines.0.lock().expect("lines").clone()).expect("utf-8");
        assert!(
            text.contains(&format!("project={}", mine.project)),
            "{text}"
        );
        assert!(text.contains(&format!("org={}", mine.org)), "{text}");
        assert!(!text.contains(&other.project.to_string()), "{text}");
        assert!(!text.contains("project=helsinki"), "{text}");
    }

    #[test]
    fn a_minted_id_is_plain_and_new_each_time() {
        let (one, two) = (fresh(), fresh());
        assert!(plain(&one) && one.len() == 32);
        assert_ne!(one, two);
    }
}
