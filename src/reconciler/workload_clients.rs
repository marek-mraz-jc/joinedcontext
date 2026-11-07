//! The Keycloak client of every `ServiceAccount` bound to a cluster workload (PF-47, T-1513), and
//! of every one that signs in from outside the cluster (PF-37, T-5626).
//!
//! An account with `spec.workload.kubernetes: { namespace, serviceAccount }` gets its derived
//! client `{project}-{name}` with federated client authentication: Keycloak accepts the pod's
//! projected Kubernetes ServiceAccount token as the client assertion, through the realm's
//! identity provider `kubernetes`, for that one subject. No secret opens it and none is read
//! here: Keycloak generates one for every confidential client, and with this authenticator a
//! `client_secret` request is refused.
//!
//! The client grants `client_credentials` only. Its audience mappers name the slug of every
//! Endpoint of the project on a space one of the account's roles scopes, and the Portal's own
//! audience when one of its roles names a `Role` (Architecture/12 §2a), and the client
//! `mqtt-{project}-{broker}` of every `MqttBroker` whose roles name it (ADR-N-048 §2.7), so the token opens the
//! doors the roles are for and no other. An App's job principal `appjob-{app}` (AP-159) is bound
//! the same way to `appjob-{project}-{app}` in the identities namespace, and its one audience is
//! the App's own Endpoint.
//!
//! An account without a workload whose `oauth-client` credential is used from outside the cluster
//! (an external system, a device) gets the same client id with `client-secret` authentication, or
//! `client-jwt` against the public keys its credentials list (T-4015, T-5626): the same audiences,
//! `client_credentials` only. Keycloak generates the secret; the Portal shows a new one once to a
//! person who may approve ServiceAccounts (`api::account_secret`) and never stores it.
//!
//! This wave is the only writer of such a client, as the App wave is of an App's: the client
//! carries `managed-by: joinedcontext` and the account it belongs to, a change made in the
//! console is written back and reported, and a managed client whose account is gone or no longer
//! bound to a workload is deleted. A client of that id without the attribute belongs to whoever
//! made it (the platform's own clients of `keycloak-clients.yaml`, say) and is never written or
//! removed; the account is reported instead.

use crate::config::ClientAuth;
use std::collections::{BTreeMap, BTreeSet};

use jc_core::kinds::service_account::{keycloak_client_id, KubernetesBinding};
use jc_core::kinds::ServiceAccountSpec;
use serde_json::{json, Value};

use super::app_clients::{audience_mapper, drift, managed, Admin, ClientOutcome, KcNamed};
use super::groups::{MANAGED_BY, MANAGED_VALUE};
use super::streams::Bentos;
use crate::store::{ListOptions, Mirror};

/// The client attribute naming the account a managed workload client belongs to,
/// `{project}/{name}`.
pub const ACCOUNT_ATTRIBUTE: &str = "joinedcontext.serviceaccount";

/// The one client scope an account's token carries (T-6051): `basic`, which writes `sub`. The
/// realm's defaults would add `acr`, `email`, `profile`, `roles` (the `account` audience and the
/// realm's default roles), `web-origins`, and `service_account` (`client_id`, `clientHost`,
/// `clientAddress`), none of which the gateway, the MQTT door or the internal listener reads:
/// they decide by `azp`, `aud`, `sub` and `exp`. Measured on Keycloak 26.8.1, a `client-secret`
/// client's token falls from 1266 to 781 bytes. Not part of [`desired`], because a PUT of the
/// client ignores its scopes and Keycloak puts `service_account` back on every create and PUT;
/// [`WorkloadClientSync`] converges them through the client's scope endpoints after each write.
pub const TOKEN_SCOPES: [&str; 1] = ["basic"];

/// `want` as the body that creates the client: [`TOKEN_SCOPES`] its only default scopes and no
/// optional one, so a fresh client never issues a token with the realm's defaults (T-6051).
pub fn creation(want: &Value) -> Value {
    let mut body = want.clone();
    body["defaultClientScopes"] = json!(TOKEN_SCOPES);
    body["optionalClientScopes"] = json!([]);
    body
}

/// The default scopes in `default` and every scope in `optional` that are not [`TOKEN_SCOPES`],
/// as (`default` or `optional`, scope) to remove, and the [`TOKEN_SCOPES`] `default` lacks.
fn scope_plan<'a>(
    default: &'a [KcNamed],
    optional: &'a [KcNamed],
) -> (Vec<(&'static str, &'a KcNamed)>, Vec<&'static str>) {
    let remove = default
        .iter()
        .filter(|scope| !TOKEN_SCOPES.contains(&scope.name.as_str()))
        .map(|scope| ("default", scope))
        .chain(optional.iter().map(|scope| ("optional", scope)))
        .collect();
    let missing = TOKEN_SCOPES
        .into_iter()
        .filter(|name| !default.iter().any(|scope| scope.name == *name))
        .collect();
    (remove, missing)
}

/// The alias of the realm's Kubernetes identity provider the clients trust (PF-47).
pub const IDENTITY_PROVIDER: &str = "kubernetes";

