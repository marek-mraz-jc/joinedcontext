//! Every `MqttBroker`'s release against a mocked API server (T-3994, ADR-N-048 §2.6, MF-54).
//!
//! One converge applies the template once per broker, by its own `mqtt-` names; a changed limit
//! reaches the release; a refused manifest is not applied and its release is not retired; a
//! broker whose manifest is gone loses its objects and no other broker does; without the template
//! nothing is applied or removed.

use jc_core::kinds::mqtt_broker::release_name;
use jc_core::kinds::MqttBrokerSpec;
use joinedcontext_portal::apps::kube::KubeClient;
use joinedcontext_portal::reconciler::mqtt_brokers::{
    Broker, MqttBrokers, HOSTNAME_KEY, PORT_KEY, TEMPLATE_KEY,
};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const NAMESPACE: &str = "jc-mqtt";

const TEMPLATE: &str = "\
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: __RELEASE__
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/part-of: mqtt-brokers
    app.kubernetes.io/managed-by: joinedcontext-portal
    app.kubernetes.io/instance: __RELEASE__
data:
  rmqtt.toml: |
    listener.tcp.external.max_connections = __CONNECTIONS__
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: __RELEASE__
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/part-of: mqtt-brokers
    app.kubernetes.io/managed-by: joinedcontext-portal
    app.kubernetes.io/instance: __RELEASE__
spec:
  template:
    metadata:
      annotations:
        checksum/config: __CONFIG_HASH__
";

fn broker(project: &str, name: &str, connections: u32) -> Broker {
    let spec: MqttBrokerSpec = serde_json::from_value(json!({
        "roles": [{"name": "camera", "serviceAccounts": ["cam"], "publish": ["cameras/${clientid}/#"]}],
        "limits": {"maxConnections": connections},
    }))
    .expect("a broker reads");
    Broker {
        project: project.to_owned(),
        name: name.to_owned(),
        release: release_name(project, name),
        spec: Ok(spec),
        certificate: None,
        door: None,
    }
}

fn refused(project: &str, name: &str) -> Broker {
    Broker {
        spec: Err("role `camera` names ServiceAccount `cam-west`, which no manifest of this project declares (MF-54)".to_owned()),
        ..broker(project, name, 10)
    }
}

/// What the cluster holds: the objects of `releases`, labelled as the reconciler labels them.
fn held(kind: &str, releases: &[&str]) -> Value {
    json!({ "items": releases.iter().map(|release| json!({
        "kind": kind,
        "metadata": { "name": release, "labels": { "app.kubernetes.io/instance": release } },
    })).collect::<Vec<_>>() })
}

/// No broker holds an edge filter (PF-129) yet; the reconciler lists the kind to retire one.
async fn no_edge_filters(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(format!(
            "/apis/traefik.io/v1alpha1/namespaces/{NAMESPACE}/middlewaretcps"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [] })))
        .mount(server)
        .await;
}

async fn cluster(template: Option<&str>, releases: &[&str]) -> MockServer {
    let server = MockServer::start().await;
    let map = format!("/api/v1/namespaces/{NAMESPACE}/configmaps/broker-template");
    let response = match template {
        Some(text) => {
            ResponseTemplate::new(200).set_body_json(json!({ "data": { TEMPLATE_KEY: text } }))
        }
        None => ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })),
    };
    Mock::given(method("GET"))
        .and(path(map))
        .respond_with(response)
        .mount(&server)
        .await;
    for (route, kind) in [
        (
            format!("/api/v1/namespaces/{NAMESPACE}/configmaps"),
            "ConfigMap",
        ),
        (
            format!("/apis/apps/v1/namespaces/{NAMESPACE}/deployments"),
            "Deployment",
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(held(kind, releases)))
            .mount(&server)
            .await;
    }
    no_edge_filters(&server).await;
    for verb in ["PATCH", "DELETE"] {
        Mock::given(method(verb))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
    }
    server
}

fn calls(received: &[Request], verb: &str) -> Vec<String> {
    let mut paths: Vec<String> = received
        .iter()
        .filter(|request| request.method.as_str() == verb)
        .map(|request| request.url.path().to_owned())
        .collect();
    paths.sort();
    paths
}

