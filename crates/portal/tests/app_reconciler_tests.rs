//! What an `App` manifest compiles into (T-0227, T-0508, AP-04, AP-05, AP-13, AP-18, AP-25…AP-28,
//! ADR-N-019).
//!
//! Two properties carry the security of this kind and both are asserted here: the app container
//! is the pod's only container and has no way in except from the APISIX edge, which is the login
//! front; and the rendered grants say exactly what the app declared it needs and nothing more.

use jc_core::kinds::{EndpointSlug, EndpointSpec, PolicySpec};
use jcctl::loader::RawManifest;
use joinedcontext_portal::apps::reconciler::{
    generate_slug, render, RenderError, Settings, APP_LABEL, APP_PORT,
};
use serde_json::{json, Value};

const APP_IMAGE: &str =
    "ghcr.io/bb/apps/air-quality@sha256:1111111111111111111111111111111111111111111111111111111111111111";

fn settings() -> Settings {
    Settings {
        host: "bb.example.com".into(),
        apex: "bb.example.com".into(),
        gateway_url: Some("http://context-gateway.jc.svc.cluster.local:8080".into()),
        service_url: Some("http://portal.joinedcontext.svc.cluster.local:8080".into()),
        namespace: "joinedcontext".into(),
        org_domain: "banskabystrica.sk".into(),
        apisix_namespace: "apisix".into(),
        image_repository: None,
        pull_secret: None,
        basemap_base: None,
        release: Some("dev".into()),
        service_account: Some("portal".into()),
    }
}

/// The manifest of the architecture chapter's own example, trimmed to what rendering reads.
fn app(overrides: Value) -> RawManifest {
    let mut spec = json!({
        "kind": "ui-rust",
        "source": { "path": "./src" },
        "build": { "rust": "1.90", "node": "22" },
        "visibility": "project",
        "lifecycle": "published",
        "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
            "types": ["AirQualityObserved", "District"],
            "attrs": ["pm10", "pm25", "location", "refDistrict"],
            "operations": ["queryEntity", "retrieveEntity"],
            "representations": ["ngsi-ld", "ogc-features", "csv"]
        }],
        "limits": { "requestsPerMinute": 600, "maxFileRows": 20000 }
    });
    for (key, value) in overrides.as_object().expect("an object of overrides") {
        spec[key] = value.clone();
    }

    serde_json::from_value(json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "App",
        "metadata": { "name": "air-quality-today", "namespace": "ovzdusie" },
        "spec": spec,
    }))
    .expect("the fixture is a manifest")
}

fn containers(deployment: &Value) -> &Vec<Value> {
    deployment["spec"]["template"]["spec"]["containers"]
        .as_array()
        .expect("a pod has containers")
}

fn container<'a>(deployment: &'a Value, name: &str) -> &'a Value {
    containers(deployment)
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no container named {name}"))
}

fn env(container: &Value, name: &str) -> Value {
    container["env"]
        .as_array()
        .expect("env")
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("no environment variable {name}"))
        .clone()
}

#[test]
fn the_pod_is_the_app_container_alone_behind_the_edge() {
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");
    let workload = rendered.workload.expect("a fullstack app runs in a pod");

    let names: Vec<&str> = containers(&workload.deployment)
        .iter()
        .map(|c| c["name"].as_str().expect("a container has a name"))
        .collect();
    assert_eq!(
        names,
        vec!["app"],
        "no sidecar: the login front is the edge's openid-connect plugin (AP-26, ADR-N-019)"
    );
    let pod = &workload.deployment["spec"]["template"];
    assert_eq!(
        pod["metadata"]["labels"][APP_LABEL], "true",
        "the label the edge's own egress policy selects app pods by"
    );
    assert_eq!(
        pod["metadata"]["labels"]["app.kubernetes.io/name"],
        "app-air-quality-today"
    );
    assert_eq!(
        pod["spec"]["volumes"],
        json!([{ "name": "tmp-app", "emptyDir": {} }]),
        "one container, one scratch volume"
    );

    let app = container(&workload.deployment, "app");
    assert_eq!(app["image"], APP_IMAGE);
    assert_eq!(
        env(app, "JC_BIND_ADDRESS")["value"],
        format!("0.0.0.0:{APP_PORT}"),
        "the app listens on the pod address; APISIX opens the connection from another pod (AP-26)"
    );
    assert_eq!(
        env(app, "JC_BASE_PATH")["value"],
        "/",
        "the App is the whole of its own host (AP-133)"
    );
    assert_eq!(
        app["ports"],
        json!([{ "name": "http", "containerPort": APP_PORT, "protocol": "TCP" }]),
        "port 8080, named http, and no other"
    );
    assert_eq!(app["readinessProbe"]["httpGet"]["path"], "/healthz");
    assert_eq!(app["readinessProbe"]["httpGet"]["port"], "http");
    assert!(
        app.get("livenessProbe").is_none(),
        "readiness gates traffic; a liveness probe on a busy app would only restart it"
    );

    let dumped = workload.deployment.to_string();
    for stale in [
        "oauth2",
        "OAUTH2",
        "client-secret",
        "cookie-secret",
        "keycloak",
        "4180",
    ] {
        assert!(
            !dumped.contains(stale),
            "{stale} survived in the Deployment"
        );
    }
}

