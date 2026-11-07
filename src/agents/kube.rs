//! The one Job a builder run is, and the two objects that hold it in (AG-33…AG-35, AG-52).
//!
//! Four objects, all named `agent-run-{id}`: a ServiceAccount that mounts no token, a
//! NetworkPolicy that lets the workspace reach the credential proxy, DNS and the mesh's control
//! plane and nothing else, the Secret holding the run's ticket, and the Job itself. Every other
//! credential stays in the proxy (ADR-N-020). The ticket reaches the container through a
//! `secretKeyRef`, so a person who may `get` pods or jobs in the namespace does not read a live
//! ticket (T-5702); the Job owns the Secret, and the garbage collector removes it with the Job.

use crate::agents::profile::Profile;
use crate::agents::run::AgentRun;
use crate::apps::kube::KubeClient;

/// The label the credential proxy's pods carry (`nameOverride` of the `agent-runner` proxy
/// release). The proxy runs in the same namespace as the workspaces it serves, which is what
/// lets the egress rule be a pod selector rather than a namespace the Portal would have to be
/// told about.
const PROXY_POD_LABEL: &str = "agent-runner-proxy";
/// The label the Portal's own pods carry (`nameOverride` of the `portal` release). The Portal
/// drives the agent server over HTTP, so it is the one peer allowed in.
const PORTAL_POD_LABEL: &str = "portal-portal";
/// Where `openhands-agent-server` listens inside the workspace.
pub const AGENT_SERVER_PORT: u16 = 8000;
/// Where `jc-agent-proxy` listens.
pub const PROXY_PORT: u16 = 8080;
/// Linkerd's inbound proxy port: a meshed peer reaches the workspace, and the workspace reaches
/// the credential proxy, here rather than on the service port.
const MESH_INBOUND_PORT: u16 = 4143;
/// The Linkerd control plane's namespace and the ports a meshed pod must reach: identity (8080),
/// destination (8086) and policy (8090).
const MESH_NAMESPACE: &str = "linkerd";
const MESH_CONTROL_PORTS: [u16; 3] = [8080, 8086, 8090];
/// The key of the run's Secret that holds its ticket.
const TICKET_KEY: &str = "ticket";

/// The Secret holding one run's ticket, named as the run's other objects. Opaque: the
/// admission policy `{release}-portal-secrets` admits `agent-run-{id}` of that type alone.
fn ticket_secret(
    name: &str,
    namespace: &str,
    labels: &serde_json::Value,
    ticket: &str,
) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "type": "Opaque",
        "metadata": { "name": name, "namespace": namespace, "labels": labels },
        "stringData": { TICKET_KEY: ticket },
    })
}

/// The workspace's environment (AG-40). Each of the three variables that carry the ticket reads
/// it from the run's Secret `secret`: the pod spec itself holds no credential.
fn workspace_env(
    run: &AgentRun,
    proxy_base: &str,
    model_name: &str,
    secret: &str,
) -> serde_json::Value {
    let ticket = serde_json::json!({ "secretKeyRef": { "name": secret, "key": TICKET_KEY } });
    serde_json::json!([
        // What the workspace may know: where the proxy is, which run it is, and the ticket
        // that proves it. No model key, no forge token, no realm secret (AG-40).
        { "name": "JC_PROXY_BASE", "value": proxy_base },
        { "name": "JC_RUN_ID", "value": run.id },
        { "name": "JC_RUN_TICKET", "valueFrom": ticket },
        { "name": "JC_APP_NAME", "value": run.app_name },
        { "name": "JC_APP_CLASS", "value": run.app_class },
        { "name": "JC_BRANCH", "value": run.branch },
        { "name": "JC_PATH_PREFIX", "value": run.path_prefix },
        // The agent server's own API key, so only the Portal can drive it. It is the run
        // ticket: one secret per run, minted once, and the proxy is what makes it worth
        // nothing to anyone else.
        { "name": "OH_SESSION_API_KEYS_0", "valueFrom": ticket },
        // The model is reached through the proxy, which is the only place a provider key
        // exists. The base URL is a workspace-visible address; the key is the ticket again.
        { "name": "LLM_BASE_URL", "value": format!("{proxy_base}/v1/llm/v1") },
        { "name": "LLM_API_KEY", "valueFrom": ticket },
        { "name": "LLM_MODEL", "value": model_name },
        { "name": "HOME", "value": "/workspace" }
    ])
}