fn reconciler(server: &MockServer) -> MqttBrokers {
    MqttBrokers::new(
        KubeClient::with_token(&server.uri(), "token").expect("a client"),
        NAMESPACE.to_owned(),
    )
}

#[tokio::test]
async fn three_brokers_in_two_projects_are_three_releases_and_a_gone_one_loses_only_its_own() {
    let server = cluster(
        Some(TEMPLATE),
        &["mqtt-helsinki-cameras", "mqtt-helsinki-gone"],
    )
    .await;
    let brokers = [
        broker("espoo", "cameras", 10),
        broker("helsinki", "cameras", 10),
        broker("helsinki", "doors", 10),
    ];
    let failed = reconciler(&server).converge(&brokers, true).await;
    assert!(failed.is_empty(), "{failed:?}");

    let received = server.received_requests().await.expect("recorded");
    let applied = calls(&received, "PATCH");
    assert_eq!(applied.len(), 6, "{applied:?}");
    for release in [
        "mqtt-espoo-cameras",
        "mqtt-helsinki-cameras",
        "mqtt-helsinki-doors",
    ] {
        assert!(applied.contains(&format!(
            "/api/v1/namespaces/{NAMESPACE}/configmaps/{release}"
        )));
        assert!(applied.contains(&format!(
            "/apis/apps/v1/namespaces/{NAMESPACE}/deployments/{release}"
        )));
    }
    assert_eq!(
        calls(&received, "DELETE"),
        [
            format!("/api/v1/namespaces/{NAMESPACE}/configmaps/mqtt-helsinki-gone"),
            format!("/apis/apps/v1/namespaces/{NAMESPACE}/deployments/mqtt-helsinki-gone"),
        ]
    );
    // The retirement asks only for what the reconciler made.
    let listed = received
        .iter()
        .find(|request| {
            request.method.as_str() == "GET" && request.url.path().ends_with("/deployments")
        })
        .expect("listed");
    assert!(listed
        .url
        .query()
        .unwrap_or_default()
        .contains("managed-by%3Djoinedcontext-portal"));
}

#[tokio::test]
async fn a_changed_limit_reaches_the_release_and_restarts_it() {
    let server = cluster(Some(TEMPLATE), &[]).await;
    let reconciler = reconciler(&server);
    assert!(reconciler
        .converge(&[broker("helsinki", "cameras", 10)], true)
        .await
        .is_empty());
    assert!(reconciler
        .converge(&[broker("helsinki", "cameras", 25)], true)
        .await
        .is_empty());
    let bodies: Vec<Value> = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .filter(|request| request.method.as_str() == "PATCH")
        .map(|request| serde_json::from_slice(&request.body).expect("JSON"))
        .collect();
    let toml = |i: usize| {
        bodies[i]["data"]["rmqtt.toml"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    assert!(
        toml(0).contains("= 10") && toml(2).contains("= 25"),
        "{bodies:?}"
    );
    let hash = |i: usize| {
        bodies[i]["spec"]["template"]["metadata"]["annotations"]["checksum/config"].clone()
    };
    assert_ne!(
        hash(1),
        hash(3),
        "the Deployment rolls when its config changes"
    );
}

#[tokio::test]
async fn a_refused_manifest_is_not_applied_and_keeps_its_running_release() {
    let server = cluster(
        Some(TEMPLATE),
        &["mqtt-helsinki-cameras", "mqtt-helsinki-doors"],
    )
    .await;
    let brokers = [
        broker("helsinki", "cameras", 10),
        refused("helsinki", "doors"),
    ];
    let failed = reconciler(&server).converge(&brokers, true).await;
    assert!(
        failed.is_empty(),
        "the refusal is the caller's to report: {failed:?}"
    );
    let received = server.received_requests().await.expect("recorded");
    assert!(calls(&received, "PATCH")
        .iter()
        .all(|p| !p.ends_with("mqtt-helsinki-doors")));
    assert_eq!(calls(&received, "DELETE"), Vec::<String>::new());
}

#[tokio::test]
async fn a_run_where_a_repository_did_not_stage_retires_nothing() {
    let server = cluster(Some(TEMPLATE), &["mqtt-helsinki-gone"]).await;
    let failed = reconciler(&server)
        .converge(&[broker("helsinki", "cameras", 10)], false)
        .await;
    assert!(failed.is_empty(), "{failed:?}");
    let received = server.received_requests().await.expect("recorded");
    assert_eq!(calls(&received, "DELETE"), Vec::<String>::new());
}

#[tokio::test]
async fn without_the_template_nothing_is_applied_or_removed_and_the_run_says_why() {
    let server = cluster(None, &["mqtt-helsinki-gone"]).await;
    let failed = reconciler(&server)
        .converge(&[broker("helsinki", "cameras", 10)], true)
        .await;
    assert!(failed["*"].contains("broker-template"), "{failed:?}");
    let received = server.received_requests().await.expect("recorded");
    assert!(calls(&received, "PATCH").is_empty());
    assert!(calls(&received, "DELETE").is_empty());
}

#[tokio::test]
async fn an_apply_the_api_server_refuses_is_that_brokers_failure_alone() {
    let server = MockServer::start().await;
    no_edge_filters(&server).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/namespaces/{NAMESPACE}/configmaps/broker-template"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": { TEMPLATE_KEY: TEMPLATE } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!(
            "/api/v1/namespaces/{NAMESPACE}/configmaps/mqtt-espoo-cameras"
        )))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({ "message": "forbidden" })))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&server)
        .await;
    let failed = reconciler(&server)
        .converge(
            &[
                broker("espoo", "cameras", 10),
                broker("helsinki", "cameras", 10),
            ],
            false,
        )
        .await;
    assert_eq!(failed.keys().collect::<Vec<_>>(), ["espoo/cameras"]);
    assert!(failed["espoo/cameras"].contains("forbidden"));
}

