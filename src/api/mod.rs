pub mod access_attributes;
pub mod access_explore;
pub mod access_matrix;
pub mod access_requests;
pub mod access_simulate;
pub mod account_secret;
pub mod activity;
pub mod agent_access;
pub mod agent_activity;
pub mod agent_registry;
pub mod agent_runs;
pub mod alerts;
pub mod app_build;
pub mod app_dashboard;
pub mod app_from_template;
pub mod app_me;
pub mod app_shapes;
pub mod app_share;
pub mod app_templates;
pub mod app_transfer;
pub mod assistant;
pub mod assistant_context;
pub mod assistant_preview;
pub mod basemap;
pub mod blueprints;
pub mod branding;
pub mod brokers;
pub mod bundle_models;
pub mod catalogue;
pub mod catalogue_draft;
pub mod catalogue_readable;
pub mod change_export;
pub mod changes;
pub mod ckan;
pub mod connectors;
pub mod costs;
pub mod data_access;
pub mod data_views;
pub mod datamodels;
pub mod datasource_test;
pub mod delete;
pub mod drafts;
pub mod drift;
pub mod dry_run;
pub mod endpoint_preview;
pub mod endpoint_standards;
pub mod endpoint_usage;
pub mod entity_comments;
pub mod entity_trash;
pub mod erasures;
pub mod export;
pub mod export_git;
pub mod federation;
pub mod feedback;
pub mod flow_map;
pub mod forms;
pub mod grafana;
pub mod health;
pub mod health_checks;
pub mod if_match;
pub mod import;
pub mod import_git;
pub mod internal;
pub mod knowledge;
pub mod knowledge_usage;
pub mod kpi_check;
pub mod list_query;
pub mod live;
pub mod mcp_confirmations;
pub mod mcp_elicitations;
pub mod mqtt_access;
pub mod mqtt_allowance;
pub mod mqtt_bindings;
pub mod mqtt_captured;
pub mod mqtt_devices;
pub mod mqtt_measured;
pub mod mqtt_test_publish;
pub mod mutate;
pub mod ops;
pub mod organization_limits;
pub mod people;
pub mod people_ids;
pub mod permissions;
pub mod pipeline_templates;
pub mod pipeline_test;
pub mod pipelines;
pub mod preferences;
pub mod projects;
pub mod quality;
pub mod resources;
pub mod restore;
pub mod secrets;
pub mod setup;
pub mod space_move;
pub mod space_retention;
pub mod space_usage;
pub mod surface_fit;
pub mod sync_sources;
pub mod temporal_audit;
pub mod type_attributes;
pub mod validation;
pub mod visualization_templates;
pub mod webhook;
pub mod workspaces;

use axum::http::Uri;
use axum::response::IntoResponse;
use axum::Router;

use crate::auth;
use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    // Routes protected by double-submit CSRF tokens (session-based UI mutations).
    let protected = Router::new()
        .merge(health::router())
        .merge(auth::oidc::router())
        .merge(agent_runs::router())
        .merge(app_build::router())
        .merge(app_dashboard::router())
        .merge(app_templates::router())
        .merge(app_from_template::router())
        .merge(app_shapes::router())
        .merge(app_me::router())
        .merge(app_share::router())
        .merge(assistant::router())
        .merge(pipeline_test::router())
        .merge(datasource_test::router())
        .merge(blueprints::router())
        .merge(branding::router())
        .merge(catalogue::router())
        .merge(catalogue_draft::router())
        .merge(catalogue_readable::router())
        .merge(data_access::router())
        .merge(endpoint_standards::router())
        .merge(changes::router())
        .merge(change_export::router())
        .merge(restore::router())
        .merge(feedback::router())
        .merge(connectors::router())
        .merge(flow_map::router())
        .merge(ckan::router())
        .merge(datamodels::router())
        .merge(drafts::router())
        .merge(activity::router())
        .merge(drift::router())
        .merge(export::router())
        .merge(app_transfer::router())
        .merge(forms::router())
        .merge(import::router())
        .merge(resources::router())
        .merge(ops::router())
        .merge(permissions::router())
        .merge(people::router())
        .merge(people_ids::router())
        .merge(access_attributes::router())
        .merge(access_simulate::router())
        .merge(access_explore::router())
        .merge(access_matrix::router())
        .merge(agent_access::router())
        .merge(account_secret::router())
        .merge(secrets::router())
        .merge(endpoint_preview::router())
        .merge(kpi_check::router())
        .merge(setup::router())
        .merge(pipelines::router())
        .merge(preferences::router())
        .merge(projects::router())
        .merge(quality::router())
        .merge(space_usage::router())
        .merge(space_retention::router())
        .merge(erasures::router())
        .merge(mcp_confirmations::router())
        .merge(mcp_elicitations::router())
        .merge(access_requests::router())
        .merge(data_views::router())
        .merge(pipeline_templates::router())
        .merge(temporal_audit::router())
        .merge(surface_fit::router())
        .merge(type_attributes::router())
        .merge(entity_trash::router())
        .merge(entity_comments::router())
        .merge(alerts::router())
        .merge(grafana::router())
        .merge(knowledge::router())
        .merge(knowledge_usage::router())
        .merge(live::router())
        .merge(sync_sources::router())
        .merge(validation::router())
        .merge(health_checks::router())
        .merge(endpoint_usage::router())
        .merge(visualization_templates::router())
        .merge(organization_limits::router())
        .merge(brokers::router())
        .merge(costs::router())
        .merge(agent_registry::router())
        .merge(agent_activity::router())
        .merge(space_move::router())
        .merge(mqtt_access::router())
        .merge(mqtt_bindings::router())
        .merge(mqtt_captured::router())
        .merge(mqtt_devices::router())
        .merge(mqtt_measured::router())
        .merge(mqtt_test_publish::router())
        .merge(mqtt_allowance::router())
        .merge(workspaces::router())
        .merge(crate::tools::model_tools::router())
        .layer(axum::middleware::from_fn(auth::csrf::require_csrf));

    // The Gitea webhook is a server-to-server call authenticated by its HMAC signature
    // (x-gitea-signature), so it must be exempt from the session guard and CSRF protection.
    // It is merged outside the require_csrf middleware layer as the sole exemption.
    Router::new()
        .merge(webhook::router())
        // A `schedule: { webhook: true }` source is driven by its own origin, which is a
        // server-to-server call with an HMAC signature and no session — the same exemption,
        // for the same reason (MF-28).
        .merge(sync_sources::webhook_router())
        // Keycloak's back-channel logout is the same kind of call: server to server, no cookie,
        // authenticated by the signature on its logout token (AP-29, CC-40).
        .merge(auth::oidc::backchannel_router())
        .merge(basemap::router())
        .merge(protected)
        .fallback(api_not_found)
}

async fn api_not_found(uri: Uri) -> impl IntoResponse {
    ApiError::NotFound(format!("API endpoint '{}' not found", uri.path()))
}