#[test]
fn the_service_publishes_the_app_port_the_edge_upstreams_to() {
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");
    let service = rendered.workload.expect("a pod").service;

    assert_eq!(service["metadata"]["name"], "app-air-quality-today");
    assert_eq!(service["spec"]["type"], "ClusterIP");
    assert_eq!(
        service["spec"]["selector"],
        json!({ "app.kubernetes.io/name": "app-air-quality-today" })
    );
    assert_eq!(
        service["spec"]["ports"],
        json!([{ "name": "http", "port": APP_PORT, "targetPort": "http", "protocol": "TCP" }]),
        "8080, the port jcctl's route table points app-{{name}} at (AP-26)"
    );
}

#[test]
fn only_the_gateway_may_open_a_connection_into_the_pod() {
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");
    let policy = rendered.workload.expect("a pod").network_policy;

    let types = policy["spec"]["policyTypes"]
        .as_array()
        .expect("both directions");
    assert!(types.contains(&json!("Ingress")) && types.contains(&json!("Egress")));
    assert_eq!(
        policy["spec"]["podSelector"],
        json!({ "matchLabels": { "app.kubernetes.io/name": "app-air-quality-today" } })
    );

    // In: APISIX alone, on the app port and on the Linkerd inbound port its meshed traffic
    // lands on; the second rule is the proxy's admin ports, which carry no request.
    let ingress = policy["spec"]["ingress"].as_array().expect("holes in");
    assert_eq!(ingress.len(), 2);
    assert_eq!(
        ingress[0]["from"],
        json!([{
            "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "apisix" } },
            "podSelector": { "matchLabels": { "app.kubernetes.io/name": "apisix" } },
        }]),
        "the label the apisix component's own pods carry"
    );
    assert_eq!(
        ingress[0]["ports"],
        json!([
            { "protocol": "TCP", "port": APP_PORT },
            { "protocol": "TCP", "port": 4143 },
        ]),
        "the app port and the mesh's inbound port, no other"
    );
    assert!(ingress[1].get("from").is_none());
    assert_eq!(
        ingress[1]["ports"],
        json!([
            { "protocol": "TCP", "port": 4190 },
            { "protocol": "TCP", "port": 4191 },
        ]),
        "an open rule names the proxy's admin ports and nothing an app serves"
    );

    // Out: the Linkerd control plane, DNS, and the gateway's pods for the endpoint, on the
    // gateway's port and the mesh's inbound port; nothing on the internet (AP-134).
    let egress = policy["spec"]["egress"].as_array().expect("four holes out");
    assert_eq!(egress.len(), 4);
    assert!(
        !policy.to_string().contains("0.0.0.0/0"),
        "no App pod reaches every address"
    );
    assert_eq!(
        egress[0],
        json!({
            "to": [{ "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "linkerd" } } }],
            "ports": [
                { "protocol": "TCP", "port": 8080 },
                { "protocol": "TCP", "port": 8086 },
                { "protocol": "TCP", "port": 8090 },
            ],
        })
    );
    assert_eq!(
        egress[1]["to"][0]["podSelector"]["matchLabels"]["k8s-app"],
        "kube-dns"
    );
    assert_eq!(
        egress[2]["to"],
        json!([{
            "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "jc" } },
            "podSelector": { "matchLabels": { "app.kubernetes.io/name": "context-gateway-gateway" } },
        }])
    );
    assert_eq!(
        egress[2]["ports"],
        json!([
            { "protocol": "TCP", "port": 8080 },
            { "protocol": "TCP", "port": 4143 },
        ])
    );
    assert_eq!(
        egress[3],
        json!({
            "to": [{
                "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "joinedcontext" } },
                "podSelector": { "matchLabels": { "app.kubernetes.io/name": "portal-portal" } },
            }],
            "ports": [
                { "protocol": "TCP", "port": 8080 },
                { "protocol": "TCP", "port": 4143 },
            ],
        }),
        "the Portal's pods for JC_ME_URL, on its port and the mesh's, nothing else of it"
    );
}

/// AP-134: a declared destination is one more hole, on its own ports, with every private range
/// inside it excepted; one that lies inside a private range opens nothing.
#[test]
fn a_declared_destination_is_the_only_way_out_and_never_a_private_range() {
    let rendered = render(
        &app(json!({
            "egress": [
                { "cidr": "203.0.113.0/24", "ports": [443] },
                { "cidr": "10.0.0.0/7", "ports": [443, 8443] },
                { "cidr": "10.1.0.0/16", "ports": [5432] },
                { "cidr": "169.254.169.254/32", "ports": [80] },
                { "cidr": "2001:db8::/32", "ports": [443] },
                { "cidr": "fc00::/8", "ports": [443] },
            ],
        })),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("an App with declared destinations renders");
    let policy = rendered.workload.expect("a pod").network_policy;
    let declared: Vec<&Value> = policy["spec"]["egress"]
        .as_array()
        .expect("egress")
        .iter()
        .skip(4)
        .collect();
    assert_eq!(
        declared,
        [
            &json!({ "to": [{ "ipBlock": { "cidr": "203.0.113.0/24" } }], "ports": [{ "protocol": "TCP", "port": 443 }] }),
            &json!({
                "to": [{ "ipBlock": { "cidr": "10.0.0.0/7", "except": ["10.0.0.0/8"] } }],
                "ports": [{ "protocol": "TCP", "port": 443 }, { "protocol": "TCP", "port": 8443 }],
            }),
            &json!({ "to": [{ "ipBlock": { "cidr": "2001:db8::/32" } }], "ports": [{ "protocol": "TCP", "port": 443 }] }),
        ],
        "the cluster's own networks, the metadata address and a ULA are never reachable"
    );
}

/// AP-134: a pod-backed App reaches its endpoint in the cluster, and without a gateway to name
/// it is not rendered at all rather than pointed at the public host.
#[test]
fn a_pod_app_without_a_gateway_is_refused() {
    for gateway_url in [
        None,
        Some("not a url".to_owned()),
        Some("http://context-gateway:8080".to_owned()),
    ] {
        let err = render(
            &app(json!({})),
            Some(APP_IMAGE),
            &generate_slug(),
            &Settings {
                gateway_url: gateway_url.clone(),
                ..settings()
            },
        )
        .expect_err("no gateway, no pod");
        assert!(
            err.to_string().contains("JC_PORTAL_GATEWAY_URL"),
            "{gateway_url:?}: {err}"
        );
    }
}

/// AP-109, AP-134: the caller's roles are asked on the Portal's Service in the cluster, so
/// without one a pod-backed App is refused rather than told the public host its NetworkPolicy
/// never admits, where every person would read as holding no role.
#[test]
fn a_pod_app_without_a_portal_service_is_refused() {
    for service_url in [
        None,
        Some("not a url".to_owned()),
        Some("http://portal:8080".to_owned()),
    ] {
        let err = render(
            &app(json!({})),
            Some(APP_IMAGE),
            &generate_slug(),
            &Settings {
                service_url: service_url.clone(),
                ..settings()
            },
        )
        .expect_err("no Portal Service, no pod");
        assert!(
            err.to_string().contains("JC_PORTAL_SERVICE_URL"),
            "{service_url:?}: {err}"
        );
    }
}

/// T-3037: the host `JC_ME_URL` names is one the pod's NetworkPolicy lets it reach. The public
/// host was not, so every pod App with roles read its steward as role-less.
#[test]
fn the_network_policy_admits_the_portal_that_jc_me_url_names() {
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");
    let workload = rendered.workload.expect("a pod");
    let me = env(container(&workload.deployment, "app"), "JC_ME_URL")["value"]
        .as_str()
        .expect("JC_ME_URL")
        .to_owned();
    assert_eq!(
        me,
        "http://portal.joinedcontext.svc.cluster.local:8080/api/v1/projects/ovzdusie/apps/air-quality-today/me"
    );
    let url: url::Url = me.parse().expect("a URL");
    let namespace = url
        .host_str()
        .and_then(|host| host.split('.').nth(1))
        .expect("a namespace");
    let port = url.port_or_known_default().expect("a port");
    let admitted = workload.network_policy["spec"]["egress"]
        .as_array()
        .expect("egress")
        .iter()
        .any(|rule| {
            rule["to"].as_array().is_some_and(|peers| {
                peers.iter().any(|peer| {
                    peer["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"]
                        == namespace
                        && peer["podSelector"]["matchLabels"]["app.kubernetes.io/name"]
                            == "portal-portal"
                })
            }) && rule["ports"]
                .as_array()
                .is_some_and(|ports| ports.iter().any(|p| p["port"] == port))
        });
    assert!(admitted, "{me} is outside {}", workload.network_policy);
}

#[test]
fn every_data_need_becomes_one_policy_that_grants_no_more_than_it_asked() {
    let rendered = render(
        &app(json!({
            "dataNeeds": [
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                    "types": ["AirQualityObserved"],
                    "attrs": ["pm10", "refDistrict"],
                    "operations": ["queryEntity", "queryTemporal"],
                    "q": "pm10>=0",
                    "geoQ": { "within": { "scopeRef": "/geo/SK/BB" } },
                    "temporalQ": { "window": "P1D" },
                    "representations": ["ngsi-ld"]
                },
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                    "types": ["District"],
                    "operations": ["retrieveEntity"],
                    "representations": ["ogc-features"]
                }
            ]
        })),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");

    assert_eq!(
        rendered.policies.len(),
        3,
        "one policy per data need (AP-05), and the history reads of the windowed one apart"
    );

    let first = &rendered.policies[0];
    assert_eq!(first.kind, "Policy");
    assert_eq!(first.metadata.name, "app-air-quality-today-1");
    assert_eq!(first.metadata.namespace.as_deref(), Some("ovzdusie"));
    assert_eq!(first.spec["assigner"], "did:web:banskabystrica.sk");
    assert_eq!(
        first.spec["operations"],
        json!(["queryEntity"]),
        "the current-state reads the need names (R8)"
    );
    assert_eq!(
        first.spec["information"][0]["entities"],
        json!([{ "type": "AirQualityObserved" }])
    );
    assert_eq!(
        first.spec["information"][0]["propertyNames"],
        json!(["pm10", "refDistrict"]),
        "an attribute the app did not declare is not readable through this grant"
    );
    assert_eq!(first.spec["q"], "pm10>=0");
    assert_eq!(
        first.spec["scopeQ"], "/geo/SK/BB",
        "a geographic confinement is a scope narrowing (ADR-N-005)"
    );
    // T-2672: CIM 009 has no `timerel` on a current-state query; the window on the Policy that
    // grants `queryEntity` made every read of the app a broker 400.
    assert!(first.spec.get("temporalQ").is_none(), "{}", first.spec);

    let history = &rendered.policies[1];
    assert_eq!(history.metadata.name, "app-air-quality-today-1-history");
    assert_eq!(history.spec["operations"], json!(["queryTemporal"]));
    assert_eq!(history.spec["temporalQ"], "timerel=after;timeAt=P-1D");
    assert_eq!(
        (
            &history.spec["q"],
            &history.spec["scopeQ"],
            &history.spec["information"]
        ),
        (
            &first.spec["q"],
            &first.spec["scopeQ"],
            &first.spec["information"]
        ),
        "the history reads are the same need, bounded in time as well"
    );

    let second = &rendered.policies[2];
    assert_eq!(second.metadata.name, "app-air-quality-today-2");
    assert_eq!(second.spec["operations"], json!(["retrieveEntity"]));
    assert!(
        second.spec.get("q").is_none()
            && second.spec.get("scopeQ").is_none()
            && second.spec.get("temporalQ").is_none(),
        "a need without constraints renders none"
    );

    for policy in &rendered.policies {
        let parsed: PolicySpec =
            serde_json::from_value(policy.spec.clone()).expect("a rendered policy parses");
        parsed.validate().expect("a rendered policy is valid");
    }
}

#[test]
fn the_endpoint_is_the_union_of_what_the_needs_asked_for() {
    let slug = generate_slug();
    let rendered =
        render(&app(json!({})), Some(APP_IMAGE), &slug, &settings()).expect("the app renders");

    let endpoint = &rendered.endpoint;
    assert_eq!(endpoint.kind, "Endpoint");
    assert_eq!(
        endpoint.metadata.name, "app-air-quality-today",
        "the name the APISIX upstream and the app's own configuration both point at (AP-05)"
    );
    assert_eq!(endpoint.spec["slug"], slug.as_str());
    assert_eq!(
        endpoint.spec["enabledRepresentations"],
        json!(["ngsi-ld", "mcp", "csv", "ogc-features"]),
        "the union of the needs' representations (AP-05) and MCP on every Endpoint (T-4319)"
    );
    assert_eq!(endpoint.spec["audience"], "project-list");
    assert_eq!(endpoint.spec["allowedProjects"], json!(["ovzdusie"]));
    assert_eq!(endpoint.spec["rateLimits"]["requestsPerMinute"], 600);
    assert_eq!(
        endpoint.spec["fileLimits"]["maxFileRows"], 20000,
        "a download limit belongs to the app's endpoint, not to its author (AP-17)"
    );

    let parsed: EndpointSpec =
        serde_json::from_value(endpoint.spec.clone()).expect("a rendered endpoint parses");
    parsed.validate().expect("a rendered endpoint is valid");

    // The pod is told the same endpoint, and nothing else about the platform.
    let app = rendered.workload.expect("a pod").deployment;
    assert_eq!(
        env(container(&app, "app"), "JC_ENDPOINT_URL")["value"],
        format!(
            "http://context-gateway.jc.svc.cluster.local:8080/api/endpoint/{}/",
            slug.as_str()
        )
    );
}

/// T-2759: the endpoints list named an app's endpoint `app-{name}` and nothing else.
#[test]
fn the_endpoint_carries_its_apps_title() {
    let slug = generate_slug();
    let mut titled = app(json!({}));
    titled.metadata.rest.insert(
        "title".to_owned(),
        json!({ "en": "Air quality today", "sk": "Kvalita ovzdušia dnes" }),
    );
    let rendered = render(&titled, Some(APP_IMAGE), &slug, &settings()).expect("the app renders");
    assert_eq!(
        rendered.endpoint.metadata.rest.get("title"),
        Some(&json!({ "en": "Air quality today", "sk": "Kvalita ovzdušia dnes" }))
    );

    // An App with no title leaves the endpoint without one rather than inventing it.
    let untitled =
        render(&app(json!({})), Some(APP_IMAGE), &slug, &settings()).expect("the app renders");
    assert_eq!(untitled.endpoint.metadata.rest.get("title"), None);
}

#[test]
fn a_public_app_tells_its_container_that_anonymous_callers_are_normal() {
    let rendered = render(
        &app(json!({ "visibility": "public" })),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");

    let deployment = rendered.workload.expect("a pod").deployment;
    assert_eq!(
        env(container(&deployment, "app"), "JC_ANONYMOUS")["value"],
        "true",
        "the edge passes anonymous requests through without X-Access-Token (AP-28)"
    );

    assert_eq!(rendered.endpoint.spec["audience"], "public");
    assert_eq!(
        rendered.policies[0].spec["assignee"],
        json!({ "kind": "role", "id": "endpoint:ovzdusie/app-air-quality-today" }),
        "anonymous callers the endpoint admits hold its caller role there alone (AP-96, GW22)"
    );

    // A project app has no such flag: an absent token there is the edge's `unauth_action: auth`
    // never having let the request through, which is a bug worth seeing.
    let project = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders")
    .workload
    .expect("a pod")
    .deployment;
    assert!(container(&project, "app")["env"]
        .as_array()
        .expect("env")
        .iter()
        .all(|e| e["name"] != "JC_ANONYMOUS"));
}

/// AP-124: `service` is withdrawn, so no App holds a service account any more (AP-08), and the
/// reconciler renders nothing for one: it is refused in words that name what to write instead.
#[test]
fn a_service_app_is_refused_and_granted_nothing() {
    let refused = render(
        &app(json!({ "kind": "service" })),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect_err("a service app is refused");
    let message = refused.to_string();
    assert!(message.contains("`service` is withdrawn"), "{message}");
    assert!(
        message.contains("`ui`") && message.contains("`ui-rust`"),
        "{message}"
    );
}

/// AP-124: until it is built, `ui-node` is refused as declared and not built yet.
#[test]
fn a_ui_node_app_is_refused_until_the_shape_is_built() {
    let refused = render(
        &app(json!({ "kind": "ui-node" })),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect_err("a ui-node app is refused");
    assert!(refused.to_string().contains("not built yet"), "{refused}");
}

/// AP-07, AP-124: a `ui-rust` App's server reaches the data with the person's token through the
/// App's endpoint, the grant made to the endpoint's caller role and never to an account.
#[test]
fn a_ui_rust_app_is_granted_through_its_endpoint_role() {
    let rendered = render(
        &app(json!({ "kind": "ui-rust" })),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");
    assert_eq!(rendered.policies[0].spec["assignee"]["kind"], json!("role"));
    assert!(rendered.workload.is_some(), "a ui-rust app runs in a pod");
}

#[test]
fn the_secret_holds_the_slug_alone_and_the_pod_does_not_reference_it() {
    let slug = EndpointSlug::new(generate_slug().as_str()).expect("a slug the app already has");

    let rendered =
        render(&app(json!({})), Some(APP_IMAGE), &slug, &settings()).expect("the app renders");
    let workload = rendered.workload.expect("a pod");

    assert_eq!(workload.secret["kind"], "Secret");
    assert_eq!(
        workload.secret["metadata"]["name"],
        "app-air-quality-today-endpoint"
    );
    assert_eq!(
        workload.secret["stringData"],
        json!({ "endpoint-slug": slug.as_str() }),
        "no client secret, no cookie secret: the reconciler owns no OIDC secret (AP-27)"
    );

    let deployment = workload.deployment.to_string();
    assert!(
        !deployment.contains("secretKeyRef") && !deployment.contains("secretName"),
        "the pod reads nothing from the Secret; the slug reaches it as JC_ENDPOINT_URL"
    );
}

#[test]
fn a_static_app_gets_its_grants_and_no_pod() {
    let rendered = render(
        // A static bundle is built by node, never by rust (AP-83).
        &app(json!({ "kind": "static", "visibility": "organization", "build": { "node": "22" } })),
        None,
        &generate_slug(),
        &settings(),
    )
    .expect("a static app renders without an image");

    assert!(
        rendered.workload.is_none(),
        "a static bundle is served by the Portal, not by a Deployment (AP-14)"
    );
    assert_eq!(rendered.policies.len(), 1);
    assert_eq!(rendered.endpoint.spec["audience"], "organization");
}

/// AP-148 (T-3359): a wasm App's interface is static and its server the shared host's, so it
/// renders its grants and no pod, with no image to ask for.
#[test]
fn a_wasm_app_gets_its_grants_and_no_pod() {
    let rendered = render(
        &app(json!({ "kind": "wasm", "visibility": "organization", "build": { "node": "22", "rust": "1.90" } })),
        None,
        &generate_slug(),
        &settings(),
    )
    .expect("a wasm app renders without an image");
    assert!(rendered.workload.is_none());
    assert_eq!(rendered.policies.len(), 1);
}

#[test]
fn an_image_that_is_not_pinned_by_digest_is_refused() {
    let refused = render(
        &app(json!({})),
        Some("ghcr.io/bb/apps/air-quality:latest"),
        &generate_slug(),
        &settings(),
    )
    .expect_err("a tag can be moved under a running pod");
    assert!(
        matches!(refused, RenderError::UnpinnedImage { .. }),
        "got {refused:?}"
    );
}

#[test]
fn an_app_that_needs_a_removed_file_is_refused_with_the_name_to_drop() {
    let refused = render(
        &app(json!({ "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
            "types": ["AirQualityObserved"],
            "operations": ["queryEntity"],
            "representations": ["ngsi-ld", "geojson"]
        }] })),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect_err("geojson left the vocabulary (T-4597)");
    let said = format!("{refused:?}");
    assert!(said.contains("`geojson` was removed"), "got {said}");
}

#[test]
fn an_app_that_is_not_deployed_yet_renders_nothing() {
    for state in ["draft", "retired"] {
        let refused = render(
            &app(json!({ "lifecycle": state })),
            Some(APP_IMAGE),
            &generate_slug(),
            &settings(),
        )
        .expect_err("only preview and published apps run (AP-18, AP-21)");
        assert!(
            matches!(refused, RenderError::NotDeployable { .. }),
            "{state}: got {refused:?}"
        );
    }
}

#[test]
fn needs_reaching_into_two_spaces_do_not_become_one_endpoint() {
    let refused = render(
        &app(json!({
            "dataNeeds": [
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                    "types": ["AirQualityObserved"],
                    "operations": ["queryEntity"]
                },
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "doprava" },
                    "types": ["TrafficFlowObserved"],
                    "operations": ["queryEntity"]
                }
            ]
        })),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect_err("an app reads through exactly one endpoint (AP-04)");
    assert!(
        matches!(refused, RenderError::SeveralSpaces { .. }),
        "got {refused:?}"
    );
}

/// AP-04 (T-2933): a `ui` App reads one further space of its project, read only, through that
/// space's public Endpoints; its needs there compile into no grant, and the App's own endpoint and
/// grants stay those of its own space.
#[test]
fn a_ui_apps_further_space_compiles_into_no_grant() {
    let rendered = render(
        &app(json!({
            "kind": "ui",
            "source": { "path": "." },
            "build": { "node": "22" },
            "dataNeeds": [
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                    "types": ["AirQualityObserved"],
                    "operations": ["queryEntity"],
                    "representations": ["ngsi-ld"]
                },
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "registre" },
                    "types": ["AdministrativeArea"],
                    "operations": ["queryEntity", "retrieveEntity"],
                    "representations": ["ogc-features"]
                }
            ]
        })),
        None,
        &generate_slug(),
        &settings(),
    )
    .expect("a ui app may read one further space");
    let endpoint: EndpointSpec =
        serde_json::from_value(rendered.endpoint.spec.clone()).expect("an Endpoint");
    assert_eq!(endpoint.context_space_ref.name(), "ovzdusie");
    assert_eq!(
        rendered.endpoint.spec["enabledRepresentations"],
        json!(["ngsi-ld", "mcp"]),
        "the further need's representations are not the own endpoint's; MCP is every Endpoint's (T-4319)"
    );
    let names: Vec<&str> = rendered
        .policies
        .iter()
        .map(|policy| policy.metadata.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["app-air-quality-today-1"],
        "one grant, of the own space"
    );
    assert!(!format!("{:?}", rendered.policies).contains("AdministrativeArea"));
}

