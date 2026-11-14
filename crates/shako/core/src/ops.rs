#![forbid(unsafe_code)]
//! Single operation registry (`OperationDef` with `compat_path`) and OData 4.01 `Service`
//! definition (S8, S9, R§9). Shaped like `kaisatsu_core::ops` to move to `jc-ops` unchanged.

use crate::auth::{Action, Authorizer, Decision, ObjectTarget, RequestCtx};
use odata_table::{EntityDef, Field, FieldKind, Operation, Rule, Scope, Service};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const ODATA_NAMESPACE: &str = "Shako";

/// How an operation appears in the OData 4.01 service (S9, R§9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ODataKind {
    EntitySetRead,
    EntitySetWrite,
    Action,
    Function,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ODataParam {
    pub name: &'static str,
    pub edm_type: &'static str,
    pub nullable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ODataOperation {
    pub kind: ODataKind,
    pub name: &'static str,
    pub binding: Option<&'static str>,
    pub binding_collection: bool,
    pub params: Vec<ODataParam>,
    pub return_type: Option<&'static str>,
}

const fn p(name: &'static str, edm_type: &'static str, nullable: bool) -> ODataParam {
    ODataParam {
        name,
        edm_type,
        nullable,
    }
}

/// Declaration of one operation across native REST, Gitea compat, OData 4.01, MCP, and CLI (S8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationDef {
    pub name: &'static str,
    pub mcp_tool: &'static str,
    pub cli_command: &'static str,
    pub connector_segment: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub entity_set: Option<&'static str>,
    pub action: Action,
    pub permission: &'static str,
    pub audit_kind: &'static str,
    pub rest_method: &'static str,
    pub rest_path: &'static str,
    pub compat_path: Option<&'static str>,
    pub input_schema: Value,
    pub output_schema: Value,
    pub odata: ODataOperation,
}

fn req_field(name: &str, kind: FieldKind, description: &str) -> Field {
    Field {
        name: name.to_owned(),
        kind,
        description: description.to_owned(),
        nullable: false,
    }
}

fn opt_field(name: &str, kind: FieldKind, description: &str) -> Field {
    Field::new(name, kind, description)
}

fn read_only_rules(perm: &str) -> Vec<Rule> {
    vec![
        Rule {
            operation: Operation::Read,
            permission: Some(perm.to_owned()),
        },
        Rule {
            operation: Operation::Export,
            permission: Some(perm.to_owned()),
        },
    ]
}

fn rw_rules(read_perm: &str, write_perm: &str) -> Vec<Rule> {
    vec![
        Rule {
            operation: Operation::Read,
            permission: Some(read_perm.to_owned()),
        },
        Rule {
            operation: Operation::Export,
            permission: Some(read_perm.to_owned()),
        },
        Rule {
            operation: Operation::Create,
            permission: Some(write_perm.to_owned()),
        },
        Rule {
            operation: Operation::Update,
            permission: Some(write_perm.to_owned()),
        },
        Rule {
            operation: Operation::Delete,
            permission: Some(write_perm.to_owned()),
        },
    ]
}

