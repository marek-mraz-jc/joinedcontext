//! The registry hosts' certificate wave through the API server and the database
//! (evidence/T-7129/design.md §3 and §7, T-7138): an approved host waits for its DNS; a `dns-ok`
//! host gets an ACME-only Ingress and a Certificate and is not served; an issued one is widened
//! and served; an object somebody else made is never touched and the host says `conflict`; the
//! eleventh certificate a day and the 21st a week for one registered domain wait; a removed host
//! loses its Ingress, then its Certificate, and only once the edge file was written.
//!
//! The store is the database `JC_PORTAL_TEST_DATABASE_URL` names (CI's portal-test jobs set it).
//! The tests run one at a time: the wave reads every registry row and the pacing ledger is the
//! instance's.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::Arc;

use serde_json::{json, Value};
use sqlx::PgPool;
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::apps::kube::KubeClient;
use joinedcontext_portal::domain_verification::Unanswered;
use joinedcontext_portal::reconciler::org_hosts::{object_name, Addresses, OrgHosts, ACME_PATH};

const NS: &str = "apisix";
const CERTS: &str = "/apis/cert-manager.io/v1/namespaces/apisix/certificates";
const INGRESSES: &str = "/apis/networking.k8s.io/v1/namespaces/apisix/ingresses";
const EDGE_IP: &str = "203.0.113.10";

static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn pool() -> PgPool {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate")
}

fn run(test: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("{test}{:x}", std::process::id() ^ nanos)
}

/// Canned DNS: the edge's host and each name given resolve to their addresses.
struct Dns(BTreeMap<String, Vec<IpAddr>>);

impl Addresses for Dns {
    fn addresses<'a>(
        &'a self,
        name: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<BTreeSet<IpAddr>, Unanswered>> {
        let found = self.0.get(name).cloned().unwrap_or_default();
        Box::pin(async move { Ok(found.into_iter().collect()) })
    }
}

fn dns(pointed: &[(&str, &str)]) -> Arc<Dns> {
    let mut map: BTreeMap<String, Vec<IpAddr>> = pointed
        .iter()
        .map(|(name, ip)| ((*name).to_owned(), vec![ip.parse().expect("an address")]))
        .collect();
    map.insert(
        "edge.test".into(),
        vec![EDGE_IP.parse().expect("an address")],
    );
    Arc::new(Dns(map))
}

fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({ "kind": "Status", "code": 404 }))
}

/// An API server holding the edge's Ingress and Certificate; creates of the objects of `hosts`
/// succeed, any other object is not there. Scoped to the test's own hosts: the wave walks every
/// registry row, other test files' rows of the shared database included.
async fn api(hosts: &[&str]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{INGRESSES}/apisix")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadata": { "name": "apisix", "annotations": { "traefik.ingress.kubernetes.io/router.tls": "true" } },
            "spec": {
                "ingressClassName": "traefik",
                "rules": [{ "host": "edge.test", "http": { "paths": [{
                    "path": "/", "pathType": "Prefix",
                    "backend": { "service": { "name": "apisix-gateway", "port": { "number": 80 } } },
                }] } }],
                "tls": [{ "hosts": ["edge.test"], "secretName": "apisix-edge-tls" }],
            },
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{CERTS}/apisix-edge")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "spec": { "issuerRef": { "name": "letsencrypt-prod", "kind": "ClusterIssuer" } },
        })))
        .mount(&server)
        .await;
    for collection in [CERTS, INGRESSES] {
        for host in hosts {
            Mock::given(method("POST"))
                .and(path(collection))
                .and(body_string_contains(object_name(host)))
                .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
                .mount(&server)
                .await;
        }
    }
    server
}