/// AP-04 (T-2933, T-2934): a further space is read only, holds no role, is one of at most two
/// and only a `ui` App's: every other shape is refused and names the space.
#[test]
fn a_further_space_that_writes_holds_a_role_is_a_third_or_a_pods_is_refused() {
    let own = json!({
        "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
        "types": ["AirQualityObserved"],
        "operations": ["queryEntity"]
    });
    let further = |space: &str, extra: Value| {
        let mut need = json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": space },
            "types": ["AdministrativeArea"],
            "operations": ["queryEntity"]
        });
        for (key, value) in extra.as_object().expect("an object") {
            need[key] = value.clone();
        }
        need
    };
    let ui = |needs: Vec<Value>| {
        render(
            &app(json!({
                "kind": "ui",
                "source": { "path": "." },
                "build": { "node": "22" },
                "visibility": "roles",
                "roles": [{ "name": "steward" }],
                "dataNeeds": needs
            })),
            None,
            &generate_slug(),
            &settings(),
        )
    };

    let writes = ui(vec![
        own.clone(),
        further(
            "registre",
            json!({ "operations": ["queryEntity", "updateAttrs"] }),
        ),
    ])
    .expect_err("a further space is read only");
    assert!(
        matches!(&writes, RenderError::FurtherSpace { space } if space == "registre"),
        "got {writes:?}"
    );
    let role = ui(vec![
        own.clone(),
        further("registre", json!({ "roles": ["steward"] })),
    ])
    .expect_err("a further space grants no role");
    assert!(
        matches!(role, RenderError::FurtherSpace { .. }),
        "got {role:?}"
    );
    // Two further spaces are read (T-2934); a third further one is refused by name.
    ui(vec![
        own.clone(),
        further("registre", json!({})),
        further("kraj", json!({})),
        further("registre", json!({ "types": ["Road"] })),
    ])
    .expect("a ui app may read two further spaces");
    let third = ui(vec![
        own.clone(),
        further("registre", json!({})),
        further("kraj", json!({})),
        further("doprava", json!({})),
    ])
    .expect_err("two further spaces, not three");
    assert!(
        matches!(&third, RenderError::SeveralSpaces { second, .. } if second == "doprava"),
        "got {third:?}"
    );
    let pod = render(
        &app(json!({ "dataNeeds": [own, further("registre", json!({}))] })),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect_err("a pod reads through its one endpoint");
    assert!(
        matches!(pod, RenderError::SeveralSpaces { .. }),
        "got {pod:?}"
    );
}

