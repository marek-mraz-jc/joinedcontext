//! The `Change` resource returned by every mutation in the portal (MF-12, CC-63).
//!
//! Mutations in joinedcontext are never direct live writes: they are proposed changes
//! routed through Git merge requests and classified into approval lanes (Green, Yellow, Red).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::resource::API_VERSION;

/// The `Change` resource describing a proposed configuration update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub api_version: String,
    pub kind: String,
    pub metadata: ChangeMeta,
    pub status: ChangeStatus,
}

impl Change {
    pub const KIND: &'static str = "Change";

    pub fn new(metadata: ChangeMeta, status: ChangeStatus) -> Self {
        Self {
            api_version: API_VERSION.to_string(),
            kind: Self::KIND.to_string(),
            metadata,
            status,
        }
    }
}

/// Metadata identifying the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeMeta {
    /// Identifier formatted as `chg-` plus eight lowercase hex characters derived
    /// from the merge request.
    pub name: String,
    pub namespace: String,
}

impl ChangeMeta {
    pub fn new(name: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: namespace.into(),
        }
    }

    /// Derives the change metadata from a pull request / merge request number.
    pub fn from_merge_request(mr_number: u64, namespace: impl Into<String>) -> Self {
        Self {
            name: format!("chg-{:08x}", mr_number),
            namespace: namespace.into(),
        }
    }

    /// A merge request of the organization repository listed under a project of layout 2:
    /// `chg-org-` and its number, since the project repository numbers its own apart (CC-87).
    pub fn from_organization_merge_request(mr_number: u64, namespace: impl Into<String>) -> Self {
        Self {
            name: format!("chg-org-{:08x}", mr_number),
            namespace: namespace.into(),
        }
    }
}

/// Approval and reconciliation status of a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeStatus {
    pub lane: Lane,
    /// The one repository the Change targets (CC-87): the organization's, or in layout 2 a
    /// project's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_request: Option<String>,
    pub plan: PlanSummary,
    pub phase: ChangePhase,
    /// The channel the Change was proposed through when it was not the person at the Portal
    /// (AG-137, ADR-N-060 §2.2): read back from the merge request's `Proposed-Via` line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_via: Option<ProposedVia>,
}

impl ChangeStatus {
    pub fn new(lane: Lane, phase: ChangePhase, plan: PlanSummary) -> Self {
        Self {
            lane,
            repository: None,
            merge_request: None,
            plan,
            phase,
            proposed_via: None,
        }
    }

    /// The channel the merge request's body names ([`ProposedVia::in_body`]).
    pub fn proposed_through(mut self, merge_request_body: &str) -> Self {
        self.proposed_via = ProposedVia::in_body(merge_request_body);
        self
    }

    /// The repository the Change's merge request is in; an empty name leaves it unsaid.
    pub fn in_repository(mut self, repository: &str) -> Self {
        self.repository = (!repository.is_empty()).then(|| repository.to_owned());
        self
    }

    pub fn with_merge_request(mut self, mr: impl Into<String>) -> Self {
        self.merge_request = Some(mr.into());
        self
    }
}

/// The channel a Change came through and the OAuth client that sent it (AG-17, AG-137).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProposedVia {
    /// `mcp`: a person's AI client over the Portal's MCP server.
    pub channel: String,
    /// The token's `azp`, at most 64 characters of `[A-Za-z0-9._:-]`.
    pub client: String,
    /// The connector the call came through (AG-144), from the stored grant the token was
    /// checked against, never from the call's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector: Option<String>,
    /// That connector's name, a DNS-1123 label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_name: Option<String>,
    /// The plan `jc_change_apply` took (T-5956): the diff the person's AI client showed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

tokio::task_local! {
    /// The channel of the call being served: the MCP door sets it around one `tools/call`, and
    /// the forge client signs every commit and merge request opened inside it.
    pub static PROPOSED_VIA: ProposedVia;
}