/// Mounts `object` as what a GET of its name answers.
async fn holds(server: &MockServer, object: Value) {
    let collection = if object["kind"] == "Certificate" {
        CERTS
    } else {
        INGRESSES
    };
    let name = object["metadata"]["name"]
        .as_str()
        .expect("a name")
        .to_owned();
    Mock::given(method("GET"))
        .and(path(format!("{collection}/{name}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(object))
        .with_priority(1)
        .mount(server)
        .await;
}

async fn fallback(server: &MockServer) {
    Mock::given(method("GET"))
        .respond_with(not_found())
        .with_priority(10)
        .mount(server)
        .await;
}

fn labelled(kind: &str, host: &str) -> Value {
    json!({
        "apiVersion": if kind == "Certificate" { "cert-manager.io/v1" } else { "networking.k8s.io/v1" },
        "kind": kind,
        "metadata": {
            "name": object_name(host),
            "namespace": NS,
            "labels": {
                "app.kubernetes.io/managed-by": "joinedcontext-portal",
                "app.kubernetes.io/component": "org-host",
            },
        },
    })
}

fn wave(server: &MockServer, pool: &PgPool, dns: Arc<Dns>) -> OrgHosts {
    OrgHosts::new(
        KubeClient::with_token(&server.uri(), "token").expect("a client"),
        NS.to_owned(),
        pool.clone(),
        dns,
    )
}

async fn row(pool: &PgPool, host: &str, state: &str) {
    sqlx::query(
        "INSERT INTO organization_hosts (host, organization, role, state, created_by, approved_by) \
         VALUES ($1, 'bbsk', 'working', $2, 'operator-a', 'operator-b')",
    )
    .bind(host)
    .bind(state)
    .execute(pool)
    .await
    .expect("a registry row");
}

async fn state(pool: &PgPool, host: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT state, detail FROM organization_hosts WHERE host = $1")
        .bind(host)
        .fetch_one(pool)
        .await
        .expect("the row")
}

/// Marks the rows removed so the next test's wave does not walk them.
async fn done(pool: &PgPool, hosts: &[&str]) {
    for host in hosts {
        joinedcontext_portal::host_registry::remove(pool, host)
            .await
            .expect("removed");
    }
}

async fn audited(pool: &PgPool, host: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT action FROM operator_audit WHERE actor = 'reconciler' AND after->>'host' = $1 ORDER BY id",
    )
    .bind(host)
    .fetch_all(pool)
    .await
    .expect("the audit")
}

/// Whether request `r` is about the objects of one of `hosts`.
fn about(r: &wiremock::Request, hosts: &[&str]) -> bool {
    hosts.iter().any(|host| {
        let name = object_name(host);
        r.url.path().contains(&name) || String::from_utf8_lossy(&r.body).contains(&name)
    })
}

async fn posted(server: &MockServer, collection: &str, hosts: &[&str]) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == collection && about(r, hosts))
        .map(|r| serde_json::from_slice(&r.body).expect("a JSON body"))
        .collect()
}

async fn writes(server: &MockServer, hosts: &[&str]) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() != "GET" && about(r, hosts))
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect()
}

async fn clear_ledger(pool: &PgPool) {
    sqlx::query("DELETE FROM organization_host_certificates")
        .execute(pool)
        .await
        .expect("ledger cleared");
}