#[test]
fn a_manifest_with_nowhere_to_put_a_secret_stays_that_way() {
    // AP-16: the App kind refuses `secretRef` at parse time, and rendering never invents one.
    let refused = render(
        &serde_json::from_value(json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "App",
            "metadata": { "name": "leaky", "namespace": "ovzdusie" },
            "spec": {
                "kind": "fullstack",
                "source": { "path": "./src" },
                "build": { "rust": "1.90" },
                "visibility": "project",
                "lifecycle": "published",
                "secretRef": { "name": "database" },
                "dataNeeds": [{
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                    "types": ["AirQualityObserved"],
                    "operations": ["queryEntity"]
                }]
            },
        }))
        .expect("a manifest envelope"),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect_err("an app has nowhere to put a secret (AP-16)");
    assert!(matches!(refused, RenderError::Spec(_)), "got {refused:?}");
}

#[test]
fn a_generated_slug_is_unguessable_and_never_the_same_twice() {
    let first = generate_slug();
    let second = generate_slug();
    assert_ne!(first, second);
    assert!(
        first.as_str().len() >= 26
            && first
                .as_str()
                .chars()
                .all(|c| matches!(c, 'a'..='z' | '2'..='7')),
        "the slug is base32 and carries at least 128 bits (EP-02)"
    );
    assert!(
        EndpointSlug::new("ovzdusie").is_err(),
        "a readable slug is not a slug (EP-02, EP-03)"
    );
}

