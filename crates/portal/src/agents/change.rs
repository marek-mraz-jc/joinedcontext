//! `change_resource` (AG-77, AG-73): the conversation changes or removes a resource of any kind
//! by its name. The platform reads the manifest, applies the model's JSON merge patch (RFC 7386)
//! and checks the result; the kind's page opens with the change filled in, or with its removal
//! dialog, and the person proposes from there. Nothing is proposed here.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::share::TOOL_FENCE;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeResource {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    /// The fields that change, as a JSON merge patch of the manifest; `null` removes a field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<Value>,
    /// A data model's change: the model editor's operations on its LinkML source (DM-13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<Vec<crate::agents::model_change::Operation>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub delete: bool,
    /// A new Dashboard: `patch` holds its spec and `layers` the new Layers its pages draw.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub create: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<NewLayer>,
    /// The rest of a request for several resources, in order (T-6531): the person reads each one
    /// with its page after this draft opens.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<NextStep>,
}

/// One resource a request asks for beyond the one drafted now.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct NextStep {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
}

/// The most next steps one answer lists: a longer list is a plan, made with jc_change_plan.
const MAX_NEXT: usize = 8;

/// The lines that tell the person what their request still needs after this draft, each with its
/// kind's page (T-6531), or nothing. The model's words never reach the text: a kind is one the
/// Portal knows and a name is a resource name, else it is left out. `may` says whether the person
/// may propose the kind; `workspace` keeps each page inside the copy the draft is in.
pub fn next_steps(
    project: &str,
    steps: &[NextStep],
    workspace: Option<&str>,
    may: impl Fn(&str) -> bool,
) -> String {
    let lines: Vec<String> = steps
        .iter()
        .filter_map(|step| {
            let info = crate::resource::by_kind(step.kind.trim())
                .filter(|info| info.scope.allows_project() || info.scope.allows_organization())?;
            let name = step.name.trim();
            let named = if is_name(name) {
                format!("{} '{name}'", info.kind)
            } else {
                info.kind.to_owned()
            };
            if !may(info.kind) {
                return Some(format!(
                    "- {named}: you may not propose a {}; ask a steward of the project",
                    info.kind
                ));
            }
            let mut route = create_route(project, info.kind, info.plural);
            if let Some(workspace) = workspace {
                route = format!("{route}?workspace={workspace}");
            }
            Some(format!("- {named}: on [its page]({route})"))
        })
        .take(MAX_NEXT)
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "\n\nStill to do for your request, in this order; ask me for the next one when this \
         draft is proposed:\n{}",
        lines.join("\n")
    )
}

/// A resource name: lowercase letters, digits and hyphens, at most 63.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A Layer a new dashboard draws, created with it (UI-18).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct NewLayer {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub spec: Value,
}

/// Words by which a person asks for a Role that changes nothing (T-6534), English and Slovak,
/// matched in the lower-cased request. ponytail: a fixed phrase list, a model-read intent when a
/// third language needs it.
const NO_WRITES: [&str; 11] = [
    "no write",
    "without writ",
    "read-only",
    "read only",
    "only read",
    "bez zápis",
    "bez zapis",
    "len na čítanie",
    "iba na čítanie",
    "len čítať",
    "iba čítať",
];

/// Why a Role draft grants more than the person asked for, if it does (T-6534, walk S-28): the
/// person said no writes, and a rule holds a verb other than `read`. Propose opens changes,
/// approve merges them and delete removes manifests: each is a write, so the draft goes back
/// to the model, which says what the ask needs and asks the person instead of granting it.
pub fn wider_than_asked(call: &ChangeResource, asked: &str) -> Option<String> {
    if call.kind.trim() != "Role" || call.delete {
        return None;
    }
    let asked = asked.to_lowercase();
    if !NO_WRITES.iter().any(|words| asked.contains(words)) {
        return None;
    }
    let rules = call.patch.as_ref()?.pointer("/spec/rules")?.as_array()?;
    let mut writes: Vec<String> = rules
        .iter()
        .flat_map(|rule| {
            let kinds = rule["kinds"]
                .as_array()
                .map(|kinds| {
                    kinds
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            rule["verbs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|verb| *verb != "read")
                .map(move |verb| format!("{verb} on {kinds}"))
        })
        .collect();
    if writes.is_empty() {
        return None;
    }
    writes.dedup();
    Some(format!(
        "the person asked for no writes, and this Role grants {}: propose opens changes, approve \
         merges them and delete removes manifests, so each is a write. Draft the Role with read \
         rules only, and say plainly what the rest of the ask needs (running a pipeline needs \
         propose and approve on Pipeline, both writes) and ask the person whether to grant it",
        writes.join("; ")
    ))
}

/// A manifest of this project's `kind`, as a new resource is written.
pub fn manifest(kind: &str, project: &str, name: &str, spec: Value) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": kind,
        "metadata": { "name": name, "namespace": project },
        "spec": spec,
    })
}