/// §7 O3 "Not served before its certificate": a host whose DNS reaches the edge is `dns-ok`, its
/// Ingress carries the ACME path alone and no TLS, its Certificate is requested and counted, and
/// it is not `served` (jc's table holds served rows only: `421`). One audit row per step.
#[tokio::test]
async fn a_host_whose_dns_reaches_the_edge_gets_an_acme_ingress_and_a_certificate_and_waits() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    clear_ledger(&pool).await;
    let host = format!("{}.bbsk.sk", run("acme"));
    row(&pool, &host, "approved").await;
    let server = api(&[&host]).await;
    fallback(&server).await;

    wave(&server, &pool, dns(&[(&host, EDGE_IP)]))
        .converge(false)
        .await;

    let (now, detail) = state(&pool, &host).await;
    assert_eq!(now, "dns-ok");
    assert_eq!(detail.as_deref(), Some("the certificate is requested"));
    let ingresses = posted(&server, INGRESSES, &[&host]).await;
    assert_eq!(ingresses.len(), 1);
    assert_eq!(ingresses[0]["metadata"]["name"], object_name(&host));
    assert!(
        ingresses[0]["spec"].get("tls").is_none(),
        "no TLS before the certificate"
    );
    assert_eq!(
        ingresses[0]["spec"]["rules"][0]["http"]["paths"],
        json!([{ "path": ACME_PATH, "pathType": "Prefix",
                 "backend": { "service": { "name": "apisix-gateway", "port": { "number": 80 } } } }])
    );
    let certificates = posted(&server, CERTS, &[&host]).await;
    assert_eq!(certificates.len(), 1);
    assert_eq!(certificates[0]["spec"]["dnsNames"], json!([host]));
    assert_eq!(
        certificates[0]["spec"]["secretName"],
        format!("{}-tls", object_name(&host))
    );
    assert_eq!(
        audited(&pool, &host).await,
        ["host.dns-ok", "ingress.created", "certificate.created"]
    );
    let counted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM organization_host_certificates WHERE host = $1 AND registered_domain = 'bbsk.sk'",
    )
    .bind(&host)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(counted, 1);
    done(&pool, &[&host]).await;
}

#[tokio::test]
async fn a_host_pointing_elsewhere_waits_for_its_dns_and_nothing_is_created() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    let host = format!("{}.bbsk.sk", run("dns"));
    let unset = format!("{}.bbsk.sk", run("unset"));
    row(&pool, &host, "approved").await;
    row(&pool, &unset, "approved").await;
    let server = api(&[&host, &unset]).await;
    fallback(&server).await;

    wave(&server, &pool, dns(&[(&host, "198.51.100.9")]))
        .converge(false)
        .await;

    let (now, detail) = state(&pool, &host).await;
    assert_eq!(now, "approved");
    assert_eq!(
        detail.as_deref(),
        Some(
            format!("waiting for DNS: {host} points elsewhere; make it a CNAME to edge.test")
                .as_str()
        )
    );
    let (_, detail) = state(&pool, &unset).await;
    assert!(
        detail
            .as_deref()
            .is_some_and(|d| d.contains("has no record yet")),
        "{detail:?}"
    );
    assert!(writes(&server, &[&host, &unset]).await.is_empty());
    done(&pool, &[&host, &unset]).await;
}

/// An issued certificate: the ACME-only Ingress is replaced by the one on `/` with its TLS
/// Secret (a delete and a create, the Role grants no update) and the host is `served`.
#[tokio::test]
async fn an_issued_certificate_widens_the_ingress_and_serves_the_host() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    let host = format!("{}.bbsk.sk", run("issued"));
    row(&pool, &host, "dns-ok").await;
    let server = api(&[&host]).await;
    let mut cert = labelled("Certificate", &host);
    cert["status"] = json!({ "conditions": [{ "type": "Ready", "status": "True" }] });
    holds(&server, cert).await;
    let mut acme = labelled("Ingress", &host);
    acme["spec"] =
        json!({ "rules": [{ "host": host, "http": { "paths": [{ "path": ACME_PATH }] } }] });
    holds(&server, acme).await;
    Mock::given(method("DELETE"))
        .and(path(format!("{INGRESSES}/{}", object_name(&host))))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    fallback(&server).await;

    wave(&server, &pool, dns(&[])).converge(false).await;

    assert_eq!(state(&pool, &host).await, ("served".to_owned(), None));
    let name = object_name(&host);
    assert_eq!(
        writes(&server, &[&host]).await,
        [
            format!("DELETE {INGRESSES}/{name}"),
            format!("POST {INGRESSES}")
        ]
    );
    let widened = &posted(&server, INGRESSES, &[&host]).await[0];
    assert_eq!(
        widened["spec"]["tls"],
        json!([{ "hosts": [host], "secretName": format!("{name}-tls") }])
    );
    assert_eq!(widened["spec"]["rules"][0]["http"]["paths"][0]["path"], "/");
    assert_eq!(
        audited(&pool, &host).await,
        [
            "certificate.issued",
            "host.cert-issued",
            "ingress.widened",
            "host.served"
        ]
    );
    done(&pool, &[&host]).await;
}