/// AP-108: APISIX is admitted from the namespace the installation runs it in, a setting; the
/// literal `apisix` never matched `dev`, where APISIX runs in `dev`.
#[test]
fn ingress_admits_apisix_from_the_namespace_the_installation_names() {
    let mut on_dev = settings();
    on_dev.apisix_namespace = "dev".into();
    let rendered = render(&app(json!({})), Some(APP_IMAGE), &generate_slug(), &on_dev)
        .expect("the app renders");
    let policy = rendered.workload.expect("a pod").network_policy;
    assert_eq!(
        policy["spec"]["ingress"][0]["from"][0]["namespaceSelector"],
        json!({ "matchLabels": { "kubernetes.io/metadata.name": "dev" } })
    );
}

/// AP-105, AP-108, AP-109: a fullstack pod pulls with the configured Secret, starts the binary at
/// `/app` as a numeric non-root user, and is told where to ask for the caller's roles.
#[test]
fn a_fullstack_pod_pulls_with_the_secret_and_runs_the_binary_as_a_numeric_user() {
    let mut with_registry = settings();
    with_registry.pull_secret = Some("app-registry".into());
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &with_registry,
    )
    .expect("the app renders");
    let deployment = rendered.workload.expect("a pod").deployment;
    let pod = &deployment["spec"]["template"]["spec"];
    assert_eq!(pod["imagePullSecrets"], json!([{ "name": "app-registry" }]));
    assert_eq!(pod["securityContext"]["runAsNonRoot"], true);
    assert_eq!(pod["securityContext"]["runAsUser"], 65532);
    let binary = container(&deployment, "app");
    assert_eq!(binary["command"], json!(["/app"]));
    assert_eq!(
        env(binary, "JC_ME_URL")["value"],
        "http://portal.joinedcontext.svc.cluster.local:8080/api/v1/projects/ovzdusie/apps/air-quality-today/me"
    );

    let without = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders")
    .workload
    .expect("a pod")
    .deployment;
    assert_eq!(
        without["spec"]["template"]["spec"]["imagePullSecrets"],
        json!([]),
        "no Secret configured, none named"
    );
}