const TRAILER: &str = "Proposed-Via: ";

impl ProposedVia {
    /// A `tools/call` of `client` (the token's `azp`). A known CIMD client (its id is the URL of
    /// its metadata document) is named by its short name, another CIMD client by its host
    /// (T-6390); a client id outside the allowed characters is named `unknown` rather than
    /// written into Git.
    pub fn mcp(client: Option<&str>) -> Self {
        let client = client
            .and_then(|c| {
                CIMD_CLIENTS
                    .iter()
                    .find(|(url, _)| *url == c)
                    .map(|(_, name)| (*name).to_owned())
                    .or_else(|| cimd_host(c))
                    .or_else(|| valid_client(c).then(|| c.to_owned()))
            })
            .unwrap_or_else(|| "unknown".to_owned());
        Self {
            channel: "mcp".to_owned(),
            client,
            connector: None,
            connector_name: None,
            plan: None,
        }
    }

    /// The connector a call comes through, as the verified token and the stored grant name it.
    pub fn through(mut self, connector: Option<&crate::connectors::Connector>) -> Self {
        if let Some(connector) = connector.filter(|c| valid_client(&c.id) && valid_client(&c.name))
        {
            self.connector = Some(connector.id.clone());
            self.connector_name = Some(connector.name.clone());
        }
        self
    }

    /// The plan an apply took; a plan id outside the charset is left unsaid.
    pub fn with_plan(mut self, plan: &str) -> Self {
        self.plan = valid_client(plan).then(|| plan.to_owned());
        self
    }

    /// The channel of the call being served, outside any [`PROPOSED_VIA`] scope none.
    pub fn current() -> Option<Self> {
        PROPOSED_VIA.try_with(Clone::clone).ok()
    }

    /// The Git trailer line (ADR-N-060 §2.2): `Proposed-Via: mcp; client=mcp-mobile`, then
    /// `; connector=<id>; connectorName=<name>; plan=<id>` for a change a connector opened (T-5973).
    pub fn trailer(&self) -> String {
        let mut line = format!("{TRAILER}{}; client={}", self.channel, self.client);
        for (key, value) in [
            ("connector", &self.connector),
            ("connectorName", &self.connector_name),
            ("plan", &self.plan),
        ] {
            if let Some(value) = value {
                line.push_str(&format!("; {key}={value}"));
            }
        }
        line
    }

    /// `message` with the trailer of the call being served, unchanged outside one.
    pub fn sign(message: &str) -> std::borrow::Cow<'_, str> {
        match Self::current() {
            Some(via) => format!("{message}\n\n{}", via.trailer()).into(),
            None => message.into(),
        }
    }

    /// A merge request body for the call being served: the channel in its first line, the
    /// trailer in its last, unchanged outside one.
    pub fn sign_body(body: &str) -> std::borrow::Cow<'_, str> {
        match Self::current() {
            Some(via) => {
                let connector = match &via.connector_name {
                    Some(name) => format!(" through connector `{name}`"),
                    None => String::new(),
                };
                format!(
                    "Proposed through an AI client over MCP (client `{}`){connector}.\n\n{body}\n\n{}",
                    via.client,
                    via.trailer()
                )
                .into()
            }
            None => body.into(),
        }
    }

    /// The channel the body's last `Proposed-Via` line names; a line a hand edit broke names
    /// none, so the page never shows a client id it could not check.
    pub fn in_body(body: &str) -> Option<Self> {
        let line = body
            .lines()
            .rev()
            .find_map(|l| l.trim().strip_prefix(TRAILER))?;
        let (channel, rest) = line.split_once("; client=")?;
        let mut parts = rest.split("; ");
        let client = parts.next()?;
        if channel != "mcp" || !valid_client(client) {
            return None;
        }
        let mut via = Self::mcp(Some(client));
        for part in parts {
            let (key, value) = part.split_once('=')?;
            if !valid_client(value) {
                return None;
            }
            let slot = match key {
                "connector" => &mut via.connector,
                "connectorName" => &mut via.connector_name,
                "plan" => &mut via.plan,
                _ => return None,
            };
            *slot = Some(value.to_owned());
        }
        Some(via)
    }
}