/// §7 O3 "Foreign objects untouched": an Ingress of that name without the Portal's labels is
/// not changed, no Certificate is ordered for it, and the host shows `conflict`.
#[tokio::test]
async fn an_object_the_portal_did_not_label_is_untouched_and_the_host_shows_conflict() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    let host = format!("{}.bbsk.sk", run("conflict"));
    row(&pool, &host, "dns-ok").await;
    let server = api(&[&host]).await;
    holds(
        &server,
        json!({ "kind": "Ingress", "metadata": { "name": object_name(&host), "labels": { "app.kubernetes.io/managed-by": "Helm" } } }),
    )
    .await;
    fallback(&server).await;

    wave(&server, &pool, dns(&[])).converge(true).await;

    let (now, detail) = state(&pool, &host).await;
    assert_eq!(now, "dns-ok");
    assert!(
        detail
            .as_deref()
            .is_some_and(|d| d.starts_with("conflict: an Ingress")),
        "{detail:?}"
    );
    assert!(
        writes(&server, &[&host]).await.is_empty(),
        "nothing written or deleted"
    );
    done(&pool, &[&host]).await;
}

/// No edge Ingress to copy: every moving host stays where it is and says why.
#[tokio::test]
async fn without_the_edge_ingress_each_moving_host_says_why() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    let host = format!("{}.bbsk.sk", run("noedge"));
    row(&pool, &host, "approved").await;
    let server = MockServer::start().await;
    fallback(&server).await;

    wave(&server, &pool, dns(&[(&host, EDGE_IP)]))
        .converge(false)
        .await;

    assert_eq!(
        state(&pool, &host).await,
        (
            "approved".to_owned(),
            Some(
                "the edge Ingress apisix is not in apisix; the apisix chart renders it".to_owned()
            )
        )
    );
    assert!(writes(&server, &[&host]).await.is_empty());
    done(&pool, &[&host]).await;
}

/// A lookup that removes the host on the way, as an operator would mid-pass.
struct RemovedWhileResolving {
    pool: PgPool,
    host: String,
}

impl Addresses for RemovedWhileResolving {
    fn addresses<'a>(
        &'a self,
        name: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<BTreeSet<IpAddr>, Unanswered>> {
        Box::pin(async move {
            if name == self.host {
                joinedcontext_portal::host_registry::remove(&self.pool, name)
                    .await
                    .expect("removed");
            }
            Ok([EDGE_IP.parse().expect("an address")].into())
        })
    }
}

/// A host an operator removed while the pass looked it up is not moved on: its state stays
/// `removed`, nothing is created and no step is audited for it.
#[tokio::test]
async fn a_host_removed_mid_pass_is_not_moved_on() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    let host = format!("{}.bbsk.sk", run("gone"));
    row(&pool, &host, "approved").await;
    let server = api(&[&host]).await;
    fallback(&server).await;
    let dns = Arc::new(RemovedWhileResolving {
        pool: pool.clone(),
        host: host.clone(),
    });
    OrgHosts::new(
        KubeClient::with_token(&server.uri(), "token").expect("a client"),
        NS.to_owned(),
        pool.clone(),
        dns,
    )
    .converge(false)
    .await;

    assert_eq!(state(&pool, &host).await.0, "removed");
    assert!(writes(&server, &[&host]).await.is_empty());
    assert!(audited(&pool, &host).await.is_empty());
}