/// AP-95, AP-126 (T-2724): a pod App's backend is handed the `#jc-config` the static host would
/// write, without `user` and without a token, so the App SDK in its `ui/` reads its endpoint as a
/// `ui` App does: its slug, the organisation's domain, its one space and the types it reads, and
/// the project's page in the Portal its entity panel links to (SDK-40, T-3399).
#[test]
fn a_pod_app_is_handed_the_sdk_configuration_of_its_one_endpoint() {
    let slug = generate_slug();
    let rendered =
        render(&app(json!({})), Some(APP_IMAGE), &slug, &settings()).expect("the app renders");
    let endpoint = rendered.endpoint.metadata.name.clone();
    let deployment = rendered.workload.expect("a pod").deployment;
    let value = env(container(&deployment, "app"), "JC_APP_CONFIG")["value"].clone();
    let config: Value =
        serde_json::from_str(value.as_str().expect("a string")).expect("JC_APP_CONFIG is JSON");
    assert_eq!(
        config,
        json!({
            "slug": slug.as_str(),
            "orgDomain": "banskabystrica.sk",
            "space": "ovzdusie",
            "transport": "origin",
            "appName": "air-quality-today",
            "endpointName": endpoint,
            "endpoints": [{
                "name": endpoint,
                "slug": slug.as_str(),
                "space": "ovzdusie",
                "types": ["AirQualityObserved", "District"],
            }],
            // SDK-40: where the entity panel links an entity, as the static host writes it.
            "portal": "https://bb.example.com/projects/ovzdusie",
        })
    );
    assert!(
        config.get("user").is_none(),
        "the person is the backend's to add, per request"
    );
}