/// The CIMD clients a person connects most (their client id is their metadata document's URL),
/// by the short name the approval page shows as "Claude Code" or "claude.ai".
const CIMD_CLIENTS: [(&str, &str); 2] = [
    (
        "https://claude.ai/oauth/claude-code-client-metadata",
        "claude-code",
    ),
    (
        "https://claude.ai/oauth/mcp-oauth-client-metadata",
        "claude-ai",
    ),
];

/// The host of a CIMD client id (`https://host/path`), when it is a valid client name.
fn cimd_host(client: &str) -> Option<String> {
    let host = client.strip_prefix("https://")?.split('/').next()?;
    (!host.is_empty() && !host.contains(':') && valid_client(host)).then(|| host.to_owned())
}

fn valid_client(client: &str) -> bool {
    (1..=64).contains(&client.len())
        && client
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// Change risk classification lane (CC-63).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Lane {
    Green,
    Yellow,
    Red,
}

/// Lifecycle phase of a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub enum ChangePhase {
    PendingApproval,
    Deploying,
    Merged,
    Applied,
    Rejected,
}

/// Counts of planned resource mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanSummary {
    #[serde(default)]
    pub create: usize,
    #[serde(default)]
    pub update: usize,
    #[serde(default)]
    pub delete: usize,
}

impl PlanSummary {
    pub fn new(create: usize, update: usize, delete: usize) -> Self {
        Self {
            create,
            update,
            delete,
        }
    }
}

/// Resource mutation operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub enum Operation {
    Create,
    Update,
    Delete,
}

/// [`classify`] for a manifest where it lives: an organization `DataModel` is the schema of every
/// project that imports it, so any change to it is `Red`, whatever its kind alone would say
/// (DM-75, DM-77).
pub fn classify_manifest(manifest: &crate::resource::ResourceEnvelope, op: Operation) -> Lane {
    if manifest.kind == "DataModel"
        && manifest.metadata.namespace.as_deref() == Some(crate::permissions::ORG_NAMESPACE)
    {
        return Lane::Red;
    }
    classify(&manifest.kind, op, &manifest.spec)
}