/// What `run` logs.
async fn logged<F: std::future::Future>(run: F) -> (F::Output, String) {
    #[derive(Clone, Default)]
    struct Sink(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let sink = Sink::default();
    let writer = sink.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    let out = run.await;
    drop(guard);
    let text =
        String::from_utf8_lossy(&sink.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
    (out, text)
}

/// A ledger that reads but refuses writes (a read-only session): the request waits, nothing is
/// counted and no Certificate is created, so the pacing never undercounts.
#[tokio::test]
async fn a_ledger_that_cannot_be_written_requests_no_certificate() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    clear_ledger(&pool).await;
    let host = format!("{}.bbsk.sk", run("readonly"));
    row(&pool, &host, "dns-ok").await;
    let server = api(&[&host]).await;
    holds(&server, labelled("Ingress", &host)).await;
    fallback(&server).await;
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL").expect("the test database");
    let options: sqlx::postgres::PgConnectOptions = url.parse().expect("a database URL");
    let read_only = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.options([("default_transaction_read_only", "on")]))
        .await
        .expect("a read-only pool");
    let hosts = OrgHosts::new(
        KubeClient::with_token(&server.uri(), "token").expect("a client"),
        NS.to_owned(),
        read_only,
        dns(&[]),
    );

    let ((), log) = logged(hosts.converge(false)).await;

    assert!(
        log.contains("the certificate ledger could not be written"),
        "{log}"
    );
    assert!(log.contains("a host's detail was not written"), "{log}");
    assert!(
        posted(&server, CERTS, &[&host]).await.is_empty(),
        "no certificate without its slot"
    );
    let counted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM organization_host_certificates WHERE host = $1")
            .bind(&host)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(counted, 0);
    assert_eq!(state(&pool, &host).await, ("dns-ok".to_owned(), None));
    done(&pool, &[&host]).await;
}

/// A refused request whose slot cannot be given back keeps the slot (errs on Let's Encrypt's
/// side), logs it, and the host says what the API server answered.
#[tokio::test]
async fn a_slot_that_cannot_be_given_back_stays_taken() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    clear_ledger(&pool).await;
    let host = format!("{}.bbsk.sk", run("keep"));
    row(&pool, &host, "dns-ok").await;
    let server = api(&[]).await;
    holds(&server, labelled("Ingress", &host)).await;
    Mock::given(method("POST"))
        .and(path(CERTS))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_json(json!({ "message": "admission webhook down" })),
        )
        .mount(&server)
        .await;
    fallback(&server).await;
    // Only this host's slot refuses its delete, so no other test's ledger is touched.
    let guard = format!("org_hosts_keep_{}", run("g").replace('.', "_"));
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE FUNCTION {guard}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
         IF OLD.host = '{host}' THEN RAISE EXCEPTION 'slot delete refused'; END IF; RETURN OLD; END $$"
    )))
    .execute(&pool)
    .await
    .expect("the guard function");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE TRIGGER {guard} BEFORE DELETE ON organization_host_certificates FOR EACH ROW EXECUTE FUNCTION {guard}()"
    )))
    .execute(&pool)
    .await
    .expect("the guard trigger");

    let ((), log) = logged(wave(&server, &pool, dns(&[])).converge(false)).await;

    for drop in [
        format!("DROP TRIGGER {guard} ON organization_host_certificates"),
        format!("DROP FUNCTION {guard}()"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(drop))
            .execute(&pool)
            .await
            .expect("the guard dropped");
    }
    assert!(
        log.contains("a failed request keeps its certificate slot"),
        "{log}"
    );
    let counted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM organization_host_certificates WHERE host = $1")
            .bind(&host)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(counted, 1, "the slot stays taken");
    let (now, detail) = state(&pool, &host).await;
    assert_eq!(now, "dns-ok");
    assert_eq!(
        detail.as_deref(),
        Some(
            format!(
                "the API server answered 500 for Certificate {}",
                object_name(&host)
            )
            .as_str()
        )
    );
    clear_ledger(&pool).await;
    done(&pool, &[&host]).await;
}