const ROUTE: &str = "\
---
apiVersion: traefik.io/v1alpha1
kind: IngressRouteTCP
metadata:
  name: __RELEASE__
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/instance: __RELEASE__
spec:
  entryPoints: [mqtts]
  routes:
    - match: HostSNI(`__NAME__.__PROJECT__.mqtt.example`)
";

/// The same release with a port: the 8883 route and the port's passthrough route, which carries
/// the Certificate's revision here as the Deployment does in the chart.
const PORT_TEMPLATE: &str = "\
---
apiVersion: traefik.io/v1alpha1
kind: IngressRouteTCP
metadata:
  name: __RELEASE__
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/instance: __RELEASE__
spec:
  entryPoints: [mqtts]
  routes:
    - match: HostSNI(`__NAME__.__PROJECT__.mqtt.example`)
---
apiVersion: traefik.io/v1alpha1
kind: IngressRouteTCP
metadata:
  name: __RELEASE__-port
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/instance: __RELEASE__
  annotations:
    joinedcontext.com/tls-revision: \"__TLS_REVISION__\"
spec:
  entryPoints: [mqtt-__PORT__]
  routes:
    - match: HostSNI(`*`)
  tls:
    passthrough: true
";

/// T-5856: a broker with a port gets the port template, its Certificate's revision filled in;
/// one whose manifest dropped its port loses what only a port renders and keeps its 8883 route;
/// a refused manifest keeps both.
#[tokio::test]
async fn a_port_is_routed_while_named_and_its_route_goes_when_the_manifest_drops_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/namespaces/{NAMESPACE}/configmaps/broker-template"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "data": { TEMPLATE_KEY: ROUTE, PORT_KEY: PORT_TEMPLATE } })),
        )
        .mount(&server)
        .await;
    let route = |name: &str, release: &str| {
        json!({ "kind": "IngressRouteTCP", "metadata": {
            "name": name, "labels": { "app.kubernetes.io/instance": release } } })
    };
    Mock::given(method("GET"))
        .and(path(format!(
            "/apis/traefik.io/v1alpha1/namespaces/{NAMESPACE}/ingressroutetcps"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [
            route("mqtt-espoo-cameras", "mqtt-espoo-cameras"),
            route("mqtt-espoo-cameras-port", "mqtt-espoo-cameras"),
            route("mqtt-helsinki-cameras", "mqtt-helsinki-cameras"),
            route("mqtt-helsinki-cameras-port", "mqtt-helsinki-cameras"),
            route("mqtt-helsinki-doors", "mqtt-helsinki-doors"),
            route("mqtt-helsinki-doors-port", "mqtt-helsinki-doors"),
        ] })))
        .mount(&server)
        .await;
    // A middleware the doors broker stamped while it carried allowedSources: gone with the list.
    Mock::given(method("GET"))
        .and(path(format!(
            "/apis/traefik.io/v1alpha1/namespaces/{NAMESPACE}/middlewaretcps"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [
            { "kind": "MiddlewareTCP", "metadata": { "name": "mqtt-helsinki-doors-sources",
                "labels": { "app.kubernetes.io/instance": "mqtt-helsinki-doors" } } },
        ] })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/apis/cert-manager.io/v1/namespaces/{NAMESPACE}/certificates"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [
            { "metadata": { "name": "mqtt-helsinki-cameras" }, "status": { "revision": 4 } },
        ] })))
        .mount(&server)
        .await;
    for verb in ["PATCH", "DELETE"] {
        Mock::given(method(verb))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
    }
    let mut cameras = broker("helsinki", "cameras", 10);
    if let Ok(spec) = cameras.spec.as_mut() {
        spec.port = Some(18835);
    }
    let brokers = [
        refused("espoo", "cameras"),
        cameras,
        broker("helsinki", "doors", 10),
    ];
    let failed = reconciler(&server).converge(&brokers, true).await;
    assert!(failed.is_empty(), "{failed:?}");

    let received = server.received_requests().await.expect("recorded");
    let routes = format!("/apis/traefik.io/v1alpha1/namespaces/{NAMESPACE}/ingressroutetcps");
    assert_eq!(
        calls(&received, "PATCH"),
        [
            format!("{routes}/mqtt-helsinki-cameras"),
            format!("{routes}/mqtt-helsinki-cameras-port"),
            format!("{routes}/mqtt-helsinki-doors"),
            // Every broker's per-address cap (T-5888).
            format!("/apis/traefik.io/v1alpha1/namespaces/{NAMESPACE}/middlewaretcps/mqtt-helsinki-cameras-connections"),
            format!("/apis/traefik.io/v1alpha1/namespaces/{NAMESPACE}/middlewaretcps/mqtt-helsinki-doors-connections"),
        ]
    );
    let port = received
        .iter()
        .find(|r| r.method.as_str() == "PATCH" && r.url.path().ends_with("-port"))
        .expect("the port route applied");
    let body: Value = serde_json::from_slice(&port.body).expect("json");
    assert_eq!(body["spec"]["entryPoints"], json!(["mqtt-18835"]));
    assert_eq!(body["spec"]["tls"], json!({ "passthrough": true }));
    assert_eq!(
        body["metadata"]["annotations"]["joinedcontext.com/tls-revision"],
        "4"
    );
    let certificates = received
        .iter()
        .filter(|r| r.url.path().ends_with("/certificates"))
        .map(|r| r.url.query().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        certificates.len(),
        1,
        "read for the broker with a port only"
    );
    assert!(
        certificates[0].contains("mqtt-helsinki-cameras"),
        "{certificates:?}"
    );
    assert_eq!(
        calls(&received, "DELETE"),
        [
            format!("{routes}/mqtt-helsinki-doors-port"),
            format!("/apis/traefik.io/v1alpha1/namespaces/{NAMESPACE}/middlewaretcps/mqtt-helsinki-doors-sources"),
        ]
    );
}