/// Builds the `odata-table::Service` for `shako` (CSDL namespace `Shako`, S9, R§9).
/// Secret fields (webhook secret, push mirror credentials) are NEVER declared here, so
/// `$filter` or `$orderby` on them fails with `UnknownProperty` (400 Bad Request).
#[must_use]
pub fn shako_odata_service() -> Service {
    Service {
        namespace: ODATA_NAMESPACE.into(),
        title: "shako OData 4.01 Git Forge Service".into(),
        sets: vec![
            EntityDef {
                set: "Instances".into(),
                type_name: "Instance".into(),
                description: "Forge instances and their quotas and usage.".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Instance UUID"),
                    req_field("Host", FieldKind::String, "Instance hostname"),
                    req_field("MaxRepos", FieldKind::Integer, "Max repositories quota"),
                    req_field(
                        "MaxStorageBytes",
                        FieldKind::Integer,
                        "Max storage bytes quota",
                    ),
                    req_field("RepoCount", FieldKind::Integer, "Current repository count"),
                    req_field("GitBytes", FieldKind::Integer, "Current Git storage bytes"),
                    req_field(
                        "PackageBytes",
                        FieldKind::Integer,
                        "Current package storage bytes",
                    ),
                ],
                rules: read_only_rules("forge:read"),
            },
            EntityDef {
                set: "Repositories".into(),
                type_name: "Repository".into(),
                description: "Git repositories hosted in the organization.".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Repository UUID"),
                    req_field("Owner", FieldKind::String, "Organization owner slug"),
                    req_field("Name", FieldKind::String, "Repository name"),
                    req_field("Project", FieldKind::String, "Project slug"),
                    req_field("Description", FieldKind::String, "Repository description"),
                    req_field("DefaultBranch", FieldKind::String, "Default branch name"),
                    req_field("Private", FieldKind::Boolean, "Whether private"),
                    req_field("Archived", FieldKind::Boolean, "Whether archived read-only"),
                    req_field("SizeBytes", FieldKind::Integer, "Repository size in bytes"),
                    req_field("CloneUrl", FieldKind::String, "HTTPS clone URL"),
                    opt_field("CreatedAt", FieldKind::DateTime, "Creation timestamp"),
                    opt_field("UpdatedAt", FieldKind::DateTime, "Last update timestamp"),
                ],
                rules: rw_rules("forge:read", "forge:write"),
            },
            EntityDef {
                set: "Branches".into(),
                type_name: "Branch".into(),
                description: "Git branches of a repository (requires RepoId eq in $filter).".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Composite key {RepoId}:{Name}"),
                    req_field("RepoId", FieldKind::String, "Repository UUID"),
                    req_field("Name", FieldKind::String, "Branch name"),
                    req_field("HeadSha", FieldKind::String, "Head commit SHA"),
                    req_field(
                        "Protected",
                        FieldKind::Boolean,
                        "Whether protected by a rule",
                    ),
                ],
                rules: read_only_rules("forge:read"),
            },
            EntityDef {
                set: "Tags".into(),
                type_name: "Tag".into(),
                description: "Git tags of a repository (requires RepoId eq in $filter).".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Composite key {RepoId}:{Name}"),
                    req_field("RepoId", FieldKind::String, "Repository UUID"),
                    req_field("Name", FieldKind::String, "Tag name"),
                    req_field("TargetSha", FieldKind::String, "Target commit SHA"),
                    req_field("Annotated", FieldKind::Boolean, "Whether annotated tag"),
                ],
                rules: read_only_rules("forge:read"),
            },
            EntityDef {
                set: "Commits".into(),
                type_name: "Commit".into(),
                description: "Git commits of a repository (requires RepoId eq in $filter).".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Composite key {RepoId}:{Sha}"),
                    req_field("RepoId", FieldKind::String, "Repository UUID"),
                    req_field("Sha", FieldKind::String, "Commit SHA-1"),
                    req_field("Ref", FieldKind::String, "Walked ref"),
                    req_field("Message", FieldKind::String, "Commit message summary"),
                    req_field("Author", FieldKind::String, "Author name"),
                    opt_field("Email", FieldKind::String, "Author email"),
                    req_field("Date", FieldKind::String, "Author timestamp"),
                    req_field(
                        "Position",
                        FieldKind::Integer,
                        "Place in the history walked from Ref, newest first; 0 is Ref's own commit",
                    ),
                ],
                rules: read_only_rules("forge:read"),
            },
            EntityDef {
                set: "PullRequests".into(),
                type_name: "PullRequest".into(),
                description: "Pull requests of repositories in the organization.".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Composite key {RepoId}:{Number}"),
                    req_field("RepoId", FieldKind::String, "Repository UUID"),
                    req_field("Number", FieldKind::Integer, "Pull request number"),
                    req_field(
                        "HtmlUrl",
                        FieldKind::String,
                        "Web address of the pull request",
                    ),
                    req_field("State", FieldKind::String, "open or closed"),
                    req_field("Title", FieldKind::String, "Title"),
                    req_field("Body", FieldKind::String, "Body markdown"),
                    req_field("Head", FieldKind::String, "Head branch name"),
                    req_field("Base", FieldKind::String, "Base branch name"),
                    req_field("HeadSha", FieldKind::String, "Head commit SHA"),
                    req_field("BaseSha", FieldKind::String, "Base commit SHA"),
                    req_field("MergeBase", FieldKind::String, "Merge base commit SHA"),
                    opt_field(
                        "Mergeable",
                        FieldKind::Boolean,
                        "Whether mergeable without conflict",
                    ),
                    req_field("Merged", FieldKind::Boolean, "Whether merged"),
                    req_field("MergeCommitSha", FieldKind::String, "Merge commit SHA"),
                    req_field("Author", FieldKind::String, "Author login"),
                    req_field("AuthorName", FieldKind::String, "Author display name"),
                    opt_field("AuthorEmail", FieldKind::String, "Author e-mail"),
                    opt_field("CreatedAt", FieldKind::DateTime, "Created timestamp"),
                    opt_field("UpdatedAt", FieldKind::DateTime, "Last update timestamp"),
                    opt_field("ClosedAt", FieldKind::DateTime, "Closed timestamp"),
                ],
                rules: rw_rules("forge:read", "forge:propose"),
            },
            EntityDef {
                set: "Reviews".into(),
                type_name: "Review".into(),
                description: "Pull request reviews.".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Review UUID"),
                    req_field("RepoId", FieldKind::String, "Repository UUID"),
                    req_field("Number", FieldKind::Integer, "Pull request number"),
                    req_field(
                        "Verdict",
                        FieldKind::String,
                        "APPROVED, REQUEST_CHANGES, or COMMENT",
                    ),
                    req_field("Body", FieldKind::String, "Review comment body"),
                    req_field("Author", FieldKind::String, "Reviewer login"),
                    req_field("CommitId", FieldKind::String, "Reviewed commit SHA"),
                    opt_field("CreatedAt", FieldKind::DateTime, "Submitted timestamp"),
                ],
                rules: rw_rules("forge:read", "forge:approve"),
            },
            EntityDef {
                set: "ProtectionRules".into(),
                type_name: "ProtectionRule".into(),
                description: "Branch protection rules.".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Composite key {RepoId}:{Pattern}"),
                    req_field("RepoId", FieldKind::String, "Repository UUID"),
                    req_field("Pattern", FieldKind::String, "Branch glob pattern"),
                    req_field(
                        "EnablePush",
                        FieldKind::Boolean,
                        "Whether direct push is enabled",
                    ),
                    req_field(
                        "EnableForcePush",
                        FieldKind::Boolean,
                        "Whether force push is enabled",
                    ),
                    req_field(
                        "BlockOnOutdatedBranch",
                        FieldKind::Boolean,
                        "Block merge when behind base",
                    ),
                    req_field(
                        "BlockOnRejectedReviews",
                        FieldKind::Boolean,
                        "Block merge on rejected review",
                    ),
                    req_field(
                        "BlockAdminMergeOverride",
                        FieldKind::Boolean,
                        "Block admin override",
                    ),
                    req_field(
                        "RequiredApprovals",
                        FieldKind::Integer,
                        "Required approval count",
                    ),
                ],
                rules: rw_rules("forge:read", "forge:admin"),
            },
            EntityDef {
                set: "Webhooks".into(),
                type_name: "Webhook".into(),
                description: "Outbound webhooks (secrets are never declared).".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Webhook UUID"),
                    req_field("RepoId", FieldKind::String, "Repository UUID"),
                    req_field("Url", FieldKind::String, "Target URL"),
                    req_field("Active", FieldKind::Boolean, "Whether active"),
                ],
                rules: rw_rules("forge:read", "forge:admin"),
            },
            EntityDef {
                set: "Deliveries".into(),
                type_name: "Delivery".into(),
                description: "Webhook delivery records (without response body).".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Delivery UUID"),
                    req_field("WebhookId", FieldKind::String, "Webhook UUID"),
                    req_field("Event", FieldKind::String, "Event kind"),
                    req_field("Status", FieldKind::String, "Delivery status"),
                    opt_field(
                        "ResponseCode",
                        FieldKind::Integer,
                        "HTTP response status code",
                    ),
                    opt_field("At", FieldKind::DateTime, "Delivery timestamp"),
                ],
                rules: read_only_rules("forge:read"),
            },
            EntityDef {
                set: "PushMirrors".into(),
                type_name: "PushMirror".into(),
                description: "Push mirrors (credentials are never declared).".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Composite key {RepoId}:{Name}"),
                    req_field("RepoId", FieldKind::String, "Repository UUID"),
                    req_field("Name", FieldKind::String, "Mirror remote name"),
                    req_field("RemoteAddress", FieldKind::String, "Target HTTPS URL"),
                    req_field(
                        "Branch",
                        FieldKind::String,
                        "The one branch pushed, fast-forward only; empty: every branch",
                    ),
                    opt_field("LastSync", FieldKind::DateTime, "Last sync timestamp"),
                    req_field("LastError", FieldKind::String, "Last sync error"),
                ],
                rules: rw_rules("forge:read", "forge:admin"),
            },
            EntityDef {
                set: "Teams".into(),
                type_name: "Team".into(),
                description: "Read-only projection of jc-auth bindings on repositories.".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Composite key {RepoId}:{Group}"),
                    req_field("RepoId", FieldKind::String, "Repository UUID"),
                    req_field("Group", FieldKind::String, "Principal or group"),
                    req_field("Verb", FieldKind::String, "Granted verb"),
                ],
                rules: read_only_rules("forge:read"),
            },
            EntityDef {
                set: "Packages".into(),
                type_name: "Package".into(),
                description: "Container and generic packages.".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Package UUID"),
                    req_field("Owner", FieldKind::String, "Owner slug"),
                    req_field("Type", FieldKind::String, "container or generic"),
                    req_field("Name", FieldKind::String, "Package name"),
                    opt_field("CreatedAt", FieldKind::DateTime, "Creation timestamp"),
                ],
                rules: rw_rules("forge:read", "forge:publish"),
            },
            EntityDef {
                set: "PackageVersions".into(),
                type_name: "PackageVersion".into(),
                description: "Published package versions and container tags/manifests.".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Version UUID"),
                    req_field("PackageId", FieldKind::String, "Package UUID"),
                    req_field("Owner", FieldKind::String, "Owner slug"),
                    req_field("Type", FieldKind::String, "container or generic"),
                    req_field("Name", FieldKind::String, "Package name"),
                    req_field("Version", FieldKind::String, "Version or tag"),
                    req_field("Digest", FieldKind::String, "SHA-256 digest"),
                    req_field("Size", FieldKind::Integer, "Size in bytes"),
                    req_field("Signed", FieldKind::Boolean, "Whether cosign-signed"),
                    opt_field("CreatedAt", FieldKind::DateTime, "Creation timestamp"),
                ],
                rules: read_only_rules("forge:read"),
            },
            EntityDef {
                set: "PackageFiles".into(),
                type_name: "PackageFile".into(),
                description: "Files belonging to generic package versions.".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "File UUID"),
                    req_field("VersionId", FieldKind::String, "Version UUID"),
                    req_field("Owner", FieldKind::String, "Owner slug"),
                    req_field("Name", FieldKind::String, "Package name"),
                    req_field("Version", FieldKind::String, "Version"),
                    req_field("File", FieldKind::String, "Filename"),
                    req_field("Digest", FieldKind::String, "SHA-256 hex digest"),
                    req_field("Size", FieldKind::Integer, "Size in bytes"),
                    opt_field("CreatedAt", FieldKind::DateTime, "Creation timestamp"),
                ],
                rules: read_only_rules("forge:read"),
            },
            EntityDef {
                set: "Events".into(),
                type_name: "Event".into(),
                description: "Forge activity and outbox events.".into(),
                key: "Id".into(),
                fields: vec![
                    req_field("Id", FieldKind::String, "Event UUID"),
                    opt_field("RepoId", FieldKind::String, "Repository UUID"),
                    req_field("EventKind", FieldKind::String, "Event type"),
                    opt_field("CreatedAt", FieldKind::DateTime, "Timestamp"),
                ],
                rules: read_only_rules("forge:read"),
            },
        ],
    }
}