/// Each reconciler step's audit row is also its log line, naming the actor and the step.
#[tokio::test]
async fn an_audited_step_is_logged_with_its_actor() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    clear_ledger(&pool).await;
    let host = format!("{}.bbsk.sk", run("logged"));
    row(&pool, &host, "approved").await;
    let server = api(&[&host]).await;
    fallback(&server).await;
    let hosts = wave(&server, &pool, dns(&[(&host, EDGE_IP)]));

    let ((), log) = logged(hosts.converge(false)).await;

    let line = log
        .lines()
        .find(|line| {
            line.contains("operator audit") && line.contains("host.dns-ok") && line.contains(&host)
        })
        .unwrap_or_else(|| panic!("no audit line for {host}: {log}"));
    assert!(line.contains("actor=\"reconciler\""), "{line}");
    done(&pool, &[&host]).await;
}

/// §7 O3 "Pacing": the eleventh new certificate in 24 hours waits, and so does the 21st in 7
/// days for one registered domain; each host says until when.
#[tokio::test]
async fn the_eleventh_certificate_a_day_and_the_twenty_first_a_week_wait() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    clear_ledger(&pool).await;
    for n in 0..10 {
        sqlx::query(
            "INSERT INTO organization_host_certificates (host, registered_domain, requested_at) \
             VALUES ($1, 'other.sk', now() - make_interval(hours => $2))",
        )
        .bind(format!("h{n}.other.sk"))
        .bind(n)
        .execute(&pool)
        .await
        .expect("a ledger row");
    }
    let host = format!("{}.bbsk.sk", run("day"));
    row(&pool, &host, "dns-ok").await;
    let server = api(&[&host]).await;
    holds(&server, labelled("Ingress", &host)).await;
    fallback(&server).await;

    wave(&server, &pool, dns(&[])).converge(false).await;

    let (_, detail) = state(&pool, &host).await;
    assert!(
        detail
            .as_deref()
            .is_some_and(|d| d.starts_with("waiting for a certificate slot until ")),
        "{detail:?}"
    );
    assert!(
        posted(&server, CERTS, &[&host]).await.is_empty(),
        "the eleventh waits"
    );
    done(&pool, &[&host]).await;

    clear_ledger(&pool).await;
    for n in 0..20 {
        sqlx::query(
            "INSERT INTO organization_host_certificates (host, registered_domain, requested_at) \
             VALUES ($1, 'bbsk.sk', now() - make_interval(hours => $2))",
        )
        .bind(format!("w{n}.bbsk.sk"))
        .bind(30 + n * 6)
        .execute(&pool)
        .await
        .expect("a ledger row");
    }
    let week = format!("{}.bbsk.sk", run("week"));
    let other = format!("{}.hel.fi", run("week"));
    row(&pool, &week, "dns-ok").await;
    row(&pool, &other, "dns-ok").await;
    let server = api(&[&week, &other]).await;
    holds(&server, labelled("Ingress", &week)).await;
    holds(&server, labelled("Ingress", &other)).await;
    fallback(&server).await;

    wave(&server, &pool, dns(&[])).converge(false).await;

    let (_, detail) = state(&pool, &week).await;
    assert!(
        detail
            .as_deref()
            .is_some_and(|d| d.starts_with("waiting for a certificate slot until ")),
        "{detail:?}"
    );
    let certificates = posted(&server, CERTS, &[&week, &other]).await;
    assert_eq!(
        certificates.len(),
        1,
        "another registered domain still has slots"
    );
    assert_eq!(certificates[0]["spec"]["dnsNames"], json!([other]));
    done(&pool, &[&week, &other]).await;
    clear_ledger(&pool).await;
}

