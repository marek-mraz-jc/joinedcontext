//! One request id per request across jc's modules, and one access line per call at a module's
//! door: the edge's job in the cluster (APISIX `request-id` plugin and access log), which jc
//! does without an edge.
//!
//! Each mount takes the caller's `X-Request-Id` when it is a plain token and mints one
//! otherwise, hands it on to the module in the request (so the Portal and the gateway, which
//! read the header, keep it for their own lines and the broker hop), runs the module inside a
//! `request` span that names the module and the id (every line the module writes for the
//! request carries both), answers with the id, and writes one `jc::access` line: method, path
//! without the query, status and milliseconds; for a refusal or a failure also the reason. Every
//! write (any method but GET and HEAD) and every 4xx/5xx keeps that INFO line. A GET or HEAD
//! answered 2xx/3xx writes its line at DEBUG and is counted into one INFO line per minute per
//! module and status ([`Tally`]): jc-prod's log rotated every 8 min at 50 lines/s, 81 % of them
//! answered reads (2026-11-21, T-7474). `RUST_LOG=info,jc::access=debug` brings the per-read
//! lines back for a debugging session.
//!
//! The Portal's health probe walks every surface of every Live Endpoint each round, one unique
//! path per call: its calls carry [`probe_mark`] (`x-probe-mark`), a secret of this process.
//! The door takes the mark off, counts the probe's answered calls into the minute line (kind
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
    jc_secret::constant_time_eq(sent.as_bytes(), probe_mark().as_bytes())
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
/// lines: a probe's answered calls are only counted ([`Tally`]).
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

/// Bytes of a request path an access line keeps: a caller's long path costs one short line.
const LOGGED_PATH: usize = 256;

/// `path` cut to [`LOGGED_PATH`] bytes on a character boundary.
fn logged(path: &str) -> &str {
    let mut end = path.len().min(LOGGED_PATH);
    while !path.is_char_boundary(end) {
        end -= 1;
    }
    &path[..end]
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
    let tally = Tally::of(module);
    router.layer(axum::middleware::from_fn(
        move |req: Request, next: Next| access(module, Door::Outer(Arc::clone(&tally)), req, next),
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
    Outer(Arc<Tally>),
    Hop,
}

/// How long one minute line counts.
const WINDOW: Duration = Duration::from_secs(60);

/// What a minute line counts: the door's answered reads, or the health probe's answered calls.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Kind {
    Read,
    Probe,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Probe => "probe",
        }
    }
}

/// The answered reads of one module's doors since the window began, by kind and status (at most
/// two kinds times the status codes axum can send: the table stays small).
struct Tally {
    module: &'static str,
    counts: Mutex<Window>,
}

/// When the window began, and its counts by kind and status.
type Window = (Instant, HashMap<(Kind, u16), u64>);

/// Each module's tally (a renamed mount shares its module's), for the minute ticker.
static TALLIES: Mutex<Vec<std::sync::Weak<Tally>>> = Mutex::new(Vec::new());

impl Tally {
    fn new(module: &'static str) -> Self {
        Self {
            module,
            counts: Mutex::new((Instant::now(), HashMap::new())),
        }
    }

    /// The tally of `module`, made and handed to the minute ticker on its first door.
    fn of(module: &'static str) -> Arc<Self> {
        let mut all = TALLIES.lock().unwrap_or_else(PoisonError::into_inner);
        all.retain(|t| t.strong_count() > 0);
        if let Some(held) = all
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .find(|t| t.module == module)
        {
            return held;
        }
        let tally = Arc::new(Self::new(module));
        all.push(Arc::downgrade(&tally));
        drop(all);
        tick();
        tally
    }

    /// One answered call of `kind` at `now`; a window older than [`WINDOW`] is written first.
    fn count(&self, kind: Kind, status: u16, now: Instant) {
        let mut held = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        if now.saturating_duration_since(held.0) >= WINDOW {
            self.write(&mut held, now);
        }
        *held.1.entry((kind, status)).or_default() += 1;
    }

