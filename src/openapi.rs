use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use utoipa::openapi::schema::{Array, Ref, Schema};
use utoipa::openapi::RefOr;
use utoipa::{Modify, OpenApi};

use crate::activity::ActivityEvent;
use crate::agents::run::{AgentRun, AgentRunEvent, AgentRunStatus};
use crate::api::activity::{ActivityList, ExportLogsServiceResponse, PartialSuccess};
use crate::api::agent_runs::{
    AnswerRequest, CreateRunRequest, CreatedRun, EventReceipt, FailedRequest, MessageRequest,
    ObservedPage, PreviewErrorRequest, PreviewObservationRequest, RelayedEvent, RunContext,
    RunList,
};
use crate::api::assistant::{AgentAccessList, OperationAccess, ProfileAccess, StartConversation};
use crate::api::blueprints::FlowRequest;
use crate::api::catalogue::{
    CatalogueClass, CatalogueContact, CatalogueDataModel, CatalogueDataset, CatalogueDatasetDetail,
    CatalogueEndpoint, CatalogueFacetValue, CatalogueFacets, CatalogueFeature, CatalogueLicence,
    CatalogueMap, CatalogueMapField, CataloguePage, CataloguePublisher, CatalogueResource,
    CatalogueSample, CatalogueTemporal, CatalogueTheme,
};
use crate::api::changes::{
    ChangeAuthor, ChangeDecision, ChangeList, ChangeProposal, ChangeSummary, OutsideMergeRequest,
};
use crate::api::ckan::{CkanStatus, InstanceSummary, PublicationStatus, ResourceLink};
use crate::api::datamodels::{
    CatalogueEntry, ModelChange as DatamodelChange, OrganizationModel, OrganizationModels,
    SourceDryRunResult,
};
use crate::api::drafts::{DraftList, PutDraftRequest};
use crate::api::dry_run::DryRunResult;
use crate::api::export::{Revision, RevisionList};
use crate::api::federation::{Edge, EdgeKind, FederationGraph, Node, NodeHealth, RegistrationCard};
use crate::api::health::{Build, Health, Readiness, Version};
use crate::api::ops::{OperationAnnotations, OperationSummary};
use crate::api::pipelines::PipelineMetrics;
use crate::api::preferences::Preferences;
use crate::api::projects::{
    DuplicateProject, OpenProject, OrganizationSummary, ProjectDetail, ProjectList, ProjectStatus,
    ProjectSummary, RegistryEntry, RegistryTag, Repoint, Usage,
};
use crate::api::resources::{ListMeta, ResourceList};
use crate::auth::oidc::{LogoutTarget, Me};
use crate::auth::{Front, Identity};
use crate::branding::Validation;
use crate::branding::{Branding, Colours, Fonts, Languages};
use crate::change::{Change, ChangeMeta, ChangePhase, ChangeStatus, Lane, PlanSummary};
use crate::error::ProblemDetails;
use crate::ops::drafts::{Draft, DraftEvent};
use crate::ops::verdict::{Finding, Level, Verdict};
use crate::permissions::{Affordance, Effective, Grant, ProjectAffordances};
use crate::plan::{FieldChange, PlanDiff};
use crate::reconciler::SyncStatus;
use crate::resource::{ResourceEnvelope, Status};
use crate::state::AppState;
use crate::tools::model_tools::{
    Artifacts, Catalogue, CatalogueModel, CatalogueSubject, GenerateRequest, ImportSdmRequest,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::openapi::openapi_json,
        crate::auth::oidc::login,
        crate::auth::oidc::callback,
        crate::auth::oidc::backchannel_logout,
        crate::api::health::health,
        crate::api::health::ready,
        crate::api::health::version,
        crate::api::branding::get_branding,
        crate::api::branding::get_asset,
        crate::auth::oidc::me,
        crate::auth::oidc::sso_check,
        crate::auth::oidc::sso_check_done,
        crate::auth::oidc::logout,
        crate::api::projects::list_projects,
        crate::api::projects::open_project,
        crate::api::projects::get_project,
        crate::api::projects::delete_project,
        crate::api::projects::duplicate_project,
        crate::api::projects::get_registry_entry,
        crate::api::projects::repoint_project,
        crate::api::resources::list,
        crate::api::resources::list_endpoints_everywhere,
        crate::api::blueprints::list_blueprints,
        crate::api::forms::list_forms,
        crate::api::forms::kind_schema,
        crate::api::blueprints::start_flow,
        crate::api::datamodels::list_organization_datamodels,
        crate::api::datamodels::get_source,
        crate::api::datamodels::put_source,
        crate::api::datamodels::share_model,
        crate::api::resources::get_resource,
        crate::api::pipelines::get_metrics,
        crate::api::pipelines::get_samples,
        crate::api::pipelines::get_bento,
        crate::api::pipelines::get_stats,
        crate::api::pipelines::get_rejected,
        crate::api::pipelines::retry_rejected,
        crate::api::pipelines::get_runs,
        crate::api::pipelines::get_pipeline_health,
        crate::api::pipelines::get_latest_runs,
        crate::api::pipelines::post_run,
        crate::api::pipelines::get_run_log,
        crate::api::pipelines::get_source_deletes,
        crate::api::pipelines::post_source_deletes,
        crate::api::pipelines::get_source_removals,
        crate::api::export::export,
        crate::api::app_transfer::export_app,
        crate::api::import::import,
        crate::api::export::revisions,
        crate::api::permissions::permissions_me,
        // The map of a generated application loads these two directly, without the typed
        // client, which is how they came to carry `utoipa::path` annotations and still be
        // absent from the document (T-2168). A route a person can call is in the contract.
        crate::api::basemap::get_style,
        crate::api::basemap::get_tile,
        crate::api::agent_runs::create_run,
        crate::api::agent_runs::list_runs,
        crate::api::agent_runs::get_run,
        crate::api::agent_runs::stream_events,
        crate::api::agent_runs::answer_question,
        crate::api::agent_runs::post_message,
        crate::api::agent_runs::post_preview_error,
        crate::api::agent_runs::post_preview_observation,
        crate::api::agent_runs::call_function,
        crate::api::agent_runs::cancel_run,
        crate::api::agent_runs::publish_run,
        crate::api::app_build::build,
        crate::api::app_build::rebuild,
        crate::api::app_build::builds,
        crate::api::app_build::restore,
        crate::api::app_dashboard::dashboard,
        crate::api::app_dashboard::propose,
        crate::api::app_shapes::rename_shapes,
        crate::api::app_me::me,
        crate::api::app_share::audience,
        crate::api::app_share::shared,
        crate::api::agent_runs::preview,
        crate::api::assistant::start_conversation,
        crate::api::assistant::get_access,
        crate::api::access_attributes::explain_attribute,
        crate::api::access_simulate::simulate_access,
        crate::api::access_explore::explore_access,
        crate::api::access_matrix::access_matrix,
        crate::api::agent_access::account_access,
        crate::api::agent_access::revoke_grant,
        crate::api::account_secret::issue_secret,
        crate::api::secrets::list_secrets,
        crate::api::secrets::propose_secret,
        crate::api::endpoint_preview::preview_endpoint,
        crate::api::kpi_check::check_kpi,
        crate::api::people::list_people,
        crate::api::people_ids::person_names,
        crate::api::people_ids::name_by_id,
        crate::api::setup::get_setup,
        crate::api::people::create_person,
        crate::api::people::get_person,
        crate::api::people::edit_person,
        crate::api::people::disable_person,
        crate::api::people::enable_person,
        crate::api::people::reset_password,
        crate::api::people::resend_invitation,
        crate::api::people::remove_second_factor,
        crate::api::people::sign_out_person,
        crate::api::people::revoke_consent,
        crate::api::people::delete_person,
        crate::api::people::remove_people,
        crate::api::validation::get_health,
        crate::api::health_checks::list_project,
        crate::api::health_checks::list_all,
        crate::api::endpoint_usage::get_endpoint_usage,
        crate::api::endpoint_usage::list_project_usage,
        crate::api::endpoint_usage::export_project_usage,
        crate::api::organization_limits::get_limits,
        crate::api::brokers::list_brokers,
        crate::api::costs::get_costs,
        crate::api::costs::export_costs,
        crate::api::agent_registry::list_agents,
        crate::api::agent_activity::list_agent_activity,
        crate::api::space_move::start_move,
        crate::api::space_move::get_move,
        crate::api::mqtt_devices::device_options,
        crate::api::mqtt_devices::add_device,
        crate::api::mqtt_measured::get_measured,
        crate::api::mqtt_access::get_mqtt_access,
        crate::api::mqtt_bindings::get_bindings,
        crate::api::mqtt_test_publish::test_publish,
        crate::api::mqtt_test_publish::watch,
        crate::api::mqtt_captured::get_captured,
        crate::api::mqtt_captured::delete_captured,
        crate::api::mqtt_allowance::get_allowance,
        crate::api::validation::list_app_checks,
        crate::api::quality::get_quality,
        crate::api::quality::get_checks,
        crate::api::quality::get_endpoint_quality,
        crate::api::space_usage::get_usage,
        crate::api::space_usage::recalculate_usage,
        crate::api::flow_map::get_flow_map,
        crate::api::space_retention::preview_retention,
        crate::api::space_retention::retention_deleted,
        crate::api::erasures::export_subject,
        crate::api::erasures::erase_subject,
        crate::api::erasures::get_erasure,
        crate::api::erasures::list_erasures,
        crate::api::erasures::approve_erasure,
        crate::api::erasures::decline_erasure,
        crate::api::mcp_confirmations::get_confirmation,
        crate::api::mcp_confirmations::decide_confirmation,
        crate::api::mcp_elicitations::get_question,
        crate::api::mcp_elicitations::decide_question,
        crate::api::access_requests::file_request,
        crate::api::access_requests::list_outgoing,
        crate::api::access_requests::list_requestable,
        crate::api::access_requests::list_incoming,
        crate::api::access_requests::approve_request,
        crate::api::access_requests::decline_request,
        crate::api::access_requests::revoke_request,
        crate::api::access_requests::list_mine,
        crate::api::access_requests::file_person_request,
        crate::api::data_views::list_views,
        crate::api::pipeline_templates::list_pipeline_templates,
        crate::api::pipeline_templates::create_pipeline_template,
        crate::api::pipeline_templates::delete_pipeline_template,
        crate::api::data_views::create_view,
        crate::api::data_views::update_view,
        crate::api::data_views::delete_view,
        crate::api::temporal_audit::temporal_audit,
        crate::api::surface_fit::surface_fit,
        crate::api::type_attributes::type_attributes,
        crate::api::entity_trash::list_trash,
        crate::api::entity_trash::keep,
        crate::api::entity_trash::forget,
        crate::api::entity_comments::list_comments,
        crate::api::entity_comments::add_comment,
        crate::api::entity_comments::remove_comment,
        crate::api::entity_comments::list_notifications,
        crate::api::grafana::grafana_page,
        crate::api::grafana::grafana_options,
        crate::api::grafana::save_visualization,
        crate::api::grafana::remove_visualization,
        crate::api::alerts::list_alerts,
        crate::api::alerts::subscribe,
        crate::api::alerts::unsubscribe,
        crate::api::alerts::mute,
        crate::api::alerts::list_notices,
        crate::api::alerts::read_notice,
        crate::api::entity_comments::read_notification,
        crate::api::knowledge::list_sources,
        crate::api::knowledge::list_pages,
        crate::api::knowledge::list_documents,
        crate::api::knowledge::try_search,
        crate::api::knowledge::list_errors,
        crate::api::knowledge::list_links,
        crate::api::knowledge::list_passages,
        crate::api::knowledge::set_inclusion,
        crate::api::knowledge::recrawl,
        crate::api::knowledge::list_files,
        crate::api::knowledge::upload_file,
        crate::api::knowledge::remove_file,
        crate::api::knowledge::deployment_usage,
        crate::api::knowledge::forget_answers,
        crate::api::assistant_context::preview_context,
        crate::api::assistant_context::deployment_context,
        crate::api::knowledge::list_conversations,
        crate::api::knowledge_usage::conversation_usage,
        crate::api::knowledge_usage::export_conversations,
        crate::api::knowledge::get_conversation,
        crate::api::knowledge::erase_conversation,
        crate::api::knowledge::same_question,
        crate::api::knowledge::chat,
        crate::api::live::watch,
        crate::api::preferences::get_preferences,
        crate::api::preferences::put_preferences,
        crate::api::preferences::add_recent,
        crate::api::app_templates::list_templates,
        crate::api::app_from_template::create_from_template,
        crate::api::app_templates::screenshot,
        crate::api::visualization_templates::fill_template,
        crate::api::ops::list_ops,
        crate::api::ops::run_op,
        crate::api::drafts::list_drafts,
        crate::api::drafts::get_draft,
        crate::api::drafts::put_draft,
        crate::api::drafts::drop_draft,
        crate::api::drafts::stream_draft_events,
        crate::api::activity::list_activity,
        crate::api::activity::stream_activity,
        crate::api::drift::list_drift,
        crate::api::drift::revert_drift,
        crate::api::drift::adopt_drift,
        crate::api::activity::ingest_activity,
        crate::api::mutate::create,
        crate::api::mutate::replace,
        crate::api::mutate::patch,
        crate::api::changes::list_changes,
        crate::api::changes::list_change_history,
        crate::api::change_export::export_change_history,
        crate::api::restore::restore_change,
        crate::api::feedback::send_feedback,
        crate::api::feedback::list_feedback,
        crate::api::feedback::feedback_screenshot,
        crate::api::restore::undo_change,
        crate::api::restore::change_manifests,
        crate::api::ckan::get_status,
        crate::api::catalogue::get_catalogue,
        crate::api::catalogue::get_dataset,
        crate::api::catalogue::get_sample,
        crate::api::catalogue::get_map,
        crate::api::catalogue_readable::get_readable,
        crate::api::data_access::get_data_access,
        crate::api::data_access::get_person_readable,
        crate::api::endpoint_standards::get_standards,
        crate::api::catalogue_draft::draft_publication,
        crate::api::changes::get_change,
        crate::api::changes::approve_change,
        crate::api::changes::reject_change,
        crate::api::delete::delete_resource,
        crate::api::sync_sources::status,
        crate::api::sync_sources::sync_now,
        crate::api::sync_sources::pause,
        crate::api::sync_sources::detach,
        crate::api::sync_sources::webhook,
        crate::api::workspaces::open_workspace,
        crate::api::workspaces::list_workspaces,
        crate::api::workspaces::get_workspace,
        crate::api::workspaces::compare_workspace,
        crate::api::workspaces::update_workspace,
        crate::api::workspaces::propose_workspace,
        crate::api::workspaces::discard_workspace,
        crate::api::workspaces::start_workspace_preview,
        crate::api::workspaces::get_workspace_preview,
        crate::api::workspaces::stop_workspace_preview,
        crate::api::webhook::gitea_webhook,
        crate::tools::model_tools::sdm_catalog,
        crate::tools::model_tools::generate,
        crate::tools::model_tools::import_sdm,
        crate::tools::model_tools::infer_schema,
        crate::api::pipeline_test::test_pipeline,
        crate::api::datasource_test::test_datasource,
        crate::api::assistant::propose_endpoint,
        crate::mcp::handle_mcp,
        crate::api::connectors::list_connectors,
        crate::api::connectors::create_connector,
        crate::api::connectors::connector_options,
        crate::api::connectors::get_connector,
        crate::api::connectors::edit_connector,
        crate::api::connectors::revoke_connector,
        crate::api::connectors::connector_activity,
        crate::api::connectors::pending_connectors,
        crate::api::connectors::approve_connector,
        crate::api::connectors::pair_tab,
        crate::api::connectors::unpair_tab,
        crate::api::connectors::tab_events,
        crate::api::connectors::organization_connectors,
        crate::api::connectors::revoke_organization_connector,
    ),
    components(schemas(
        crate::api::access_attributes::AttributeExplained,
        crate::api::grafana::GrafanaPage,
        crate::api::grafana::VisualizationItem,
        crate::api::grafana::EndpointOption,
        crate::api::grafana::AttributeOption,
        crate::api::grafana::GrafanaOptions,
        crate::api::grafana::NewVisualization,
        crate::grafana::Kind,
        crate::api::access_simulate::SimulateRequest,
        crate::api::access_simulate::Who,
        crate::api::access_simulate::Simulated,
        crate::api::access_simulate::Resolved,
        crate::api::access_explore::ExploreRequest,
        crate::api::access_explore::Reached,
        crate::api::access_explore::Explored,
        crate::api::access_matrix::Matrix,
        crate::api::access_matrix::MatrixPrincipal,
        crate::api::access_matrix::MatrixVia,
        crate::api::access_matrix::MatrixCell,
        crate::api::access_matrix::MatrixEdge,
        crate::api::agent_access::AccountAccess,
        crate::api::agent_access::RevokeRequest,
        crate::api::account_secret::IssuedSecret,
        crate::api::secrets::StoredSecret,
        crate::api::secrets::SecretList,
        crate::api::secrets::SecretWrite,
        crate::permissions::AccountGrant,
        crate::permissions::AccountRule,
        crate::permissions::GrantState,
        crate::people::Consent,
        crate::api::endpoint_preview::PreviewRequest,
        crate::api::endpoint_preview::DraftProjection,
        crate::api::endpoint_preview::Previewed,
        crate::api::kpi_check::KpiCheckRequest,
        crate::api::kpi_check::KpiChecked,
        crate::api::kpi_check::KpiProblem,
        crate::api::kpi_check::KpiSource,
        crate::api::kpi_check::KpiPreview,
        crate::api::kpi_check::KpiValue,
        crate::api::knowledge::InclusionRequest,
        crate::api::knowledge::ChatRequest,
        crate::api::knowledge::ChatTurn,
        crate::api::validation::ValidationHealth,
        crate::api::health_checks::HealthChecks,
        crate::endpoint_usage::EndpointUsage,
        crate::endpoint_usage::Day,
        crate::api::endpoint_usage::ProjectUsage,
        crate::api::endpoint_usage::EndpointTotal,
        crate::health_log::Check,
        crate::health_log::Point,
        crate::health_log::Kind,
        crate::health_log::Status,
        crate::api::organization_limits::OrganizationLimits,
        crate::api::brokers::Brokers,
        crate::api::costs::Costs,
        crate::costs::CostPrices,
        crate::costs::PricedItem,
        crate::costs::PricedVolume,
        crate::costs::Split,
        crate::costs::ProjectCost,
        crate::costs::PlatformCost,
        crate::costs::SpaceCost,
        crate::costs::AssistantCost,
        crate::api::agent_registry::AgentRegistry,
        crate::api::agent_registry::AgentItem,
        crate::api::agent_registry::Owner,
        crate::api::agent_registry::ActsAs,
        crate::api::agent_registry::Spend,
        crate::api::agent_registry::AgentState,
        crate::api::agent_activity::AgentActivity,
        crate::api::agent_activity::ActivityRun,
        crate::api::agent_activity::ActivityStep,
        crate::api::agent_activity::StepVerdict,
        crate::api::agent_activity::AgentAlert,
        crate::api::agent_activity::ActivityChoices,
        crate::api::agent_activity::AlertKind,
        crate::api::brokers::Broker,
        crate::api::brokers::Health,
        crate::api::space_move::MoveRequest,
        crate::space_move::Move,
        crate::space_move::MoveState,
        crate::space_move::Digest,
        crate::api::mqtt_devices::DeviceOptions,
        crate::api::mqtt_devices::DeviceRole,
        crate::api::mqtt_devices::AddDevice,
        crate::api::mqtt_measured::Measured,
        crate::api::mqtt_measured::AccountSeen,
        crate::api::mqtt_measured::RatePoint,
        crate::api::mqtt_access::Access,
        crate::api::mqtt_access::HeldRole,
        crate::api::mqtt_access::OutOfWindow,
        crate::api::mqtt_bindings::Bindings,
        crate::api::mqtt_bindings::Binding,
        crate::api::mqtt_bindings::Hop,
        crate::api::mqtt_bindings::AttributeBinding,
        crate::api::mqtt_test_publish::TestPublish,
        crate::api::mqtt_test_publish::DryRun,
        crate::api::mqtt_test_publish::Step,
        crate::api::mqtt_test_publish::Outcome,
        crate::api::mqtt_test_publish::Watched,
        crate::api::mqtt_test_publish::Refusal,
        crate::api::mqtt_captured::Captured,
        crate::api::mqtt_captured::CapturedMessage,
        crate::api::mqtt_allowance::Allowance,
        crate::api::mqtt_allowance::Meter,
        crate::api::mqtt_allowance::Ceiling,
        crate::api::mqtt_allowance::PortInUse,
        crate::api::organization_limits::LimitEntry,
        crate::api::organization_limits::LimitOrigin,
        crate::api::organization_limits::ProjectQuota,
        crate::api::organization_limits::QuotaUse,
        crate::api::validation::CheckHealth,
        crate::api::validation::CheckState,
        crate::api::validation::Digest,
        crate::api::validation::Counts,
        crate::api::validation::Failure,
        crate::api::validation::FailureVerdict,
        crate::api::validation::Point,
        crate::api::validation::AppChecks,
        crate::quality::SpaceQuality,
        crate::dataset_score::DatasetScore,
        crate::dataset_score::ScorePart,
        crate::dataset_score::Part,
        crate::dataset_score::Text,
        crate::quality_checks::SpaceChecks,
        crate::quality_checks::RuleChecks,
        crate::quality_checks::Counts,
        crate::quality_checks::Windows,
        crate::space_usage::SpaceUsage,
        crate::flow_map::FlowMap,
        crate::flow_map::FlowNode,
        crate::flow_map::FlowEdge,
        crate::flow_map::FlowKind,
        crate::flow_map::FlowRole,
        crate::flow_map::FlowRun,
        crate::flow_map::FlowRelation,
        crate::flow_map::FlowGrant,
        crate::flow_map::FlowGrantState,
        crate::space_usage::SpaceBytes,
        crate::api::space_retention::RetentionDraft,
        crate::access_requests::NewRequest,
        crate::access_requests::TypeGrant,
        crate::access_requests::EndpointName,
        crate::access_requests::Operations,
        crate::api::access_requests::Decision,
        crate::api::access_requests::Requestable,
        crate::api::access_requests::RequestableType,
        crate::api::access_requests::PersonRequest,
        crate::api::space_retention::RetentionCounts,
        crate::api::erasures::SubjectRequest,
        crate::api::erasures::ApproveRequest,
        crate::api::mcp_elicitations::DecisionRequest,
        crate::api::mcp_confirmations::Confirmation,
        crate::mcp::elicitation::Question,
        crate::mcp::elicitation::ElicitationDecision,
        crate::api::erasures::DeclineRequest,
        crate::reconciler::space_rules::RetentionCount,
        crate::api::data_views::ViewList,
        crate::api::pipeline_templates::TemplateList,
        crate::ops::pipeline_templates::PipelineTemplate,
        crate::ops::pipeline_templates::CreateTemplate,
        crate::pipeline_validation::ClassAttributes,
        crate::api::surface_fit::SurfaceFit,
        crate::api::surface_fit::SurfaceGap,
        crate::api::temporal_audit::TemporalAudit,
        crate::temporal_audit::AuditRow,
        crate::temporal_audit::ObservedAt,
        crate::temporal_audit::Verdict,
        crate::pipeline_validation::Attribute,
        crate::pipeline_validation::Unit,
        crate::pipeline_validation::RelationshipTarget,
        crate::ops::data_views::DataView,
        crate::ops::data_views::ViewConfig,
        crate::ops::data_views::SortKey,
        crate::ops::data_views::ColourRule,
        crate::ops::data_views::CreateView,
        crate::ops::data_views::UpdateView,
        crate::quality::RuleCount,
        crate::quality::Freshness,
        crate::quality::FreshState,
        crate::api::validation::AppCheck,
        crate::api::validation::AppCheckState,
        crate::api::app_build::AppBuild,
        crate::api::app_build::Rebuild,
        crate::api::app_build::AppBuilds,
        crate::api::app_build::BuildEntry,
        crate::api::app_build::RestoreRequest,
        crate::api::app_build::Restored,
        crate::api::app_dashboard::AppDashboard,
        crate::api::app_dashboard::EditOffer,
        crate::api::app_dashboard::DashboardEdit,
        crate::api::app_dashboard::DashboardProposal,
        crate::api::app_me::AppMe,
        crate::api::app_share::AppAudience,
        crate::api::app_share::RoleAudience,
        crate::api::app_share::SharedApp,
        crate::git::WorkflowRun,
        AgentRun,
        AgentRunEvent,
        AgentRunStatus,
        CreateRunRequest,
        CreatedRun,
        StartConversation,
        AgentAccessList,
        ProfileAccess,
        OperationAccess,
        RunList,
        RunContext,
        RelayedEvent,
        EventReceipt,
        AnswerRequest,
        MessageRequest,
        PreviewErrorRequest,
        PreviewObservationRequest,
        ObservedPage,
        FailedRequest,
        Health,
        Readiness,
        Build,
        Version,
        Effective,
        Grant,
        Affordance,
        ProjectAffordances,
        Branding,
        Colours,
        Fonts,
        Languages,
        CkanStatus,
        CataloguePage,
        CatalogueDataset,
        CatalogueDatasetDetail,
        CataloguePublisher,
        CatalogueLicence,
        CatalogueFacets,
        CatalogueFacetValue,
        CatalogueTemporal,
        CatalogueContact,
        CatalogueResource,
        CatalogueEndpoint,
        CatalogueDataModel,
        CatalogueClass,
        CatalogueTheme,
        crate::api::catalogue_readable::ReadableCatalogue,
        crate::api::catalogue_readable::ReadableDataset,
        crate::api::data_access::DataAccess,
        crate::api::data_access::AccessColumn,
        crate::api::data_access::AccessRow,
        crate::api::data_access::AccessCell,
        crate::api::data_access::GrantingRule,
        CatalogueSample,
        CatalogueMap,
        CatalogueMapField,
        CatalogueFeature,
        crate::api::endpoint_standards::EndpointStandards,
        crate::api::endpoint_standards::StandardGuide,
        crate::api::endpoint_standards::EndpointLicence,
        crate::api::endpoint_standards::Example,
        crate::api::catalogue_draft::CatalogueDraftRequest,
        crate::api::catalogue_draft::CatalogueDraft,
        crate::api::catalogue_draft::DraftAttribute,
        crate::api::catalogue_draft::Withheld,
        InstanceSummary,
        PublicationStatus,
        ResourceLink,
        FederationGraph,
        Node,
        NodeHealth,
        Edge,
        EdgeKind,
        RegistrationCard,
        Identity,
        Me,
        Front,
        LogoutTarget,
        ProblemDetails,
        ResourceEnvelope,
        crate::api::mutate::ResourceProposal,
        crate::api::import::ImportOptions,
        crate::ops::DraftRef,
        Status,
        ResourceList,
        ListMeta,
        DuplicateProject,
        RegistryEntry,
        RegistryTag,
        Repoint,
        OpenProject,
        ProjectDetail,
        ProjectStatus,
        Usage,
        ProjectList,
        ProjectSummary,
        OrganizationSummary,
        Change,
        ChangeMeta,
        ChangeStatus,
        ChangePhase,
        crate::api::sync_sources::SyncSourceStatus,
        crate::api::sync_sources::SyncRunReport,
        crate::api::sync_sources::PauseRequest,
        crate::ops::workspaces::OpenRequest,
        crate::ops::previews::Preview,
        crate::ops::previews::PreviewEndpoint,
        crate::ops::workspaces::UpdateRequest,
        crate::ops::workspaces::Resolution,
        crate::ops::workspaces::UpdateReport,
        crate::ops::workspaces::WorkspaceView,
        crate::ops::workspaces::WorkspaceList,
        crate::ops::workspaces::Workspace,
        crate::ops::workspaces::Scope,
        crate::ops::workspaces::ScopedResource,
        crate::ops::workspaces::PreviewState,
        crate::ops::workspaces::Comparison,
        crate::ops::workspaces::FileConflict,
        crate::plan::FieldConflict,
        crate::plan::Side,
        crate::api::import::ImportReport,
        crate::api::import::ConflictPolicy,
        Lane,
        PlanSummary,
        PlanDiff,
        FieldChange,
        DryRunResult,
        SourceDryRunResult,
        OrganizationModels,
        OrganizationModel,
        CatalogueEntry,
        DatamodelChange,
        OperationSummary,
        OperationAnnotations,
        ChangeProposal,
        ChangeList,
        ChangeDecision,
        OutsideMergeRequest,
        ChangeSummary,
        ChangeAuthor,
        SyncStatus,
        PipelineMetrics,
        crate::pipeline_stats::PipelineStats,
        crate::pipeline_stats::NodeWindows,
        crate::api::pipelines::RejectedPage,
        crate::api::pipelines::RetryRequest,
        crate::api::pipelines::RetryAnswer,
        crate::api::pipelines::RunList,
        crate::api::pipelines::HealthList,
        crate::api::pipelines::LatestRunList,
        crate::api::pipelines::LatestRun,
        crate::pipeline_health::Health,
        crate::pipeline_health::State,
        crate::pipeline_health::ErrorRatio,
        crate::api::pipelines::RunStarted,
        crate::api::pipelines::LogPage,
        crate::api::pipelines::SourceDecision,
        crate::api::pipelines::SourceChoice,
        crate::api::pipelines::SourceDecided,
        crate::pipeline_source_deletes::Overview,
        crate::pipeline_source_deletes::Hold,
        crate::pipeline_source_deletes::HoldReason,
        crate::pipeline_source_deletes::Removal,
        crate::pipeline_source_deletes::CompleteRun,
        crate::pipeline_log::Run,
        crate::pipeline_log::LogLine,
        crate::pipeline_log::Outcome,
        crate::pipeline_outcomes::Rejected,
        GenerateRequest,
        ImportSdmRequest,
        Artifacts,
        Catalogue,
        CatalogueSubject,
        CatalogueModel,
        Preferences,
        crate::api::preferences::Place,
        crate::api::app_templates::AppTemplates,
        crate::api::app_from_template::TemplateApp,
        crate::agents::samples::Template,
        crate::api::preferences::RecentPlaces,
        Revision,
        RevisionList,
        FlowRequest,
        ActivityEvent,
        ActivityList,
        ExportLogsServiceResponse,
        PartialSuccess,
        Draft,
        DraftEvent,
        DraftList,
        PutDraftRequest,
        Verdict,
        Finding,
        Level,
        Validation,
    )),
    info(
        title = "joinedcontext Portal API",
        version = "0.1.0",
        description = "Administrative and platform management REST API for joinedcontext Portal"
    ),
    tags(
        (name = "system", description = "System operations"),
        (name = "auth", description = "Sign-in, sign-out and the current identity"),
        (name = "resources", description = "Resource operations"),
        (name = "people", description = "The people of the organization's realm (PF-90)"),
        (name = "organization", description = "What a new organization still lacks (PF-90, UI-82)"),
        (name = "permissions", description = "What the caller may do in a project (PF-50)"),
        (name = "tools", description = "Model Tools schema generation and preview"),
        (name = "ops", description = "One operation registry behind every door (AG-59, ADR-N-021)"),
        (name = "drafts", description = "Shared manifest drafts every window shares (AG-61, UI-47)"),
        (name = "preferences", description = "The signed-in person's own UI preferences"),
        (name = "connectors", description = "A person's scoped connectors, each an MCP server at /c/{id} (AG-142, ADR-N-064)"),
        (name = "access", description = "ServiceAccounts, their API keys and effective grants"),
        (name = "basemap", description = "Map tiles and styles for application views, proxied so no coordinate leaves the platform (AP-67)"),
        (name = "knowledge", description = "What the knowledge assistant holds of a project's sources, read and steered (API/01 §34, AG-113)")
    ),
    modifiers(&JcCoreSchemas, &SharedRefusals)
)]
pub struct ApiDoc;

