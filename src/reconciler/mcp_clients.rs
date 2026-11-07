//! The Keycloak client of every named MCP server (EP-96, ADR-N-043 §2.2, T-3156).
//!
//! A `McpServer` `{project}/{name}` is its own RFC 8707 resource, and an AI client signs a person
//! in to it with the client `mcp-{project}-{name}`: public, PKCE S256, consent on, the callbacks
//! of the AI clients the hub knows and no others, no dynamic registration. Its tokens name the
//! server as their audience, which the gateway accepts at `/api/mcp/{project}/{name}` alone, and
//! carry the person's groups so a project-bound member can check them. The client grants nothing:
//! every member call is still that Endpoint's own Policy decision with the person's token.
//!
//! This wave is the only writer of such a client: it carries `managed-by: joinedcontext` and the
//! server it belongs to, a change made in the console is written back and reported, and a managed
//! client whose server is gone is deleted. A client of that id this platform did not create is
//! never written or removed; the server is reported instead.

use crate::config::ClientAuth;
use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::app_clients::{app_mappers, drift, managed, Admin, ClientOutcome};
use super::groups::{MANAGED_BY, MANAGED_VALUE};
use crate::store::{ListOptions, Mirror};

/// The client attribute naming the server a managed client belongs to, `{project}/{name}`.
pub const SERVER_ATTRIBUTE: &str = "joinedcontext.mcpserver";

/// Where an AI client returns a person after sign-in: the callbacks the hub's client knows
/// (deployment `components/context-gateway/keycloak-clients.yaml`, `mcp-hub`).
pub const CALLBACKS: [&str; 4] = [
    "https://claude.ai/api/mcp/auth_callback",
    "https://claude.com/api/mcp/auth_callback",
    "https://chatgpt.com/connector_platform_oauth_redirect",
    "https://platform.openai.com/chatkit-oauth-callback",
];

/// `mcp-{project}-{name}`, the client a connector signs in with (EP-96).
pub fn client_id(project: &str, name: &str) -> String {
    format!("mcp-{project}-{name}")
}

/// The client a server should have, as Keycloak's representation (EP-96).
pub fn desired(project: &str, name: &str) -> Value {
    json!({
        "clientId": client_id(project, name),
        "name": format!("MCP server {project}/{name}"),
        "protocol": "openid-connect",
        "enabled": true,
        "publicClient": true,
        "bearerOnly": false,
        "consentRequired": true,
        "standardFlowEnabled": true,
        "implicitFlowEnabled": false,
        "directAccessGrantsEnabled": false,
        "serviceAccountsEnabled": false,
        "redirectUris": CALLBACKS,
        "attributes": {
            "pkce.code.challenge.method": "S256",
            MANAGED_BY: MANAGED_VALUE,
            SERVER_ATTRIBUTE: format!("{project}/{name}"),
        },
    })
}

/// The organization's built-in server over everything the caller may read, which the gateway
/// serves at `/api/mcp/org/all` without a manifest (EP-122, T-4530).
pub const ORG_ALL: &str = "all";

/// The servers of the mirror as `(project, name)`, the organization's (`org`) and its built-in
/// `org/all` included, sorted so a run is deterministic. One whose client id would not fit
/// Keycloak names no client.
pub fn declared(mirror: &Mirror) -> Vec<(String, String)> {
    // `namespaces` lists the projects; the organization's servers live in `org` beside them.
    let mut servers: Vec<(String, String)> = mirror
        .namespaces()
        .into_iter()
        .chain(std::iter::once(
            crate::api::blueprints::ORG_NAMESPACE.to_owned(),
        ))
        .flat_map(|project| {
            mirror
                .list(&project, "McpServer", &ListOptions::default())
                .items
                .into_iter()
                .map(move |envelope| (project.clone(), envelope.metadata.name))
        })
        .chain(std::iter::once((
            crate::api::blueprints::ORG_NAMESPACE.to_owned(),
            ORG_ALL.to_owned(),
        )))
        .filter(|(project, name)| client_id(project, name).len() <= 255)
        .collect();
    servers.sort();
    servers.dedup();
    servers
}

/// The realm's MCP server clients, as the reconciler's own client may write them.
pub struct McpClientSync {
    admin: Admin,
}

impl McpClientSync {
    /// `None` when the issuer is not a realm URL.
    pub fn new(issuer: &str, client_id: String, auth: ClientAuth) -> Option<Self> {
        Some(Self {
            admin: Admin::new(issuer, client_id, auth)?,
        })
    }

