//! The gateway's wiring from its [`Config`]: the endpoint table read from the repository, the
//! realm's keys, the anonymous OAuth server, the token exchange, the Portal pollers and the
//! repository reaper, then the router. The binary and a host that mounts the gateway beside
//! other modules (`jc`) build it the same way; only the listeners are the caller's.

use crate::app::{router, Gateway};
use crate::auth::jwks;
use crate::auth::token::Verifier;
use crate::config::Config;
use crate::pdp::PolicyPdp;
use crate::proxy::Broker;
use crate::store;
use std::sync::Arc;

/// The one outbound client: the broker hop (R46).
pub fn broker_of(config: &Config) -> Result<Broker, String> {
    let Some(path) = config.egress_ca_bundle.as_ref() else {
        return Broker::verified(config.broker_url.clone())
            .map(|broker| broker.with_template(config.broker_url_template.clone()))
            .map_err(|error| error.to_string());
    };
    let bundle = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Broker::trusting(config.broker_url.clone(), &bundle)
        .map(|broker| broker.with_template(config.broker_url_template.clone()))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// What [`build`] made: the public router, the gateway behind it, and the MQTT brokers' internal
/// router when `JC_GATEWAY_INTERNAL_BIND` is set (the caller listens for both).
pub struct Built {
    /// Every surface, with the trace layer.
    pub app: axum::Router,
    /// The gateway the routers serve.
    pub gateway: Arc<Gateway>,
    /// The CONNECT checks of the MQTT brokers, for `config.internal_bind` alone.
    pub internal: Option<axum::Router>,
}

/// Builds the gateway that forwards to `broker` and spawns its background work on the current
/// runtime. An error is a deployment fault the gateway refuses to start with.
pub async fn build(config: &Config, broker: Broker) -> Result<Built, String> {
    let checkouts = config
        .projects_dir
        .clone()
        .map(|projects| store::Checkouts {
            projects,
            assembly: config.assembly_dir.clone(),
            renders: Default::default(),
        });
    let (
        endpoints,
        spaces,
        accounts,
        federations,
        agreements,
        limits,
        servers,
        homes,
        brokers,
        mappings,
        publishers,
        provenance,
    ) = match &config.repo_dir {
        None => (
            Vec::new(),
            Vec::new(),
            crate::auth::accounts::ServiceAccounts::new(),
            crate::federation::Federations::new(),
            crate::auth::dataspace_token::Agreements::new(),
            store::Limits::default(),
            Vec::new(),
            store::SpaceBrokers::default(),
            crate::mqtt::Brokers::new(),
            Default::default(),
            Default::default(),
            Default::default(),
        ),
        Some(dir) => match store::load_from(dir, checkouts.as_ref()) {
            Ok(loaded) => {
                tracing::info!(
                    endpoints = loaded.0.len(),
                    spaces = loaded.1.len(),
                    accounts = loaded.2.len(),
                    dir = %dir.display(),
                    "repository loaded"
                );
                loaded
            }
            Err(error) => {
                // An unreadable repository is a deployment fault, not a request fault:
                // serving an empty table would answer 404 for endpoints that exist.
                tracing::error!(%error, dir = %dir.display(), "cannot read the manifest repository");
                return Err(format!("cannot read the manifest repository: {error}"));
            }
        },
    };
    // The hub's `limit` and `lastN` defaults and maxima are the broker's own (T-4804).
    tokio::spawn(crate::mcp::limits::follow(broker.clone()));
    let gateway = Gateway::new(broker, Box::new(PolicyPdp), config.org_domain.clone())
        .budget_writes(config.write_budget_mib)
        .limit_in_flight(
            config.max_in_flight,
            config.max_in_flight_per_caller,
            config.max_downloads,
        )
        .serve_loadtest(config.loadtest_client.clone(), config.loadtest_in_flight);
    let mut gateway = gateway.serve(endpoints).serve_spaces(spaces);
    gateway.public_max_age = config.public_max_age;
    gateway.replace_federation(federations);
    gateway.replace_agreements(agreements);
    gateway.replace_limits(limits);
    gateway.replace_servers(servers);
    gateway.replace_mappings(mappings);
    gateway.replace_publishers(publishers);
    gateway.replace_provenance(provenance);
    gateway.broker.route(&homes);
    gateway.replace_brokers(brokers);

    // The realm's keys are refreshed in the background; a request never fetches (PF-46).
    let gateway = match (&config.oidc_issuer, &config.oidc_jwks_url) {
        (Some(issuer), Some(jwks_url)) => {
            let verifier = Arc::new(Verifier::new(issuer.clone()));
            if let Err(error) = jwks::refresh(&verifier, jwks_url).await {
                tracing::error!(%error, "cannot load the realm signing keys");
                return Err(format!("cannot load the realm signing keys: {error}"));
            }
            tokio::spawn(jwks::keep_current(Arc::clone(&verifier), jwks_url.clone()));
            gateway.authenticate(verifier, accounts, config.public_url.clone())
        }
        _ => {
            tracing::warn!(
                "no realm configured: only endpoints with audience `public` will answer"
            );
            gateway
        }
    };

    // The anonymous OAuth server (ADR-N-052 §2.2): config checked the key and the public URL.
    let gateway = match (&config.oauth_key, &config.public_url) {
        (Some(key), Some(public_url)) => {
            match crate::oauth::Issuer::new(public_url, key, config.oauth_redirect_hosts.clone()) {
                Ok(issuer) => gateway.issue_anonymous_tokens(issuer),
                Err(error) => {
                    tracing::error!(%error, "cannot start the anonymous OAuth server");
                    return Err(format!("cannot start the anonymous OAuth server: {error}"));
                }
            }
        }
        _ => gateway,
    };

    let gateway = match &config.personal_keys {
        Some(dir) => gateway.personal_keys_in(dir.clone()),
        None => gateway,
    };

    // A read over a space with `identity: caller` registrations exchanges the caller's token,
    // as the gateway's own client, for the member brokers' audiences (T-3999, ADR-N-047 §2.4).
    // Without a client such a read answers 503; it is never forwarded as anyone else.
    let gateway = match (config.token_url(), config.oidc_client.as_ref()) {
        (Some(token_url), Some((id, credential))) => gateway.exchange_with(
            crate::federation::exchange::Exchanger::new(token_url, id.clone(), credential.clone()),
        ),
        _ => gateway,
    };

    // The Portal's internal listener answers a workload it can name (PF-46, AG-52): the
    // gateway's own client, audience-bound to that listener. Without a client configured the
    // pollers send no token and the Portal refuses them, which says so in the log rather
    // than relying on the port's NetworkPolicy.
    let token = match (config.token_url(), config.oidc_client.as_ref()) {
        (Some(token_url), Some((id, credential))) => Some(std::sync::Arc::new(
            crate::previews::WorkloadToken::with_credential(
                token_url,
                id.clone(),
                credential.clone(),
            ),
        )),
        _ => None,
    };
    // PF-41, T-2572: under `enforce` a write waits for the Organization's verified domain, which
    // the Portal keeps; `report` asks nobody and refuses nothing.
    let domain_gate = Arc::new(crate::domain_gate::DomainGate::new(
        config.domain_verification,
    ));
    if let (crate::domain_gate::Mode::Enforce, Some(url)) = (
        config.domain_verification,
        config.domain_verifications_url.clone(),
    ) {
        tokio::spawn(Arc::clone(&domain_gate).follow(url, token.clone()));
    }
    let gateway = gateway
        .deliver_privately_to(config.egress_private_hosts.clone())
        .gate_writes_on(domain_gate);
    // The publishers' pages, searched by the assistant for the hub and the servers (EP-133).
    let gateway = match config.assistant_url.as_deref() {
        Some(url) => gateway.search_documents_with(crate::mcp::documents::Documents::new(
            url,
            crate::mcp::documents::hub_sources(&config.hub_knowledge_sources),
        )),
        None => gateway,
    };
    // The publishers' Usage tab: counts per Endpoint and surface, nothing personal (T-5118).
    if let Some(url) = config.usage_url.clone() {
        tokio::spawn(crate::usage::follow(url, token.clone()));
    }
    // A disabled person or a removed ServiceAccount is refused within one poll, not at their
    // token's expiry (T-5568, TR-03187 AR-6).
    if let (Some(url), Some(verifier)) = (config.revocations_url.clone(), gateway.verifier.clone())
    {
        match crate::revocations::Revocations::new(url, token.clone(), verifier) {
            Ok(poller) => {
                tokio::spawn(poller.follow());
            }
            Err(error) => {
                tracing::error!(%error, "cannot follow the revoked subjects");
                return Err(format!("cannot follow the revoked subjects: {error}"));
            }
        }
    }
    // Who called which MCP tool, on each project's Activity (T-5322).
    if let Some(url) = config.activity_url.clone() {
        tokio::spawn(crate::activity::follow(url));
    }
    // A person's own connectors are the Portal's to keep and the gateway's to serve (EP-123).
    let gateway = Arc::new(match config.connectors_url.clone() {
        Some(url) => gateway.read_personal_connectors(crate::mcp::personal::Personal::new(
            url,
            token.clone(),
            crate::mcp::personal::KEEP,
        )),
        None => gateway,
    });
    // The repository is a cache of the enforcement point's decisions, so it is followed
    // rather than read once: a revoked Policy or ServiceAccount stops granting within a
    // second, without a restart (R48, EP-19, OPS-45).
    if let Some(dir) = config.repo_dir.clone() {
        let reaper = crate::pdp::reaper::Reaper::new(Arc::clone(&gateway), dir);
        let reaper = match checkouts {
            Some(checkouts) => reaper.with_checkouts(checkouts),
            None => reaper,
        };
        // Workspace previews ride the same reload: the poller writes, the reaper loads
        // (Architecture/06 §7.2).
        let reaper = match config.previews_url.clone() {
            Some(url) => {
                let mirror = crate::previews::Mirror::new(config.previews_dir.clone());
                if token.is_none() {
                    tracing::warn!(
                        "previews are listed without a workload token: set JC_OIDC_ISSUER, \
                         JC_OIDC_CLIENT_ID and JC_OIDC_CLIENT_ASSERTION_FILE (or \
                         JC_OIDC_CLIENT_SECRET)"
                    );
                }
                tokio::spawn(mirror.follow(url, token.clone()));
                reaper.with_previews(config.previews_dir.clone())
            }
            None => reaper,
        };
        tokio::spawn(async move {
            let error = reaper.run().await;
            tracing::error!(%error, "the repository reaper stopped; exiting so a revoked grant never stays");
            std::process::exit(1);
        });
    }

    let internal = config.internal_bind.map(|_| {
        crate::mqtt::internal_router(
            Arc::clone(&gateway),
            config.mqtt_runner_host.clone(),
            config.mqtt_portal_host.clone(),
            config.mqtt_broker_host.clone(),
            config.mqtt_portal_identity.clone(),
        )
    });
    let app = router(Arc::clone(&gateway)).layer(tower_http::trace::TraceLayer::new_for_http());
    Ok(Built {
        app,
        gateway,
        internal,
    })
}