/// The subject a pod's projected token carries for a Kubernetes ServiceAccount.
pub fn subject(binding: &KubernetesBinding) -> String {
    format!(
        "system:serviceaccount:{}:{}",
        binding.namespace, binding.service_account
    )
}

/// The client a workload-bound account should have, as Keycloak's representation (PF-47).
pub fn desired(project: &str, name: &str, binding: &KubernetesBinding) -> Value {
    json!({
        "clientId": keycloak_client_id(project, name),
        "name": format!("ServiceAccount {project}/{name}"),
        "protocol": "openid-connect",
        "enabled": true,
        "publicClient": false,
        "bearerOnly": false,
        "standardFlowEnabled": false,
        "implicitFlowEnabled": false,
        "directAccessGrantsEnabled": false,
        "serviceAccountsEnabled": true,
        "clientAuthenticatorType": "federated-jwt",
        "attributes": {
            "jwt.credential.issuer": IDENTITY_PROVIDER,
            "jwt.credential.sub": subject(binding),
            "oauth2.device.authorization.grant.enabled": "false",
            MANAGED_BY: MANAGED_VALUE,
            ACCOUNT_ATTRIBUTE: format!("{project}/{name}"),
        },
    })
}

/// How an account outside the cluster proves itself (PF-37, PF-47): by the secret Keycloak keeps,
/// which a person issues once through the Portal (`api::account_secret`), or by the public keys
/// its credentials list, with which a device signs its assertion (T-4015).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientCredential {
    /// `client-secret`: Keycloak generates it; the Portal never reads or stores it here.
    Secret,
    /// `client-jwt` against these keys, as the JWKS string Keycloak keeps.
    Keys(String),
}

impl ClientCredential {
    /// What the account's `oauth-client` credentials ask for, or `None` when it has none or is
    /// bound to a workload (whose client is [`desired`]). Keys win over a bare credential: one
    /// client has one authenticator, and a key pair never travels as a secret does.
    pub fn of(spec: &ServiceAccountSpec) -> Option<Self> {
        if spec.workload.is_some() || spec.credentials.is_empty() {
            return None;
        }
        let mut kids = BTreeSet::new();
        let keys: Vec<&jc_core::kinds::service_account::PublicJwk> = spec
            .credentials
            .iter()
            .filter_map(|credential| credential.jwks.as_ref())
            .flat_map(|jwks| &jwks.keys)
            .filter(|key| kids.insert(key.kid.as_str()))
            .collect();
        if keys.is_empty() {
            return Some(Self::Secret);
        }
        Some(Self::Keys(json!({ "keys": keys }).to_string()))
    }
}

/// The client of an account that signs in from outside the cluster (PF-37, T-5626): confidential,
/// `client_credentials` only, its authenticator what [`ClientCredential`] says. No secret is in
/// it: Keycloak generates one for a `client-secret` client, and the Portal shows a new one once.
pub fn desired_external(project: &str, name: &str, credential: &ClientCredential) -> Value {
    let mut want = json!({
        "clientId": keycloak_client_id(project, name),
        "name": format!("ServiceAccount {project}/{name}"),
        "protocol": "openid-connect",
        "enabled": true,
        "publicClient": false,
        "bearerOnly": false,
        "standardFlowEnabled": false,
        "implicitFlowEnabled": false,
        "directAccessGrantsEnabled": false,
        "serviceAccountsEnabled": true,
        "clientAuthenticatorType": "client-secret",
        "attributes": {
            "oauth2.device.authorization.grant.enabled": "false",
            MANAGED_BY: MANAGED_VALUE,
            ACCOUNT_ATTRIBUTE: format!("{project}/{name}"),
        },
    });
    if let ClientCredential::Keys(jwks) = credential {
        want["clientAuthenticatorType"] = json!("client-jwt");
        want["attributes"]["use.jwks.url"] = json!("false");
        want["attributes"]["use.jwks.string"] = json!("true");
        want["attributes"]["jwks.string"] = json!(jwks);
    }
    want
}

/// The audiences an account's token carries (PF-47): the slug of every Endpoint of `project` on
/// a space one of its roles scopes, and `portal` when it acts on the Portal. Sorted, no repeats.
pub fn audiences(
    mirror: &Mirror,
    project: &str,
    spec: &ServiceAccountSpec,
    portal: &str,
) -> Vec<String> {
    let spaces: BTreeSet<&str> = spec
        .roles
        .iter()
        .filter_map(|granted| granted.scope.context_space.as_deref())
        .collect();
    let mut found: BTreeSet<String> = mirror
        .list(project, "Endpoint", &ListOptions::default())
        .items
        .into_iter()
        .filter(|endpoint| {
            endpoint
                .spec
                .get("contextSpaceRef")
                .and_then(crate::resource::reference_name)
                .is_some_and(|space| spaces.contains(space))
        })
        .filter_map(|endpoint| {
            endpoint
                .spec
                .get("slug")
                .and_then(Value::as_str)
                .filter(|slug| !slug.is_empty())
                .map(str::to_owned)
        })
        .collect();
    if crate::permissions::account_holds_portal_role(mirror, spec) {
        found.insert(portal.to_owned());
    }
    found.into_iter().collect()
}