/// The kinds one sentence creates from the chat (AG-45, UI-45): the kind's page opens on the
/// checked draft, the form is already filled and the person proposes it. A Dashboard is created
/// with its layers by its own step; every other kind is changed from the chat, never created,
/// because its create needs a page of its own (a space is completed from files, an endpoint is
/// shared, a role is granted). A DataModel is created from the editor's operations instead of a
/// patch (`create_model`), and its editor opens on the draft.
pub const CREATABLE: [&str; 4] = ["ContextSpace", "DataSource", "Pipeline", "Role"];

/// The manifest of a new resource, from the fields the model sent as its `patch`: its `spec`, and
/// `metadata.title` when the sentence named one. The kind, the name and the namespace are the
/// platform's; a patch that carries no spec is refused in words the model can act on.
pub fn new_manifest(
    kind: &str,
    namespace: &str,
    name: &str,
    patch: Option<&Value>,
) -> Result<Value, String> {
    if !CREATABLE.contains(&kind) {
        return Err(format!(
            "a new {kind} is not created from the chat; the kinds created here are {}",
            CREATABLE.join(", ")
        ));
    }
    let patch = patch.ok_or_else(|| {
        format!(
            "a new {kind} carries its fields as the patch: {{\"spec\": {{…}}}}, and its title as \
             {{\"metadata\": {{\"title\": {{\"en\": \"…\"}}}}}}"
        )
    })?;
    let manifest = patched(&manifest(kind, namespace, name, json!({})), patch)?;
    if !manifest["spec"]
        .as_object()
        .is_some_and(|spec| !spec.is_empty())
    {
        return Err(format!(
            "a new {kind} needs a spec; the patch carried none: {patch}"
        ));
    }
    Ok(manifest)
}

/// The spec a new resource's `patch` carries: `{spec: {…}}` as a merge patch of an empty
/// manifest, or the spec itself.
pub fn spec_of(patch: &Value) -> Option<Value> {
    let spec = patch.get("spec").unwrap_or(patch);
    spec.is_object().then(|| spec.clone())
}