    /// Writes the window's lines when it is [`WINDOW`] old (the ticker's call).
    fn flush(&self, now: Instant) {
        let mut held = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        if now.saturating_duration_since(held.0) >= WINDOW {
            self.write(&mut held, now);
        }
    }

    /// One INFO line per kind and status counted since the window began, and a new window. The
    /// line names the module, kind, status, count and seconds: never a path, query or caller.
    fn write(&self, held: &mut Window, now: Instant) {
        let seconds = now.saturating_duration_since(held.0).as_secs();
        let mut lines: Vec<_> = held.1.drain().collect();
        lines.sort_unstable();
        for ((kind, status), calls) in lines {
            let (module, kind) = (self.module, kind.name());
            tracing::info!(target: "jc::access", module, kind, status, calls, seconds, "answered in the last minute");
        }
        held.0 = now;
    }
}

/// Starts the one minute ticker on the runtime building the routers, so an idle door's last
/// minute is written too; without a runtime the next counted call writes it.
fn tick() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    STARTED.call_once(|| {
        runtime.spawn(async {
            let mut every = tokio::time::interval(WINDOW);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                let live: Vec<_> = TALLIES
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .iter()
                    .filter_map(std::sync::Weak::upgrade)
                    .collect();
                let now = Instant::now();
                for tally in live {
                    tally.flush(now);
                }
            }
        });
    });
}

/// GET and HEAD read; every other method writes and keeps its line.
fn reads(method: &Method) -> bool {
    method == Method::GET || method == Method::HEAD
}

