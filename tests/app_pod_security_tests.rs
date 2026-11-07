//! Every pod the Portal renders for an App passes Pod Security `restricted` (T-2832, AP-116, AP-108).
//!
//! App pods and their build pods run in a namespace that enforces `restricted` (AP-116), and the
//! admission controller refuses the whole pod when one container breaks it. That is how
//! `linkerd-init` (NET_ADMIN, NET_RAW) left every App at 0/1 on dev: the fix is Linkerd in CNI mode
//! on the cluster, which adds no init container, and these tests keep the Portal's own part of the
//! pod inside the profile, so a container or a volume added here is refused in CI and not by the
//! cluster.
//!
//! The rules are the `restricted` profile of the Pod Security Standards, as the admission
//! controller applies them to a pod spec.

use jcctl::loader::RawManifest;
use joinedcontext_portal::apps::build_pods::{self, Class};
use joinedcontext_portal::apps::reconciler::{generate_slug, render, Settings};
use serde_json::{json, Value};

const APP_IMAGE: &str =
    "ghcr.io/bb/apps/air-quality@sha256:1111111111111111111111111111111111111111111111111111111111111111";

/// The volume types `restricted` admits.
const RESTRICTED_VOLUMES: [&str; 8] = [
    "configMap",
    "csi",
    "downwardAPI",
    "emptyDir",
    "ephemeral",
    "persistentVolumeClaim",
    "projected",
    "secret",
];

/// What `restricted` refuses in one pod spec, one line per breach, empty when it admits the pod.
fn restricted_violations(pod: &Value) -> Vec<String> {
    let mut breaches = Vec::new();
    for host in ["hostNetwork", "hostPID", "hostIPC"] {
        if pod[host] == true {
            breaches.push(format!("{host} is set"));
        }
    }
    for volume in pod["volumes"].as_array().into_iter().flatten() {
        let kind = volume
            .as_object()
            .and_then(|fields| fields.keys().find(|key| key.as_str() != "name"))
            .map(String::as_str)
            .unwrap_or("none");
        if !RESTRICTED_VOLUMES.contains(&kind) {
            breaches.push(format!("volume {} is {kind}", volume["name"]));
        }
    }
    let pod_context = &pod["securityContext"];
    let seccomp_ok = |context: &Value| {
        matches!(
            context["seccompProfile"]["type"].as_str(),
            Some("RuntimeDefault" | "Localhost")
        )
    };
    for list in ["initContainers", "containers", "ephemeralContainers"] {
        for container in pod[list].as_array().into_iter().flatten() {
            let name = container["name"].as_str().unwrap_or("?");
            let context = &container["securityContext"];
            if context["privileged"] == true {
                breaches.push(format!("{name} is privileged"));
            }
            if context["allowPrivilegeEscalation"] != false {
                breaches.push(format!(
                    "{name} does not set allowPrivilegeEscalation: false"
                ));
            }
            let drops_all = context["capabilities"]["drop"]
                .as_array()
                .is_some_and(|drop| drop.iter().any(|capability| capability == "ALL"));
            if !drops_all {
                breaches.push(format!("{name} does not drop ALL capabilities"));
            }
            for added in context["capabilities"]["add"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if added != "NET_BIND_SERVICE" {
                    breaches.push(format!("{name} adds capability {added}"));
                }
            }
            let non_root = context["runAsNonRoot"] == true
                || (context["runAsNonRoot"].is_null() && pod_context["runAsNonRoot"] == true);
            if !non_root {
                breaches.push(format!("{name} may run as root"));
            }
            let user = if context["runAsUser"].is_null() {
                &pod_context["runAsUser"]
            } else {
                &context["runAsUser"]
            };
            if user == 0 {
                breaches.push(format!("{name} runs as user 0"));
            }
            if !(seccomp_ok(context)
                || (context["seccompProfile"].is_null() && seccomp_ok(pod_context)))
            {
                breaches.push(format!(
                    "{name} has no RuntimeDefault or Localhost seccomp profile"
                ));
            }
            for port in container["ports"].as_array().into_iter().flatten() {
                if port["hostPort"].as_u64().is_some_and(|host| host != 0) {
                    breaches.push(format!("{name} binds host port {}", port["hostPort"]));
                }
            }
        }
    }
    breaches
}

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
        pull_secret: Some("ghcr-pull".into()),
        basemap_base: None,
        release: Some("dev".into()),
        service_account: Some("portal".into()),
    }
}

