//! One request id per request across jc's modules, and one access line per call at a module's
//! door: the edge's job in the cluster (APISIX `request-id` plugin and access log), which jc
//! does without an edge.
//!
//! Each mount takes the caller's `X-Request-Id` when it is a plain token and mints one
//! otherwise, hands it on to the module in the request (so the Portal and the gateway, which
//! read the header, keep it for their own lines and the broker hop), runs the module inside a
//! `request` span that names the module and the id (every line the module writes for the
//! request carries both), answers with the id, and writes one `jc::access` line: method, path
//! without the query, status and milliseconds; for a refusal or a failure also the reason. An
//! answered call without a caller's id is written once a minute per method and path ([`Sampler`]).
//!
//! The Portal's health probe walks every surface of every Live Endpoint each round, one unique
//! path per call: its calls carry [`probe_mark`] (`x-probe-mark`), a secret of this process.
//! The door takes the mark off, writes the probe's answered calls once a minute per door (key
//! `probe`), and runs the call in a `probe` span; the broker hop finds the call's id among the
//! probes in flight and does the same. Every line below WARN inside a `probe` span but the
//! door's own is left out ([`quiet_probes`]): a refusal, a failure and every warning keep
//! their line, and the Portal writes one line per round with a line per failed surface.
//!
//! The same door counts the call into jc's one metrics recorder for the modules that record
//! nothing of their own (shako, kaisatsu, daiya, dataroom, ekiben): `<module>_http_requests_total`
//! by status and `<module>_http_request_duration_seconds`. Antares, the gateway and the Portal
//! keep their own request series.

use axum::Router;
use axum::extract::Request;
use axum::http::Method;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tracing::Instrument;

pub const HEADER: HeaderName = HeaderName::from_static("x-request-id");
/// The Portal's `health_probe::MARK_HEADER`; not `x-jc-*`, which the gateway takes off itself.
pub const PROBE_HEADER: HeaderName = HeaderName::from_static("x-probe-mark");
/// The sampler's key for every answered probe call at one door.
const PROBE_KEY: &str = "probe";

static PROBE_MARK: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// Ids of the probe calls in flight, and of those that ended under [`PROBE_GRACE`] ago (when
/// they ended): the broker hop knows a probe by its id, which the gateway hands on, so no header
/// a caller can send makes a call quiet.
static PROBES: Mutex<Vec<(String, Option<Instant>)>> = Mutex::new(Vec::new());
/// How long an ended probe call's id still names a probe: the websocket hub a probe's upgrade
/// starts opens its broker upstream under the probe's id just after the `101` went out.
const PROBE_GRACE: Duration = Duration::from_secs(10);

/// This process's probe mark (drawn once, never logged).
pub fn probe_mark() -> &'static str {
    PROBE_MARK.get_or_init(fresh)
}

/// The request carried this process's mark; the header is taken off either way.
fn take_mark(req: &mut Request) -> bool {
    let Some(sent) = req.headers_mut().remove(&PROBE_HEADER) else {
        return false;
    };
    let mark = probe_mark().as_bytes();
    let sent = sent.as_bytes();
    sent.len() == mark.len() && sent.iter().zip(mark).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
}

fn probes() -> std::sync::MutexGuard<'static, Vec<(String, Option<Instant>)>> {
    PROBES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `id` names a probe in flight, or one that ended under [`PROBE_GRACE`] before `now`.
fn is_probe(id: &str, now: Instant) -> bool {
    probes().iter().any(|(held, ended)| {
        held == id && ended.is_none_or(|at| now.duration_since(at) < PROBE_GRACE)
    })
}

/// Keeps `id` among the probes while it lives and for [`PROBE_GRACE`] after.
struct InFlight(String);

impl InFlight {
    fn new(id: &str) -> Self {
        let now = Instant::now();
        let mut ids = probes();
        ids.retain(|(_, ended)| ended.is_none_or(|at| now.duration_since(at) < PROBE_GRACE));
        ids.push((id.to_owned(), None));
        Self(id.to_owned())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let now = Instant::now();
        for (held, ended) in probes().iter_mut() {
            if *held == self.0 {
                *ended = Some(now);
            }
        }
    }
}

/// The span name the [`quiet_probes`] filter keeps quiet.
const PROBE_SPAN: &str = "probe";