async fn access(module: &'static str, door: Door, mut req: Request, next: Next) -> Response {
    let marked = take_mark(&mut req);
    let id = callers_id(&req).filter(|_| !marked).unwrap_or_else(fresh);
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
            let path = logged(&path);
            tracing::info!(target: "jc::access", module, %method, path, status = code, ms, reason, "refused");
        } else if let Door::Outer(tally) = &door {
            let path = logged(&path);
            let kind = match (probe, reads(&method)) {
                (true, _) => Some(Kind::Probe),
                (false, true) => Some(Kind::Read),
                (false, false) => None,
            };
            match kind {
                Some(kind) => {
                    tracing::debug!(target: "jc::access", module, %method, path, status = code, ms, "answered");
                    tally.count(kind, code, started);
                }
                None => {
                    tracing::info!(target: "jc::access", module, %method, path, status = code, ms, "answered");
                }
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

    /// A module that logs one line, refuses `/closed` and `/forbidden`, fails `/fail`, takes a
    /// POST at `/open` and answers a GET there with the id it was handed.
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
                })
                .post(|| async { StatusCode::CREATED }),
            )
            .route("/closed", get(|| async { StatusCode::UNAUTHORIZED }))
            .route("/forbidden", get(|| async { StatusCode::FORBIDDEN }))
            .route("/fail", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
    }

    async fn call(path: &str, id: Option<&str>) -> (Response, String) {
        call_at(layer("shako", module()), path, id).await
    }

    async fn call_at(app: Router, path: &str, id: Option<&str>) -> (Response, String) {
        call_with(app, Method::GET, path, id).await
    }

    /// The INFO lines `f` writes, as `init_tracing` keeps them by default.
    fn captured(f: impl FnOnce()) -> String {
        let buf = Buffer::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        String::from_utf8(buf.0.lock().expect("log").clone()).expect("utf8")
    }

    async fn call_with(
        app: Router,
        method: Method,
        path: &str,
        id: Option<&str>,
    ) -> (Response, String) {
        let buf = Buffer::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        let mut req = Request::builder()
            .method(method)
            .uri(format!("{path}?secret=q"));
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
        assert!(
            !log.contains("jc::access"),
            "an answered read is only counted: {log}"
        );
        assert!(
            !log.contains("secret=q"),
            "the query stays out of the log: {log}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_answered_read_writes_no_line_and_is_counted_in_the_next_minute_line() {
        let app = layer("t7474-reads", module());
        let mut log = String::new();
        for (path, id) in [
            ("/open", None),
            ("/open", None),
            ("/open", Some("smokelogs-traced-0001")),
        ] {
            log += &call_at(app.clone(), path, id).await.1;
        }
        let (_, head) = call_with(app.clone(), Method::HEAD, "/open", None).await;
        log += &head;
        assert!(!log.contains("jc::access"), "{log}");
        let tally = Tally::of("t7474-reads");
        let minute = captured(|| tally.flush(Instant::now() + WINDOW));
        let lines: Vec<_> = minute
            .lines()
            .filter(|l| l.contains("jc::access"))
            .collect();
        assert_eq!(lines.len(), 1, "{minute}");
        for want in [
            "module=\"t7474-reads\"",
            "kind=\"read\"",
            "status=200",
            "calls=4",
            "answered in the last minute",
        ] {
            assert!(lines[0].contains(want), "{want}: {minute}");
        }
        for leak in ["/open", "secret", "smokelogs"] {
            assert!(!minute.contains(leak), "{leak}: {minute}");
        }
        let again = captured(|| tally.flush(Instant::now() + WINDOW + WINDOW));
        assert!(!again.contains("jc::access"), "a new window: {again}");
    }

    /// deploy/rehearsal/up.sh's switch: the smoke's own reads keep their line, nobody else's.
    #[tokio::test(flavor = "current_thread")]
    async fn a_span_filter_brings_back_the_read_lines_of_one_request_id_prefix() {
        use tracing_subscriber::Layer;
        use tracing_subscriber::layer::SubscriberExt;
        let filter = "info,jc::access[request{request_id=smokelogs-.*}]=debug";
        let app = layer("t7474-switch", module());
        let mut log = String::new();
        for id in ["smokelogs-0123456789ab", "apisix-0123456789ab"] {
            let buf = Buffer::default();
            let writer = buf.clone();
            let fmt = tracing_subscriber::fmt::layer()
                .with_writer(move || writer.clone())
                .with_ansi(false);
            let subscriber = tracing_subscriber::registry()
                .with(fmt.with_filter(tracing_subscriber::EnvFilter::new(filter)));
            let req = Request::builder()
                .uri("/open")
                .header(HEADER, id)
                .body(Body::empty())
                .expect("request");
            let _guard = tracing::subscriber::set_default(subscriber);
            app.clone().oneshot(req).await.expect("answer");
            log += &String::from_utf8(buf.0.lock().expect("log").clone()).expect("utf8");
        }
        let access: Vec<_> = log.lines().filter(|l| l.contains("jc::access")).collect();
        assert_eq!(access.len(), 1, "{log}");
        assert!(
            access[0].contains("request_id=smokelogs-0123456789ab")
                && access[0].contains("status=200"),
            "{log}"
        );
    }

    #[test]
    fn a_minute_line_is_written_by_the_next_call_without_a_ticker_and_sums_by_status() {
        let tally = Tally::new("t7474-lazy");
        let t0 = Instant::now();
        let log = captured(|| {
            tally.count(Kind::Read, 200, t0);
            tally.count(Kind::Read, 200, t0 + Duration::from_secs(10));
            tally.count(Kind::Read, 304, t0 + Duration::from_secs(20));
            tally.count(Kind::Probe, 200, t0 + Duration::from_secs(30));
        });
        assert!(!log.contains("jc::access"), "{log}");
        let log = captured(|| tally.count(Kind::Read, 200, t0 + WINDOW + WINDOW));
        let lines: Vec<_> = log.lines().filter(|l| l.contains("jc::access")).collect();
        assert_eq!(lines.len(), 3, "{log}");
        assert!(
            lines[0].contains("kind=\"read\"")
                && lines[0].contains("status=200")
                && lines[0].contains("calls=2"),
            "{log}"
        );
        assert!(
            lines[1].contains("status=304") && lines[1].contains("calls=1"),
            "{log}"
        );
        assert!(
            lines[2].contains("kind=\"probe\"") && lines[2].contains("calls=1"),
            "{log}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_write_a_refusal_and_a_failure_each_keep_their_line() {
        let app = layer("t7474-writes", module());
        let (response, log) = call_with(app.clone(), Method::POST, "/open", None).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let lines: Vec<_> = log.lines().filter(|l| l.contains("jc::access")).collect();
        assert_eq!(lines.len(), 1, "{log}");
        assert!(
            lines[0].contains("method=POST")
                && lines[0].contains("status=201")
                && lines[0].contains("answered"),
            "{log}"
        );
        for (path, status, reason) in [
            ("/forbidden", "status=403", "reason=\"Forbidden\""),
            ("/fail", "status=500", "reason=\"Internal Server Error\""),
        ] {
            for _ in 0..2 {
                let (_, log) = call_at(app.clone(), path, None).await;
                let lines: Vec<_> = log.lines().filter(|l| l.contains("jc::access")).collect();
                assert_eq!(lines.len(), 1, "{log}");
                assert!(
                    lines[0].contains(status) && lines[0].contains(reason),
                    "{log}"
                );
            }
        }
        let minute = captured(|| Tally::of("t7474-writes").flush(Instant::now() + WINDOW));
        assert!(
            !minute.contains("jc::access"),
            "writes and refusals are not counted: {minute}"
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
    async fn a_long_path_is_cut_in_its_access_line() {
        let long = format!("/{}", "a".repeat(4000));
        let (response, log) = call(&long, Some("smokelogs-long-path-01")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let line = log
            .lines()
            .find(|l| l.contains("smokelogs-long-path-01") && l.contains("jc::access"))
            .expect("access line");
        assert!(
            line.contains(&format!("path=\"/{}\"", "a".repeat(255))),
            "{line}"
        );
        assert!(line.len() < 1024, "{line}");
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
        let subscriber = tracing_subscriber::registry().with(
            fmt.with_filter(quiet_probes())
                .with_filter(tracing_subscriber::filter::LevelFilter::INFO),
        );
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
    async fn a_probe_round_is_counted_and_keeps_refusals_and_warnings() {
        let app = layer("t7474-probe", probed());
        let mut log = String::new();
        for slug in ["a", "b", "c", "d", "e"] {
            let (body, line) = probe_call(app.clone(), &format!("/s/{slug}"), probe_mark()).await;
            assert_eq!(body, "clean", "the mark stops at the door");
            log += &line;
        }
        log += &probe_call(app.clone(), "/closed", probe_mark()).await.1;
        let count = |what: &str| log.lines().filter(|l| l.contains(what)).count();
        assert_eq!(count("answered"), 0, "{log}");
        assert_eq!(count("refused"), 1, "{log}");
        assert_eq!(count("forwarded"), 0, "{log}");
        assert_eq!(count("slow"), 5, "{log}");
        let minute = captured(|| Tally::of("t7474-probe").flush(Instant::now() + WINDOW));
        assert!(
            minute.contains("kind=\"probe\"") && minute.contains("calls=5"),
            "{minute}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_forged_mark_is_an_ordinary_call() {
        let app = layer("t7474-forged", probed());
        let mut log = String::new();
        for slug in ["a", "b"] {
            let (body, line) =
                probe_call(app.clone(), &format!("/s/{slug}"), "0123456789abcdef").await;
            assert_eq!(body, "clean");
            log += &line;
        }
        let count = |what: &str| log.lines().filter(|l| l.contains(what)).count();
        assert_eq!(count("forwarded"), 2, "{log}");
        let minute = captured(|| Tally::of("t7474-forged").flush(Instant::now() + WINDOW));
        assert!(
            minute.contains("kind=\"read\"") && minute.contains("calls=2"),
            "{minute}"
        );
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
