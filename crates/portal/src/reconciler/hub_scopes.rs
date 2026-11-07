//! The `endpoint:{slug}` client scopes of the MCP hub's Keycloak client (T-2490, ADR-N-025 §4,
//! EP-88).
//!
//! The deployment renders the client `mcp-hub`; this wave renders one optional client scope
//! `endpoint:{slug}` on it per Endpoint that serves MCP (every Endpoint but one with
//! `spec.mcp: false`, EP-24). A connector asks for the scopes of the Endpoints the person picked,
//! consent shows them, and the gateway lets a hub token reach exactly the Endpoints its scopes
//! name. A scope grants no data: each call is still the Endpoint's own Policy decision.
//!
//! This wave is the only writer of such a scope. A managed scope carries `managed-by:
//! joinedcontext` and the Endpoint it stands for; one whose Endpoint is gone or stopped serving
//! MCP is deleted. A scope of that name this platform did not create is never written and is
//! reported. On `mcp-hub` itself the links are the deployment's (T-3324): the T-2725 deny keeps
//! this client off the hub, and the admin-permissions Job (after each sync) and CronJob (every 5
//! minutes, T-6388) link our scopes as optional and unlink a stray `endpoint:` scope or a
//! default one, which every token would carry without the person asking. This wave reads the links and reports a scope that waits for that run.

use crate::config::ClientAuth;
use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{json, Value};

use super::app_clients::{managed, Admin, ClientOutcome};
use super::groups::{MANAGED_BY, MANAGED_VALUE};
use crate::store::{ListOptions, Mirror};

/// The hub's client, rendered by the deployment (`components/context-gateway/keycloak-clients.yaml`).
pub const HUB_CLIENT: &str = "mcp-hub";

/// The scope attribute naming the Endpoint a managed scope stands for, `{project}/{name}`.
pub const ENDPOINT_ATTRIBUTE: &str = "joinedcontext.endpoint";

/// The Endpoint condition that says whether its scope is linked to the hub (T-3324).
pub const CONDITION: &str = "HubLinked";

/// What a scope not yet linked to the hub reports: the deployment links it (T-3324).
pub const PENDING: &str =
    "in the one-connector list within 5 minutes, when the realm's admin-permissions CronJob links it to the hub";

/// The prefix the gateway reads a hub token's Endpoints from (`Claims::endpoint_scopes`).
pub const SCOPE_PREFIX: &str = "endpoint:";

/// One Endpoint that serves MCP, as this wave needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubEndpoint {
    pub project: String,
    pub name: String,
    pub slug: String,
}

impl HubEndpoint {
    pub fn label(&self) -> String {
        format!("{}/{}", self.project, self.name)
    }

    pub fn scope(&self) -> String {
        format!("{SCOPE_PREFIX}{}", self.slug)
    }
}

/// The client scope an Endpoint should have, as Keycloak's representation.
pub fn desired(endpoint: &HubEndpoint) -> Value {
    json!({
        "name": endpoint.scope(),
        "description": format!("Reach the Endpoint {} through the MCP hub", endpoint.label()),
        "protocol": "openid-connect",
        "attributes": {
            "include.in.token.scope": "true",
            "display.on.consent.screen": "true",
            "consent.screen.text": format!("Endpoint {}", endpoint.label()),
            MANAGED_BY: MANAGED_VALUE,
            ENDPOINT_ATTRIBUTE: endpoint.label(),
        },
    })
}

/// The Endpoints of the mirror that serve MCP, sorted so a run is deterministic. A slug outside
/// the opaque lowercase alphabet names no scope: `validate` is what reports the Endpoint.
pub fn serving(mirror: &Mirror) -> Vec<HubEndpoint> {
    let mut endpoints: Vec<HubEndpoint> = mirror
        .namespaces()
        .into_iter()
        .flat_map(|project| {
            mirror
                .list(&project, "Endpoint", &ListOptions::default())
                .items
                .into_iter()
                .filter_map(move |envelope| {
                    let served = crate::api::catalogue::served_representations(&envelope.spec);
                    let slug = envelope.spec["slug"].as_str()?;
                    let opaque = (8..=64).contains(&slug.len())
                        && slug
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
                    (opaque && served.iter().any(|r| r == "mcp")).then(|| HubEndpoint {
                        project: project.clone(),
                        name: envelope.metadata.name,
                        slug: slug.to_owned(),
                    })
                })
        })
        .collect();
    endpoints.sort_by(|a, b| (&a.project, &a.name).cmp(&(&b.project, &b.name)));
    endpoints
}