/// Schedules one run's workspace. `Ok(false)` means this Portal has no cluster to schedule in,
/// which is the development shape: the caller drives the agent server itself and is handed the
/// ticket in the answer.
///
/// What the workspace is started with, and all it is allowed to know (AG-40):
///
/// - `JC_PROXY_BASE` — the credential proxy, the one address a run may call out to.
/// - `JC_RUN_ID` — the run this workspace is, as the Portal records it.
/// - `JC_RUN_TICKET` — the bearer that proves it to the proxy: minted for this run, spent with
///   it, and worth nothing anywhere else.
/// - `JC_APP_NAME`, `JC_APP_CLASS` — the application being built and what kind it is.
/// - `JC_BRANCH` — the branch the run proposes its change on.
/// - `JC_PATH_PREFIX` — where the built application will be served, so the code it writes uses
///   the right base path.
///
/// No model key, no forge token and no realm secret is among them: each of those stays in the
/// proxy, which is the whole reason a run talks to one.
#[allow(clippy::too_many_arguments)]
pub async fn schedule_workspace_job(
    kube: Option<&KubeClient>,
    namespace: &str,
    portal_namespace: &str,
    run: &AgentRun,
    ticket: &str,
    proxy_base: &str,
    profile: &Profile,
    ttl_secs: i64,
) -> Result<bool, String> {
    let Some(kube) = kube else {
        tracing::info!(
            run_id = %run.id,
            "no cluster: the workspace is not scheduled and the ticket goes to the caller"
        );
        return Ok(false);
    };

    let name = format!("agent-run-{}", run.id);
    let labels = serde_json::json!({
        "app.kubernetes.io/name": name,
        "app.kubernetes.io/part-of": "joinedcontext",
        "joinedcontext.com/run-id": run.id,
    });

    let service_account = serde_json::json!({
        "apiVersion": "v1",
        "kind": "ServiceAccount",
        "metadata": { "name": name, "namespace": namespace, "labels": labels },
        // AG-34: the workspace has no business talking to the Kubernetes API at all.
        "automountServiceAccountToken": false,
    });

    let network_policy = workspace_policy(&name, namespace, portal_namespace, &labels, &run.id);

    let job = serde_json::json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": { "name": name, "namespace": namespace, "labels": labels },
        "spec": {
            // One attempt: a builder run is a conversation, and replaying half of one against
            // a ticket that is already spent would only spend the budget twice (AG-41).
            "backoffLimit": 0,
            // The same number the run record expires on, so the pod and the record cannot
            // disagree about when the run is over (AG-43).
            "activeDeadlineSeconds": ttl_secs,
            "ttlSecondsAfterFinished": 600,
            "template": {
                "metadata": { "labels": labels, "annotations": mesh_annotations() },
                "spec": {
                    "restartPolicy": "Never",
                    "automountServiceAccountToken": false,
                    "serviceAccountName": name,
                    "enableServiceLinks": false,
                    "securityContext": {
                        "runAsNonRoot": true,
                        "runAsUser": 10001,
                        "fsGroup": 10001,
                        "seccompProfile": { "type": "RuntimeDefault" }
                    },
                    "containers": [{
                        "name": "agent-server",
                        "image": profile.image,
                        // The upstream image's own entrypoint, with the port and the session
                        // key this run alone knows. Nothing is built on top of it: a custom
                        // image would be a second thing to sign and pin (ADR-N-020).
                        "command": ["openhands-agent-server"],
                        "args": [
                            "--host", "0.0.0.0",
                            format!("--port={AGENT_SERVER_PORT}"),
                        ],
                        "securityContext": {
                            "readOnlyRootFilesystem": true,
                            "allowPrivilegeEscalation": false,
                            "capabilities": { "drop": ["ALL"] }
                        },
                        "resources": {
                            "requests": { "cpu": profile.cpu, "memory": profile.memory },
                            "limits": { "cpu": profile.cpu, "memory": profile.memory }
                        },
                        "ports": [{ "name": "agent", "containerPort": AGENT_SERVER_PORT }],
                        "env": workspace_env(run, proxy_base, &profile.model_name, &name),
                        "volumeMounts": [
                            { "name": "workspace", "mountPath": "/workspace" },
                            // `readOnlyRootFilesystem` leaves nowhere to write a temporary
                            // file, and every toolchain wants one.
                            { "name": "tmp", "mountPath": "/tmp" }
                        ]
                    }],
                    "volumes": [
                        { "name": "workspace", "emptyDir": { "sizeLimit": profile.ephemeral_storage } },
                        { "name": "tmp", "emptyDir": { "sizeLimit": "1Gi" } }
                    ]
                }
            }
        }
    });

    // The policy before the Job: a pod that starts before the rule that holds it in would have
    // a window with the namespace's default egress, whatever that is.
    kube.apply(&service_account)
        .await
        .map_err(|err| err.to_string())?;
    kube.apply(&network_policy)
        .await
        .map_err(|err| err.to_string())?;
    let mut secret = ticket_secret(&name, namespace, &labels, ticket);
    kube.apply(&secret).await.map_err(|err| err.to_string())?;
    kube.apply(&job).await.map_err(|err| err.to_string())?;
    // The Job owns the Secret, so the garbage collector removes it with the Job after
    // `ttlSecondsAfterFinished`. Without the owner the Secret stays until the run's deletion
    // removes it, so a failure here costs nothing but that.
    match kube.get("batch/v1", "Job", namespace, &name).await {
        Ok(Some(created)) if created["metadata"]["uid"].is_string() => {
            secret["metadata"]["ownerReferences"] = serde_json::json!([{
                "apiVersion": "batch/v1",
                "kind": "Job",
                "name": name,
                "uid": created["metadata"]["uid"],
            }]);
            if let Err(err) = kube.apply(&secret).await {
                tracing::warn!(run_id = %run.id, error = %err, "the run's ticket Secret is not owned by its Job");
            }
        }
        Ok(_) => {
            tracing::warn!(run_id = %run.id, "the run's Job is not readable: its ticket Secret has no owner")
        }
        Err(err) => {
            tracing::warn!(run_id = %run.id, error = %err, "the run's ticket Secret is not owned by its Job")
        }
    }

    Ok(true)
}