/// jc-core's types describe themselves with schemars, the Portal's with utoipa. The fields typed
/// by jc-core point at named components (so the generated TypeScript keeps `ObjectMeta`, `Phase`,
/// `Condition` and `Text` as it always had them) and this modifier fills those components in from the
/// crate's own JSON Schema, so the document can never drift from the tagged contract.
struct JcCoreSchemas;

impl Modify for JcCoreSchemas {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.schemas.insert(
            "ObjectMeta".into(),
            schemars_schema::<jc_core::ObjectMeta>(),
        );
        components
            .schemas
            .insert("Phase".into(), schemars_schema::<jc_core::Phase>());
        components
            .schemas
            .insert("Text".into(), schemars_schema::<jc_core::i18n::Text>());
        components
            .schemas
            .insert("Condition".into(), schemars_schema::<jc_core::Condition>());
    }
}

/// Every registry operation as its own entry (AG-59, AG-146, T-5948): the input and output schemas,
/// the title and description `tools/list` publishes, and the rule the door applies
/// (`x-jc-requires`), all from the one summary `ops::summary` makes, so the OpenAPI document and
/// the MCP tool list cannot say two different things. The generic `/ops/{name}` entry stays for
/// callers that pick the operation at run time. The entries go in as JSON, after utoipa built the
/// rest: its schema types drop keywords a registry schema uses (`examples`, `oneOf` members), and a
/// copy that loses them would no longer be the schema the door enforces.
pub fn document_text() -> String {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(render).clone()
}