const HOSTNAME_TEMPLATE: &str = "\
---
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: __RELEASE__
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/instance: __RELEASE__
spec:
  secretName: __RELEASE__-tls
  dnsNames: [__HOSTNAME__]
---
apiVersion: traefik.io/v1alpha1
kind: IngressRouteTCP
metadata:
  name: __RELEASE__
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/instance: __RELEASE__
spec:
  entryPoints: [mqtts]
  routes:
    - match: HostSNI(`__HOSTNAME__`)
  tls:
    secretName: __TLS_SECRET__
";

/// T-5874: a custom hostname is routed by its name with a Certificate issued by HTTP-01, and waits
/// for DNS until that Certificate is Ready; with the admin's certificate the route serves it and
/// the issued Certificate goes.
#[tokio::test]
async fn a_custom_hostname_is_routed_by_its_name_and_waits_for_dns_until_issued() {
    let server = MockServer::start().await;
    no_edge_filters(&server).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/namespaces/{NAMESPACE}/configmaps/broker-template"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "data": { TEMPLATE_KEY: ROUTE, HOSTNAME_KEY: HOSTNAME_TEMPLATE } }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/apis/cert-manager.io/v1/namespaces/{NAMESPACE}/certificates"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [
            { "metadata": { "name": "mqtt-helsinki-cameras",
                "labels": { "app.kubernetes.io/instance": "mqtt-helsinki-cameras" } },
              "status": { "conditions": [{ "type": "Ready", "status": "False" }] } },
            { "metadata": { "name": "mqtt-helsinki-doors",
                "labels": { "app.kubernetes.io/instance": "mqtt-helsinki-doors" } },
              "status": { "conditions": [{ "type": "Ready", "status": "True" }] } },
        ] })))
        .mount(&server)
        .await;
    for verb in ["GET", "PATCH", "DELETE"] {
        Mock::given(method(verb))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [] })))
            .mount(&server)
            .await;
    }
    let on = |project: &str, name: &str, hostname: &str, certificate: Option<&str>| {
        let mut broker = broker(project, name, 10);
        if let Ok(spec) = broker.spec.as_mut() {
            spec.hostname = Some(hostname.to_owned());
        }
        broker.certificate = certificate.map(str::to_owned);
        broker
    };
    let brokers = [
        on("helsinki", "cameras", "mqtt.helsinki.example", None),
        on(
            "helsinki",
            "doors",
            "doors.helsinki.example",
            Some("helsinki-doors-cert"),
        ),
    ];
    let reconciler = reconciler(&server);
    let failed = reconciler.converge(&brokers, true).await;
    assert!(failed.is_empty(), "{failed:?}");

    let received = server.received_requests().await.expect("recorded");
    let patched = |suffix: &str| -> Value {
        let request = received
            .iter()
            .find(|r| r.method.as_str() == "PATCH" && r.url.path().ends_with(suffix))
            .unwrap_or_else(|| panic!("{suffix} applied"));
        serde_json::from_slice(&request.body).expect("json")
    };
    let certificate = patched("/certificates/mqtt-helsinki-cameras");
    assert_eq!(
        certificate["spec"]["dnsNames"],
        json!(["mqtt.helsinki.example"])
    );
    let route = patched("/ingressroutetcps/mqtt-helsinki-cameras");
    assert_eq!(
        route["spec"]["routes"][0]["match"],
        "HostSNI(`mqtt.helsinki.example`)"
    );
    assert_eq!(
        route["spec"]["tls"]["secretName"],
        "mqtt-helsinki-cameras-tls"
    );
    let route = patched("/ingressroutetcps/mqtt-helsinki-doors");
    assert_eq!(route["spec"]["tls"]["secretName"], "helsinki-doors-cert");
    assert!(
        !calls(&received, "PATCH").contains(&format!(
            "/apis/cert-manager.io/v1/namespaces/{NAMESPACE}/certificates/mqtt-helsinki-doors"
        )),
        "the admin's certificate is served, none issued"
    );
    assert!(
        calls(&received, "DELETE").contains(&format!(
            "/apis/cert-manager.io/v1/namespaces/{NAMESPACE}/certificates/mqtt-helsinki-doors"
        )),
        "the issued Certificate the admin's replaced goes"
    );

    let waiting = reconciler.waiting_for_dns(&brokers).await;
    assert_eq!(
        waiting.keys().collect::<Vec<_>>(),
        ["helsinki/cameras"],
        "the admin's certificate waits for nothing"
    );
    assert!(waiting["helsinki/cameras"]
        .starts_with("waiting for DNS: point a CNAME from mqtt.helsinki.example"));
}