/// Classifies a resource operation into an approval lane per
/// `docs/Architecture/06-configuration-as-code.md` §4 and CC-63.
///
/// Precedence:
/// - `Operation::Delete` is ALWAYS `Red`, without exception (CC-19, CC-39, PF-13).
/// - `Endpoint` with `spec.audience == "public"`, and `App` with `spec.visibility == "public"` or a
///   non-empty `spec.egress`, are `Red` (public exposure, PF-72, AP-120, AP-134).
/// - Federation edges and data-space edges (`ContextSourceRegistration`, `SharedSpaceReference`,
///   `DataSpaceParticipant`, `DataOffer`, `DataAgreement`) are `Red`.
/// - Identity and access kinds (`ServiceAccount`, `Role`, `RoleBinding`, `Group`, `Policy`,
///   `ScopeDefinition`, `Organization`, `Project`, `DelegationPolicy`) are `Red` (PF-52, PF-62,
///   AG-151): who is in a group is who a binding names, so a membership is reviewed like the
///   binding itself.
/// - `Environment` is `Red` (CC-73, CC-75): one file decides the domain of every URN, the image
///   every workload runs and where a `secretRef` is resolved, for a whole environment at once.
/// - `AssistantDeployment` on the `public`, `ckan` or `iframe` channel is `Red`: it answers anyone
///   and spends the installation's model budget (MF-52, Architecture/06 §4).
/// - `ContextSpace` with `spec.isSandbox == true` is `Green` (ephemeral sandbox, CC-67).
/// - `Dashboard` and `Layer` are `Green`.
/// - Everything else defaults to `Yellow`.
pub fn classify(kind: &str, op: Operation, spec: &serde_json::Value) -> Lane {
    if op == Operation::Delete {
        return Lane::Red;
    }

    if kind == "Endpoint" && spec.get("audience").and_then(|v| v.as_str()) == Some("public") {
        return Lane::Red;
    }
    // An App anyone opens without a login is public exposure too (AP-120), and so is one whose
    // pod may send what it reads to a declared destination (AP-134).
    // ponytail: every Change of an App that declares one is red, not only the one that adds or
    // widens it; the lane sees the new spec alone.
    if kind == "App"
        && (spec.get("visibility").and_then(|v| v.as_str()) == Some("public")
            || spec
                .get("egress")
                .and_then(|v| v.as_array())
                .is_some_and(|egress| !egress.is_empty()))
    {
        return Lane::Red;
    }

    // A named MCP server open to anyone is public exposure of its member Endpoints: declaring or
    // widening one to `public` takes a publisher's approval (MF-53, ADR-N-043).
    if kind == "McpServer" && spec.get("audience").and_then(|v| v.as_str()) == Some("public") {
        return Lane::Red;
    }

    if kind == "AssistantDeployment"
        && spec
            .get("channel")
            .and_then(|v| v.as_str())
            .is_some_and(|channel| channel != "internal")
    {
        return Lane::Red;
    }

    match kind {
        "ContextSourceRegistration"
        | "SharedSpaceReference"
        | "DataSpaceParticipant"
        | "DataOffer"
        | "DataAgreement"
        | "ServiceAccount"
        | "Role"
        | "RoleBinding"
        | "Group"
        // A broker brings its own database, client and share of the node, and is where every
        // space without a `brokerRef` lives (PF-110, ADR-N-047).
        | "ContextBroker"
        // Its roles grant ServiceAccounts publish and subscribe on topics: access (ADR-N-048).
        | "MqttBroker"
        | "Environment"
        | "Policy"
        | "ScopeDefinition"
        | "Organization"
        | "Project"
        // The ceiling of every connector a person grants an AI client: what it may read and
        // write is access, reviewed like a Role (AG-151).
        | "DelegationPolicy" => Lane::Red,

        "ContextSpace" if spec.get("isSandbox").and_then(|v| v.as_bool()) == Some(true) => {
            Lane::Green
        }

        "Dashboard" | "Layer" => Lane::Green,

        _ => Lane::Yellow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---------------------------------------------------------------------------------------
    // T-2107 `with_merge_request`, T-2108 `Change::new` (T-4454 removed `from_mr_str`).
    //
    // The contract that matters: a change's `name` reaches a URL path
    // (`/api/v1/projects/{project}/changes/{name}`), so **whatever string a forge hands us, the name
    // is `chg-` and eight lowercase hex digits** — never a traversal, a space, or anything a path
    // would have to escape. And the same merge request always derives the same name, or a change
    // would be a new one on every poll.
    // ---------------------------------------------------------------------------------------

    fn pending_status() -> ChangeStatus {
        ChangeStatus::new(
            Lane::Green,
            ChangePhase::PendingApproval,
            PlanSummary::new(1, 0, 0),
        )
    }

    #[test]
    fn a_merge_request_number_names_the_change_and_the_namespace_stays_apart() {
        assert_eq!(
            ChangeMeta::from_merge_request(17, "helsinki").name,
            "chg-00000011"
        );
        // Two merge requests are two changes.
        assert_ne!(
            ChangeMeta::from_merge_request(17, "helsinki").name,
            ChangeMeta::from_merge_request(18, "helsinki").name,
        );
        // The namespace is carried, never folded into the name: the same MR number in two projects
        // is two changes, told apart by where they live.
        let a = ChangeMeta::from_merge_request(17, "helsinki");
        let b = ChangeMeta::from_merge_request(17, "banskabystrica");
        assert_eq!(a.name, b.name);
        assert_ne!(a.namespace, b.namespace);
    }

    #[test]
    fn a_merge_request_is_recorded_as_it_came_and_replaces_the_one_before_it() {
        // `with_merge_request` is a builder on the status: the value is what the forge said (a URL or
        // a number), kept for the link back, while the *name* is the derived one above. Setting it
        // twice keeps the last, so a re-poll cannot leave two.
        let status = pending_status()
            .with_merge_request("https://gitea.example/org/repo/pulls/17")
            .with_merge_request("https://gitea.example/org/repo/pulls/18");
        assert_eq!(
            status.merge_request.as_deref(),
            Some("https://gitea.example/org/repo/pulls/18"),
        );
        // An empty string is still a value, not an absence: a caller that has no merge request does
        // not call this, and a `Some("")` that came back from a forge is visible rather than hidden.
        assert_eq!(
            pending_status()
                .with_merge_request("")
                .merge_request
                .as_deref(),
            Some(""),
        );
        assert_eq!(pending_status().merge_request, None);
    }

    #[test]
    fn a_change_carries_its_own_api_version_and_kind_and_nothing_of_the_caller() {
        let change = Change::new(
            ChangeMeta::from_merge_request(17, "helsinki"),
            pending_status(),
        );
        assert_eq!(change.api_version, API_VERSION);
        assert_eq!(change.kind, Change::KIND);
        assert_eq!(change.metadata.name, "chg-00000011");

        // Serialized, it is the envelope the API documents: camelCase, the kind spelled out, and no
        // field a client would have to guess at.
        let json = serde_json::to_value(&change).expect("serialize");
        assert_eq!(json["apiVersion"], API_VERSION);
        assert_eq!(json["kind"], "Change");
        assert_eq!(json["metadata"]["namespace"], "helsinki");
    }

    /// PF-13: a deletion of any managed resource takes the explicit protocol, never a lighter lane.
    #[test]
    fn deletions_always_land_in_red_lane() {
        let empty_spec = json!({});
        // Deletion always wins over Green kinds
        assert_eq!(
            classify("Dashboard", Operation::Delete, &empty_spec),
            Lane::Red
        );
        assert_eq!(classify("Layer", Operation::Delete, &empty_spec), Lane::Red);
        let sandbox_spec = json!({ "isSandbox": true });
        assert_eq!(
            classify("ContextSpace", Operation::Delete, &sandbox_spec),
            Lane::Red
        );

        // Deletion wins over Yellow kinds
        assert_eq!(
            classify("DataModel", Operation::Delete, &empty_spec),
            Lane::Red
        );
        assert_eq!(
            classify("Pipeline", Operation::Delete, &empty_spec),
            Lane::Red
        );

        // Deletion on Endpoint regardless of audience
        assert_eq!(
            classify(
                "Endpoint",
                Operation::Delete,
                &json!({ "audience": "internal" })
            ),
            Lane::Red
        );
        assert_eq!(
            classify(
                "Endpoint",
                Operation::Delete,
                &json!({ "audience": "public" })
            ),
            Lane::Red
        );
    }

    /// AP-120, T-2690: an App reachable without a login is public exposure, like a public
    /// Endpoint; one without `visibility` is `project` and stays yellow.
    #[test]
    fn a_public_app_is_red_and_any_other_app_yellow() {
        for op in [Operation::Create, Operation::Update] {
            assert_eq!(
                classify("App", op, &json!({ "visibility": "public" })),
                Lane::Red
            );
            for other in [
                json!({ "visibility": "project" }),
                json!({ "visibility": "roles" }),
                json!({}),
            ] {
                assert_eq!(classify("App", op, &other), Lane::Yellow, "{other}");
            }
        }
    }

    /// AP-134: an App whose pod may reach a declared destination takes the red lane; an empty
    /// list is no destination.
    #[test]
    fn an_app_that_declares_egress_is_red() {
        for op in [Operation::Create, Operation::Update] {
            assert_eq!(
                classify(
                    "App",
                    op,
                    &json!({ "visibility": "project", "egress": [{ "cidr": "203.0.113.0/24", "ports": [443] }] })
                ),
                Lane::Red
            );
            assert_eq!(
                classify("App", op, &json!({ "visibility": "project", "egress": [] })),
                Lane::Yellow
            );
        }
    }

    #[test]
    fn endpoint_audience_classification() {
        let public_spec = json!({ "audience": "public" });
        assert_eq!(
            classify("Endpoint", Operation::Create, &public_spec),
            Lane::Red
        );
        assert_eq!(
            classify("Endpoint", Operation::Update, &public_spec),
            Lane::Red
        );

        let internal_spec = json!({ "audience": "internal" });
        assert_eq!(
            classify("Endpoint", Operation::Create, &internal_spec),
            Lane::Yellow
        );
        assert_eq!(
            classify("Endpoint", Operation::Update, &internal_spec),
            Lane::Yellow
        );

        let omitted_audience = json!({});
        assert_eq!(
            classify("Endpoint", Operation::Create, &omitted_audience),
            Lane::Yellow
        );
    }

    #[test]
    fn federation_and_dataspace_edges_are_red() {
        let kinds = [
            "ContextSourceRegistration",
            "SharedSpaceReference",
            "DataSpaceParticipant",
            "DataOffer",
            "DataAgreement",
        ];
        let spec = json!({});
        for kind in kinds {
            assert_eq!(
                classify(kind, Operation::Create, &spec),
                Lane::Red,
                "{kind} create must be Red"
            );
            assert_eq!(
                classify(kind, Operation::Update, &spec),
                Lane::Red,
                "{kind} update must be Red"
            );
        }
    }

    #[test]
    fn identity_and_access_kinds_are_red() {
        let kinds = [
            "ServiceAccount",
            "Role",
            "RoleBinding",
            "Group",
            "Policy",
            "ScopeDefinition",
            "Organization",
            "Project",
            "DelegationPolicy",
            // Not identity, but the same weight: the overlay that decides the domain, the
            // images and the secret backend of a whole environment (CC-73, T-0882).
            "Environment",
        ];
        let spec = json!({});
        for kind in kinds {
            assert_eq!(
                classify(kind, Operation::Create, &spec),
                Lane::Red,
                "{kind} create must be Red"
            );
            assert_eq!(
                classify(kind, Operation::Update, &spec),
                Lane::Red,
                "{kind} update must be Red"
            );
        }
    }

    #[test]
    fn context_space_sandbox_is_green_while_regular_space_is_yellow() {
        let sandbox = json!({ "isSandbox": true });
        assert_eq!(
            classify("ContextSpace", Operation::Create, &sandbox),
            Lane::Green
        );
        assert_eq!(
            classify("ContextSpace", Operation::Update, &sandbox),
            Lane::Green
        );

        let regular = json!({ "isSandbox": false });
        assert_eq!(
            classify("ContextSpace", Operation::Create, &regular),
            Lane::Yellow
        );
        assert_eq!(
            classify("ContextSpace", Operation::Update, &regular),
            Lane::Yellow
        );

        let omitted = json!({});
        assert_eq!(
            classify("ContextSpace", Operation::Create, &omitted),
            Lane::Yellow
        );
    }

    #[test]
    fn dashboards_and_layers_are_green() {
        let spec = json!({});
        assert_eq!(classify("Dashboard", Operation::Create, &spec), Lane::Green);
        assert_eq!(classify("Dashboard", Operation::Update, &spec), Lane::Green);
        assert_eq!(classify("Layer", Operation::Create, &spec), Lane::Green);
        assert_eq!(classify("Layer", Operation::Update, &spec), Lane::Green);
    }

    // DM-75, DM-77: an organization's data model is red wherever it is classified; a project's
    // stays as its kind says.
    #[test]
    fn an_organization_data_model_is_red_and_a_projects_is_not() {
        let manifest = |namespace: &str| {
            serde_json::from_value::<crate::resource::ResourceEnvelope>(json!({
                "apiVersion": API_VERSION,
                "kind": "DataModel",
                "metadata": { "name": "air", "namespace": namespace },
                "spec": { "linkml": "./air.linkml.yaml", "version": "1.0.0" }
            }))
            .expect("envelope")
        };
        for op in [Operation::Create, Operation::Update] {
            assert_eq!(classify_manifest(&manifest("org"), op), Lane::Red);
            assert_eq!(classify_manifest(&manifest("helsinki"), op), Lane::Yellow);
        }
    }

    #[test]
    fn everything_else_is_yellow() {
        let spec = json!({});
        let kinds = [
            "DataModel",
            "Pipeline",
            "App",
            "Subscription",
            "Entity",
            "Mapping",
            "SyncSource",
            "Blueprint",
            "UnknownResourceKind",
        ];
        for kind in kinds {
            assert_eq!(
                classify(kind, Operation::Create, &spec),
                Lane::Yellow,
                "{kind} create must be Yellow"
            );
            assert_eq!(
                classify(kind, Operation::Update, &spec),
                Lane::Yellow,
                "{kind} update must be Yellow"
            );
        }
    }

    #[test]
    fn change_meta_name_formatting() {
        let meta = ChangeMeta::from_merge_request(42, "ovzdusie");
        assert_eq!(meta.name, "chg-0000002a");
        assert_eq!(meta.namespace, "ovzdusie");
    }

    #[test]
    fn change_json_serialization() {
        let change = Change::new(
            ChangeMeta::from_merge_request(1, "ovzdusie"),
            ChangeStatus::new(
                Lane::Green,
                ChangePhase::PendingApproval,
                PlanSummary::new(1, 0, 0),
            )
            .with_merge_request("https://git.example.sk/pulls/1"),
        );

        let serialized = serde_json::to_value(&change).expect("serialize change");
        assert_eq!(serialized["apiVersion"], "joinedcontext.com/v1alpha1");
        assert_eq!(serialized["kind"], "Change");
        assert_eq!(serialized["metadata"]["name"], "chg-00000001");
        assert_eq!(serialized["metadata"]["namespace"], "ovzdusie");
        assert_eq!(serialized["status"]["lane"], "green");
        assert_eq!(serialized["status"]["phase"], "PendingApproval");
        assert_eq!(
            serialized["status"]["mergeRequest"],
            "https://git.example.sk/pulls/1"
        );
        assert_eq!(serialized["status"]["plan"]["create"], 1);
        assert_eq!(serialized["status"]["plan"]["update"], 0);
        assert_eq!(serialized["status"]["plan"]["delete"], 0);
    }

    // T-5609: a Change proposed over MCP says so in Git and on the page (AG-17, AG-137).

    #[tokio::test]
    async fn a_call_over_mcp_signs_the_commit_and_the_merge_request() {
        let via = ProposedVia::mcp(Some("mcp-mobile"));
        let (message, body) = PROPOSED_VIA
            .scope(via, async {
                (
                    ProposedVia::sign("Update Endpoint bikes").into_owned(),
                    ProposedVia::sign_body("Proposed update.").into_owned(),
                )
            })
            .await;
        assert_eq!(
            message,
            "Update Endpoint bikes\n\nProposed-Via: mcp; client=mcp-mobile"
        );
        assert!(body.starts_with("Proposed through an AI client over MCP (client `mcp-mobile`)."));
        assert_eq!(
            ProposedVia::in_body(&body),
            Some(ProposedVia::mcp(Some("mcp-mobile")))
        );
    }

    #[test]
    fn outside_a_call_over_mcp_nothing_is_signed() {
        assert_eq!(
            ProposedVia::sign("Update Endpoint bikes"),
            "Update Endpoint bikes"
        );
        assert_eq!(
            ProposedVia::sign_body("Proposed update."),
            "Proposed update."
        );
        assert_eq!(ProposedVia::in_body("Proposed update."), None);
    }

    #[test]
    fn a_client_id_outside_the_charset_is_never_written_or_shown() {
        assert_eq!(ProposedVia::mcp(Some("evil\n<b>x</b>")).client, "unknown");
        assert_eq!(ProposedVia::mcp(Some(&"a".repeat(65))).client, "unknown");
        assert_eq!(ProposedVia::mcp(None).client, "unknown");
        // T-6390: a CIMD client id is a URL; the known ones go by name, another by its host.
        for (azp, client) in [
            (
                "https://claude.ai/oauth/claude-code-client-metadata",
                "claude-code",
            ),
            (
                "https://claude.ai/oauth/mcp-oauth-client-metadata",
                "claude-ai",
            ),
            (
                "https://chatgpt.com/connector/oauth/metadata.json",
                "chatgpt.com",
            ),
            ("https://user:pw@evil.example/x", "unknown"),
            ("http://plain.example/x", "unknown"),
        ] {
            assert_eq!(ProposedVia::mcp(Some(azp)).client, client, "{azp}");
        }
        assert_eq!(
            ProposedVia::in_body("Proposed-Via: mcp; client=<img src=x>"),
            None
        );
        assert_eq!(ProposedVia::in_body("Proposed-Via: person; client=x"), None);
    }

    #[test]
    fn a_change_a_connector_opened_names_it_and_its_plan_in_the_trailer_and_the_body() {
        let connector = crate::connectors::Connector {
            id: "abcdefghijklmnopqrstuvwxyz".into(),
            name: "doprava-in-claude".into(),
            ..crate::connectors::tests::sample()
        };
        let via = ProposedVia::mcp(Some("claude-code"))
            .through(Some(&connector))
            .with_plan("p2345234523452345234523452");
        assert_eq!(
            via.trailer(),
            "Proposed-Via: mcp; client=claude-code; connector=abcdefghijklmnopqrstuvwxyz; \
             connectorName=doprava-in-claude; plan=p2345234523452345234523452"
        );
        let body = format!("Proposed update.\n\n{}", via.trailer());
        assert_eq!(ProposedVia::in_body(&body), Some(via.clone()));
        let status = ChangeStatus::new(
            Lane::Yellow,
            ChangePhase::PendingApproval,
            PlanSummary::default(),
        )
        .proposed_through(&body);
        assert_eq!(
            serde_json::to_value(&status).expect("serialize")["proposedVia"],
            json!({
                "channel": "mcp",
                "client": "claude-code",
                "connector": "abcdefghijklmnopqrstuvwxyz",
                "connectorName": "doprava-in-claude",
                "plan": "p2345234523452345234523452",
            })
        );
        // A hand edit that adds a key or a value outside the charset shows nothing it cannot check.
        assert_eq!(
            ProposedVia::in_body("Proposed-Via: mcp; client=c; owner=root"),
            None
        );
        assert_eq!(
            ProposedVia::in_body("Proposed-Via: mcp; client=c; connectorName=<b>x</b>"),
            None
        );
        // A connector name outside the charset is never written.
        let odd = crate::connectors::Connector {
            name: "x\ny".into(),
            ..connector
        };
        assert_eq!(
            ProposedVia::mcp(Some("c")).through(Some(&odd)).connector,
            None
        );
    }

    #[test]
    fn the_status_carries_the_channel_its_merge_request_names() {
        let status = ChangeStatus::new(
            Lane::Yellow,
            ChangePhase::PendingApproval,
            PlanSummary::default(),
        )
        .proposed_through("x\n\nProposed-Via: mcp; client=mcp-mobile");
        let serialized = serde_json::to_value(&status).expect("serialize");
        assert_eq!(
            serialized["proposedVia"],
            json!({ "channel": "mcp", "client": "mcp-mobile" })
        );
    }
}