fn render() -> String {
    let mut api = ApiDoc::openapi();
    let entries: Vec<(String, serde_json::Value)> = crate::ops::registry()
        .iter()
        .map(|op| {
            let path = format!("/api/v1/projects/{{project}}/ops/{}", op.name);
            (path, operation_entry(&crate::ops::summary(op)))
        })
        .collect();
    // A placeholder per entry keeps utoipa's member order for the rest of the document, so the
    // committed `ui/openapi.json` changes only where an operation is added.
    for (path, _) in &entries {
        api.paths
            .paths
            .insert(path.clone(), utoipa::openapi::PathItem::default());
    }
    let mut text = serde_json::to_string_pretty(&api).unwrap_or_default();
    for (path, entry) in entries {
        let placeholder = format!("    {}: {{}}", serde_json::Value::from(path.as_str()));
        let body = serde_json::to_string_pretty(&entry)
            .unwrap_or_default()
            .replace('\n', "\n    ");
        let filled = format!("    {}: {body}", serde_json::Value::from(path.as_str()));
        text = text.replacen(&placeholder, &filled, 1);
    }
    text
}

/// The OpenAPI path item of one operation, as JSON.
pub fn operation_entry(op: &crate::ops::OperationSummary) -> serde_json::Value {
    let problem = |description: &str| {
        serde_json::json!({
            "description": description,
            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ProblemDetails" } } }
        })
    };
    serde_json::json!({
        "post": {
            "tags": ["ops"],
            "operationId": op.name,
            "summary": op.title,
            "description": op.description,
            "parameters": [{
                "name": "project", "in": "path", "required": true,
                "description": "Project slug", "schema": { "type": "string" }
            }],
            "requestBody": {
                "required": true,
                "content": { "application/json": {
                    "schema": op.input_schema,
                    "example": crate::ops::example_of(&op.input_schema)
                } }
            },
            "responses": {
                "200": {
                    "description": "The operation's answer",
                    "content": { "application/json": { "schema": op.output_schema } }
                },
                "202": {
                    "description": "The change the operation opened, waiting for a person's approval",
                    "content": { "application/json": { "schema": { "type": "object" } } }
                },
                "401": problem("Unauthorized"),
                "403": problem("Forbidden: the caller lacks what x-jc-requires names"),
                "404": problem("The project or an object the input names is not there"),
                "422": problem("The input does not match the schema; the problem names the path")
            },
            "x-jc-requires": op.requires
        }
    })
}