/// The attributes a layer's encodings and popup name, in order, each once.
pub fn named_attributes(layer: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let encoded = ["/colorBy/property", "/sizeBy/property"]
        .into_iter()
        .filter_map(|pointer| layer.pointer(pointer).and_then(Value::as_str));
    let popup = layer
        .get("popupProperties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    for name in encoded.chain(popup) {
        if !names.iter().any(|known| known == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// The call of the tool named `tool` in a model answer, when the answer holds one.
pub fn call_of<T: DeserializeOwned>(answer: &str, tool: &str) -> Option<Result<T, String>> {
    TOOL_FENCE.captures_iter(answer).find_map(|fence| {
        let value = serde_json::from_str::<Value>(&fence[1]).ok()?;
        (value.get("tool").and_then(Value::as_str) == Some(tool)).then(|| {
            serde_json::from_value::<T>(value)
                .map_err(|err| format!("the {tool} call does not parse: {err}"))
        })
    })
}

/// The `change_resource` call in a model answer, when the answer is one.
pub fn tool_call(answer: &str) -> Option<Result<ChangeResource, String>> {
    call_of(answer, "change_resource")
}

/// The manifest with the patch applied and without status (MF-04). The kind, the name and the
/// namespace stay: another one would be another resource.
pub fn patched(current: &Value, patch: &Value) -> Result<Value, String> {
    if !patch.is_object() {
        return Err(
            "the patch is a JSON merge patch: an object holding only the fields that change"
                .to_owned(),
        );
    }
    let mut before = current.clone();
    if let Value::Object(fields) = &mut before {
        fields.remove("status");
    }
    let mut after = before.clone();
    crate::plan::merge_patch(&mut after, patch);
    if let Value::Object(fields) = &mut after {
        fields.remove("status");
    }
    for pointer in [
        "/apiVersion",
        "/kind",
        "/metadata/name",
        "/metadata/namespace",
    ] {
        if after.pointer(pointer) != before.pointer(pointer) {
            return Err(format!(
                "{pointer} stays as it is; a different one would be another resource"
            ));
        }
    }
    if after == before {
        return Err("the patch changes nothing in the manifest".to_owned());
    }
    Ok(after)
}

/// What a pipeline's test exercises: what it reads, how it maps and where it writes.
const TESTED: [&str; 5] = ["source", "compute", "output", "targetEndpoint", "class"];

/// Whether a pipeline change reaches what its test exercises; a pause, a period, a schedule or a
/// quota does not (PL-45).
pub fn reaches_the_test(before: &Value, after: &Value) -> bool {
    TESTED
        .iter()
        .any(|field| before["spec"].get(field) != after["spec"].get(field))
}

/// The Portal page a kind is changed on: its own section where it has one, else its list.
fn page<'a>(kind: &str, plural: &'a str) -> &'a str {
    match kind {
        "ContextSpace" => "spaces",
        "DataModel" => "models",
        "Dashboard" | "Layer" => "dashboards",
        // Project settings holds them since T-2606, one tab each (Architecture/09 §14.3).
        "ServiceAccount" => "settings/service-accounts",
        "Role" => "settings/roles",
        "RoleBinding" => "settings/members",
        "CkanInstance" => "ckan",
        _ => plural,
    }
}

/// The sections whose page opens a kind's empty create form at `/{section}/new` (T-2474): the
/// list pages that host a routed form (`useCreateForm`, `useOpenFromAddress`, `useFormRoute` in
/// the UI). Any other section answers `/new` with "this form cannot be opened" (T-2582).
const NEW_FORMS: [&str; 9] = [
    "spaces",
    "endpoints",
    "pipelines",
    "policies",
    "subscriptions",
    "csrs",
    "datasources",
    "syncsources",
    "settings/service-accounts",
];

/// The section a kind's create form opens in, from its plural or its kind as the model spells
/// it: `/projects/{project}/{section}/new` (T-2577, T-2582, AG-73). What is no kind, or a kind
/// whose page has no create form of its own, is refused in words the model acts on.
pub fn section(plural_or_kind: &str) -> Result<&'static str, String> {
    if let Some(tab) = organization_tab(plural_or_kind) {
        return Ok(tab);
    }
    let section = page_of(plural_or_kind)?;
    if NEW_FORMS.contains(&section) {
        Ok(section)
    } else {
        let kind = crate::resource::by_plural(plural_or_kind)
            .or_else(|| crate::resource::by_kind(plural_or_kind))
            .map_or(plural_or_kind, |i| i.kind);
        if CREATABLE.contains(&kind) {
            return Err(format!(
                "a new {kind} is drafted, not opened empty: call describe_tool with name \
                 `create-resource`, then change_resource with `create: true`"
            ));
        }
        Err(format!(
            "a new {kind} has no create form to open empty; open its page ('{section}') with \
             `resource`, `spaces`, `models` or the page that shows it, and tell the person which \
             button there creates one"
        ))
    }
}

/// The kinds that live in the organization alone, each created and edited on its own tab of the
/// Organization page (`/organization/{tab}`, Architecture/09 §14.1). Their manifests and drafts
/// are kept under `org`, so a project's list neither shows them nor opens their drafts (T-2845).
const ORGANIZATION_TABS: [(&str, &str); 4] = [
    ("Blueprint", "blueprints"),
    ("AgentProfile", "agentprofiles"),
    ("DataSpaceParticipant", "dataspaceparticipants"),
    ("Environment", "environments"),
];