/// AP-67: with a basemap configured a pod App is handed its project's style, the one the static
/// host writes for a `ui` App, so its map never falls back to a plain background; without one
/// the key is absent (the equality above).
#[test]
fn a_pod_app_is_handed_its_projects_basemap_when_one_is_configured() {
    let settings = Settings {
        basemap_base: Some("https://portal.bb.example.com/".into()),
        ..settings()
    };
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings,
    )
    .expect("renders");
    let deployment = rendered.workload.expect("a pod").deployment;
    let value = env(container(&deployment, "app"), "JC_APP_CONFIG")["value"].clone();
    let config: Value = serde_json::from_str(value.as_str().expect("a string")).expect("JSON");
    assert_eq!(
        config["basemap"],
        "https://portal.bb.example.com/api/v1/projects/ovzdusie/basemap/default/style.json"
    );
}

/// AP-96: the app's grants are held on its own endpoint alone. A need without roles goes to the
/// endpoint's caller role, a need with roles to one Policy per role, each exactly the need, and
/// the endpoint carries the roles with their subjects so the gateway can hand them out (AP-97).
#[test]
fn a_role_gated_need_is_granted_to_that_role_on_the_apps_endpoint_alone() {
    let rendered = render(
        &app(json!({
            "kind": "static",
            "source": { "path": "." },
            "build": { "node": "22" },
            "visibility": "roles",
            "roles": [
                { "name": "viewer" },
                { "name": "steward" },
                { "name": "auditor" }
            ],
            "access": [
                { "role": "viewer", "subjects": [{ "group": "bb-operations" }] },
                { "role": "steward", "subjects": [{ "user": "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14" }] }
            ],
            "dataNeeds": [
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                    "types": ["AirQualityObserved"],
                    "operations": ["queryEntity"]
                },
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                    "types": ["AirQualityObserved"],
                    "attrs": ["stewardNote"],
                    "operations": ["updateAttrs"],
                    "roles": ["steward", "auditor"]
                }
            ]
        })),
        None,
        &generate_slug(),
        &settings(),
    )
    .expect("a static app with roles renders");

    let endpoint = &rendered.endpoint.spec;
    assert_eq!(endpoint["callerRole"], true);
    assert_eq!(endpoint["audience"], "project-list");
    assert_eq!(endpoint["allowedProjects"], json!(["ovzdusie"]));
    assert_eq!(
        endpoint["roles"],
        json!([
            { "name": "viewer", "subjects": [{ "group": "bb-operations" }] },
            { "name": "steward", "subjects": [{ "user": "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14" }] }
        ]),
        "a role nobody holds gives nobody anything and is left out"
    );
    let parsed: EndpointSpec =
        serde_json::from_value(endpoint.clone()).expect("the endpoint parses");
    parsed.validate().expect("the endpoint is valid");

    let granted: Vec<(&str, &Value)> = rendered
        .policies
        .iter()
        .map(|p| (p.metadata.name.as_str(), &p.spec["assignee"]["id"]))
        .collect();
    assert_eq!(
        granted,
        [
            (
                "app-air-quality-today-1",
                &json!("endpoint:ovzdusie/app-air-quality-today")
            ),
            (
                "app-air-quality-today-2-steward",
                &json!("endpoint:ovzdusie/app-air-quality-today/steward")
            ),
            (
                "app-air-quality-today-2-auditor",
                &json!("endpoint:ovzdusie/app-air-quality-today/auditor")
            ),
        ]
    );
    for policy in &rendered.policies[1..] {
        assert_eq!(policy.spec["operations"], json!(["updateAttrs"]));
        assert_eq!(
            policy.spec["information"][0]["propertyNames"],
            json!(["stewardNote"]),
            "a role's grant is the need and never more (AP-06)"
        );
        let parsed: PolicySpec =
            serde_json::from_value(policy.spec.clone()).expect("a rendered policy parses");
        parsed.validate().expect("a rendered policy is valid");
    }
}