/// The refusal every session write shares (PF-50, T-1656). The CSRF guard stands in front of
/// every write the API router mounts, and every write needs a verb the caller may lack, so each
/// one can answer `403` with the problem body whatever its own annotation lists. The
/// server-to-server doors sit outside the guard and answer for their own signatures, so they
/// keep what they document. An operation that documents its own `403` keeps its own wording.
struct SharedRefusals;

impl Modify for SharedRefusals {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::{ContentBuilder, ResponseBuilder};

        let forbidden = ResponseBuilder::new()
            .description(
                "Forbidden: the CSRF token is missing or does not match, or the caller lacks the \
                 verb this write needs",
            )
            .content(
                "application/json",
                ContentBuilder::new()
                    .schema(Some(Ref::from_schema_name("ProblemDetails")))
                    .build(),
            )
            .build();
        for (path, item) in openapi.paths.paths.iter_mut() {
            if path.starts_with("/api/v1/webhooks/") || path == "/api/v1/auth/backchannel-logout" {
                continue;
            }
            for operation in [
                item.post.as_mut(),
                item.put.as_mut(),
                item.patch.as_mut(),
                item.delete.as_mut(),
            ]
            .into_iter()
            .flatten()
            {
                operation
                    .responses
                    .responses
                    .entry("403".to_owned())
                    .or_insert_with(|| RefOr::T(forbidden.clone()));
            }
        }
    }
}