/// Removes one run's four objects. Removing what is not there succeeds, so a cancellation of a
/// run that never got a pod is not an error.
pub async fn delete_workspace_job(kube: Option<&KubeClient>, namespace: &str, run_id: &str) {
    if let Some(kube) = kube {
        let name = format!("agent-run-{run_id}");
        let _ = kube.delete("batch/v1", "Job", namespace, &name).await;
        let _ = kube
            .delete("networking.k8s.io/v1", "NetworkPolicy", namespace, &name)
            .await;
        let _ = kube.delete("v1", "ServiceAccount", namespace, &name).await;
        let _ = kube.delete("v1", "Secret", namespace, &name).await;
    }
}

/// The workspace's Linkerd annotations (F-PLT1-008). The credential proxy honours a run's ticket
/// from the run's own workload alone, and `agent-run-{id}` is the identity the mesh gives this
/// pod. The proxy runs as a native sidecar (an init container that keeps running), so the Job
/// still completes when the agent server exits.
fn mesh_annotations() -> serde_json::Value {
    serde_json::json!({
        "linkerd.io/inject": "enabled",
        "config.alpha.linkerd.io/proxy-enable-native-sidecar": "true",
    })
}

/// The rule that holds a workspace in: the Portal in, the credential proxy, DNS and the mesh's
/// control plane out, and nothing else.
fn workspace_policy(
    name: &str,
    namespace: &str,
    portal_namespace: &str,
    labels: &serde_json::Value,
    run_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": { "name": name, "namespace": namespace, "labels": labels },
        "spec": {
            "podSelector": { "matchLabels": { "joinedcontext.com/run-id": run_id } },
            "policyTypes": ["Ingress", "Egress"],
            "ingress": [{
                // The Portal drives the agent server. Nothing else reaches the workspace, and
                // the workspace is not a Service, so nothing else can find it either. The
                // namespace is named as well as the label: an empty `namespaceSelector` would
                // admit any pod anywhere carrying that label, and a pod's labels are the one
                // thing a namespace's own owner writes (AG-34, AG-39).
                "from": [{
                    "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": portal_namespace } },
                    "podSelector": { "matchLabels": { "app.kubernetes.io/name": PORTAL_POD_LABEL } }
                }],
                "ports": [
                    { "protocol": "TCP", "port": AGENT_SERVER_PORT },
                    { "protocol": "TCP", "port": MESH_INBOUND_PORT }
                ]
            }],
            "egress": [
                {
                    "to": [{
                        "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "kube-system" } },
                        "podSelector": { "matchLabels": { "k8s-app": "kube-dns" } }
                    }],
                    "ports": [
                        { "protocol": "UDP", "port": 53 },
                        { "protocol": "TCP", "port": 53 }
                    ]
                },
                {
                    // The credential proxy, by pod, in this namespace. A rule with no `to`
                    // would open the port on every address in the cluster, which is how a
                    // workspace would reach the broker or Keycloak directly (AG-40).
                    "to": [{
                        "podSelector": { "matchLabels": { "app.kubernetes.io/name": PROXY_POD_LABEL } }
                    }],
                    "ports": [
                        { "protocol": "TCP", "port": PROXY_PORT },
                        { "protocol": "TCP", "port": MESH_INBOUND_PORT }
                    ]
                },
                {
                    // The mesh's control plane: without it the pod gets no identity, and the
                    // credential proxy refuses a ticket from a workload it cannot name.
                    "to": [{
                        "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": MESH_NAMESPACE } }
                    }],
                    "ports": MESH_CONTROL_PORTS
                        .map(|port| serde_json::json!({ "protocol": "TCP", "port": port }))
                }
            ]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> AgentRun {
        serde_json::from_value(serde_json::json!({
            "id": "r1", "project": "helsinki", "appName": "air", "endpointName": "air",
            "endpointSlug": "air", "endpoints": [], "profile": "analyst", "kind": "data",
            "unattended": false, "appClass": "ui", "visibility": "project", "prompt": "p",
            "promptDigest": "sha256:p", "dataNeeds": [], "allowsWrite": false, "branch": "b",
            "pathPrefix": "p/", "status": "building", "ticketHash": "x", "files": {},
            "steps": 0, "tokensUsed": 0, "createdBy": "ana@hel.fi", "origin": "person",
            "starter": null, "createdAt": "2026-10-25T00:00:00Z", "expiresAt": "2099-01-01T00:00:00Z",
        }))
        .expect("a run")
    }

    #[test]
    fn the_workspace_env_reads_every_ticket_variable_from_the_runs_secret() {
        let env = workspace_env(&run(), "http://proxy:8080", "m", "agent-run-r1");
        let from_secret: Vec<&str> = env
            .as_array()
            .expect("a list")
            .iter()
            .filter(|var| {
                var["valueFrom"]["secretKeyRef"]
                    == serde_json::json!({ "name": "agent-run-r1", "key": "ticket" })
            })
            .filter_map(|var| var["name"].as_str())
            .collect();
        assert_eq!(
            from_secret,
            ["JC_RUN_TICKET", "OH_SESSION_API_KEYS_0", "LLM_API_KEY"]
        );
    }

    /// F-PLT1-008: the proxy honours a ticket from the run's own mesh identity alone, so the
    /// workspace is meshed and its rule lets the mesh in and out.
    #[test]
    fn the_workspace_is_meshed_and_its_rule_lets_the_mesh_through() {
        assert_eq!(mesh_annotations()["linkerd.io/inject"], "enabled");
        assert_eq!(
            mesh_annotations()["config.alpha.linkerd.io/proxy-enable-native-sidecar"],
            "true",
            "a sidecar that outlives the agent server would hold the Job open"
        );
        let labels = serde_json::json!({ "joinedcontext.com/run-id": "r1" });
        let policy = workspace_policy("agent-run-r1", "dev", "jc-portal", &labels, "r1");
        let spec = &policy["spec"];
        let ports = |rule: &serde_json::Value| -> Vec<u64> {
            rule["ports"]
                .as_array()
                .map(|ports| ports.iter().filter_map(|p| p["port"].as_u64()).collect())
                .unwrap_or_default()
        };
        assert_eq!(ports(&spec["ingress"][0]), [8000, 4143]);
        assert_eq!(
            spec["ingress"][0]["from"][0]["namespaceSelector"]["matchLabels"]
                ["kubernetes.io/metadata.name"],
            "jc-portal"
        );
        assert_eq!(
            ports(&spec["egress"][1]),
            [8080, 4143],
            "the credential proxy"
        );
        assert_eq!(
            spec["egress"][2]["to"][0]["namespaceSelector"]["matchLabels"]
                ["kubernetes.io/metadata.name"],
            "linkerd"
        );
        assert_eq!(ports(&spec["egress"][2]), [8080, 8086, 8090]);
        assert_eq!(
            spec["egress"].as_array().map(Vec::len),
            Some(3),
            "DNS, the proxy and the control plane, nothing else"
        );
    }

    #[test]
    fn the_ticket_secret_is_opaque_and_named_as_the_run() {
        let secret = ticket_secret("agent-run-r1", "dev", &serde_json::json!({}), "tk-live");
        assert_eq!(secret["type"], "Opaque");
        assert_eq!(secret["metadata"]["name"], "agent-run-r1");
        assert_eq!(
            secret["stringData"],
            serde_json::json!({ "ticket": "tk-live" })
        );
    }
}