/// The hub client's scopes, as the reconciler's own client may write them.
pub struct HubScopeSync {
    admin: Admin,
}

impl HubScopeSync {
    /// `None` when the issuer is not a realm URL.
    pub fn new(issuer: &str, client_id: String, auth: ClientAuth) -> Option<Self> {
        Some(Self {
            admin: Admin::new(issuer, client_id, auth)?,
        })
    }

    /// Brings the realm's `endpoint:` scopes to [`serving`], reads the hub client's links (the
    /// deployment writes them, T-3324), and answers one outcome per scope touched or waiting
    /// (`*` for the run as a whole).
    pub async fn converge(&self, mirror: &Mirror) -> Vec<ClientOutcome> {
        match self.run(mirror).await {
            Ok(outcomes) => outcomes,
            Err(err) => {
                let mut outcome = ClientOutcome::of("*");
                outcome.error = Some(err);
                vec![outcome]
            }
        }
    }

    async fn run(&self, mirror: &Mirror) -> Result<Vec<ClientOutcome>, String> {
        use reqwest::Method;
        let token = self.admin.token().await?;
        let Some(hub) = self.admin.find(&token, HUB_CLIENT).await? else {
            // The deployment has not rendered the hub yet: nothing to scope, and nothing wrong.
            let mut outcome = ClientOutcome::of("*");
            outcome.warnings.push(format!(
                "the realm has no {HUB_CLIENT} client; no hub scope is rendered"
            ));
            return Ok(vec![outcome]);
        };
        let Some(hub) = hub.get("id").and_then(Value::as_str).map(str::to_owned) else {
            return Err(format!("the realm answered {HUB_CLIENT} without an id"));
        };

        let wanted = serving(mirror);
        let mut outcomes = Vec::new();
        // Keycloak lists every client scope of the realm in one answer (the endpoint takes no
        // paging); indexed by name once, so a wave is linear in the Endpoints (T-4445). The first
        // of two scopes with one name is the one a linear search found.
        let listed: Vec<Value> = self.admin.get(&token, "/client-scopes").await?;
        let mut by_name: HashMap<&str, &Value> = HashMap::with_capacity(listed.len());
        for scope in &listed {
            if let Some(name) = scope["name"].as_str() {
                by_name.entry(name).or_insert(scope);
            }
        }

        // Create or write back each wanted scope; a foreign one of that name is left alone.
        let mut created = false;
        for endpoint in &wanted {
            let want = desired(endpoint);
            match want["name"]
                .as_str()
                .and_then(|name| by_name.get(name).copied())
            {
                None => {
                    self.admin
                        .send(&token, Method::POST, "/client-scopes", Some(&want))
                        .await
                        .map_err(|err| format!("creating {}: {err}", endpoint.scope()))?;
                    created = true;
                }
                Some(scope) if !ours(scope) => {
                    let mut outcome = ClientOutcome::of(&endpoint.scope());
                    outcome.error = Some(format!(
                        "the realm holds a client scope {} this wave did not create for {}; it is \
                         left alone and not linked to {HUB_CLIENT}, so the hub cannot reach the \
                         Endpoint until it is renamed or removed (EP-88)",
                        endpoint.scope(),
                        endpoint.label()
                    ));
                    outcomes.push(outcome);
                }
                Some(scope) => {
                    let differs = want["attributes"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .any(|(key, value)| scope["attributes"].get(key) != Some(value))
                        || scope["description"] != want["description"];
                    if differs {
                        let Some(id) = scope["id"].as_str() else {
                            return Err(format!(
                                "the realm listed {} without an id",
                                endpoint.scope()
                            ));
                        };
                        let mut body = want.clone();
                        body["id"] = json!(id);
                        self.admin
                            .send(
                                &token,
                                Method::PUT,
                                &format!("/client-scopes/{id}"),
                                Some(&body),
                            )
                            .await?;
                        let mut outcome = ClientOutcome::of(&endpoint.scope());
                        outcome.drift.push(
                            "the scope was changed in the realm; it was written back".to_owned(),
                        );
                        outcomes.push(outcome);
                    }
                }
            }
        }

        // A managed scope no serving Endpoint names is this wave's to delete, which unlinks it too.
        let names: BTreeMap<String, &HubEndpoint> = wanted
            .iter()
            .map(|endpoint| (endpoint.scope(), endpoint))
            .collect();
        for scope in &listed {
            let Some(name) = scope["name"].as_str() else {
                continue;
            };
            if !ours(scope) || names.contains_key(name) {
                continue;
            }
            let Some(id) = scope["id"].as_str() else {
                return Err(format!("the realm listed {name} without an id"));
            };
            self.admin
                .send(
                    &token,
                    Method::DELETE,
                    &format!("/client-scopes/{id}"),
                    None,
                )
                .await?;
            let mut outcome = ClientOutcome::of(name);
            outcome.drift.push(
                "its Endpoint is gone or no longer serves MCP, so the scope was removed".to_owned(),
            );
            outcomes.push(outcome);
        }

        // The ids of our scopes, after the creations above.
        let scopes: Vec<Value> = match created {
            true => self.admin.get(&token, "/client-scopes").await?,
            false => listed,
        };
        let linkable: BTreeMap<String, String> = scopes
            .iter()
            .filter(|scope| ours(scope))
            .filter_map(|scope| {
                let name = scope["name"]
                    .as_str()
                    .filter(|name| names.contains_key(*name))?;
                Some((scope["id"].as_str()?.to_owned(), name.to_owned()))
            })
            .collect();

        // The hub's links are the deployment's (T-3324): the T-2725 deny keeps this client off
        // `mcp-hub`, and the admin-permissions Job links our scopes as optional and unlinks any
        // other of the prefix on each run. They are read here and reported, never written.
        let client = format!("/clients/{hub}");
        for kind in ["default-client-scopes", "optional-client-scopes"] {
            let linked: Vec<Value> = self.admin.get(&token, &format!("{client}/{kind}")).await?;
            let linked_ids: HashSet<&str> = linked
                .iter()
                .filter_map(|scope| scope["id"].as_str())
                .collect();
            for scope in &linked {
                let (Some(id), Some(name)) = (scope["id"].as_str(), scope["name"].as_str()) else {
                    continue;
                };
                let stray = name.starts_with(SCOPE_PREFIX)
                    && (kind == "default-client-scopes" || !linkable.contains_key(id));
                if stray {
                    let mut outcome = ClientOutcome::of(name);
                    let link = match kind {
                        "default-client-scopes" => "a default",
                        _ => "an optional",
                    };
                    outcome.warnings.push(format!(
                        "it is linked to {HUB_CLIENT} as {link} scope, which this platform does \
                         not render; the next deployment sync unlinks it"
                    ));
                    outcomes.push(outcome);
                }
            }
            if kind == "optional-client-scopes" {
                for name in linkable
                    .iter()
                    .filter(|(id, _)| !linked_ids.contains(id.as_str()))
                    .map(|(_, name)| name)
                {
                    let mut outcome = ClientOutcome::of(name);
                    outcome.warnings.push(PENDING.to_owned());
                    outcomes.push(outcome);
                }
            }
        }
        Ok(outcomes)
    }
}

/// The `HubLinked` condition of the Endpoint whose scope is `scope`, from one run's outcomes, as
/// `(status, reason, message)`; `None` when the run stopped before the scopes (`*`).
pub fn linked(
    outcomes: &[ClientOutcome],
    scope: &str,
) -> Option<(&'static str, &'static str, String)> {
    if outcomes.iter().any(|o| o.app == "*") {
        return None;
    }
    let mine = outcomes.iter().filter(|o| o.app == scope);
    if let Some(err) = mine.clone().find_map(|o| o.error.clone()) {
        return Some(("False", "RealmRefused", err));
    }
    if mine.flat_map(|o| &o.warnings).any(|w| w == PENDING) {
        return Some(("False", "AwaitsDeploymentSync", PENDING.to_owned()));
    }
    Some((
        "True",
        "Linked",
        "a hub token that names this Endpoint reaches it".to_owned(),
    ))
}

/// Whether this wave made `scope`: managed, and standing for an Endpoint. Which Endpoint is its
/// content, written back like any other drift when the Endpoint moved.
fn ours(scope: &Value) -> bool {
    managed(scope) && scope["attributes"].get(ENDPOINT_ATTRIBUTE).is_some()
}