/// A jc-core type's draft-07 schema as a utoipa schema. Subschemas are inlined so no
/// `#/definitions/` reference is left pointing outside the OpenAPI components.
fn schemars_schema<T: schemars::JsonSchema>() -> RefOr<Schema> {
    let mut settings = schemars::gen::SchemaSettings::draft07();
    settings.inline_subschemas = true;
    let root = schemars::gen::SchemaGenerator::new(settings).into_root_schema_for::<T>();
    let value = serde_json::to_value(root.schema).unwrap_or_default();
    serde_json::from_value(value).unwrap_or_else(|err| {
        // A schema this crate cannot express is a build defect, not a runtime condition: the
        // openapi test catches it before it ships. Documented as a free object until then.
        tracing::error!(error = %err, "jc-core schema is not a valid OpenAPI schema");
        RefOr::T(Schema::Object(Default::default()))
    })
}

pub fn object_meta_ref() -> Ref {
    Ref::from_schema_name("ObjectMeta")
}

pub fn text_ref() -> Ref {
    Ref::from_schema_name("Text")
}

pub fn phase_ref() -> Ref {
    Ref::from_schema_name("Phase")
}

pub fn conditions_ref() -> Array {
    Array::new(Ref::from_schema_name("Condition"))
}

/// The document itself: a client that generates its types from the Portal fetches this, so it is
/// a path of the surface like any other (MF-11, T-2361).
#[utoipa::path(
    get,
    path = "/api/v1/openapi.json",
    tag = "meta",
    responses((status = 200, description = "The OpenAPI 3.1 document of this Portal"))
)]
pub async fn openapi_json() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        document_text(),
    )
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/openapi.json", get(openapi_json))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// T-5948: one entry per operation, filled where utoipa left its placeholder, and the text
    /// stays valid JSON with the generic door beside the entries.
    #[test]
    fn every_operation_has_its_entry_in_the_document() {
        let doc: serde_json::Value =
            serde_json::from_str(&document_text()).expect("the document is JSON");
        let paths = doc["paths"].as_object().expect("paths");
        for op in crate::ops::registry() {
            let entry = &paths[&format!("/api/v1/projects/{{project}}/ops/{}", op.name)]["post"];
            assert_eq!(entry["operationId"], op.name);
            assert_eq!(
                entry["requestBody"]["content"]["application/json"]["schema"],
                (op.input)()
            );
            assert_eq!(
                entry["x-jc-requires"]["verb"],
                crate::ops::requires(op).verb
            );
        }
        assert!(paths.contains_key("/api/v1/projects/{project}/ops/{name}"));
        let op = crate::ops::find("jc_schema_get").expect("registered");
        let entry = operation_entry(&crate::ops::summary(op));
        assert_eq!(
            entry["post"]["requestBody"]["content"]["application/json"]["example"],
            json!({ "kind": "Pipeline" })
        );
        let text = document_text();
        let unfilled = crate::ops::registry()
            .iter()
            .find(|op| text.contains(&format!("/ops/{}\": {{}}", op.name)));
        assert!(unfilled.is_none(), "a placeholder was left unfilled");
    }
}