/// Leaves out every event below WARN inside a `probe` span except the door's own `jc::access`
/// lines, which the sampler already thins out.
pub fn quiet_probes<S>() -> impl tracing_subscriber::layer::Filter<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::filter::dynamic_filter_fn(|meta, cx| {
        if !meta.is_event()
            || *meta.level() <= tracing::Level::WARN
            || meta.target() == "jc::access"
        {
            return true;
        }
        !cx.lookup_current()
            .is_some_and(|span| span.scope().any(|s| s.name() == PROBE_SPAN))
    })
}

/// Letters, digits and dashes, 8 to 64 of them (the Portal's rule): anything else could forge
/// a log line or carry more than a reference.
fn plain(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn fresh() -> String {
    let mut bytes = [0u8; 16];
    // A failed draw leaves zeros: still a well-formed id, never a reason to refuse the call.
    let _ = getrandom::fill(&mut bytes);
    hex::encode(bytes)
}

/// The caller's id when it is plain (the first one).
fn callers_id(req: &Request) -> Option<String> {
    req.headers()
        .get(&HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|id| plain(id))
        .map(str::to_owned)
}

/// `router` behind the access layer of `module`.
pub fn layer(module: &'static str, router: Router) -> Router {
    let sampler = Arc::new(Sampler::default());
    router.layer(axum::middleware::from_fn(
        move |req: Request, next: Next| {
            access(module, Door::Outer(Arc::clone(&sampler)), req, next)
        },
    ))
}

/// `router` behind the access layer of `module` on a hop inside `jc` (the gateway's way into the
/// broker): the request's id and span as [`layer`], and an access line only for a refusal or a
/// failure. The door the request came in at wrote its answered line under the same id: one line
/// per request across modules (the gate of 2026-11-09 counted 39.5k of the broker's 61.8k access
/// lines on the hop, each repeating a gateway line).
pub fn hop_layer(module: &'static str, router: Router) -> Router {
    router.layer(axum::middleware::from_fn(
        move |req: Request, next: Next| access(module, Door::Hop, req, next),
    ))
}

enum Door {
    Outer(Arc<Sampler>),
    Hop,
}

/// How long an answered call is the only line of its kind (module door, method, path): a poll
/// answered every few seconds writes one line a minute, the next one saying how many went
/// unwritten (`unlogged`). A call whose caller sent its own `X-Request-Id` asked to be traced and
/// is always written, and so is every refusal and failure. The request counters keep every call.
/// The Portal's sync polls wrote 675 kaisatsu lines a minute on an idle prod rehearsal
/// (2026-11-09), all `answered`.
const QUIET_FOR: Duration = Duration::from_secs(60);
/// Kinds remembered at most per door: past it every answer is written, so a walk over unique
/// paths never grows the table without bound.
const KINDS: usize = 4096;

#[derive(Default)]
struct Sampler(Mutex<HashMap<(Method, String), (Instant, u64)>>);

impl Sampler {
    /// `Some(unlogged)` when the answer is written (the first of its kind in [`QUIET_FOR`], with
    /// how many of its kind went unwritten before it), `None` when it is only counted.
    fn admit(&self, method: &Method, path: &str, now: Instant) -> Option<u64> {
        let mut seen = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let key = (method.clone(), path.to_owned());
        if let Some((at, unlogged)) = seen.get_mut(&key) {
            if now.duration_since(*at) < QUIET_FOR {
                *unlogged += 1;
                return None;
            }
            let before = *unlogged;
            *at = now;
            *unlogged = 0;
            return Some(before);
        }
        if seen.len() >= KINDS {
            seen.retain(|_, (at, _)| now.duration_since(*at) < QUIET_FOR);
        }
        if seen.len() < KINDS {
            seen.insert(key, (now, 0));
        }
        Some(0)
    }
}

async fn access(module: &'static str, door: Door, mut req: Request, next: Next) -> Response {
    let marked = take_mark(&mut req);
    let traced = callers_id(&req).filter(|_| !marked);
    let id = traced.clone().unwrap_or_else(fresh);
    let probe = match door {
        Door::Outer(_) => marked,
        Door::Hop => is_probe(&id, Instant::now()),
    };
    let _in_flight = (probe && matches!(door, Door::Outer(_))).then(|| InFlight::new(&id));
    // `plain` or hex: always a valid header value.
    let value = HeaderValue::from_str(&id).unwrap_or(HeaderValue::from_static("invalid"));
    req.headers_mut().insert(HEADER, value.clone());
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let span = if probe {
        tracing::info_span!("probe", module, request_id = %id)
    } else {
        tracing::info_span!("request", module, request_id = %id)
    };
    let started = Instant::now();
    let mut response = next.run(req).instrument(span.clone()).await;
    let status = response.status();
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    record(module, status.as_u16(), started.elapsed().as_secs_f64());
    span.in_scope(|| {
        let code = status.as_u16();
        if status.is_server_error() || status.is_client_error() {
            let reason = status.canonical_reason().unwrap_or("refused");
            tracing::info!(target: "jc::access", module, %method, path, status = code, ms, reason, "refused");
        } else if let Door::Outer(sampler) = &door {
            let unlogged = match traced {
                Some(_) => Some(0),
                None if probe => sampler.admit(&method, PROBE_KEY, started),
                None => sampler.admit(&method, &path, started),
            };
            match unlogged {
                Some(0) => {
                    tracing::info!(target: "jc::access", module, %method, path, status = code, ms, "answered");
                }
                Some(unlogged) => {
                    tracing::info!(target: "jc::access", module, %method, path, status = code, ms, unlogged, "answered");
                }
                None => {}
            }
        }
    });
    response.headers_mut().insert(HEADER, value);
    response
}

/// The modules whose own code records their request series (`antares_http_requests_total`,
/// `jc_gateway_*`, `jc_portal_*`): a second series of the same name would collide.
const RECORDS_ITS_OWN: [&str; 3] = ["antares", "gateway", "portal"];

/// One answered call of `module` into the process recorder.
fn record(module: &'static str, status: u16, seconds: f64) {
    if RECORDS_ITS_OWN.contains(&module) {
        return;
    }
    metrics::counter!(format!("{module}_http_requests_total"), "status" => status.to_string())
        .increment(1);
    metrics::histogram!(format!("{module}_http_request_duration_seconds")).record(seconds);
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::routing::get;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::ErrorKind::Other)?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A module that logs one line, refuses `/closed` and answers the id it was handed.
    fn module() -> Router {
        Router::new()
            .route(
                "/open",
                get(|req: Request| async move {
                    tracing::info!(target: "shako_api::x", "inner line");
                    req.headers()
                        .get(&HEADER)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned()
                }),
            )
            .route("/closed", get(|| async { StatusCode::UNAUTHORIZED }))
    }

    async fn call(path: &str, id: Option<&str>) -> (Response, String) {
        call_at(layer("shako", module()), path, id).await
    }

    async fn call_at(app: Router, path: &str, id: Option<&str>) -> (Response, String) {
        let buf = Buffer::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        let mut req = Request::builder().uri(format!("{path}?secret=q"));
        if let Some(id) = id {
            req = req.header("x-request-id", id);
        }
        let req = req.body(Body::empty()).expect("request");
        let _guard = tracing::subscriber::set_default(subscriber);
        let response = app.oneshot(req).await.expect("answer");
        let log = String::from_utf8(buf.0.lock().expect("log").clone()).expect("utf8");
        (response, log)
    }

    fn echoed(r: &Response) -> &str {
        r.headers()
            .get(&HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_callers_id_reaches_the_module_its_lines_and_the_answer() {
        let (response, log) = call("/open", Some("smokelogs-0123456789")).await;
        assert_eq!(echoed(&response), "smokelogs-0123456789");
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body");
        assert_eq!(&body[..], b"smokelogs-0123456789");
        let inner = log
            .lines()
            .find(|l| l.contains("inner line"))
            .expect("inner line");
        assert!(inner.contains("request_id=smokelogs-0123456789"), "{log}");
        assert!(inner.contains("module=\"shako\""), "{log}");
        let access: Vec<_> = log.lines().filter(|l| l.contains("jc::access")).collect();
        assert_eq!(access.len(), 1, "{log}");
        assert!(
            access[0].contains("status=200") && access[0].contains("path=\"/open\""),
            "{log}"
        );
        assert!(
            !log.contains("secret=q"),
            "the query stays out of the log: {log}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_refusal_logs_one_line_with_its_reason() {
        let (response, log) = call("/closed", Some("smokelogs-refused-01")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let lines: Vec<_> = log
            .lines()
            .filter(|l| l.contains("smokelogs-refused-01"))
            .collect();
        assert_eq!(lines.len(), 1, "{log}");
        assert!(
            lines[0].contains("status=401") && lines[0].contains("reason=\"Unauthorized\""),
            "{log}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_hop_logs_only_refusals_and_its_lines_keep_the_callers_id() {
        let hop = || hop_layer("antares", module());
        let (response, log) = call_at(hop(), "/open", Some("smokelogs-hop-answered")).await;
        assert_eq!(echoed(&response), "smokelogs-hop-answered");
        assert!(!log.contains("jc::access"), "{log}");
        let inner = log
            .lines()
            .find(|l| l.contains("inner line"))
            .expect("inner line");
        assert!(inner.contains("request_id=smokelogs-hop-answered"), "{log}");
        let (response, log) = call_at(hop(), "/closed", Some("smokelogs-hop-refused")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let lines: Vec<_> = log.lines().filter(|l| l.contains("jc::access")).collect();
        assert_eq!(lines.len(), 1, "{log}");
        assert!(
            lines[0].contains("request_id=smokelogs-hop-refused")
                && lines[0].contains("status=401")
                && lines[0].contains("reason=\"Unauthorized\""),
            "{log}"
        );
    }

    #[test]
    fn an_answer_is_written_once_a_minute_per_kind_with_the_count_it_left_out() {
        let sampler = Sampler::default();
        let t0 = Instant::now();
        let get = Method::GET;
        assert_eq!(sampler.admit(&get, "/q/views/a", t0), Some(0));
        assert_eq!(sampler.admit(&get, "/q/views/b", t0), Some(0));
        let later = t0 + Duration::from_secs(10);
        assert_eq!(sampler.admit(&get, "/q/views/a", later), None);
        assert_eq!(sampler.admit(&Method::POST, "/q/views/a", later), Some(0));
        assert_eq!(sampler.admit(&get, "/q/views/a", later), None);
        assert_eq!(sampler.admit(&get, "/q/views/a", t0 + QUIET_FOR), Some(2));
        assert_eq!(sampler.admit(&get, "/q/views/a", t0 + QUIET_FOR), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_poll_writes_one_answered_line_and_every_refusal_and_traced_call_its_own() {
        let app = layer("kaisatsu", module());
        let mut log = String::new();
        for (path, id) in [
            ("/open", None),
            ("/open", None),
            ("/open", None),
            ("/open", Some("smokelogs-traced-0001")),
            ("/closed", None),
            ("/closed", None),
        ] {
            log += &call_at(app.clone(), path, id).await.1;
        }
        let access: Vec<_> = log.lines().filter(|l| l.contains("jc::access")).collect();
        let answered = access.iter().filter(|l| l.contains("answered")).count();
        let refused = access.iter().filter(|l| l.contains("refused")).count();
        assert_eq!((answered, refused), (2, 2), "{log}");
        assert!(
            access
                .iter()
                .any(|l| l.contains("answered") && l.contains("request_id=smokelogs-traced-0001")),
            "{log}"
        );
    }

    /// A module whose every path logs an INFO and a WARN line and says whether a mark reached it.
    fn probed() -> Router {
        Router::new()
            .route(
                "/s/{slug}",
                get(|req: Request| async move {
                    tracing::info!(target: "context_gateway::x", "forwarded");
                    tracing::warn!(target: "context_gateway::x", "slow");
                    if req.headers().contains_key(&PROBE_HEADER) {
                        "marked"
                    } else {
                        "clean"
                    }
                }),
            )
            .route("/closed", get(|| async { StatusCode::UNAUTHORIZED }))
    }

    async fn probe_call(app: Router, path: &str, mark: &str) -> (String, String) {
        use tracing_subscriber::Layer;
        use tracing_subscriber::layer::SubscriberExt;
        let buf = Buffer::default();
        let writer = buf.clone();
        let fmt = tracing_subscriber::fmt::layer()
            .with_writer(move || writer.clone())
            .with_ansi(false);
        let subscriber = tracing_subscriber::registry().with(fmt.with_filter(quiet_probes()));
        let req = Request::builder()
            .uri(path)
            .header(PROBE_HEADER, mark)
            .body(Body::empty())
            .expect("request");
        let _guard = tracing::subscriber::set_default(subscriber);
        let response = app.oneshot(req).await.expect("answer");
        let body = axum::body::to_bytes(response.into_body(), 64)
            .await
            .expect("body");
        let log = String::from_utf8(buf.0.lock().expect("log").clone()).expect("utf8");
        (String::from_utf8_lossy(&body).into_owned(), log)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_probe_round_writes_one_answered_line_and_keeps_refusals_and_warnings() {
        let app = layer("gateway", probed());
        let mut log = String::new();
        for slug in ["a", "b", "c", "d", "e"] {
            let (body, line) = probe_call(app.clone(), &format!("/s/{slug}"), probe_mark()).await;
            assert_eq!(body, "clean", "the mark stops at the door");
            log += &line;
        }
        log += &probe_call(app.clone(), "/closed", probe_mark()).await.1;
        let count = |what: &str| log.lines().filter(|l| l.contains(what)).count();
        assert_eq!(count("answered"), 1, "{log}");
        assert_eq!(count("refused"), 1, "{log}");
        assert_eq!(count("forwarded"), 0, "{log}");
        assert_eq!(count("slow"), 5, "{log}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_forged_mark_is_an_ordinary_call() {
        let app = layer("gateway", probed());
        let mut log = String::new();
        for slug in ["a", "b"] {
            let (body, line) =
                probe_call(app.clone(), &format!("/s/{slug}"), "0123456789abcdef").await;
            assert_eq!(body, "clean");
            log += &line;
        }
        let count = |what: &str| log.lines().filter(|l| l.contains(what)).count();
        assert_eq!((count("answered"), count("forwarded")), (2, 2), "{log}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_hop_keeps_a_probe_in_flight_quiet() {
        let hop = hop_layer("antares", probed());
        let id = "probe-in-flight-0001";
        let call = |hop: Router| async move {
            let req = Request::builder()
                .uri("/s/x")
                .header(HEADER, id)
                .body(Body::empty())
                .expect("request");
            let buf = Buffer::default();
            let writer = buf.clone();
            use tracing_subscriber::Layer;
            use tracing_subscriber::layer::SubscriberExt;
            let fmt = tracing_subscriber::fmt::layer()
                .with_writer(move || writer.clone())
                .with_ansi(false);
            let subscriber = tracing_subscriber::registry().with(fmt.with_filter(quiet_probes()));
            let _guard = tracing::subscriber::set_default(subscriber);
            hop.oneshot(req).await.expect("answer");
            String::from_utf8(buf.0.lock().expect("log").clone()).expect("utf8")
        };
        let plain = call(hop.clone()).await;
        assert!(plain.contains("forwarded"), "{plain}");
        let in_flight = InFlight::new(id);
        let quiet = call(hop.clone()).await;
        assert!(
            !quiet.contains("forwarded") && quiet.contains("slow"),
            "{quiet}"
        );
        drop(in_flight);
        let ended = Instant::now();
        assert!(
            is_probe(id, ended),
            "an ended probe's id holds for the grace"
        );
        assert!(!is_probe(
            id,
            ended + PROBE_GRACE + Duration::from_millis(5)
        ));
    }

    #[test]
    fn every_module_without_series_of_its_own_records_its_calls() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            for module in crate::module::Module::ALL {
                record(module.name(), 200, 0.01);
                record(module.name(), 401, 0.002);
            }
        });
        let text = handle.render();
        for module in ["shako", "kaisatsu", "daiya", "dataroom", "ekiben"] {
            assert!(
                text.contains(&format!("{module}_http_requests_total{{status=\"200\"}} 1"))
                    && text.contains(&format!("{module}_http_requests_total{{status=\"401\"}} 1"))
                    && text.contains(&format!("{module}_http_request_duration_seconds")),
                "{module}: {text}"
            );
        }
        for module in RECORDS_ITS_OWN {
            assert!(
                !text.contains(&format!("{module}_http_")),
                "{module}: {text}"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_forged_or_missing_id_is_replaced() {
        for sent in [None, Some("a b level=error forged"), Some("short")] {
            let (response, log) = call("/open", sent).await;
            let id = echoed(&response).to_owned();
            assert!(
                id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()),
                "{id}"
            );
            assert!(log.contains(&format!("request_id={id}")), "{log}");
            assert!(!log.contains("forged"), "{log}");
        }
    }
}