fn app(kind: &str) -> RawManifest {
    serde_json::from_value(json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "App",
        "metadata": { "name": "air-quality-today", "namespace": "ovzdusie" },
        "spec": {
            "kind": kind,
            "source": { "path": "./src" },
            "build": { "rust": "1.90", "node": "22" },
            "visibility": "project",
            "lifecycle": "published",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                "types": ["AirQualityObserved"],
                "attrs": ["pm10"],
                "operations": ["queryEntity"],
                "representations": ["ngsi-ld"]
            }]
        },
    }))
    .expect("the fixture is a manifest")
}

fn build_settings() -> build_pods::Settings {
    build_pods::Settings {
        namespace: "dev-ovzdusie-apps".into(),
        forge: "http://gitea-http.dev.svc.cluster.local:3000".into(),
        node_image: "registry.example.org/builder@sha256:aa".into(),
        rust_image: "registry.example.org/builder-rust@sha256:bb".into(),
        cache_size: "1Gi".into(),
    }
}

/// AP-116, AP-108: the App Deployment of the pod-shaped class, by its new and its old name
/// (AP-124), is admitted by `restricted`.
#[test]
fn every_app_pod_passes_the_restricted_pod_security_profile() {
    for kind in ["ui-rust", "fullstack"] {
        let rendered = render(&app(kind), Some(APP_IMAGE), &generate_slug(), &settings())
            .unwrap_or_else(|error| panic!("a {kind} app renders: {error}"));
        let workload = rendered
            .workload
            .unwrap_or_else(|| panic!("a {kind} app runs in a pod"));
        let pod = &workload.deployment["spec"]["template"]["spec"];
        assert_eq!(
            restricted_violations(pod),
            Vec::<String>::new(),
            "a {kind} App pod breaks Pod Security restricted: {pod}"
        );
    }
}

/// AP-116, AP-108: the build Job of either class is admitted by `restricted` too.
#[test]
fn every_build_pod_passes_the_restricted_pod_security_profile() {
    for class in [Class::Node, Class::Rust] {
        let job = build_pods::job(&build_settings(), class, "air-quality-today", 7);
        let pod = &job["spec"]["template"]["spec"];
        assert_eq!(
            restricted_violations(pod),
            Vec::<String>::new(),
            "a {class:?} build pod breaks Pod Security restricted: {pod}"
        );
    }
}

/// AP-116: the injected `linkerd-init` of a mesh without CNI mode is exactly what `restricted`
/// refuses, so the profile check above would have caught the dev failure if the Portal had added it.
#[test]
fn a_linkerd_init_container_with_net_admin_is_refused_by_the_profile() {
    let rendered = render(
        &app("fullstack"),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");
    let mut pod = rendered.workload.expect("a pod").deployment["spec"]["template"]["spec"].clone();
    pod["initContainers"] = json!([{
        "name": "linkerd-init",
        "image": "cr.l5d.io/linkerd/proxy-init:v2.4.1",
        "securityContext": {
            "allowPrivilegeEscalation": false,
            "capabilities": { "add": ["NET_ADMIN", "NET_RAW"] },
            "runAsNonRoot": true,
            "runAsUser": 65534,
            "seccompProfile": { "type": "RuntimeDefault" },
        },
    }]);
    let breaches = restricted_violations(&pod);
    assert!(
        breaches
            .iter()
            .any(|breach| breach == "linkerd-init adds capability \"NET_ADMIN\""),
        "{breaches:?}"
    );
    assert!(
        breaches
            .iter()
            .any(|breach| breach == "linkerd-init does not drop ALL capabilities"),
        "{breaches:?}"
    );
}

/// The checker refuses what `restricted` refuses at pod level: a host namespace and a host path.
#[test]
fn a_host_path_or_a_host_network_is_refused_by_the_profile() {
    let pod = json!({
        "hostNetwork": true,
        "securityContext": { "runAsNonRoot": true, "seccompProfile": { "type": "RuntimeDefault" } },
        "volumes": [{ "name": "node", "hostPath": { "path": "/" } }],
        "containers": [],
    });
    assert_eq!(
        restricted_violations(&pod),
        vec![
            "hostNetwork is set".to_owned(),
            "volume \"node\" is hostPath".to_owned()
        ]
    );
}