/// Computes the `odata-table::Scope` for `ctx` (applied BEFORE `$filter`, S9).
#[must_use]
pub fn scope_for_context(ctx: &RequestCtx, authorizer: &dyn Authorizer) -> Scope {
    let inst_target = ObjectTarget::Instance {
        instance_id: ctx.instance_id,
        org_id: ctx.org_id,
    };
    let mut held = Vec::new();
    for (act, perm) in [
        (Action::Read, "forge:read"),
        (Action::Propose, "forge:propose"),
        (Action::Write, "forge:write"),
        (Action::Approve, "forge:approve"),
        (Action::Admin, "forge:admin"),
        (Action::Publish, "forge:publish"),
    ] {
        if authorizer.check(ctx, act, &inst_target).allowed {
            held.push(perm);
        }
    }
    Scope::default().holding(&held)
}

/// Single operation registry for `shako` (S8, S9, S19).
#[derive(Debug, Clone)]
pub struct OperationRegistry {
    pub service: Service,
    ops: Vec<OperationDef>,
}

impl Default for OperationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl OperationRegistry {
    #[must_use]
    pub fn new() -> Self {
        use ODataKind::{Action as A, EntitySetRead, Function as F};
        let service = shako_odata_service();
        let mut ops = Vec::new();

        // 1. Entity set list operations
        for (set, mcp_name, cli_name, segment) in [
            ("Instances", "jc_forge_quota", "quota", "forge"),
            (
                "Repositories",
                "jc_forge_repositories_list",
                "repos",
                "forge",
            ),
            ("Branches", "jc_forge_branches_list", "branches", "forge"),
            ("Tags", "jc_forge_tags_list", "tags", "forge"),
            ("Commits", "jc_forge_commits_list", "commits", "forge"),
            ("PullRequests", "jc_forge_pulls_list", "pulls", "forge"),
            ("Reviews", "jc_forge_reviews_list", "reviews", "forge"),
            (
                "ProtectionRules",
                "jc_forge_protections_list",
                "protections",
                "forge",
            ),
            ("Webhooks", "jc_forge_webhooks_list", "webhooks", "forge"),
            (
                "Deliveries",
                "jc_forge_deliveries_list",
                "deliveries",
                "forge",
            ),
            ("PushMirrors", "jc_forge_mirrors_list", "mirrors", "forge"),
            ("Teams", "jc_forge_teams_list", "teams", "forge"),
            ("Packages", "jc_forge_packages_list", "packages", "apps"),
            (
                "PackageVersions",
                "jc_forge_package_versions_list",
                "package-versions",
                "apps",
            ),
            (
                "PackageFiles",
                "jc_forge_package_files_list",
                "package-files",
                "apps",
            ),
            ("Events", "jc_forge_events_list", "events", "forge"),
        ] {
            ops.push(OperationDef {
                name: set,
                mcp_tool: mcp_name,
                cli_command: cli_name,
                connector_segment: segment,
                title: Box::leak(format!("List {set}").into_boxed_str()),
                description: Box::leak(format!("Query the {set} OData entity set.").into_boxed_str()),
                entity_set: Some(set),
                action: Action::Read,
                permission: "forge:read",
                audit_kind: "READ",
                rest_method: "GET",
                rest_path: Box::leak(format!("/odata/{set}").into_boxed_str()),
                compat_path: None,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "repo": { "type": "string", "description": "Repository name or owner/repo" },
                        "filter": { "type": "string", "description": "OData $filter expression" }
                    }
                }),
                output_schema: json!({ "type": "object" }),
                odata: ODataOperation {
                    kind: EntitySetRead,
                    name: set,
                    binding: None,
                    binding_collection: false,
                    params: Vec::new(),
                    return_type: None,
                },
            });
        }

        // 2. All Actions & Functions of R§9
        let decls: Vec<OperationDef> = vec![
            OperationDef {
                name: "CreateRepository",
                mcp_tool: "jc_forge_create_repository",
                cli_command: "create-repo",
                connector_segment: "forge",
                title: "Create Repository",
                description: "Create a new Git repository in the organization.",
                entity_set: Some("Repositories"),
                action: Action::Write,
                permission: "forge:write",
                audit_kind: "REPO_CREATE",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories",
                compat_path: Some("/api/v1/orgs/{owner}/repos"),
                input_schema: json!({"type":"object","properties":{"owner":{"type":"string"},"name":{"type":"string"},"description":{"type":"string"},"private":{"type":"boolean"},"autoInit":{"type":"boolean"},"defaultBranch":{"type":"string"}},"required":["name"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "CreateRepository",
                    binding: None,
                    binding_collection: false,
                    params: vec![
                        p("Owner", "Edm.String", true),
                        p("Name", "Edm.String", false),
                        p("Description", "Edm.String", true),
                        p("Private", "Edm.Boolean", true),
                        p("AutoInit", "Edm.Boolean", true),
                        p("DefaultBranch", "Edm.String", true),
                    ],
                    return_type: Some("Shako.Repository"),
                },
            },
            OperationDef {
                name: "ImportRepository",
                mcp_tool: "jc_forge_import_repository",
                cli_command: "import-repo",
                connector_segment: "forge",
                title: "Import / Migrate Repository",
                description: "Create a repository by copying an existing repository on this forge.",
                entity_set: Some("Repositories"),
                action: Action::Write,
                permission: "forge:write",
                audit_kind: "REPO_MIGRATE",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/import",
                compat_path: Some("/api/v1/repos/migrate"),
                input_schema: json!({"type":"object","properties":{"sourceOwner":{"type":"string"},"sourceName":{"type":"string"},"repoOwner":{"type":"string"},"repoName":{"type":"string"},"description":{"type":"string"},"private":{"type":"boolean"}},"required":["sourceOwner","sourceName","repoOwner","repoName"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "ImportRepository",
                    binding: None,
                    binding_collection: false,
                    params: vec![
                        p("SourceOwner", "Edm.String", false),
                        p("SourceName", "Edm.String", false),
                        p("RepoOwner", "Edm.String", false),
                        p("RepoName", "Edm.String", false),
                        p("Description", "Edm.String", true),
                        p("Private", "Edm.Boolean", true),
                    ],
                    return_type: Some("Shako.Repository"),
                },
            },
            OperationDef {
                name: "Archive",
                mcp_tool: "jc_forge_archive_repository",
                cli_command: "archive-repo",
                connector_segment: "forge",
                title: "Archive Repository",
                description: "Mark a repository as archived (read-only).",
                entity_set: Some("Repositories"),
                action: Action::Admin,
                permission: "forge:admin",
                audit_kind: "REPO_ARCHIVE",
                rest_method: "PATCH",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}",
                compat_path: Some("/api/v1/repos/{owner}/{repo}"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"}},"required":["repo"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "Archive",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: Vec::new(),
                    return_type: Some("Shako.Repository"),
                },
            },
            OperationDef {
                name: "Delete",
                mcp_tool: "jc_forge_delete_repository",
                cli_command: "delete-repo",
                connector_segment: "forge",
                title: "Delete Repository",
                description: "Delete a repository and its objects.",
                entity_set: Some("Repositories"),
                action: Action::Admin,
                permission: "forge:admin",
                audit_kind: "REPO_DELETE",
                rest_method: "DELETE",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}",
                compat_path: Some("/api/v1/repos/{owner}/{repo}"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"}},"required":["repo"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "Delete",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: Vec::new(),
                    return_type: None,
                },
            },
            OperationDef {
                name: "CreateBranch",
                mcp_tool: "jc_forge_create_branch",
                cli_command: "create-branch",
                connector_segment: "forge",
                title: "Create Branch",
                description: "Create a branch from an existing branch or commit.",
                entity_set: Some("Branches"),
                action: Action::Propose,
                permission: "forge:propose",
                audit_kind: "BRANCH_CREATE",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/branches",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/branches"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"newBranchName":{"type":"string"},"oldBranchName":{"type":"string"}},"required":["repo","newBranchName"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "CreateBranch",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![
                        p("NewBranchName", "Edm.String", false),
                        p("OldBranchName", "Edm.String", true),
                    ],
                    return_type: Some("Shako.Branch"),
                },
            },
            OperationDef {
                name: "DeleteBranch",
                mcp_tool: "jc_forge_delete_branch",
                cli_command: "delete-branch",
                connector_segment: "forge",
                title: "Delete Branch",
                description: "Delete an unprotected branch.",
                entity_set: Some("Branches"),
                action: Action::Propose,
                permission: "forge:propose",
                audit_kind: "BRANCH_DELETE",
                rest_method: "DELETE",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/branches/{branch}",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/branches/{branch}"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"branch":{"type":"string"}},"required":["repo","branch"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "DeleteBranch",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![p("Branch", "Edm.String", false)],
                    return_type: None,
                },
            },
            OperationDef {
                name: "CommitFiles",
                mcp_tool: "jc_forge_commit_files",
                cli_command: "commit-files",
                connector_segment: "forge",
                title: "Commit Files",
                description: "Create, update, upload, or delete multiple files in one commit with blob SHA preconditions.",
                entity_set: Some("Commits"),
                action: Action::Propose,
                permission: "forge:propose",
                audit_kind: "COMMIT_FILES",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/contents",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/contents"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"branch":{"type":"string"},"message":{"type":"string"},"files":{"type":"array"}},"required":["repo","branch","message","files"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "CommitFiles",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![
                        p("Branch", "Edm.String", true),
                        p("Message", "Edm.String", false),
                        p("Author", "Edm.Untyped", true),
                        p("Committer", "Edm.Untyped", true),
                        p("Files", "Edm.Untyped", false),
                    ],
                    return_type: Some("Edm.Untyped"),
                },
            },
            OperationDef {
                name: "OpenPullRequest",
                mcp_tool: "jc_forge_open_pull_request",
                cli_command: "open-pr",
                connector_segment: "forge",
                title: "Open Pull Request",
                description: "Open a pull request between two branches.",
                entity_set: Some("PullRequests"),
                action: Action::Propose,
                permission: "forge:propose",
                audit_kind: "PR_OPEN",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/pulls",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/pulls"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"head":{"type":"string"},"base":{"type":"string"},"title":{"type":"string"},"body":{"type":"string"}},"required":["repo","head","base","title"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "OpenPullRequest",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![
                        p("Head", "Edm.String", false),
                        p("Base", "Edm.String", false),
                        p("Title", "Edm.String", false),
                        p("Body", "Edm.String", true),
                    ],
                    return_type: Some("Shako.PullRequest"),
                },
            },
            OperationDef {
                name: "Review",
                mcp_tool: "jc_forge_review_pull_request",
                cli_command: "review-pr",
                connector_segment: "forge",
                title: "Review Pull Request",
                description: "Submit a review (APPROVED, REQUEST_CHANGES, or COMMENT) on a pull request.",
                entity_set: Some("Reviews"),
                action: Action::Approve,
                permission: "forge:approve",
                audit_kind: "PR_REVIEW",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/pulls/{number}/reviews",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/pulls/{number}/reviews"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"number":{"type":"integer"},"event":{"type":"string"},"body":{"type":"string"}},"required":["repo","number","event"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "Review",
                    binding: Some("PullRequest"),
                    binding_collection: false,
                    params: vec![
                        p("Event", "Edm.String", false),
                        p("Body", "Edm.String", true),
                        p("CommitId", "Edm.String", true),
                    ],
                    return_type: Some("Shako.Review"),
                },
            },
            OperationDef {
                name: "Merge",
                mcp_tool: "jc_forge_merge_pull_request",
                cli_command: "merge-pr",
                connector_segment: "forge",
                title: "Merge Pull Request",
                description: "Merge a pull request (merge, squash, or fast-forward-only) verifying HeadCommitId.",
                entity_set: Some("PullRequests"),
                action: Action::Write,
                permission: "forge:write",
                audit_kind: "PR_MERGE",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/pulls/{number}/merge",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/pulls/{number}/merge"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"number":{"type":"integer"},"style":{"type":"string"},"message":{"type":"string"},"headCommitId":{"type":"string"},"author":{"type":"object","properties":{"name":{"type":"string"},"email":{"type":"string"}}}},"required":["repo","number","style"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "Merge",
                    binding: Some("PullRequest"),
                    binding_collection: false,
                    params: vec![
                        p("Style", "Edm.String", false),
                        p("Message", "Edm.String", true),
                        p("HeadCommitId", "Edm.String", true),
                        p("DeleteBranchAfterMerge", "Edm.Boolean", true),
                        p("Author", "Edm.Untyped", true),
                    ],
                    return_type: Some("Edm.Untyped"),
                },
            },
            OperationDef {
                name: "UpdateBranch",
                mcp_tool: "jc_forge_update_pull_request_branch",
                cli_command: "update-pr-branch",
                connector_segment: "forge",
                title: "Update Pull Request Branch",
                description: "Merge the base branch into the pull request's head branch.",
                entity_set: Some("PullRequests"),
                action: Action::Propose,
                permission: "forge:propose",
                audit_kind: "PR_UPDATE_BRANCH",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/pulls/{number}/update",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/pulls/{number}/update"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"number":{"type":"integer"},"style":{"type":"string"}},"required":["repo","number"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "UpdateBranch",
                    binding: Some("PullRequest"),
                    binding_collection: false,
                    params: vec![p("Style", "Edm.String", true)],
                    return_type: Some("Edm.Untyped"),
                },
            },
            OperationDef {
                name: "Close",
                mcp_tool: "jc_forge_close_pull_request",
                cli_command: "close-pr",
                connector_segment: "forge",
                title: "Close Pull Request",
                description: "Close a pull request without merging.",
                entity_set: Some("PullRequests"),
                action: Action::Propose,
                permission: "forge:propose",
                audit_kind: "PR_CLOSE",
                rest_method: "PATCH",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/pulls/{number}",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/pulls/{number}"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"number":{"type":"integer"}},"required":["repo","number"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "Close",
                    binding: Some("PullRequest"),
                    binding_collection: false,
                    params: Vec::new(),
                    return_type: Some("Shako.PullRequest"),
                },
            },
            OperationDef {
                name: "Protect",
                mcp_tool: "jc_forge_protect_branch",
                cli_command: "protect",
                connector_segment: "forge",
                title: "Protect Branch",
                description: "Create or update a branch protection rule.",
                entity_set: Some("ProtectionRules"),
                action: Action::Admin,
                permission: "forge:admin",
                audit_kind: "BRANCH_PROTECT",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/protections",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/branch_protections"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"rule":{"type":"object"}},"required":["repo","rule"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "Protect",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![p("Rule", "Edm.Untyped", false)],
                    return_type: Some("Shako.ProtectionRule"),
                },
            },
            OperationDef {
                name: "Unprotect",
                mcp_tool: "jc_forge_unprotect_branch",
                cli_command: "unprotect",
                connector_segment: "forge",
                title: "Remove Branch Protection",
                description: "Delete a branch protection rule by pattern.",
                entity_set: Some("ProtectionRules"),
                action: Action::Admin,
                permission: "forge:admin",
                audit_kind: "BRANCH_UNPROTECT",
                rest_method: "DELETE",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/protections/{pattern}",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/branch_protections/{pattern}"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"pattern":{"type":"string"}},"required":["repo","pattern"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "Unprotect",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![p("Pattern", "Edm.String", false)],
                    return_type: None,
                },
            },
            OperationDef {
                name: "AddPushMirror",
                mcp_tool: "jc_forge_add_push_mirror",
                cli_command: "add-mirror",
                connector_segment: "forge",
                title: "Add Push Mirror",
                description: "Add an HTTPS push mirror to an allow-listed host.",
                entity_set: Some("PushMirrors"),
                action: Action::Admin,
                permission: "forge:admin",
                audit_kind: "MIRROR_ADD",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/mirrors",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/push_mirrors"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"remoteAddress":{"type":"string"},"remoteUsername":{"type":"string"},"secretRef":{"type":"string"},"interval":{"type":"string"},"syncOnCommit":{"type":"boolean"}},"required":["repo","remoteAddress"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "AddPushMirror",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![
                        p("RemoteAddress", "Edm.String", false),
                        p("RemoteUsername", "Edm.String", true),
                        p("Credential", "Edm.String", true),
                        p("SecretRef", "Edm.String", true),
                        p("Interval", "Edm.String", true),
                        p("SyncOnCommit", "Edm.Boolean", true),
                        p("Branch", "Edm.String", true),
                    ],
                    return_type: Some("Shako.PushMirror"),
                },
            },
            OperationDef {
                name: "RemovePushMirror",
                mcp_tool: "jc_forge_remove_push_mirror",
                cli_command: "remove-mirror",
                connector_segment: "forge",
                title: "Remove Push Mirror",
                description: "Remove a push mirror from a repository.",
                entity_set: Some("PushMirrors"),
                action: Action::Admin,
                permission: "forge:admin",
                audit_kind: "MIRROR_REMOVE",
                rest_method: "DELETE",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/mirrors/{name}",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/push_mirrors/{name}"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"name":{"type":"string"}},"required":["repo","name"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "RemovePushMirror",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![p("Name", "Edm.String", false)],
                    return_type: None,
                },
            },
            OperationDef {
                name: "SyncPushMirrors",
                mcp_tool: "jc_forge_sync_push_mirrors",
                cli_command: "sync-mirrors",
                connector_segment: "forge",
                title: "Sync Push Mirrors",
                description: "Immediately push all configured mirrors of the repository.",
                entity_set: Some("PushMirrors"),
                action: Action::Admin,
                permission: "forge:admin",
                audit_kind: "MIRROR_SYNC",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/mirrors/sync",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/push_mirrors-sync"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"}},"required":["repo"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "SyncPushMirrors",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: Vec::new(),
                    return_type: Some("Edm.Untyped"),
                },
            },
            OperationDef {
                name: "AddWebhook",
                mcp_tool: "jc_forge_add_webhook",
                cli_command: "add-webhook",
                connector_segment: "forge",
                title: "Add Webhook",
                description: "Register an outbound webhook delivering shako's native events, signed with an inline secret or a secretRef.",
                entity_set: Some("Webhooks"),
                action: Action::Admin,
                permission: "forge:admin",
                audit_kind: "WEBHOOK_ADD",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/webhooks",
                compat_path: None,
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"url":{"type":"string"},"secretRef":{"type":"string"}},"required":["repo","url","secretRef"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "AddWebhook",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![
                        p("Url", "Edm.String", false),
                        p("Secret", "Edm.String", true),
                        p("SecretRef", "Edm.String", true),
                        p("PreviousSecretRef", "Edm.String", true),
                        p("Events", "Collection(Edm.String)", true),
                        p("Active", "Edm.Boolean", true),
                    ],
                    return_type: Some("Shako.Webhook"),
                },
            },
            OperationDef {
                name: "RedeliverWebhook",
                mcp_tool: "jc_forge_redeliver_webhook",
                cli_command: "redeliver-webhook",
                connector_segment: "forge",
                title: "Redeliver Webhook",
                description: "Trigger a test/redelivery of a webhook.",
                entity_set: Some("Webhooks"),
                action: Action::Admin,
                permission: "forge:admin",
                audit_kind: "WEBHOOK_REDELIVER",
                rest_method: "POST",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/webhooks/{id}/redeliver",
                compat_path: None,
                input_schema: json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "RedeliverWebhook",
                    binding: Some("Webhook"),
                    binding_collection: false,
                    params: Vec::new(),
                    return_type: Some("Shako.Delivery"),
                },
            },
            OperationDef {
                name: "PublishPackage",
                mcp_tool: "jc_forge_publish_package",
                cli_command: "publish-package",
                connector_segment: "apps",
                title: "Publish Package",
                description: "Publish a generic package file.",
                entity_set: Some("Packages"),
                action: Action::Publish,
                permission: "forge:publish",
                audit_kind: "PACKAGE_PUBLISH",
                rest_method: "PUT",
                rest_path: "/api/packages/{owner}/generic/{pkg}/{ver}/{file}",
                compat_path: Some("/api/packages/{owner}/generic/{pkg}/{ver}/{file}"),
                input_schema: json!({"type":"object","properties":{"owner":{"type":"string"},"package":{"type":"string"},"version":{"type":"string"},"file":{"type":"string"},"contentBase64":{"type":"string"}},"required":["package","version","file","contentBase64"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: A,
                    name: "PublishPackage",
                    binding: None,
                    binding_collection: false,
                    params: vec![
                        p("Owner", "Edm.String", true),
                        p("Package", "Edm.String", false),
                        p("Version", "Edm.String", false),
                        p("File", "Edm.String", false),
                        p("ContentBase64", "Edm.String", false),
                    ],
                    return_type: Some("Shako.PackageFile"),
                },
            },
            OperationDef {
                name: "Tree",
                mcp_tool: "jc_forge_tree",
                cli_command: "tree",
                connector_segment: "forge",
                title: "Read Repository Tree",
                description: "Read the tree of a repository at a ref.",
                entity_set: None,
                action: Action::Read,
                permission: "forge:read",
                audit_kind: "TREE_READ",
                rest_method: "GET",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/trees/{ref}",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/git/trees/{ref}"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"ref":{"type":"string"},"recursive":{"type":"boolean"}},"required":["repo"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: F,
                    name: "Tree",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![
                        p("Ref", "Edm.String", false),
                        p("Path", "Edm.String", true),
                        p("Recursive", "Edm.Boolean", true),
                        p("Skip", "Edm.Int32", true),
                        p("Top", "Edm.Int32", true),
                    ],
                    return_type: Some("Edm.Untyped"),
                },
            },
            OperationDef {
                name: "File",
                mcp_tool: "jc_forge_file",
                cli_command: "file",
                connector_segment: "forge",
                title: "Read Repository File",
                description: "Read a file's content and blob SHA at a ref.",
                entity_set: None,
                action: Action::Read,
                permission: "forge:read",
                audit_kind: "FILE_READ",
                rest_method: "GET",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/contents/{path}",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/contents/{path}"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"ref":{"type":"string"},"path":{"type":"string"}},"required":["repo","path"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: F,
                    name: "File",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![
                        p("Ref", "Edm.String", false),
                        p("Path", "Edm.String", false),
                    ],
                    return_type: Some("Edm.Untyped"),
                },
            },
            OperationDef {
                name: "History",
                mcp_tool: "jc_forge_history",
                cli_command: "history",
                connector_segment: "forge",
                title: "Read Commit History",
                description: "Read commit history for a ref and optional path.",
                entity_set: None,
                action: Action::Read,
                permission: "forge:read",
                audit_kind: "HISTORY_READ",
                rest_method: "GET",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/commits",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/commits"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"ref":{"type":"string"},"path":{"type":"string"}},"required":["repo"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: F,
                    name: "History",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![
                        p("Ref", "Edm.String", false),
                        p("Path", "Edm.String", true),
                        p("Top", "Edm.Int32", true),
                    ],
                    return_type: Some("Edm.Untyped"),
                },
            },
            OperationDef {
                name: "Compare",
                mcp_tool: "jc_forge_compare",
                cli_command: "compare",
                connector_segment: "forge",
                title: "Compare Refs",
                description: "Compare two refs and return changed files and merge base.",
                entity_set: None,
                action: Action::Read,
                permission: "forge:read",
                audit_kind: "COMPARE_READ",
                rest_method: "GET",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/compare",
                compat_path: None,
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"base":{"type":"string"},"head":{"type":"string"}},"required":["repo","base","head"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: F,
                    name: "Compare",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![
                        p("Base", "Edm.String", false),
                        p("Head", "Edm.String", false),
                    ],
                    return_type: Some("Edm.Untyped"),
                },
            },
            OperationDef {
                name: "Mergeable",
                mcp_tool: "jc_forge_mergeable",
                cli_command: "mergeable",
                connector_segment: "forge",
                title: "Check Pull Request Mergeability",
                description: "Check whether a pull request merges cleanly in memory.",
                entity_set: Some("PullRequests"),
                action: Action::Read,
                permission: "forge:read",
                audit_kind: "PR_MERGEABLE",
                rest_method: "GET",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/pulls/{number}/mergeable",
                compat_path: None,
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"number":{"type":"integer"}},"required":["repo","number"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: F,
                    name: "Mergeable",
                    binding: Some("PullRequest"),
                    binding_collection: false,
                    params: Vec::new(),
                    return_type: Some("Edm.Untyped"),
                },
            },
            OperationDef {
                name: "CommitStatus",
                mcp_tool: "jc_forge_commit_status",
                cli_command: "commit-status",
                connector_segment: "forge",
                title: "Read Commit Status",
                description: "Read the combined CI state and the statuses of a commit.",
                entity_set: None,
                action: Action::Read,
                permission: "forge:read",
                audit_kind: "STATUS_READ",
                rest_method: "GET",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/commits/{sha}/status",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/commits/{sha}/status"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"sha":{"type":"string"}},"required":["repo","sha"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: F,
                    name: "CommitStatus",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![p("Sha", "Edm.String", false)],
                    return_type: Some("Edm.Untyped"),
                },
            },
            OperationDef {
                name: "ArchiveLink",
                mcp_tool: "jc_forge_archive_link",
                cli_command: "archive-link",
                connector_segment: "forge",
                title: "Repository Archive Link",
                description: "Return the download link for a zip or tar.gz archive of a ref.",
                entity_set: None,
                action: Action::Read,
                permission: "forge:read",
                audit_kind: "ARCHIVE_LINK",
                rest_method: "GET",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/archive/{ref}",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/archive/{ref}"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"ref":{"type":"string"},"format":{"type":"string"}},"required":["repo","ref"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: F,
                    name: "ArchiveLink",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![
                        p("Ref", "Edm.String", false),
                        p("Format", "Edm.String", false),
                    ],
                    return_type: Some("Edm.String"),
                },
            },
            OperationDef {
                name: "BundleLink",
                mcp_tool: "jc_forge_bundle_link",
                cli_command: "bundle-link",
                connector_segment: "forge",
                title: "Repository Bundle Link",
                description: "Return the download link for a git bundle v2 of a ref.",
                entity_set: None,
                action: Action::Read,
                permission: "forge:read",
                audit_kind: "BUNDLE_LINK",
                rest_method: "GET",
                rest_path: "/api/v1/projects/{project}/repositories/{repo}/bundle/{ref}",
                compat_path: Some("/api/v1/repos/{owner}/{repo}/archive/{ref}.bundle"),
                input_schema: json!({"type":"object","properties":{"repo":{"type":"string"},"ref":{"type":"string"}},"required":["repo","ref"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: F,
                    name: "BundleLink",
                    binding: Some("Repository"),
                    binding_collection: false,
                    params: vec![p("Ref", "Edm.String", false)],
                    return_type: Some("Edm.String"),
                },
            },
            OperationDef {
                name: "Allowed",
                mcp_tool: "jc_forge_allowed",
                cli_command: "allowed",
                connector_segment: "forge",
                title: "Check Operation Permission",
                description: "Check whether the caller is allowed to execute an operation on a target without revealing hidden objects.",
                entity_set: None,
                action: Action::Read,
                permission: "forge:read",
                audit_kind: "ALLOWED_CHECK",
                rest_method: "GET",
                rest_path: "/api/v1/projects/{project}/allowed",
                compat_path: None,
                input_schema: json!({"type":"object","properties":{"operation":{"type":"string"},"object":{"type":"string"}},"required":["operation"]}),
                output_schema: json!({"type":"object"}),
                odata: ODataOperation {
                    kind: F,
                    name: "Allowed",
                    binding: None,
                    binding_collection: false,
                    params: vec![
                        p("Operation", "Edm.String", false),
                        p("Object", "Edm.String", true),
                    ],
                    return_type: Some("Edm.Untyped"),
                },
            },
            OperationDef {
                name: "Me",
                mcp_tool: "jc_forge_me",
                cli_command: "me",
                connector_segment: "forge",
                title: "Current Account",
                description: "The account the caller's credential signs in as: the login a branch rule's allow-lists name (T-6706).",
                entity_set: None,
                action: Action::Read,
                permission: "forge:read",
                audit_kind: "ME_READ",
                rest_method: "GET",
                rest_path: "/api/v1/user",
                compat_path: Some("/api/v1/user"),
                input_schema: json!({"type":"object","properties":{}}),
                output_schema: json!({"type":"object","properties":{"Login":{"type":"string"}}}),
                odata: ODataOperation {
                    kind: F,
                    name: "Me",
                    binding: None,
                    binding_collection: false,
                    params: vec![],
                    return_type: Some("Edm.Untyped"),
                },
            },
        ];

        ops.extend(decls);
        Self { service, ops }
    }

    #[must_use]
    pub fn operations(&self) -> &[OperationDef] {
        &self.ops
    }

    #[must_use]
    pub fn find(&self, name: &str) -> Option<&OperationDef> {
        let short = name.strip_prefix("Shako.").unwrap_or(name);
        self.ops.iter().find(|op| {
            op.name.eq_ignore_ascii_case(short)
                || op.mcp_tool.eq_ignore_ascii_case(short)
                || op.cli_command.eq_ignore_ascii_case(short)
                || op.odata.name.eq_ignore_ascii_case(short)
        })
    }

    /// Evaluates `Allowed(operation, object)` using the exact same `Authorizer` decision as the call,
    /// and never reveals a hidden or cross-organization record (S8, S9).
    #[must_use]
    pub fn check_allowed(
        &self,
        authorizer: &dyn Authorizer,
        ctx: &RequestCtx,
        operation: &str,
        target: &ObjectTarget,
        target_exists_in_org: bool,
    ) -> Decision {
        let action = self
            .find(operation)
            .map(|op| op.action)
            .or_else(|| Action::parse(operation))
            .unwrap_or(Action::Admin);

        if !target_exists_in_org {
            return Decision::deny_hidden("target not found");
        }
        let dec = authorizer.check(ctx, action, target);
        if !dec.visible {
            Decision::deny_hidden("target not found")
        } else {
            dec
        }
    }

    /// Renders the complete OData 4.01 CSDL `$metadata` XML document (S9).
    pub fn csdl_xml(&self) -> Result<String, String> {
        let csdl = self.service.model();
        let mut xml = Vec::new();
        odata_csdl::write_xml(&csdl, &odata_csdl::Limits::strict(), &mut xml)
            .map_err(|e| format!("CSDL write failed: {e}"))?;
        let mut text = String::from_utf8(xml).map_err(|e| e.to_string())?;

        let ns = &self.service.namespace;
        let container_at = text
            .find("<EntityContainer")
            .ok_or_else(|| "CSDL missing EntityContainer".to_owned())?;
        let line_start = text[..container_at].rfind('\n').map_or(0, |i| i + 1);
        let indent = text[line_start..container_at].to_owned();
        let inner = format!("{indent}  ");

        let mut ops_xml = String::new();
        let mut imports_xml = String::new();
        for op in &self.ops {
            let o = &op.odata;
            let tag = match o.kind {
                ODataKind::Action => "Action",
                ODataKind::Function => "Function",
                ODataKind::EntitySetRead | ODataKind::EntitySetWrite => continue,
            };
            let is_bound = o.binding.is_some();
            ops_xml.push_str(&format!(
                "{indent}<{tag} Name=\"{}\" IsBound=\"{is_bound}\">\n",
                o.name
            ));
            if let Some(b) = o.binding {
                let ty = if o.binding_collection {
                    format!("Collection({ns}.{b})")
                } else {
                    format!("{ns}.{b}")
                };
                ops_xml.push_str(&format!(
                    "{inner}<Parameter Name=\"bindingParameter\" Type=\"{ty}\" Nullable=\"false\"/>\n"
                ));
            }
            for prm in &o.params {
                let nullable = if prm.nullable {
                    ""
                } else {
                    " Nullable=\"false\""
                };
                ops_xml.push_str(&format!(
                    "{inner}<Parameter Name=\"{}\" Type=\"{}\"{nullable}/>\n",
                    prm.name, prm.edm_type
                ));
            }
            if let Some(rt) = o.return_type {
                ops_xml.push_str(&format!("{inner}<ReturnType Type=\"{rt}\"/>\n"));
            }
            ops_xml.push_str(&format!("{indent}</{tag}>\n"));
            if !is_bound {
                let (imp_tag, attr) = if tag == "Action" {
                    ("ActionImport", "Action")
                } else {
                    ("FunctionImport", "Function")
                };
                imports_xml.push_str(&format!(
                    "{inner}<{imp_tag} Name=\"{}\" {attr}=\"{ns}.{}\"/>\n",
                    o.name, o.name
                ));
            }
        }
        text.insert_str(line_start, &ops_xml);
        let close = text
            .find("</EntityContainer>")
            .ok_or_else(|| "CSDL missing </EntityContainer>".to_owned())?;
        let close_line = text[..close].rfind('\n').map_or(0, |i| i + 1);
        text.insert_str(close_line, &imports_xml);
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_completeness_and_csdl_snapshot() {
        let reg = OperationRegistry::new();
        assert!(reg.operations().len() >= 36);
        for op in reg.operations() {
            assert!(!op.name.is_empty());
            assert!(!op.mcp_tool.is_empty());
            assert!(!op.cli_command.is_empty());
            assert!(!op.permission.is_empty());
        }
        let xml = reg.csdl_xml().expect("valid csdl");
        for required in [
            "Namespace=\"Shako\"",
            "EntitySet Name=\"Instances\"",
            "EntitySet Name=\"Repositories\"",
            "EntitySet Name=\"Branches\"",
            "EntitySet Name=\"Tags\"",
            "EntitySet Name=\"Commits\"",
            "EntitySet Name=\"PullRequests\"",
            "EntitySet Name=\"Reviews\"",
            "EntitySet Name=\"ProtectionRules\"",
            "EntitySet Name=\"Webhooks\"",
            "EntitySet Name=\"Deliveries\"",
            "EntitySet Name=\"PushMirrors\"",
            "EntitySet Name=\"Teams\"",
            "EntitySet Name=\"Packages\"",
            "Action Name=\"Merge\"",
            "Action Name=\"CommitFiles\"",
            "Function Name=\"Tree\"",
            "Function Name=\"Allowed\"",
            "Function Name=\"CommitStatus\"",
            "Property Name=\"UpdatedAt\"",
        ] {
            assert!(xml.contains(required), "CSDL missing {required}");
        }
        // Secrets are never declared in CSDL: no entity type carries a secret or its reference;
        // a `SecretRef` action parameter only names a secret the reconciler resolves.
        assert!(!xml.contains("Property Name=\"SecretRef\""));
        assert!(!xml.contains("Property Name=\"PreviousSecretRef\""));
        assert!(!xml.contains("RemotePassword"));
        // An inline hook secret or mirror credential is a write-only action parameter, never a
        // property any entity answers.
        assert!(!xml.contains("Property Name=\"Secret\""));
        assert!(!xml.contains("Property Name=\"Credential\""));
    }
}