/// What a bound account's token is for.
#[derive(Debug, Clone)]
enum Reach {
    /// The doors its roles open, by [`audiences`].
    Roles(ServiceAccountSpec),
    /// The Endpoints a Pipeline names, by slug, by [`named_endpoints`]: never every Endpoint of
    /// the spaces its derived roles scope, which grew a Bratislava token past the edge's header
    /// limit (T-5757).
    Pipeline(Vec<String>),
    /// One Endpoint and nothing else: an App's job principal (AP-159).
    Endpoint(String),
    /// The Portal's internal listener and nothing else: a project's pipeline-test harness
    /// (PL-43, T-4050), whose one call is its own capture route.
    Internal,
}

/// One account bound to a workload, or signing in from outside (`binding` `None`), as this wave
/// needs it.
#[derive(Debug, Clone)]
struct BoundAccount {
    project: String,
    name: String,
    reach: Reach,
    auth: Auth,
    /// A pipeline identity (the project's `pipelines` account or a Pipeline's own), whose
    /// streams report their outcomes to the Portal's internal listener (T-4038).
    reports: bool,
}

/// What opens an account's client.
#[derive(Debug, Clone)]
enum Auth {
    /// A pod's projected token for this Kubernetes ServiceAccount: [`desired`].
    Workload(KubernetesBinding),
    /// A secret or a key pair held outside the cluster: [`desired_external`].
    External(ClientCredential),
}

impl BoundAccount {
    fn binding(&self) -> Option<&KubernetesBinding> {
        match &self.auth {
            Auth::Workload(binding) => Some(binding),
            Auth::External(_) => None,
        }
    }

    fn label(&self) -> String {
        format!("{}/{}", self.project, self.name)
    }
}

/// The accounts of the mirror that name a workload, sorted so a run is deterministic. One that
/// does not parse binds nothing: `validate` is what reports it.
fn bound(mirror: &Mirror) -> Vec<BoundAccount> {
    let mut accounts: Vec<BoundAccount> = mirror
        .namespaces()
        .into_iter()
        .flat_map(|project| {
            mirror
                .list(&project, "ServiceAccount", &ListOptions::default())
                .items
                .into_iter()
                .filter_map(move |envelope| {
                    let spec: ServiceAccountSpec = serde_json::from_value(envelope.spec).ok()?;
                    let auth = match &spec.workload {
                        Some(workload) => Auth::Workload(workload.kubernetes.clone()),
                        None => Auth::External(ClientCredential::of(&spec)?),
                    };
                    Some(BoundAccount {
                        project: project.clone(),
                        reports: envelope.metadata.name
                            == crate::reconciler::streams::PIPELINE_ACCOUNT,
                        name: envelope.metadata.name,
                        reach: Reach::Roles(spec),
                        auth,
                    })
                })
        })
        .collect();
    accounts.sort_by(|a, b| (&a.project, &a.name).cmp(&(&b.project, &b.name)));
    accounts
}

/// Every Pipeline's own account, derived as the gateway derives it (PL-19): bound to its own
/// Kubernetes ServiceAccount in the runner's `namespace`, its audiences the Endpoints it names
/// ([`named_endpoints`], T-5757), its mapping read from `bentos`; and each project's harness
/// principal beside them (T-4050).
fn pipeline_accounts(mirror: &Mirror, namespace: &str, bentos: &Bentos) -> Vec<BoundAccount> {
    let mut accounts = Vec::new();
    for project in mirror.namespaces() {
        let slugs: BTreeMap<String, String> = mirror
            .list(&project, "Endpoint", &ListOptions::default())
            .items
            .into_iter()
            .filter_map(|endpoint| {
                let slug = endpoint.spec.get("slug").and_then(Value::as_str)?;
                (!slug.is_empty()).then(|| (endpoint.metadata.name, slug.to_owned()))
            })
            .collect();
        let space_of = |endpoint: &str| {
            mirror
                .get(&project, "Endpoint", endpoint)
                .and_then(|envelope| {
                    let reference = envelope.spec.get("contextSpaceRef")?;
                    reference
                        .as_str()
                        .or_else(|| reference.get("name").and_then(Value::as_str))
                        .map(str::to_owned)
                })
        };
        for envelope in mirror
            .list(&project, "Pipeline", &ListOptions::default())
            .items
        {
            // jc-core refuses the name; one in the mirror anyway must not share the harness's client.
            if envelope.metadata.name == jc_core::kinds::pipeline_identity::HARNESS {
                continue;
            }
            let mapping = format!(
                "{}\n{}",
                envelope.spec,
                bentos
                    .get(&(project.clone(), envelope.metadata.name.clone()))
                    .map_or("", String::as_str)
            );
            let Ok(spec) = serde_json::from_value::<jc_core::kinds::PipelineSpec>(envelope.spec)
            else {
                continue;
            };
            let derived = jc_core::kinds::pipeline_identity::derive(
                &project,
                &envelope.metadata.name,
                &spec,
                namespace,
                // Only the account is read here; its Policies are the gateway's, assigned there.
                "",
                space_of,
            );
            let Some(binding) = derived
                .account
                .workload
                .as_ref()
                .map(|w| w.kubernetes.clone())
            else {
                continue;
            };
            accounts.push(BoundAccount {
                project: project.clone(),
                name: derived.name,
                reach: Reach::Pipeline(named_endpoints(&spec, &mapping, &slugs)),
                auth: Auth::Workload(binding),
                reports: true,
            });
        }
        accounts.push(harness_account(&project, namespace));
    }
    accounts.sort_by(|a, b| (&a.project, &a.name).cmp(&(&b.project, &b.name)));
    accounts
}