/// Delete order: the row is `removed` first (jc answers `421`), the edge file drops its route,
/// then the wave deletes the Ingress and after it the Certificate; never before the edge file
/// was written this pass, and never an object it did not label.
/// A Certificate request Kubernetes refuses gives its slot back (T-7138 bounce row 6): the
/// ledger counts a request before it is sent, so a refusal must not keep a slot taken.
#[tokio::test]
async fn a_refused_certificate_request_gives_its_slot_back() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    clear_ledger(&pool).await;
    let host = format!("{}.bbsk.sk", run("refused"));
    row(&pool, &host, "dns-ok").await;
    let server = api(&[]).await;
    holds(&server, labelled("Ingress", &host)).await;
    Mock::given(method("POST"))
        .and(path(CERTS))
        .and(body_string_contains(object_name(&host)))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({ "message": "denied by admission" })),
        )
        .mount(&server)
        .await;
    fallback(&server).await;

    wave(&server, &pool, dns(&[])).converge(false).await;

    assert_eq!(
        posted(&server, CERTS, &[&host]).await.len(),
        1,
        "the request was sent"
    );
    let counted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM organization_host_certificates WHERE host = $1")
            .bind(&host)
            .fetch_one(&pool)
            .await
            .expect("a count");
    assert_eq!(counted, 0, "a refused request holds no slot");
    let (state, detail) = state(&pool, &host).await;
    assert_eq!(state, "dns-ok");
    assert!(
        detail.is_some_and(|d| d.contains("Certificate")),
        "the refusal is named"
    );
    assert!(!audited(&pool, &host)
        .await
        .contains(&"certificate.created".to_owned()));
    done(&pool, &[&host]).await;
}

/// The Certificate of a removed host stays while its Ingress does (T-7138 bounce row 6): an
/// Ingress whose delete failed would otherwise serve the host from a Secret nobody renews.
#[tokio::test]
async fn a_certificate_waits_while_its_ingress_could_not_be_deleted() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    let host = format!("{}.bbsk.sk", run("stuck"));
    row(&pool, &host, "served").await;
    done(&pool, &[&host]).await;
    let name = object_name(&host);
    let server = api(&[&host]).await;
    let selector =
        "app.kubernetes.io/managed-by=joinedcontext-portal,app.kubernetes.io/component=org-host";
    for (collection, kind, answer) in [(CERTS, "Certificate", 200), (INGRESSES, "Ingress", 500)] {
        Mock::given(method("GET"))
            .and(path(collection))
            .and(query_param("labelSelector", selector))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "items": [labelled(kind, &host)] })),
            )
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("{collection}/{name}")))
            .respond_with(ResponseTemplate::new(answer).set_body_json(json!({})))
            .mount(&server)
            .await;
    }
    fallback(&server).await;

    wave(&server, &pool, dns(&[])).converge(true).await;

    assert_eq!(
        writes(&server, &[&host]).await,
        [format!("DELETE {INGRESSES}/{name}")]
    );
    assert!(
        audited(&pool, &host).await.is_empty(),
        "nothing was deleted"
    );
}

#[tokio::test]
async fn a_removed_host_loses_its_ingress_then_its_certificate_after_the_edge_file() {
    let _one = ONE_AT_A_TIME.lock().await;
    let pool = pool().await;
    let host = format!("{}.bbsk.sk", run("removed"));
    row(&pool, &host, "served").await;
    done(&pool, &[&host]).await;
    let name = object_name(&host);
    let server = api(&[&host]).await;
    let selector =
        "app.kubernetes.io/managed-by=joinedcontext-portal,app.kubernetes.io/component=org-host";
    let mut unlabelled = labelled("Ingress", "kept.bbsk.sk");
    unlabelled["metadata"]["labels"] = json!({});
    for (collection, kind) in [(CERTS, "Certificate"), (INGRESSES, "Ingress")] {
        let mut items = vec![labelled(kind, &host)];
        if kind == "Ingress" {
            items.push(unlabelled.clone());
        }
        Mock::given(method("GET"))
            .and(path(collection))
            .and(query_param("labelSelector", selector))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": items })))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("{collection}/{name}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
    }
    fallback(&server).await;

    wave(&server, &pool, dns(&[])).converge(false).await;
    assert!(
        writes(&server, &[&host]).await.is_empty(),
        "the edge file was not written: nothing goes"
    );

    wave(&server, &pool, dns(&[])).converge(true).await;
    assert_eq!(
        writes(&server, &[&host]).await,
        [
            format!("DELETE {INGRESSES}/{name}"),
            format!("DELETE {CERTS}/{name}")
        ]
    );
    assert_eq!(
        audited(&pool, &host).await,
        ["ingress.deleted", "certificate.deleted"]
    );
}