/// The Organization tab a kind is changed on, from its plural or its kind as the model spells
/// it; `None` for a kind a project holds.
pub fn organization_tab(plural_or_kind: &str) -> Option<&'static str> {
    let info = crate::resource::by_plural(plural_or_kind)
        .or_else(|| crate::resource::by_kind(plural_or_kind))?;
    ORGANIZATION_TABS
        .iter()
        .find(|(kind, _)| *kind == info.kind)
        .map(|(_, tab)| *tab)
}

/// The address of the list a kind is changed on: its Organization tab, or its project page.
fn list_address(project: &str, kind: &str, plural: &str) -> String {
    match organization_tab(kind) {
        Some(tab) => format!("/organization/{tab}"),
        None => format!("/projects/{project}/{}", page(kind, plural)),
    }
}

/// The page a kind's resources and drafts open on, from its plural or its kind as the model
/// spells it (T-2768): a plural is not a page (`datamodels` is `models`).
pub fn page_of(plural_or_kind: &str) -> Result<&'static str, String> {
    let info = crate::resource::by_plural(plural_or_kind)
        .or_else(|| crate::resource::by_kind(plural_or_kind))
        .ok_or_else(|| format!("'{plural_or_kind}' is not a kind of the Portal"))?;
    Ok(page(info.kind, info.plural))
}

/// Where the person reviews the change: the kind's page with the resource's editor open
/// (`?edit=`), or its removal dialog (`?delete=`). The endpoint form opens from the draft it is
/// handed, as it did for `edit_endpoint`.
/// Where the person reviews a new resource: the kind's page, which opens on the draft the
/// conversation kept for them (`?draft=`, appended by the dock from the event's draft) and fills
/// the form from it (AG-45, UI-45).
pub fn create_route(project: &str, kind: &str, plural: &str) -> String {
    list_address(project, kind, plural)
}

/// The page one held object opens on (T-6486): an Endpoint's own page, an organization's Role or
/// Group on its tab of the Organization page, any other object in the editor of its kind.
pub fn object_route(project: &str, kind: &str, plural: &str, name: &str) -> String {
    if project == crate::permissions::ORG_NAMESPACE && organization_tab(kind).is_none() {
        let tab = match kind {
            "Role" => "roles",
            "Group" => "groups",
            "RoleBinding" => "members",
            "ServiceAccount" => "service-accounts",
            _ => "settings",
        };
        return format!("/organization/{tab}");
    }
    match kind {
        "Endpoint" => format!("{}/{name}", list_address(project, kind, plural)),
        _ => route(project, kind, plural, name, false),
    }
}

/// A Portal route as the whole address a person outside the Portal opens: a connector's client
/// has no host to resolve a path against.
pub fn address(config: &crate::config::Config, route: &str) -> String {
    let base = config.public_base_url.as_str().trim_end_matches('/');
    format!("{base}{route}")
}