    /// Brings every declared server's client to [`desired`], deletes the managed server clients
    /// no server names, and answers one outcome per server touched (`*` for a failure before any
    /// was looked at).
    pub async fn converge(&self, mirror: &Mirror) -> Vec<ClientOutcome> {
        let token = match self.admin.token().await {
            Ok(token) => token,
            Err(err) => {
                let mut outcome = ClientOutcome::of("*");
                outcome.error = Some(err);
                return vec![outcome];
            }
        };
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
                outcome.error = Some(format!("listing the MCP server clients: {err}"));
                return vec![outcome];
            }
        };

        let servers = declared(mirror);
        let wanted: BTreeSet<String> = servers.iter().map(|(p, n)| format!("{p}/{n}")).collect();
        let mut outcomes = Vec::new();
        for (project, name) in &servers {
            outcomes.push(self.converge_one(&token, project, name).await);
        }

        // A managed server client no declared server names is this wave's to remove.
        for client in listed.as_array().into_iter().flatten() {
            let Some(server) = client
                .get("attributes")
                .and_then(|a| a.get(SERVER_ATTRIBUTE))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !managed(client) || wanted.contains(server) {
                continue;
            }
            let mut outcome = ClientOutcome::of(server);
            match client.get("id").and_then(Value::as_str) {
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
                    Ok(_) => outcome
                        .drift
                        .push("the server is gone, so its client was removed".to_owned()),
                    Err(err) => outcome.error = Some(err),
                },
                None => outcome.error = Some("the realm listed a client without an id".to_owned()),
            }
            outcomes.push(outcome);
        }
        outcomes
    }

    async fn converge_one(&self, token: &str, project: &str, name: &str) -> ClientOutcome {
        let label = format!("{project}/{name}");
        let mut outcome = ClientOutcome::of(&label);
        let id = client_id(project, name);
        let want = desired(project, name);

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
            .and_then(|a| a.get(SERVER_ATTRIBUTE))
            .and_then(Value::as_str);
        let uuid = match held.as_ref() {
            Some(client) if !managed(client) || owner != Some(label.as_str()) => {
                outcome.error = Some(format!(
                    "the realm holds a client {id} this wave did not create for this server; it \
                     is left alone and no AI client can sign in to the server until it is renamed \
                     or removed (EP-96)"
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
                    .send(token, reqwest::Method::POST, "/clients", Some(&want))
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
        // Its own audience, so the gateway knows the token is for this server, and the person's
        // groups, so a project-bound member can check them (AP-113); no other mapper.
        if let Err(err) = self
            .admin
            .converge_mappers(
                token,
                &uuid,
                &app_mappers(std::slice::from_ref(&id)),
                "the server",
                &mut outcome,
            )
            .await
        {
            outcome.error = Some(format!("mappers of {id}: {err}"));
        }
        if let Err(err) = self
            .admin
            .drop_optional_scopes(token, &uuid, &mut outcome)
            .await
        {
            let scopes = format!("scopes of {id}: {err}");
            outcome.error = Some(match outcome.error.take() {
                Some(mappers) => format!("{mappers}; {scopes}"),
                None => scopes,
            });
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    fn server(project: &str, name: &str) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "McpServer".to_owned(),
            metadata: ObjectMeta::new(name, project),
            spec: json!({ "members": [{ "kind": "Endpoint", "name": "bikes", "namespace": "helsinki" }], "audience": "organization" }),
            status: None,
        }
    }

    #[test]
    fn the_client_is_public_pkce_with_consent_and_only_the_known_callbacks() {
        let want = desired("helsinki", "mobility");
        assert_eq!(want["clientId"], "mcp-helsinki-mobility");
        assert_eq!(want["publicClient"], true);
        assert_eq!(want["consentRequired"], true);
        assert_eq!(want["directAccessGrantsEnabled"], false);
        assert_eq!(want["serviceAccountsEnabled"], false);
        assert_eq!(want["implicitFlowEnabled"], false);
        assert_eq!(want["attributes"]["pkce.code.challenge.method"], "S256");
        assert_eq!(want["attributes"][SERVER_ATTRIBUTE], "helsinki/mobility");
        assert_eq!(want["redirectUris"], json!(CALLBACKS));
        assert!(
            want.get("secret").is_none(),
            "a public client holds no secret"
        );
    }

    #[test]
    fn every_server_of_every_namespace_is_declared_once_and_sorted() {
        let mirror = Mirror::new();
        mirror.upsert(server("helsinki", "mobility"));
        mirror.upsert(server("org", "city"));
        mirror.upsert(server("espoo", "air"));
        assert_eq!(
            declared(&mirror),
            vec![
                ("espoo".to_owned(), "air".to_owned()),
                ("helsinki".to_owned(), "mobility".to_owned()),
                ("org".to_owned(), "all".to_owned()),
                ("org".to_owned(), "city".to_owned()),
            ]
        );
        // `org/all` has a client with no manifest, once even when one is written for it.
        mirror.upsert(server("org", "all"));
        let empty = Mirror::new();
        assert_eq!(declared(&empty), vec![("org".to_owned(), "all".to_owned())]);
        assert_eq!(declared(&mirror).len(), 4);
    }
}