/// The slugs of the Endpoints a Pipeline names (T-5757): its outputs' and its Endpoint source's,
/// as [`jc_core::kinds::pipeline_identity::derive`] resolves them, and each slug of `slugs` (its
/// project's Endpoints by name) that `mapping`, its manifest and `bento.yaml` text, calls as
/// `api/endpoint/{slug}`. A slug of another project, or none, names nothing. Sorted, no repeats.
fn named_endpoints(
    spec: &jc_core::kinds::PipelineSpec,
    mapping: &str,
    slugs: &BTreeMap<String, String>,
) -> Vec<String> {
    let own: BTreeSet<&str> = slugs.values().map(String::as_str).collect();
    let called = mapping.split("api/endpoint/").skip(1).filter_map(|rest| {
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        own.get(&rest[..end]).copied()
    });
    let declared = spec
        .outputs()
        .into_iter()
        .map(|output| output.target_endpoint.local_id().to_owned())
        .chain(
            spec.source
                .as_ref()
                .and_then(|source| source.endpoint_ref.as_ref())
                .map(|endpoint| endpoint.name().to_owned()),
        )
        .filter_map(|name| slugs.get(&name).map(String::as_str))
        .collect::<Vec<_>>();
    declared
        .into_iter()
        .chain(called)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// `found` with every MQTT broker whose roles name the account (ADR-N-048 §2.7) and, for a
/// pipeline identity, the Portal's internal listener its streams' outcome, rejected and samples
/// sinks post to (T-4038; the listener still admits it only for its own project's pipelines,
/// T-3193). Sorted, no repeats.
fn reporting(mirror: &Mirror, account: &BoundAccount, mut found: Vec<String>) -> Vec<String> {
    found.extend(super::mqtt_brokers::audiences_of(
        mirror,
        &account.project,
        &account.name,
    ));
    if account.reports {
        found.push(crate::auth::internal::INTERNAL_AUDIENCE.to_owned());
    }
    found.sort();
    found.dedup();
    found
}

/// A project's pipeline-test harness principal (PL-43, T-4050): `pl-pipeline-test`, bound like a
/// pipeline's account to `pl-{project}-pipeline-test` in the runner's `namespace`, so the token
/// service mints its token and the harness and the DataSource probes present no secret. It
/// reaches the Portal's internal listener alone and holds no Policy.
fn harness_account(project: &str, namespace: &str) -> BoundAccount {
    use jc_core::kinds::pipeline_identity::{account_name, kubernetes_service_account, HARNESS};
    BoundAccount {
        project: project.to_owned(),
        name: account_name(HARNESS),
        reach: Reach::Internal,
        auth: Auth::Workload(KubernetesBinding {
            namespace: namespace.to_owned(),
            service_account: kubernetes_service_account(project, HARNESS),
        }),
        reports: false,
    }
}

/// The job principal of every published `wasm` App with `spec.server.jobs` and its own Endpoint
/// (AP-154, AP-159): `appjob-{app}`, bound to `appjob-{project}-{app}` in the identities
/// `namespace`, its one audience that Endpoint. An App without jobs, or whose Endpoint is not
/// committed yet, has none: nothing would run as it, or nothing could it call.
fn app_job_accounts(mirror: &Mirror, namespace: &str) -> Vec<BoundAccount> {
    use jc_core::kinds::app_identity;
    let mut accounts: Vec<BoundAccount> = super::wasm_shards::PlacedApp::all(
        &mirror.matching(|env| env.kind == "App" || env.kind == "Endpoint"),
    )
    .into_iter()
    .filter(|app| !app.jobs.is_empty())
    .filter_map(|app| {
        let slug = app.endpoint.filter(|slug| !slug.is_empty())?;
        let binding = serde_json::from_value(json!({
            "namespace": namespace,
            "serviceAccount": app_identity::kubernetes_service_account(&app.project, &app.name),
        }))
        .ok()?;
        Some(BoundAccount {
            name: app_identity::account_name(&app.name),
            project: app.project,
            reach: Reach::Endpoint(slug),
            reports: false,
            auth: Auth::Workload(binding),
        })
    })
    .collect();
    accounts.sort_by(|a, b| (&a.project, &a.name).cmp(&(&b.project, &b.name)));
    accounts
}

/// The label every Kubernetes ServiceAccount of a Pipeline's account carries, so a run finds the
/// ones it made and nothing else (PL-19, T-1508).
pub const PIPELINE_IDENTITY_LABEL: &str = "joinedcontext.com/pipeline-identity";

/// The label every Kubernetes ServiceAccount of an App's job principal carries (AP-159).
pub const APP_JOB_IDENTITY_LABEL: &str = "joinedcontext.com/app-job-identity";

/// The Kubernetes ServiceAccount of each account, labelled `label`: no pod mounts it, the token
/// service mints its tokens (Architecture/12 §3), so its own token is never automounted anywhere.
fn service_accounts(accounts: Vec<BoundAccount>, label: &str) -> Vec<Value> {
    accounts
        .iter()
        .filter_map(|account| {
            let binding = account.binding()?;
            Some(json!({
                "apiVersion": "v1",
                "kind": "ServiceAccount",
                "metadata": {
                    "name": binding.service_account,
                    "namespace": binding.namespace,
                    "labels": {
                        label: "true",
                        "app.kubernetes.io/managed-by": "joinedcontext-portal",
                    },
                    "annotations": { ACCOUNT_ATTRIBUTE: account.label() },
                },
                "automountServiceAccountToken": false,
            }))
        })
        .collect()
}

/// The Kubernetes ServiceAccount of every Pipeline's own account in `namespace` (PL-19).
pub fn pipeline_service_accounts(mirror: &Mirror, namespace: &str) -> Vec<Value> {
    service_accounts(
        pipeline_accounts(mirror, namespace, &Bentos::new()),
        PIPELINE_IDENTITY_LABEL,
    )
}

/// The Kubernetes ServiceAccount of every App's job principal in `namespace` (AP-159).
pub fn app_job_service_accounts(mirror: &Mirror, namespace: &str) -> Vec<Value> {
    service_accounts(app_job_accounts(mirror, namespace), APP_JOB_IDENTITY_LABEL)
}

/// Applies `wanted` in `namespace`, deletes a ServiceAccount labelled `label` that `wanted` does
/// not name, and answers what failed. One without the label, or whose name `derived` refuses, is
/// never touched.
async fn converge_service_accounts(
    kube: &crate::apps::kube::KubeClient,
    namespace: &str,
    wanted: Vec<Value>,
    label: &str,
    derived: fn(&str) -> bool,
) -> Vec<String> {
    let mut failed = Vec::new();
    for account in &wanted {
        if let Err(err) = kube.apply(account).await {
            failed.push(format!("{}: {err}", account["metadata"]["name"]));
        }
    }
    let names: BTreeSet<&str> = wanted
        .iter()
        .filter_map(|a| a["metadata"]["name"].as_str())
        .collect();
    let listed = match kube
        .list("v1", "ServiceAccount", namespace, &format!("{label}=true"))
        .await
    {
        Ok(listed) => listed,
        Err(err) => {
            failed.push(format!("listing {namespace}: {err}"));
            return failed;
        }
    };
    for stale in listed
        .iter()
        .filter_map(|a| a["metadata"]["name"].as_str())
        .filter(|name| derived(name) && !names.contains(name))
    {
        if let Err(err) = kube.delete("v1", "ServiceAccount", namespace, stale).await {
            failed.push(format!("{stale}: {err}"));
        }
    }
    failed
}

/// Brings the pipelines' Kubernetes ServiceAccounts in `namespace` to the mirror's Pipelines
/// (PL-19); only a labelled `pl-…` one is ever deleted.
pub async fn converge_pipeline_service_accounts(
    kube: &crate::apps::kube::KubeClient,
    mirror: &Mirror,
    namespace: &str,
) -> Vec<String> {
    converge_service_accounts(
        kube,
        namespace,
        pipeline_service_accounts(mirror, namespace),
        PIPELINE_IDENTITY_LABEL,
        jc_core::kinds::pipeline_identity::is_derived,
    )
    .await
}

/// Brings the Apps' job ServiceAccounts in the identities `namespace` to the mirror's Apps
/// (AP-159): one per App with jobs, deleted when the App or its jobs go; only a labelled
/// `appjob-…` one is ever deleted.
pub async fn converge_app_job_service_accounts(
    kube: &crate::apps::kube::KubeClient,
    mirror: &Mirror,
    namespace: &str,
) -> Vec<String> {
    converge_service_accounts(
        kube,
        namespace,
        app_job_service_accounts(mirror, namespace),
        APP_JOB_IDENTITY_LABEL,
        jc_core::kinds::app_identity::is_derived,
    )
    .await
}

/// The accounts that may not have a client this run, with the reason: Keycloak finds a federated
/// client by the assertion's subject, so one Kubernetes ServiceAccount opens one account at most.
/// When several bind the same subject, the one whose managed client already holds it keeps it and
/// the others are refused; when none does yet, all of them are, so a later binding can never take
/// a subject from the account that had it first.
fn contested(accounts: &[BoundAccount], listed: &Value) -> BTreeMap<String, String> {
    let mut by_subject: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (account, binding) in accounts.iter().filter_map(|a| Some((a, a.binding()?))) {
        by_subject
            .entry(subject(binding))
            .or_default()
            .push(account.label());
    }
    let holder_of = |subject: &str| {
        listed.as_array().into_iter().flatten().find_map(|client| {
            let attributes = client.get("attributes")?;
            (managed(client)
                && attributes.get("jwt.credential.sub").and_then(Value::as_str) == Some(subject))
            .then(|| {
                attributes
                    .get(ACCOUNT_ATTRIBUTE)?
                    .as_str()
                    .map(str::to_owned)
            })
            .flatten()
        })
    };
    let mut refused = BTreeMap::new();
    for (subject, labels) in by_subject.into_iter().filter(|(_, l)| l.len() > 1) {
        let holder = holder_of(&subject).filter(|holder| labels.contains(holder));
        for label in labels
            .iter()
            .filter(|label| Some(*label) != holder.as_ref())
        {
            let others: Vec<&str> = labels
                .iter()
                .filter(|other| *other != label)
                .map(String::as_str)
                .collect();
            refused.insert(
                label.clone(),
                match &holder {
                    Some(holder) => format!(
                        "{subject} already opens the client of {holder}; one Kubernetes \
                         ServiceAccount opens one account, so this binding gets no client (PF-47)"
                    ),
                    None => format!(
                        "{subject} is bound by {} as well; one Kubernetes ServiceAccount opens \
                         one account, so none of them gets a client until one binding is left \
                         (PF-47)",
                        others.join(", ")
                    ),
                },
            );
        }
    }
    refused
}

/// Refuses every token a subject was issued until now (T-5568): the Portal's `revoke_subject`,
/// which the gateway reads back through `/internal/revocations`.
pub type Revoke =
    std::sync::Arc<dyn Fn(String) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync>;

/// The realm's workload clients, as the reconciler's own client may write them.
pub struct WorkloadClientSync {
    admin: Admin,
    /// The Portal's own audience, `portal-api`: what a token needs to act on the Portal.
    portal: String,
    /// The pipeline runner's namespace, when every Pipeline's own account gets its client too
    /// (PL-19); `None` leaves pipelines to their project's `pipelines` account.
    pipelines: Option<String>,
    /// The identities namespace, when every App's job principal gets its client too (AP-159).
    app_jobs: Option<String>,
    /// What ends a removed client's tokens at once; `None` leaves them valid until they expire.
    revoke: Option<Revoke>,
}

impl WorkloadClientSync {
    /// `None` when the issuer is not a realm URL. The login client is the Portal's audience as
    /// well as the identity that writes the clients.
    pub fn new(issuer: &str, client_id: String, auth: ClientAuth) -> Option<Self> {
        Some(Self {
            portal: client_id.clone(),
            admin: Admin::new(issuer, client_id, auth)?,
            pipelines: None,
            app_jobs: None,
            revoke: None,
        })
    }

    /// Revokes the service-account user of each client this sync removes, so a deleted
    /// ServiceAccount's tokens stop opening anything within the gateway's next poll (T-5568).
    pub fn with_revoke(mut self, revoke: Revoke) -> Self {
        self.revoke = Some(revoke);
        self
    }

    /// Gives every Pipeline's own account its federated client, bound to its Kubernetes
    /// ServiceAccount in the runner's namespace `namespace` (PL-19, T-1508).
    pub fn with_pipelines(mut self, namespace: String) -> Self {
        self.pipelines = Some(namespace);
        self
    }

    /// Gives every App's job principal its federated client, bound to its Kubernetes
    /// ServiceAccount in the identities namespace `namespace` (AP-159, T-3539).
    pub fn with_app_jobs(mut self, namespace: String) -> Self {
        self.app_jobs = Some(namespace);
        self
    }

    /// Brings every bound account's client to what [`desired`] and [`audiences`] say, deletes the
    /// managed workload clients no bound account names, and answers one outcome per account
    /// touched (`*` for a failure before any was looked at). `bentos` is the mapping beside each
    /// Pipeline, read for the Endpoints it calls (T-5757).
    pub async fn converge(&self, mirror: &Mirror, bentos: &Bentos) -> Vec<ClientOutcome> {
        let mut outcomes = Vec::new();
        let token = match self.admin.token().await {
            Ok(token) => token,
            Err(err) => {
                let mut outcome = ClientOutcome::of("*");
                outcome.error = Some(err);
                return vec![outcome];
            }
        };

        // The realm's clients first: who already holds a subject decides a contested one.
        let listed = match self
            .admin
            .send(
                &token,
                reqwest::Method::GET,
                "/clients?briefRepresentation=false&max=2000",
                None,
            )
            .await
        {
            Ok(listed) => listed.unwrap_or(Value::Null),
            Err(err) => {
                let mut outcome = ClientOutcome::of("*");
                outcome.error = Some(format!("listing the workload clients: {err}"));
                return vec![outcome];
            }
        };

        let mut accounts = bound(mirror);
        if let Some(namespace) = &self.pipelines {
            accounts.extend(pipeline_accounts(mirror, namespace, bentos));
        }
        if let Some(namespace) = &self.app_jobs {
            accounts.extend(app_job_accounts(mirror, namespace));
        }
        let wanted: BTreeSet<String> = accounts.iter().map(BoundAccount::label).collect();
        let refused = contested(&accounts, &listed);
        for account in &accounts {
            if let Some(reason) = refused.get(&account.label()) {
                let mut outcome = ClientOutcome::of(&account.label());
                outcome.error = Some(reason.clone());
                outcomes.push(outcome);
                continue;
            }
            let audiences = match &account.reach {
                Reach::Roles(spec) => reporting(
                    mirror,
                    account,
                    audiences(mirror, &account.project, spec, &self.portal),
                ),
                Reach::Pipeline(slugs) => reporting(mirror, account, slugs.clone()),
                Reach::Endpoint(slug) => vec![slug.clone()],
                Reach::Internal => vec![crate::auth::internal::INTERNAL_AUDIENCE.to_owned()],
            };
            outcomes.push(self.converge_one(&token, account, &audiences).await);
        }

        // A managed workload client no bound account names is this wave's to remove (PF-47).
        for client in listed.as_array().into_iter().flatten() {
            let Some(account) = client
                .get("attributes")
                .and_then(|a| a.get(ACCOUNT_ATTRIBUTE))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !managed(client) || wanted.contains(account) {
                continue;
            }
            let mut outcome = ClientOutcome::of(account);
            let uuid = client.get("id").and_then(Value::as_str);
            if let Some(uuid) = uuid {
                self.end_tokens(&token, uuid, &mut outcome).await;
            }
            match uuid {
                Some(uuid) => match self
                    .admin
                    .send(
                        &token,
                        reqwest::Method::DELETE,
                        &format!("/clients/{uuid}"),
                        None,
                    )
                    .await
                {
                    Ok(_) => outcome.drift.push(
                        "the account is gone or no longer bound to a workload, so its client was removed"
                            .to_owned(),
                    ),
                    Err(err) => outcome.error = Some(err),
                },
                None => outcome.error = Some("the realm listed a client without an id".to_owned()),
            }
            outcomes.push(outcome);
        }
        outcomes
    }

    /// Revokes the service-account user of the client `uuid` before the client goes: Keycloak
    /// deletes that user with it, and its tokens would open the gateway until they expire. A
    /// lookup that fails is a warning, never a reason to keep the client.
    async fn end_tokens(&self, token: &str, uuid: &str, outcome: &mut ClientOutcome) {
        let Some(revoke) = self.revoke.as_ref() else {
            return;
        };
        let user = self
            .admin
            .send(
                token,
                reqwest::Method::GET,
                &format!("/clients/{uuid}/service-account-user"),
                None,
            )
            .await;
        match user.map(|user| {
            user.as_ref()
                .and_then(|u| u.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        }) {
            Ok(Some(id)) => revoke(id).await,
            Ok(None) => {}
            Err(err) => outcome.warnings.push(format!(
                "the removed client's tokens stay valid until they expire: {err}"
            )),
        }
    }

    /// The client's scopes brought to [`TOKEN_SCOPES`] (T-6051): every other default or optional
    /// scope removed, a missing one added from the realm's. Each removal is reported once, when it
    /// happens.
    async fn converge_scopes(
        &self,
        token: &str,
        uuid: &str,
        outcome: &mut ClientOutcome,
    ) -> Result<(), String> {
        use reqwest::Method;
        let client = format!("/clients/{uuid}");
        let default: Vec<KcNamed> = self
            .admin
            .get(token, &format!("{client}/default-client-scopes"))
            .await?;
        let optional: Vec<KcNamed> = self
            .admin
            .get(token, &format!("{client}/optional-client-scopes"))
            .await?;
        let (remove, missing) = scope_plan(&default, &optional);
        for (kind, scope) in &remove {
            self.admin
                .send(
                    token,
                    Method::DELETE,
                    &format!("{client}/{kind}-client-scopes/{}", scope.id),
                    None,
                )
                .await?;
        }
        if !remove.is_empty() {
            let names: Vec<&str> = remove.iter().map(|(_, s)| s.name.as_str()).collect();
            outcome.drift.push(format!(
                "the client's tokens carried the scopes {}, which the account does not read; they were removed",
                names.join(", ")
            ));
        }
        if missing.is_empty() {
            return Ok(());
        }
        let realm: Vec<KcNamed> = self.admin.get(token, "/client-scopes").await?;
        for name in missing {
            let Some(scope) = realm.iter().find(|scope| scope.name == name) else {
                return Err(format!(
                    "the realm has no client scope {name}, so the account's tokens carry no sub"
                ));
            };
            self.admin
                .send(
                    token,
                    Method::PUT,
                    &format!("{client}/default-client-scopes/{}", scope.id),
                    None,
                )
                .await?;
        }
        Ok(())
    }

    async fn converge_one(
        &self,
        token: &str,
        account: &BoundAccount,
        audiences: &[String],
    ) -> ClientOutcome {
        let mut outcome = ClientOutcome::of(&account.label());
        let id = keycloak_client_id(&account.project, &account.name);
        let want = match &account.auth {
            Auth::External(credential) => {
                desired_external(&account.project, &account.name, credential)
            }
            Auth::Workload(binding) => desired(&account.project, &account.name, binding),
        };

        let held = match self.admin.find(token, &id).await {
            Ok(held) => held,
            Err(err) => {
                outcome.error = Some(err);
                return outcome;
            }
        };
        let owner = held
            .as_ref()
            .and_then(|client| client.get("attributes"))
            .and_then(|a| a.get(ACCOUNT_ATTRIBUTE))
            .and_then(Value::as_str);
        let uuid = match held.as_ref() {
            // The platform's own client of an external account (`keycloak-clients.yaml`) serves
            // it as it is: its owner rotates it, so there is nothing to report.
            Some(client) if account.binding().is_none() && !managed(client) => return outcome,
            Some(client) if !managed(client) || owner != Some(account.label().as_str()) => {
                outcome.error = Some(format!(
                    "the realm holds a client {id} this wave did not create for this account; it \
                     is left alone and the workload has no identity until it is renamed or \
                     removed (PF-47)"
                ));
                return outcome;
            }
            Some(client) => {
                let Some(uuid) = client.get("id").and_then(Value::as_str).map(str::to_owned) else {
                    outcome.error = Some(format!("the realm answered {id} without an id"));
                    return outcome;
                };
                let found = drift(&want, client);
                if !found.is_empty() {
                    if let Err(err) = self
                        .admin
                        .send(
                            token,
                            reqwest::Method::PUT,
                            &format!("/clients/{uuid}"),
                            Some(&want),
                        )
                        .await
                    {
                        outcome.error = Some(err);
                        return outcome;
                    }
                    outcome.drift = found;
                }
                uuid
            }
            None => {
                if let Err(err) = self
                    .admin
                    .send(
                        token,
                        reqwest::Method::POST,
                        "/clients",
                        Some(&creation(&want)),
                    )
                    .await
                {
                    outcome.error = Some(err);
                    return outcome;
                }
                match self.admin.find(token, &id).await {
                    Ok(Some(client)) => match client.get("id").and_then(Value::as_str) {
                        Some(uuid) => uuid.to_owned(),
                        None => {
                            outcome.error = Some(format!("the realm answered {id} without an id"));
                            return outcome;
                        }
                    },
                    Ok(None) => {
                        outcome.error = Some(format!("{id} was created and cannot be read back"));
                        return outcome;
                    }
                    Err(err) => {
                        outcome.error = Some(err);
                        return outcome;
                    }
                }
            }
        };
        if let Err(err) = self.converge_scopes(token, &uuid, &mut outcome).await {
            outcome.error = Some(format!("client scopes of {id}: {err}"));
            return outcome;
        }
        if let Err(err) = self
            .admin
            .converge_mappers(
                token,
                &uuid,
                &audiences
                    .iter()
                    .map(|a| audience_mapper(a))
                    .collect::<Vec<_>>(),
                "the account",
                &mut outcome,
            )
            .await
        {
            outcome.error = Some(format!("audiences of {id}: {err}"));
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes(names: &[&str]) -> Vec<KcNamed> {
        names
            .iter()
            .map(|name| KcNamed {
                id: format!("id-{name}"),
                name: (*name).to_owned(),
            })
            .collect()
    }

    /// T-6051: the realm's defaults on a Keycloak 26.8.1 service-account client, as measured.
    #[test]
    fn every_scope_but_basic_goes_and_basic_stays() {
        let default = scopes(&[
            "web-origins",
            "service_account",
            "acr",
            "roles",
            "profile",
            "basic",
            "email",
        ]);
        let optional = scopes(&["address", "phone", "offline_access"]);
        let (remove, missing) = scope_plan(&default, &optional);
        let removed: Vec<(&str, &str)> = remove
            .iter()
            .map(|(kind, scope)| (*kind, scope.id.as_str()))
            .collect();
        assert_eq!(
            removed,
            vec![
                ("default", "id-web-origins"),
                ("default", "id-service_account"),
                ("default", "id-acr"),
                ("default", "id-roles"),
                ("default", "id-profile"),
                ("default", "id-email"),
                ("optional", "id-address"),
                ("optional", "id-phone"),
                ("optional", "id-offline_access"),
            ]
        );
        assert!(missing.is_empty());
    }

    #[test]
    fn a_trimmed_client_needs_nothing_and_one_without_basic_gets_it() {
        let trimmed = scopes(&["basic"]);
        let (remove, missing) = scope_plan(&trimmed, &[]);
        assert!(remove.is_empty());
        assert!(missing.is_empty());
        let bare = scopes(&["service_account"]);
        let (remove, missing) = scope_plan(&bare, &[]);
        assert_eq!(remove.len(), 1);
        assert_eq!(missing, vec!["basic"]);
    }

    /// Every kind of account client is created with `basic` alone (T-6051): a workload's, an
    /// external one's with a secret, and a device's with keys.
    #[test]
    fn every_client_kind_is_created_with_basic_alone() {
        let binding = KubernetesBinding {
            namespace: "env-dept".to_owned(),
            service_account: "adapter".to_owned(),
        };
        for want in [
            desired("helsinki", "adapter", &binding),
            desired_external("helsinki", "vendor", &ClientCredential::Secret),
            desired_external(
                "helsinki",
                "sensor",
                &ClientCredential::Keys(r#"{"keys":[]}"#.to_owned()),
            ),
        ] {
            let body = creation(&want);
            assert_eq!(body["defaultClientScopes"], json!(["basic"]), "{want}");
            assert_eq!(body["optionalClientScopes"], json!([]), "{want}");
            assert_eq!(body["clientId"], want["clientId"]);
            // The drift check compares `desired` alone, which names no scope: Keycloak puts
            // `service_account` back on every PUT, so a scope there would rewrite every run.
            assert!(want.get("defaultClientScopes").is_none());
        }
    }
}
