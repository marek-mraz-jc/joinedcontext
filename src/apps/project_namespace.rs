//! A project's own namespace for its pod-backed Apps, `{release}-{project}-apps` (AP-116,
//! AP-117, ADR-N-030 §6).
//!
//! The namespace carries the labels the admission policy checks before it lets the Portal create
//! or delete it, Pod Security `restricted`, a default-deny NetworkPolicy that admits the APISIX
//! pods only, and the RoleBinding that gives the Portal the rights it deploys an App with there
//! and nowhere else. Each App adds its own narrower policy on top (AP-15); policies add up, so
//! the namespace's is the floor for any pod no App policy selects.

use serde_json::{json, Value};

use jc_core::annotations::GENERATED_BY;

use super::reconciler::{Settings, APP_PORT, GENERATOR, LINKERD_INBOUND};

/// The label that names the project a namespace holds the Apps of.
pub const PROJECT_LABEL: &str = "joinedcontext.com/project";
/// The label the admission policy requires on every namespace the Portal creates (AP-117).
pub const MANAGED_BY_LABEL: &str = "joinedcontext.com/managed-by";
/// Its one value.
pub const MANAGED_BY: &str = "joinedcontext-portal";
/// The label that names the installation a namespace belongs to, which the APISIX egress policy
/// selects the release's apps namespaces by.
pub const RELEASE_LABEL: &str = "joinedcontext.com/release";
/// The ClusterRole the RoleBinding names: `{release}-portal-apps`, which the deployment creates
/// with the verbs an App's four objects need and binds nowhere itself.
pub fn apps_role(release: &str) -> String {
    format!("{release}-portal-apps")
}

/// The objects of one project's apps namespace in the order they are applied: the Namespace, then
/// the RoleBinding, which the Portal needs before it may write the NetworkPolicy there.
pub fn objects(
    project: &str,
    namespace: &str,
    release: &str,
    service_account: &str,
    settings: &Settings,
) -> [Value; 3] {
    let labels = json!({
        PROJECT_LABEL: project,
        MANAGED_BY_LABEL: MANAGED_BY,
        "app.kubernetes.io/part-of": "joinedcontext",
    });
    let meta = |name: &str| {
        json!({
            "name": name,
            "namespace": namespace,
            "labels": labels,
            "annotations": { GENERATED_BY: GENERATOR },
        })
    };
    let mut namespace_labels = labels.clone();
    namespace_labels[RELEASE_LABEL] = json!(release);
    for mode in ["enforce", "audit", "warn"] {
        namespace_labels[format!("pod-security.kubernetes.io/{mode}")] = json!("restricted");
    }
    [
        json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": namespace,
                "labels": namespace_labels,
                "annotations": { GENERATED_BY: GENERATOR },
            },
        }),
        json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "RoleBinding",
            "metadata": meta("joinedcontext-portal"),
            "roleRef": {
                "apiGroup": "rbac.authorization.k8s.io",
                "kind": "ClusterRole",
                "name": apps_role(release),
            },
            "subjects": [{
                "kind": "ServiceAccount",
                "name": service_account,
                "namespace": settings.namespace,
            }],
        }),
        json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": meta("default-deny"),
            "spec": {
                "podSelector": {},
                "policyTypes": ["Ingress", "Egress"],
                // APISIX in, on the app port and on the Linkerd inbound proxy that meshed
                // traffic lands on; nothing out. An App's own policy opens what it needs.
                "ingress": [{
                    "from": [{
                        "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": settings.apisix_namespace } },
                        "podSelector": { "matchLabels": { "app.kubernetes.io/name": "apisix" } },
                    }],
                    "ports": [
                        { "protocol": "TCP", "port": APP_PORT },
                        { "protocol": "TCP", "port": LINKERD_INBOUND },
                    ],
                }],
                "egress": [],
            },
        }),
    ]
}

/// The pull Secret for the project's namespace: the one in the Portal's namespace, its type and
/// data only, so nothing the API server added travels with it (AP-108).
pub fn pull_secret_copy(source: &Value, name: &str, namespace: &str) -> Option<Value> {
    let data = source.get("data")?.as_object()?;
    let kind = source.get("type").and_then(Value::as_str)?;
    Some(json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": { MANAGED_BY_LABEL: MANAGED_BY },
            "annotations": { GENERATED_BY: GENERATOR },
        },
        "type": kind,
        "data": data,
    }))
}