/// AP-96: before roles the grant named `app-{name}`, a role no token carries, so a project app
/// read nothing; now every caller the endpoint admits holds the caller role it is granted to.
#[test]
fn a_project_app_is_granted_to_the_endpoints_caller_role() {
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");
    assert_eq!(rendered.endpoint.spec["callerRole"], true);
    assert!(rendered.endpoint.spec.get("roles").is_none());
    assert_eq!(
        rendered.policies[0].spec["assignee"],
        json!({ "kind": "role", "id": "endpoint:ovzdusie/app-air-quality-today" })
    );
}

/// T-5208 (EP-159): a calendar is served only for a listed output, so an App that names `ical`
/// gets the output `events` its subscribe link (`ical/v1/events.ics`) reads, and an App that
/// does not name `ical` gets none.
#[test]
fn an_app_that_names_ical_gets_the_events_calendar() {
    let with = |representations: Value| {
        render(
            &app(json!({
                "kind": "ui",
                "source": { "path": "." },
                "build": { "node": "22" },
                "dataNeeds": [{
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "podujatia" },
                    "types": ["Event"],
                    "operations": ["queryEntity"],
                    "representations": representations
                }]
            })),
            None,
            &generate_slug(),
            &settings(),
        )
        .expect("a ui app")
    };
    let rendered = with(json!(["ngsi-ld", "ical"]));
    let endpoint: EndpointSpec =
        serde_json::from_value(rendered.endpoint.spec.clone()).expect("an Endpoint");
    endpoint.validate().expect("a valid Endpoint");
    let names: Vec<&str> = endpoint
        .ical_outputs()
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(names, ["events"]);
    assert!(with(json!(["ngsi-ld"]))
        .endpoint
        .spec
        .get("representations")
        .is_none());
}