pub fn route(project: &str, kind: &str, plural: &str, name: &str, delete: bool) -> String {
    let list = list_address(project, kind, plural);
    match (kind, delete) {
        ("Endpoint", false) => list,
        (_, true) => format!("{list}?delete={name}"),
        (_, false) => format!("{list}?edit={name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(kind: &str, name: &str) -> NextStep {
        NextStep {
            kind: kind.into(),
            name: name.into(),
        }
    }

    /// T-6531 (walk T-6490, S-26 and S-91): the rest of a several-resource request is named with
    /// each kind's page, inside the copy; a kind the person may not propose says who can.
    #[test]
    fn the_rest_of_a_request_is_named_with_each_page() {
        let steps = [
            step("DataModel", "vendor-model"),
            step("Endpoint", "vendor-write"),
            step("Policy", "vendor-read"),
        ];
        let text = next_steps("helsinki", &steps, Some("vendor"), |kind| kind != "Policy");
        assert!(text.contains("Still to do"), "{text}");
        assert!(
            text.contains("- DataModel 'vendor-model': on [its page](/projects/helsinki/models?workspace=vendor)"),
            "{text}"
        );
        assert!(
            text.contains("- Endpoint 'vendor-write': on [its page]("),
            "{text}"
        );
        assert!(
            text.contains("- Policy 'vendor-read': you may not propose"),
            "{text}"
        );
        assert!(
            text.find("DataModel") < text.find("Endpoint"),
            "kept in the model's order: {text}"
        );
    }

    /// The model's words never reach the person's text: an unknown kind is left out and a name
    /// that is no resource name is not quoted.
    #[test]
    fn a_next_step_carries_only_known_kinds_and_resource_names() {
        let steps = [
            step("Spaceship", "x"),
            step("Pipeline", "](https://evil.example) click"),
        ];
        let text = next_steps("helsinki", &steps, None, |_| true);
        assert!(!text.contains("Spaceship"), "{text}");
        assert!(!text.contains("evil"), "{text}");
        assert!(
            text.contains("- Pipeline: on [its page](/projects/helsinki/"),
            "{text}"
        );
        assert_eq!(next_steps("helsinki", &[], None, |_| true), "");
        assert_eq!(
            next_steps("helsinki", &[step("Nope", "a")], None, |_| true),
            ""
        );
        let many: Vec<NextStep> = (0..20)
            .map(|i| step("Pipeline", &format!("p{i}")))
            .collect();
        assert_eq!(
            next_steps("helsinki", &many, None, |_| true)
                .matches("\n- ")
                .count(),
            MAX_NEXT
        );
    }

    fn role(rules: Value) -> ChangeResource {
        ChangeResource {
            kind: "Role".into(),
            name: "analyst".into(),
            patch: Some(json!({ "spec": { "rules": rules } })),
            create: true,
            ..ChangeResource::default()
        }
    }

    /// T-6534, walk S-28: "no writes" with "run pipelines" drafted Pipeline propose + approve;
    /// that Role goes back with each write it grants, and a read-only one passes.
    #[test]
    fn a_role_with_writes_is_refused_when_the_person_asked_for_none() {
        let asked = "Create a role Analyst: read all Endpoints, run pipelines, no writes.";
        let walked = role(json!([
            { "kinds": ["Endpoint"], "verbs": ["read"] },
            { "kinds": ["Pipeline"], "verbs": ["propose", "approve"] }
        ]));
        let reason = wider_than_asked(&walked, asked).expect("refused");
        assert!(
            reason.contains("propose on Pipeline; approve on Pipeline"),
            "{reason}"
        );
        assert!(reason.contains("ask the person"), "{reason}");
        let read_only = role(json!([
            { "kinds": ["Endpoint", "Pipeline"], "verbs": ["read"] }
        ]));
        assert_eq!(wider_than_asked(&read_only, asked), None);
        // The same writes asked for in words pass, as does a Slovak "read only" Role without them.
        assert_eq!(
            wider_than_asked(&walked, "Create a role Operator that runs pipelines"),
            None
        );
        assert!(wider_than_asked(&walked, "Rola Analytik, len na čítanie").is_some());
        // Another kind and a removal are not a Role's grants.
        let pipeline = ChangeResource {
            kind: "Pipeline".into(),
            ..walked.clone()
        };
        assert_eq!(wider_than_asked(&pipeline, asked), None);
        let removal = ChangeResource {
            delete: true,
            ..walked
        };
        assert_eq!(wider_than_asked(&removal, asked), None);
    }

    /// T-6489, walk S-28: a Role is drafted with its rules, and the empty form the model asked
    /// for first answers how, instead of sending it to a list.
    #[test]
    fn a_new_role_is_drafted_and_its_empty_form_says_how() {
        let patch = json!({ "spec": { "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] } });
        let role = new_manifest("Role", "helsinki", "analyst", Some(&patch)).expect("drafted");
        assert_eq!(role["kind"], "Role");
        assert_eq!(role["metadata"]["namespace"], "helsinki");
        assert_eq!(role["spec"], patch["spec"]);
        let refused = section("roles").expect_err("no empty form");
        assert!(refused.contains("create-resource"), "{refused}");
        assert!(section("Role")
            .expect_err("no empty form")
            .contains("a new Role"));
        // A kind the chat does not create still goes to its page.
        let dashboards = section("dashboards").expect_err("no empty form");
        assert!(dashboards.contains("its page"), "{dashboards}");
    }

    fn pipeline() -> Value {
        json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Pipeline",
            "metadata": { "name": "hel-news", "namespace": "helsinki" },
            "spec": { "enabled": true, "schedule": { "every": "15m" }, "compute": { "kind": "bloblang" } },
            "status": { "phase": "Live" }
        })
    }

    #[test]
    fn a_merge_patch_changes_only_the_fields_it_names_and_null_removes_one() {
        let after = patched(
            &pipeline(),
            &json!({ "spec": { "enabled": false, "compute": null } }),
        )
        .expect("patched");
        assert_eq!(
            after["spec"],
            json!({ "enabled": false, "schedule": { "every": "15m" } })
        );
        assert!(after.get("status").is_none(), "status is never written");
    }

    #[test]
    fn a_schedule_a_mapping_and_a_source_address_are_patched_in_place() {
        let mapped = patched(
            &pipeline(),
            &json!({ "spec": { "period": "5m", "compute": { "bloblang": "root.availableBikeNumber = this.num_bikes_available" } } }),
        )
        .expect("patched");
        assert_eq!(mapped["spec"]["period"], "5m");
        // A merge patch keeps the compute's kind beside the new mapping.
        assert_eq!(
            mapped["spec"]["compute"],
            json!({ "kind": "bloblang", "bloblang": "root.availableBikeNumber = this.num_bikes_available" })
        );

        let source = json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "DataSource",
            "metadata": { "name": "hsl-bikes", "namespace": "helsinki" },
            "spec": { "type": "http", "http": { "url": "https://feeds.example/a.json", "timeout": "10s" } }
        });
        let moved = patched(
            &source,
            &json!({ "spec": { "http": { "url": "https://feeds.example/b.json" } } }),
        )
        .expect("patched");
        assert_eq!(
            moved["spec"]["http"],
            json!({ "url": "https://feeds.example/b.json", "timeout": "10s" })
        );
    }

    #[test]
    fn a_pause_or_a_schedule_does_not_reach_the_test_and_a_mapping_does() {
        let paused =
            patched(&pipeline(), &json!({ "spec": { "enabled": false } })).expect("paused");
        assert!(!reaches_the_test(&pipeline(), &paused));
        let every = patched(
            &pipeline(),
            &json!({ "spec": { "schedule": { "every": "5m" } } }),
        )
        .expect("rescheduled");
        assert!(!reaches_the_test(&pipeline(), &every));
        let mapped = patched(
            &pipeline(),
            &json!({ "spec": { "compute": { "bloblang": "root = this" } } }),
        )
        .expect("mapped");
        assert!(reaches_the_test(&pipeline(), &mapped));
    }

    #[test]
    fn a_patch_that_renames_changes_nothing_or_is_no_object_is_refused() {
        let renamed = patched(&pipeline(), &json!({ "metadata": { "name": "other" } }));
        assert!(renamed.is_err_and(|e| e.contains("/metadata/name")));
        let same = patched(&pipeline(), &json!({ "spec": { "enabled": true } }));
        assert!(same.is_err_and(|e| e.contains("changes nothing")));
        let listed = patched(&pipeline(), &json!(["suspended"]));
        assert!(listed.is_err_and(|e| e.contains("merge patch")));
    }

    #[test]
    fn a_new_dashboard_reads_its_spec_either_way_and_its_layers_name_their_attributes_once() {
        let spec = json!({ "title": "Bikes", "pages": [{ "layers": ["stations"] }] });
        assert_eq!(spec_of(&json!({ "spec": spec })), Some(spec.clone()));
        assert_eq!(spec_of(&spec), Some(spec.clone()));
        assert_eq!(spec_of(&json!(["pages"])), None);
        let call = tool_call("```json\n{\"tool\":\"change_resource\",\"kind\":\"Dashboard\",\"name\":\"bikes\",\"create\":true,\"patch\":{\"spec\":{}},\"layers\":[{\"name\":\"stations\",\"spec\":{\"entityType\":\"BikeHireDockingStation\"}}]}\n```")
            .expect("a call")
            .expect("parses");
        assert!(call.create);
        assert_eq!(call.layers[0].name, "stations");
        let layer = json!({
            "colorBy": { "property": "availableBikeNumber" },
            "sizeBy": { "property": "capacity" },
            "popupProperties": ["name", "availableBikeNumber"]
        });
        assert_eq!(
            named_attributes(&layer),
            ["availableBikeNumber", "capacity", "name"]
        );
        assert_eq!(
            manifest("Layer", "helsinki", "stations", json!({}))["metadata"],
            json!({ "name": "stations", "namespace": "helsinki" })
        );
    }

    #[test]
    fn the_call_is_read_from_the_fence_with_a_patch_or_a_removal() {
        let answer = "Pausing it.\n\n```json\n{\"tool\":\"change_resource\",\"kind\":\"Pipeline\",\"name\":\"hel-news\",\"patch\":{\"spec\":{\"enabled\":false}}}\n```\n";
        let call = tool_call(answer).expect("a call").expect("parses");
        assert_eq!(
            (call.kind.as_str(), call.name.as_str()),
            ("Pipeline", "hel-news")
        );
        assert!(!call.delete);
        let removal = "```json\n{\"tool\":\"change_resource\",\"kind\":\"ContextSpace\",\"name\":\"test-space\",\"delete\":true}\n```";
        assert!(tool_call(removal).expect("a call").expect("parses").delete);
        assert!(tool_call("```json\n{\"tool\":\"navigate\",\"route\":\"/\"}\n```").is_none());
    }

    #[test]
    fn each_kind_opens_on_its_own_page() {
        assert_eq!(
            route("helsinki", "Pipeline", "pipelines", "hel-news", false),
            "/projects/helsinki/pipelines?edit=hel-news"
        );
        assert_eq!(
            route("helsinki", "ContextSpace", "spaces", "test-space", true),
            "/projects/helsinki/spaces?delete=test-space"
        );
        assert_eq!(
            route("helsinki", "Endpoint", "endpoints", "helsinki-bikes", false),
            "/projects/helsinki/endpoints"
        );
        assert_eq!(
            route("helsinki", "DataModel", "datamodels", "air", false),
            "/projects/helsinki/models?edit=air"
        );
        assert_eq!(
            route(
                "helsinki",
                "ContextSourceRegistration",
                "csrs",
                "zvolen",
                true
            ),
            "/projects/helsinki/csrs?delete=zvolen"
        );
    }

    /// AG-45: one sentence carries the name and the fields into the form, so the person types
    /// nothing again. A new resource's page opens on the draft, not on a resource that exists.
    #[test]
    fn a_new_resource_carries_the_name_the_spec_and_the_title_the_sentence_gave() {
        let space = new_manifest(
            "ContextSpace",
            "helsinki",
            "ovzdusie",
            Some(&json!({
                "metadata": { "title": { "en": "Air quality" } },
                "spec": { "defaultLocale": "en", "isSandbox": false }
            })),
        )
        .expect("a new space");
        assert_eq!(space["metadata"]["name"], "ovzdusie");
        assert_eq!(space["metadata"]["namespace"], "helsinki");
        assert_eq!(space["metadata"]["title"]["en"], "Air quality");
        assert_eq!(space["spec"]["defaultLocale"], "en");
        assert_eq!(space["apiVersion"], "joinedcontext.com/v1alpha1");

        let source = new_manifest(
            "DataSource",
            "helsinki",
            "aq-opendata",
            Some(&json!({ "spec": { "type": "http", "http": { "url": "https://opendata.example.org/aq.json" } } })),
        )
        .expect("a new data source");
        assert_eq!(
            source["spec"]["http"]["url"],
            "https://opendata.example.org/aq.json"
        );
        assert_eq!(source["metadata"]["title"], Value::Null);
    }

    #[test]
    fn a_new_resource_without_a_spec_or_of_a_kind_with_its_own_page_is_refused_with_the_reason() {
        let no_spec = new_manifest(
            "Pipeline",
            "helsinki",
            "bikes",
            Some(&json!({ "metadata": { "title": { "en": "Bikes" } } })),
        )
        .expect_err("a pipeline without a spec");
        assert!(no_spec.contains("needs a spec"), "{no_spec}");

        let nothing = new_manifest("DataSource", "helsinki", "aq", None).expect_err("no patch");
        assert!(nothing.contains("\"spec\""), "{nothing}");

        // A name the patch tries to change would be another resource; `patched` holds that line.
        let renamed = new_manifest(
            "ContextSpace",
            "helsinki",
            "ovzdusie",
            Some(&json!({ "metadata": { "name": "other" }, "spec": { "defaultLocale": "en" } })),
        )
        .expect_err("a renamed space");
        assert!(renamed.contains("metadata/name"), "{renamed}");

        for kind in ["DataModel", "Endpoint", "RoleBinding", "Dashboard", "Layer"] {
            let refused = new_manifest(kind, "helsinki", "x", Some(&json!({ "spec": { "a": 1 } })))
                .expect_err("not created from the chat");
            assert!(
                refused.contains("ContextSpace, DataSource, Pipeline"),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_new_resource_opens_the_kinds_page_and_the_dock_appends_its_draft() {
        assert_eq!(
            create_route("helsinki", "ContextSpace", "spaces"),
            "/projects/helsinki/spaces"
        );
        assert_eq!(
            create_route("helsinki", "DataSource", "datasources"),
            "/projects/helsinki/datasources"
        );
        assert_eq!(
            create_route("helsinki", "Pipeline", "pipelines"),
            "/projects/helsinki/pipelines"
        );
    }

    // T-2845: an organization kind opens on its Organization tab, where its form reads the
    // draft kept under `org`, never on a project's list of it.
    /// T-6486: the page a read names is the object's own, in the project or the organization
    /// that holds it, and the address carries the instance's host.
    #[test]
    fn a_held_object_names_its_own_page() {
        for (project, kind, plural, name, page) in [
            (
                "hel",
                "Pipeline",
                "pipelines",
                "p",
                "/projects/hel/pipelines?edit=p",
            ),
            (
                "hel",
                "DataModel",
                "datamodels",
                "m",
                "/projects/hel/models?edit=m",
            ),
            (
                "hel",
                "Endpoint",
                "endpoints",
                "e",
                "/projects/hel/endpoints/e",
            ),
            (
                "hel",
                "Role",
                "roles",
                "r",
                "/projects/hel/settings/roles?edit=r",
            ),
            ("org", "Role", "roles", "r", "/organization/roles"),
            ("org", "Group", "groups", "g", "/organization/groups"),
            (
                "org",
                "Organization",
                "organizations",
                "o",
                "/organization/settings",
            ),
            (
                "org",
                "Blueprint",
                "blueprints",
                "b",
                "/organization/blueprints?edit=b",
            ),
        ] {
            assert_eq!(object_route(project, kind, plural, name), page);
        }
        let mut config = crate::config::Config::for_tests();
        config.public_base_url = "https://portal.example.org/".parse().expect("url");
        assert_eq!(
            address(&config, "/projects/hel/pipelines?edit=p"),
            "https://portal.example.org/projects/hel/pipelines?edit=p"
        );
    }

    #[test]
    fn an_organization_kind_opens_on_its_organization_tab_not_in_the_project() {
        for (kind, plural) in [
            ("Blueprint", "blueprints"),
            ("AgentProfile", "agentprofiles"),
            ("DataSpaceParticipant", "dataspaceparticipants"),
            ("Environment", "environments"),
        ] {
            assert_eq!(
                create_route("helsinki", kind, plural),
                format!("/organization/{plural}")
            );
            assert_eq!(
                route("helsinki", kind, plural, "x", false),
                format!("/organization/{plural}?edit=x")
            );
            assert_eq!(
                route("helsinki", kind, plural, "x", true),
                format!("/organization/{plural}?delete=x")
            );
            assert_eq!(organization_tab(kind), Some(plural));
            assert_eq!(organization_tab(plural), Some(plural));
            assert_eq!(section(kind), Ok(plural), "its tab hosts the empty form");
        }
    }

    #[test]
    fn a_project_kind_keeps_its_project_page() {
        assert_eq!(organization_tab("Pipeline"), None);
        assert_eq!(organization_tab("ServiceAccount"), None);
        assert_eq!(organization_tab("nonsense"), None);
        assert_eq!(
            route("helsinki", "Role", "roles", "reader", false),
            "/projects/helsinki/settings/roles?edit=reader"
        );
        assert_eq!(
            route("helsinki", "Endpoint", "endpoints", "air", false),
            "/projects/helsinki/endpoints"
        );
        assert_eq!(section("pipelines"), Ok("pipelines"));
        assert!(
            section("dataoffers").is_err(),
            "a project list without a routed form"
        );
    }
}
