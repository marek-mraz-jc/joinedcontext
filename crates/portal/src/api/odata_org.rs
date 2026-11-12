//! The OData service of one Organization at `/{org}/api/v1/` (ADR-N-070, API/13 §2a, T-6810):
//! reads only; the bound write actions follow in T-6811.
//!
//! - `projects`, keyed by `name`: the Organization's projects the caller may read. Each project
//!   kind of the per-project service ([`crate::api::odata`]) is a navigation of a project
//!   (`projects('air')/endpoints`, or key-as-segment `projects/air/endpoints`) and an
//!   organization-wide set keyed by `(project,name)` (`endpoints(project='air',name='x')`).
//! - `me` and `Organization`, the caller and the Organization, as singletons.
//! - `$expand` one level on a project's navigations; `$filter`, `$select`, `$orderby`, `$top`,
//!   `$skip`, `$count` and `/$count` by `odata-table`'s evaluator; pages of at most 1000 rows
//!   (fewer with `Prefer: odata.maxpagesize`) and a `$skiptoken` signed for the Organization, the
//!   caller and the query. What the service does not serve is refused, never ignored.
//!
//! The Organization is the request's pin ([`crate::organization_host`]: jc's host registry,
//! a bound host's row or a shared host's first path segment), else the installation's when it
//! holds one Organization; never a header, query option, key or URN. Every project read goes
//! through [`crate::api::odata::rows_of`], the registry operation and its own check, and a
//! project of another Organization or one the caller may not read is not there (404).
//!
//! REST compat shares the root: [`negotiate`] sends a request with `OData-Version`,
//! `OData-MaxVersion`, a `$` option, `$metadata`, the service document or a key predicate here,
//! every other one to the REST routes, and marks both `Vary: OData-Version, OData-MaxVersion`.

use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::body::Bytes;
use axum::extract::{FromRequestParts, Request, State};
use axum::extract::{Path, Query};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use hmac::{Hmac, Mac};
use odata_csdl::{
    EntityType, Model, NavigationProperty, NavigationPropertyBinding, Property, TypeRef,
};
use odata_table::{
    key_options, query, Answer, EntityDef, Field, FieldKind, MemoryTable, Rule, Scope, Service,
    Table,
};
use odata_url::{
    EdmModel, ExpandKind, ExpandPath, Key, KeyValue, Literal, ODataUri, QueryOptions, Segment,
    UrlErrorKind,
};
use serde_json::{json, Map, Value};
use sha2::Sha256;

use crate::api::dry_run::DryRunQuery;

use crate::api::odata::{
    bad, key_value, limits, not_found, not_implemented, rows_of, ListSet, Refusal, MAX_ROWS,
    MAX_TOP, NAMESPACE, SERVED,
};
use crate::api::odata_ops;
use crate::api::ops::caller_of;
use crate::auth::session::{CurrentUser, Front};
use crate::ops::Caller;
use crate::permissions::ORG_NAMESPACE;
use crate::resource::is_dns1123;
use crate::state::AppState;

/// The set of the Organization's projects.
const PROJECTS: &str = "projects";
/// The member every organization-wide row carries: its project.
const PROJECT: &str = "project";
/// The evaluator's own unique key of an organization-wide row (`project/name`); never answered
/// and no property of `$metadata`, so no option can name it.
const ROW_KEY: &str = "_key";
/// The singletons, answered without the evaluator.
const ME: &str = "me";
const ORGANIZATION: &str = "Organization";
/// How long a `$skiptoken` stays good.
const TOKEN_TTL_SECS: u64 = 3600;
/// Options the service does not serve: 501, never ignored (API/13 §2a).
const REFUSED: [&str; 7] = [
    "$search",
    "$apply",
    "$compute",
    "$levels",
    "$index",
    "$schemaversion",
    "$deltatoken",
];

/// The Organization service, built once from the per-project one: the public model (composite
/// keys, navigations) the URLs are parsed and `$metadata` written from, and the evaluator's
/// model with one unique key per set.
struct OrgServed {
    public: Arc<Model>,
    service: Service,
    evaluated: Arc<Model>,
}

fn project_fields() -> Vec<Field> {
    let mut name = Field::new("name", FieldKind::String, "The project's name, e.g. air");
    name.nullable = false;
    vec![
        name,
        Field::new("title", FieldKind::String, "Its title, e.g. Air quality"),
        Field::new(
            "organization",
            FieldKind::String,
            "The Organization it belongs to",
        ),
        Field::new(
            "sample",
            FieldKind::Boolean,
            "Whether it is a sample project",
        ),
    ]
}

/// The project kinds of the per-project service: each a navigation and an organization-wide set.
/// Organization-scoped kinds (`Project`, `Organization`, the Organization's bindings and groups)
/// are no project's navigation: `projects` and the `Organization` singleton answer for them.
fn kinds() -> impl Iterator<Item = &'static ListSet> {
    SERVED
        .sets
        .iter()
        .filter(|set| project_kind(&set.type_name))
}

fn project_kind(type_name: &str) -> bool {
    crate::resource::by_kind(type_name)
        .is_none_or(|info| !matches!(info.scope, jc_core::Scope::Organization))
}

/// The Organization's own Roles (no project row carries them since T-6817), a set of their own
/// type so the project kind `Role` keeps its `(project,name)` key.
const ORG_ROLES: &str = "organizationRoles";
const ORG_ROLE_TYPE: &str = "OrganizationRole";

/// The Organization sets (T-6822): each Organization-scoped kind but `Project` and
/// `Organization` (served as `projects` and the singleton), keyed by name, with its type.
fn org_kinds() -> impl Iterator<Item = (&'static str, &'static str, &'static ListSet)> {
    SERVED
        .sets
        .iter()
        .filter(|set| {
            !set.endpoint_list
                && !project_kind(&set.type_name)
                && !matches!(set.type_name.as_str(), "Project" | ORGANIZATION)
        })
        .map(|set| (set.set.as_str(), set.type_name.as_str(), set))
        .chain(
            kinds()
                .find(|set| set.type_name == "Role")
                .map(|set| (ORG_ROLES, ORG_ROLE_TYPE, set)),
        )
}

/// The Organization set `name`: its rows' kind, read in the `org` namespace.
fn org_set(name: &str) -> Option<&'static ListSet> {
    org_kinds()
        .find(|(set, ..)| *set == name)
        .map(|(.., set)| set)
}

/// Whether the manifest `name` of `kind` in the `org` namespace is `org`'s own: a RoleBinding
/// belongs to the Organization its scope reaches; every other Organization-scoped manifest names
/// no Organization, so it is the installation's (ADR-N-058 section 2.2) and no other one reads it.
fn owned_by(state: &AppState, org: &str, kind: &str, name: &str) -> bool {
    let m = &state.mirror;
    let owner = if kind == "RoleBinding" {
        m.get(ORG_NAMESPACE, kind, name)
            .and_then(|env| crate::permissions::binding_organization(m, &env.spec))
    } else {
        crate::permissions::installation_organization(m)
    };
    owner.as_deref() == Some(org)
}

/// Whether the caller reads the Organization set of `set` in `org`: the Organization owns that
/// kind, and a binding at the Organization itself grants reading it
/// ([`crate::permissions::holds_at_organization`]); a project's binding reads none of it.
fn org_set_readable(state: &AppState, user: &CurrentUser, org: &str, set: &ListSet) -> bool {
    let installation = crate::permissions::installation_organization(&state.mirror);
    (set.type_name == "RoleBinding" || installation.as_deref() == Some(org))
        && crate::permissions::holds_at_organization(
            state,
            &user.0.identity,
            org,
            &set.type_name,
            jc_core::kinds::Verb::Read,
            None,
        )
}

static ORG: LazyLock<OrgServed> = LazyLock::new(|| {
    let read = || {
        vec![Rule {
            operation: odata_table::Operation::Read,
            permission: None,
        }]
    };
    let mut public_sets = vec![EntityDef {
        set: PROJECTS.into(),
        type_name: "Project".into(),
        description: "The Organization's projects the caller may read".into(),
        key: "name".into(),
        fields: project_fields(),
        rules: read(),
    }];
    for def in SERVED
        .service
        .sets
        .iter()
        .filter(|def| project_kind(&def.type_name))
    {
        let mut def = def.clone();
        let mut project = Field::new(PROJECT, FieldKind::String, "The project it lives in");
        project.nullable = false;
        // A Workspace row names its project already.
        def.fields.retain(|f| f.name != PROJECT);
        def.fields.insert(0, project);
        public_sets.push(def);
    }
    for (set, type_name, kind) in org_kinds() {
        if let Some(def) = SERVED.service.sets.iter().find(|def| def.set == kind.set) {
            let mut def = def.clone();
            def.set = set.into();
            def.type_name = type_name.into();
            def.description = format!("The Organization's own {}", kind.set);
            public_sets.push(def);
        }
    }
    let mut evaluated_sets = public_sets.clone();
    for def in evaluated_sets.iter_mut().skip(1) {
        let mut key = Field::new(ROW_KEY, FieldKind::String, "project/name");
        key.nullable = false;
        def.fields.push(key);
        def.key = ROW_KEY.into();
    }
    let service = Service {
        namespace: NAMESPACE.into(),
        title: "JoinedContext Organization".into(),
        sets: evaluated_sets,
    };
    let evaluated = Arc::new(service.model());
    let public = Arc::new(public_model(
        Service {
            namespace: NAMESPACE.into(),
            title: "JoinedContext Organization".into(),
            sets: public_sets,
        }
        .model(),
    ));
    OrgServed {
        public,
        service,
        evaluated,
    }
});

/// The public model: each kind keyed by `(project,name)`, each a navigation of `Project` bound
/// to its organization-wide set, and the singletons' entity types.
fn public_model(mut model: Model) -> Model {
    let navigations: Vec<(String, String, String)> = kinds()
        .map(|set| {
            (
                set.set.clone(),
                format!("{NAMESPACE}.{}", set.type_name),
                set.key.to_owned(),
            )
        })
        .collect();
    for schema in &mut model.schemas {
        for ty in &mut schema.entity_types {
            if ty.name == "Project" {
                for (set, target, _) in &navigations {
                    let mut nav =
                        NavigationProperty::new(set.clone(), TypeRef::collection(target.clone()));
                    nav.nullable = false;
                    ty.navigation_properties.push(nav);
                }
            } else if let Some((_, _, key)) = navigations
                .iter()
                .find(|(_, target, _)| target.rsplit('.').next() == Some(ty.name.as_str()))
            {
                ty.key = vec![PROJECT.into(), key.clone()];
            }
        }
        let mut person = EntityType::new("Person");
        person.key = vec!["subject".into()];
        person.properties = vec![
            Property::new("subject", TypeRef::single("Edm.String")).required(),
            Property::new("username", TypeRef::single("Edm.String")),
            Property::new("name", TypeRef::single("Edm.String")),
            Property::new("email", TypeRef::single("Edm.String")),
        ];
        let mut organization = EntityType::new(ORGANIZATION);
        organization.key = vec!["name".into()];
        organization.properties = vec![
            Property::new("name", TypeRef::single("Edm.String")).required(),
            Property::new("title", TypeRef::single("Edm.String")),
        ];
        schema.entity_types.push(person);
        schema.entity_types.push(organization);
        if let Some(container) = schema.entity_container.as_mut() {
            for set in &mut container.entity_sets {
                if set.name == PROJECTS {
                    set.bindings = navigations
                        .iter()
                        .map(|(nav, _, _)| NavigationPropertyBinding {
                            path: nav.clone(),
                            target: nav.clone(),
                        })
                        .collect();
                }
            }
        }
    }
    model
}

/// The public model with only the sets in `visible`: a kind the caller reads in none of their
/// projects is no entity set, no entity type and no navigation of `Project` (API/13 §2a).
fn visible_model(visible: &BTreeSet<String>) -> Model {
    let hidden: Vec<(&str, &str)> = kinds()
        .map(|set| (set.set.as_str(), set.type_name.as_str()))
        .chain(org_kinds().map(|(set, type_name, _)| (set, type_name)))
        .filter(|(set, _)| !visible.contains(*set))
        .collect();
    let mut model = (*ORG.public).clone();
    for schema in &mut model.schemas {
        schema
            .entity_types
            .retain(|ty| !hidden.iter().any(|(_, name)| ty.name == *name));
        for ty in &mut schema.entity_types {
            ty.navigation_properties
                .retain(|nav| !hidden.iter().any(|(set, _)| nav.name == *set));
        }
        if let Some(container) = schema.entity_container.as_mut() {
            container
                .entity_sets
                .retain(|set| !hidden.iter().any(|(name, _)| set.name == *name));
            for set in &mut container.entity_sets {
                set.bindings
                    .retain(|b| !hidden.iter().any(|(name, _)| b.path == *name));
            }
        }
    }
    model
}

/// The sets the caller reads in at least one of `projects`: `projects` always, each kind by its
/// door's own read check ([`ListSet::readable`]).
fn visible_sets(
    state: &AppState,
    user: &CurrentUser,
    org: &str,
    projects: &[String],
) -> BTreeSet<String> {
    let mut visible: BTreeSet<String> = kinds()
        .filter(|set| {
            projects
                .iter()
                .any(|project| set.readable(state, &user.0.identity, project))
        })
        .map(|set| set.set.clone())
        .collect();
    visible.extend(
        org_kinds()
            .filter(|(.., set)| org_set_readable(state, user, org, set))
            .map(|(name, ..)| name.to_owned()),
    );
    visible.insert(PROJECTS.into());
    visible
}

/// CSDL XML of `model`, the singletons added to its container.
fn metadata_xml(model: &Model) -> Result<String, String> {
    let mut xml = Vec::new();
    odata_csdl::write_xml(model, &odata_csdl::Limits::strict(), &mut xml)
        .map_err(|e| e.to_string())?;
    String::from_utf8(xml)
        .map_err(|e| e.to_string())
        .map(|xml| with_singletons_xml(&xml))
}

/// The singletons in CSDL XML, beside odata-csdl's container, which has no singleton yet.
// ponytail: text beside the model as the actions are; a model member once odata-csdl has one.
fn with_singletons_xml(xml: &str) -> String {
    let singletons = format!(
        r#"<Singleton Name="{ME}" Type="{NAMESPACE}.Person"/><Singleton Name="{ORGANIZATION}" Type="{NAMESPACE}.{ORGANIZATION}"/>"#
    );
    match xml.rfind("</EntityContainer>") {
        Some(at) => {
            let mut out = xml.to_owned();
            out.insert_str(at, &singletons);
            out
        }
        None => xml.to_owned(),
    }
}

/// CSDL JSON of the public model, the singletons added to its container.
fn metadata_json(model: &Model) -> Result<String, String> {
    let mut out = Vec::new();
    odata_csdl::write_json(model, &odata_csdl::Limits::strict(), &mut out)
        .map_err(|e| e.to_string())?;
    let mut doc: Value = serde_json::from_slice(&out).map_err(|e| e.to_string())?;
    if let Some(container) = doc
        .get_mut(NAMESPACE)
        .and_then(|schema| schema.get_mut("Service"))
        .and_then(Value::as_object_mut)
    {
        container.insert(ME.into(), json!({ "$Type": format!("{NAMESPACE}.Person") }));
        container.insert(
            ORGANIZATION.into(),
            json!({ "$Type": format!("{NAMESPACE}.{ORGANIZATION}") }),
        );
    }
    Ok(doc.to_string())
}

/// An OData error object with API/09's problem members inside (API/13 §2a).
fn error(status: StatusCode, slug: &str, message: &str) -> Response {
    let title = status.canonical_reason().unwrap_or("Error");
    let body = json!({ "error": {
        "code": slug,
        "message": message,
        "type": crate::error::type_uri(slug),
        "title": title,
        "status": status.as_u16(),
    }});
    (status, headers("application/json"), body.to_string()).into_response()
}

fn slug_of(status: StatusCode) -> &'static str {
    match status {
        StatusCode::NOT_FOUND => "resource-not-found",
        StatusCode::UNAUTHORIZED => "unauthorized",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_IMPLEMENTED => "not-implemented",
        StatusCode::PRECONDITION_FAILED => "precondition-failed",
        StatusCode::CONFLICT => "conflict",
        s if s.is_server_error() => "internal-error",
        _ => "invalid-request",
    }
}

fn refused(r: Refusal) -> Response {
    error(r.0, slug_of(r.0), &r.2)
}

/// A refusal on the way to the answer, boxed: a response is large.
type Refused = Box<Response>;

fn no(r: Refusal) -> Refused {
    Box::new(refused(r))
}

/// A refusal of another door (the operation's problem+json, the session's 401) as the OData
/// error object, its `type` kept.
async fn as_odata_error(response: Response) -> Response {
    let status = response.status();
    let etag = response.headers().get(header::ETAG).cloned();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap_or_default();
    let problem: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let message = ["detail", "title"]
        .iter()
        .find_map(|m| problem.get(*m).and_then(Value::as_str))
        .or_else(|| problem.pointer("/error/message").and_then(Value::as_str))
        .unwrap_or("refused")
        .to_owned();
    let slug = problem
        .get("type")
        .and_then(Value::as_str)
        .and_then(|t| t.rsplit('/').next())
        .unwrap_or_else(|| slug_of(status))
        .to_owned();
    let mut answer = error(status, &slug, &message);
    if let Some(etag) = etag {
        answer.headers_mut().insert(header::ETAG, etag);
    }
    answer
}

fn headers(content_type: &'static str) -> [(header::HeaderName, HeaderValue); 3] {
    [
        (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
        (
            header::HeaderName::from_static("odata-version"),
            HeaderValue::from_static("4.01"),
        ),
        (header::CACHE_CONTROL, HeaderValue::from_static("private")),
    ]
}

const JSON: &str = "application/json;odata.metadata=minimal";

/// The Organization of the running request: its pin, else the installation's when it holds one.
fn organization(state: &AppState) -> Option<String> {
    crate::organization_host::organization().or_else(|| {
        (!crate::permissions::several_organizations(&state.mirror))
            .then(|| crate::permissions::installation_organization(&state.mirror))
            .flatten()
    })
}

/// The public root of the service: the Organization host's base (the registry's, never the
/// request's `Host` or `X-Forwarded-Host`) followed by `api/v1/`.
fn service_root(state: &AppState) -> String {
    format!(
        "{}/api/v1/",
        crate::organization_host::public_base(&state.config)
            .as_str()
            .trim_end_matches('/')
    )
}

/// Whether `project` is the Organization's and the caller may read it.
fn readable(state: &AppState, user: &CurrentUser, org: &str, project: &str) -> bool {
    is_dns1123(project)
        && crate::permissions::organization_of(&state.mirror, project).as_deref() == Some(org)
        && crate::api::projects::readable(state, &user.0.identity, project)
}

fn project_row(state: &AppState, name: &str, org: &str) -> Value {
    let manifest = state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Project", name);
    let sample = manifest.as_ref().is_some_and(|p| {
        p.metadata
            .labels
            .get("joinedcontext.com/sample")
            .map(String::as_str)
            == Some("true")
    });
    json!({
        "name": name,
        "title": manifest.and_then(|p| p.metadata.title),
        "organization": org,
        "sample": sample,
    })
}

/// The caller's rows of one kind in one project, each with its project and evaluator key.
async fn kind_rows(
    set: &ListSet,
    caller: &Caller,
    state: &AppState,
    project: &str,
) -> Result<Vec<Value>, Refused> {
    let rows = rows_of(set, caller, state, project).await?;
    // A kind that lives in the project or the Organization (Role): the registry answers both
    // homes inside a project (PF-68), but a row keyed (project,name) is the project's own copy,
    // as the REST list answers it; the Organization's are no row of any project (T-6817).
    let own_only = !set.endpoint_list
        && crate::resource::by_kind(&set.type_name)
            .is_some_and(|info| matches!(info.scope, jc_core::Scope::OrganizationOrProject));
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let Value::Object(mut row) = row else {
                return None;
            };
            let key = row.get(set.key).map(|k| match k {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })?;
            if own_only && state.mirror.get(project, &set.type_name, &key).is_none() {
                return None;
            }
            row.insert(PROJECT.into(), Value::String(project.to_owned()));
            row.insert(ROW_KEY.into(), Value::String(format!("{project}/{key}")));
            let tag = etag_of(state, set, project, &key, &row);
            row.insert(ETAG.into(), Value::String(tag));
            Some(Value::Object(row))
        })
        .collect())
}

/// The member every entity carries its `ETag` in (API/13 §2a).
const ETAG: &str = "@odata.etag";

/// An entity's `ETag`: a manifest's own (API/13 §3, what the REST door compares), else a SHA-256
/// of the row the caller reads (a Change, a Workspace, an Endpoints page row).
fn etag_of(
    state: &AppState,
    set: &ListSet,
    project: &str,
    key: &str,
    row: &Map<String, Value>,
) -> String {
    if !set.endpoint_list && crate::resource::by_kind(&set.type_name).is_some() {
        if let Some(manifest) = state.mirror.get(project, &set.type_name, key) {
            return manifest.etag();
        }
    }
    use sha2::Digest;
    let mut visible = row.clone();
    visible.remove(ROW_KEY);
    visible.remove(ETAG);
    let digest = Sha256::digest(Value::Object(visible).to_string().as_bytes());
    format!(
        "\"{}\"",
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

/// The `ETag` of each evaluator key in `rows`, for the answer after its projection.
fn etags(rows: &[Value]) -> std::collections::HashMap<String, Value> {
    rows.iter()
        .filter_map(|row| {
            Some((
                row.get(ROW_KEY)?.as_str()?.to_owned(),
                row.get(ETAG)?.clone(),
            ))
        })
        .collect()
}

/// Puts each answered row's `ETag` back, by its key: `$select` never drops it.
fn tag(answer: &mut Answer, tags: &std::collections::HashMap<String, Value>) {
    for (row, key) in answer.rows.iter_mut().zip(&answer.keys) {
        let tag = key.as_str().and_then(|k| tags.get(k));
        if let (Value::Object(row), Some(tag)) = (row, tag) {
            row.insert(ETAG.into(), tag.clone());
        }
    }
}

fn strip_key(rows: &mut [Value]) {
    for row in rows {
        if let Value::Object(row) = row {
            row.remove(ROW_KEY);
        }
    }
}

fn too_many(set: &str) -> Refused {
    no(bad(format!(
        "{set} holds more than {MAX_ROWS} rows you may read here, more than one OData read covers; \
         narrow it with $filter on a project, e.g. projects('air')/{set}"
    )))
}

/// The `$skiptoken` key: minted once per process, so a token outlives no restart (the client
/// starts its read again).
// ponytail: one key per process; a shared key from a secretRef when the Portal runs replicas.
static TOKEN_KEY: LazyLock<Vec<u8>> = LazyLock::new(|| crate::auth::csrf::new_token().into_bytes());

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn token_mac(org: &str, subject: &str, query: &str, inner: &str, expiry: u64) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&TOKEN_KEY).expect("any key length");
    for part in [org, subject, query, inner, &expiry.to_string()] {
        mac.update(part.as_bytes());
        mac.update(&[0]);
    }
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The canonical text of a read without its `$skiptoken`: what a token is signed over.
fn canonical(path: &[Segment], options: &QueryOptions) -> String {
    let mut options = options.clone();
    options.skiptoken = None;
    ODataUri::Resource {
        path: path.to_vec(),
        options,
    }
    .to_string()
}

fn sign(org: &str, subject: &str, query: &str, inner: &str) -> String {
    let expiry = now() + TOKEN_TTL_SECS;
    format!(
        "{inner}.{expiry}.{}",
        token_mac(org, subject, query, inner, expiry)
    )
}

/// The evaluator's token inside a signed one, if this service wrote it for this caller and read
/// and it has not expired.
fn verify(org: &str, subject: &str, query: &str, token: &str) -> Option<String> {
    let mut parts = token.rsplitn(3, '.');
    let (tag, expiry, inner) = (parts.next()?, parts.next()?, parts.next()?);
    let expiry: u64 = expiry.parse().ok()?;
    let want = token_mac(org, subject, query, inner, expiry);
    let same = crate::auth::csrf::constant_time_eq(&want, tag);
    (same && expiry >= now()).then(|| inner.to_owned())
}

/// The read the caller asked: the target below the root, its query, how many rows a page holds.
struct Read<'a> {
    state: &'a AppState,
    user: &'a CurrentUser,
    caller: Caller,
    org: String,
    page: u64,
}

impl Read<'_> {
    fn table(&self, set: &str, rows: Vec<Value>) -> Result<MemoryTable, Refused> {
        let info = ORG
            .service
            .info(ORG.evaluated.clone(), set)
            .ok_or_else(|| no(not_found(format!("no entity set {set}"))))?;
        MemoryTable::new(info, rows).map_err(|e| no(Refusal::from(e)))
    }

    /// Every project of the Organization the caller may read, as rows of `projects`.
    fn projects(&self) -> Vec<Value> {
        self.state
            .mirror
            .namespaces()
            .into_iter()
            .filter(|name| readable(self.state, self.user, &self.org, name))
            .map(|name| project_row(self.state, &name, &self.org))
            .collect()
    }

    /// The caller's rows of `set` in `projects`, at most [`MAX_ROWS`].
    async fn rows(&self, set: &ListSet, projects: &[String]) -> Result<Vec<Value>, Refused> {
        let mut all = Vec::new();
        for project in projects {
            match kind_rows(set, &self.caller, self.state, project).await {
                Ok(rows) => all.extend(rows),
                // A kind the caller may not list in a project they read is no rows of theirs.
                Err(r) if matches!(r.status(), StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) => {}
                Err(r) => return Err(r),
            }
            if all.len() > MAX_ROWS {
                return Err(too_many(&set.set));
            }
        }
        Ok(all)
    }

    async fn collection(
        &self,
        set: &str,
        context: &str,
        path: &[Segment],
        options: &QueryOptions,
        rows: Vec<Value>,
    ) -> Result<(Answer, Value), Refused> {
        let tags = etags(&rows);
        let table = self.table(set, rows)?;
        let query_text = canonical(path, options);
        let mut options = options.clone();
        if let Some(token) = options.skiptoken.take() {
            let inner = verify(
                &self.org,
                &self.caller.identity.subject,
                &query_text,
                &token,
            )
            .ok_or_else(|| {
                refused(bad(
                    "the $skiptoken is not one this service wrote for you and this read, or \
                         it expired; read the first page again",
                ))
            })?;
            options.skiptoken = Some(inner);
        }
        let mut answer = query(&table, &options, &Scope::all(), self.page)
            .await
            .map_err(|e| no(Refusal::from(e)))?;
        tag(&mut answer, &tags);
        strip_key(&mut answer.rows);
        let mut body = Map::new();
        let root = service_root(self.state);
        let context = match &answer.select {
            Some(names) => format!("{root}$metadata#{context}({})", names.join(",")),
            None => format!("{root}$metadata#{context}"),
        };
        body.insert("@odata.context".into(), Value::String(context));
        if let Some(count) = answer.count {
            body.insert("@odata.count".into(), json!(count));
        }
        if let Some(next) = answer.next.as_ref() {
            let mut next_options = options.clone();
            next_options.skiptoken = Some(sign(
                &self.org,
                &self.caller.identity.subject,
                &query_text,
                &next.to_string(),
            ));
            let next = ODataUri::Resource {
                path: path.to_vec(),
                options: next_options,
            };
            body.insert(
                "@odata.nextLink".into(),
                Value::String(format!("{root}{next}")),
            );
        }
        Ok((answer, Value::Object(body)))
    }
}

fn single_key(key: &Key) -> Result<Value, Refused> {
    match key {
        Key::Single(KeyValue::Literal(literal)) => key_value(literal).map_err(no),
        _ => Err(no(bad(
            "a project is keyed by its name, e.g. projects('air')",
        ))),
    }
}

fn kind(name: &str) -> Option<&'static ListSet> {
    kinds().find(|set| set.set == name)
}

/// The navigations `$expand` names: one level on a project's kinds, nothing nested.
fn expansions(options: &QueryOptions) -> Result<Vec<&'static ListSet>, Refused> {
    let Some(items) = &options.expand else {
        return Ok(Vec::new());
    };
    items
        .iter()
        .map(|item| {
            let ExpandPath::Path(path) = &item.path else {
                return Err(no(not_implemented("$expand=*")));
            };
            let [name] = path.as_slice() else {
                return Err(no(bad(
                    "$expand names one navigation of a project, e.g. endpoints",
                )));
            };
            let set = kind(name).ok_or_else(|| {
                refused(bad(format!(
                    "a project has no navigation {name}; $metadata lists them"
                )))
            })?;
            match &item.kind {
                ExpandKind::Ref => Err(no(not_implemented("$expand with /$ref"))),
                ExpandKind::Entities {
                    expand: Some(_), ..
                } => Err(no(bad("$expand goes one level deep"))),
                ExpandKind::Entities {
                    levels: Some(_), ..
                } => Err(no(not_implemented("$levels"))),
                ExpandKind::Entities {
                    select: Some(_), ..
                } => Err(no(not_implemented("options inside $expand"))),
                ExpandKind::Entities { .. } => Ok(set),
            }
        })
        .collect()
}

/// Puts each expanded kind's rows under its navigation in each project row.
async fn expand(
    read: &Read<'_>,
    rows: &mut [Value],
    names: &[Value],
    sets: &[&'static ListSet],
) -> Result<(), Refused> {
    let mut total = 0;
    for (row, name) in rows.iter_mut().zip(names) {
        let Some(project) = name.get("name").and_then(Value::as_str) else {
            continue;
        };
        for set in sets {
            let mut related = read.rows(set, &[project.to_owned()]).await?;
            total += related.len();
            if total > MAX_ROWS {
                return Err(too_many(&set.set));
            }
            strip_key(&mut related);
            if let Value::Object(row) = row {
                row.insert(set.set.clone(), Value::Array(related));
            }
        }
    }
    Ok(())
}

fn ok_json(body: &Value) -> Response {
    (StatusCode::OK, headers(JSON), body.to_string()).into_response()
}

fn count_text(n: u64) -> Response {
    (StatusCode::OK, headers("text/plain"), n.to_string()).into_response()
}

/// Options asking only for the count of what `options` filters.
fn counting(options: &QueryOptions) -> QueryOptions {
    QueryOptions {
        filter: options.filter.clone(),
        aliases: options.aliases.clone(),
        top: Some(0),
        count: Some(true),
        ..QueryOptions::default()
    }
}

/// One entity: the first row of `answer`, else 404.
fn entity(root: &str, context: &str, answer: Answer, what: &str) -> Response {
    let Some(Value::Object(row)) = answer.rows.into_iter().next() else {
        return refused(not_found(format!("no {what} you may read")));
    };
    let mut body = Map::new();
    body.insert(
        "@odata.context".into(),
        Value::String(format!("{root}$metadata#{context}/$entity")),
    );
    let tag = row.get(ETAG).and_then(Value::as_str).map(str::to_owned);
    body.extend(row);
    let mut response = ok_json(&Value::Object(body));
    if let Some(tag) = tag.and_then(|t| HeaderValue::from_str(&t).ok()) {
        response.headers_mut().insert(header::ETAG, tag);
    }
    response
}

async fn by_key(
    read: &Read<'_>,
    set: &str,
    rows: Vec<Value>,
    key: Value,
    options: &QueryOptions,
    context: &str,
    what: &str,
) -> Response {
    let tags = etags(&rows);
    let table = match read.table(set, rows) {
        Ok(table) => table,
        Err(r) => return *r,
    };
    let answered = match key_options(table.info(), &key, options.select.clone()) {
        Ok(by) => query(&table, &by, &Scope::all(), 1).await,
        Err(e) => Err(e),
    };
    match answered {
        Ok(mut answer) => {
            tag(&mut answer, &tags);
            strip_key(&mut answer.rows);
            entity(&service_root(read.state), context, answer, what)
        }
        Err(e) => refused(Refusal::from(e)),
    }
}

/// `projects/air/endpoints` read as `projects('air')/endpoints` (key-as-segment, API/13 §2a).
fn key_as_segment(target: &str) -> Result<String, Refused> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let Some(rest) = path.strip_prefix("projects/") else {
        return Ok(target.to_owned());
    };
    if rest.starts_with('$') {
        return Ok(target.to_owned());
    }
    let (project, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if !is_dns1123(project) {
        return Err(no(not_found(format!("no project {project} you may read"))));
    }
    let mut out = format!("projects('{project}')");
    if !tail.is_empty() {
        out.push('/');
        out.push_str(&key_as_segment_kind(tail));
    }
    if !query.is_empty() {
        out.push('?');
        out.push_str(query);
    }
    Ok(out)
}

/// `endpoints/counters` read as `endpoints('counters')`.
fn key_as_segment_kind(tail: &str) -> String {
    match tail.split_once('/') {
        Some((set, key)) if kind(set).is_some() && is_dns1123(key) => format!("{set}('{key}')"),
        _ => tail.to_owned(),
    }
}

/// The value of a named member of a compound key.
fn compound(pairs: &[(String, KeyValue)], name: &str) -> Option<Value> {
    pairs
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, v)| match v {
            KeyValue::Literal(Literal::String(s)) => Some(Value::String(s.clone())),
            KeyValue::Literal(Literal::Int(i)) => Some(json!(i)),
            _ => None,
        })
}

/// The answer to one resource path below the root.
async fn resource(read: &Read<'_>, target: &str) -> Response {
    let target = match key_as_segment(target) {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let parsed = match odata_url::parse(&target, &EdmModel(&ORG.public), &limits()) {
        Ok(parsed) => parsed,
        Err(e) => {
            return refused(match e.kind {
                UrlErrorKind::NotImplemented(_) => not_implemented(&e.to_string()),
                UrlErrorKind::Unresolved => {
                    let name = target.split(['(', '/', '?']).next().unwrap_or_default();
                    if name != PROJECTS && kind(name).is_none() && org_set(name).is_none() {
                        not_found(format!(
                            "no entity set {name}; $metadata lists the entity sets"
                        ))
                    } else {
                        bad(format!(
                            "{e}: $metadata lists the entity sets and their properties"
                        ))
                    }
                }
                _ => bad(e.to_string()),
            })
        }
    };
    let ODataUri::Resource { path, options } = parsed else {
        return refused(not_implemented("this resource"));
    };
    let sets = match expansions(&options) {
        Ok(sets) => sets,
        Err(r) => return *r,
    };
    let mut plain = options.clone();
    plain.expand = None;
    match path.as_slice() {
        [Segment::EntitySet(s)] | [Segment::EntitySet(s), Segment::Count] if s == PROJECTS => {
            let rows = read.projects();
            if matches!(path.last(), Some(Segment::Count)) {
                return match read
                    .collection(PROJECTS, PROJECTS, &path, &counting(&plain), rows)
                    .await
                {
                    Ok((answer, _)) => count_text(answer.count.unwrap_or(0)),
                    Err(r) => *r,
                };
            }
            let (answer, mut body) = match read
                .collection(PROJECTS, PROJECTS, &path, &plain, rows.clone())
                .await
            {
                Ok(done) => done,
                Err(r) => return *r,
            };
            let mut value = answer.rows;
            if !sets.is_empty() {
                // The same page again, all properties, to know each row's project.
                let mut named = plain.clone();
                named.select = None;
                let names = match read
                    .collection(PROJECTS, PROJECTS, &path, &named, rows)
                    .await
                {
                    Ok((a, _)) => a.rows,
                    Err(r) => return *r,
                };
                if let Err(r) = expand(read, &mut value, &names, &sets).await {
                    return *r;
                }
            }
            body["value"] = Value::Array(value);
            ok_json(&body)
        }
        [Segment::EntitySet(s), Segment::Key(key)] if s == PROJECTS => {
            let name = match single_key(key) {
                Ok(name) => name,
                Err(r) => return *r,
            };
            let project = name.as_str().unwrap_or_default().to_owned();
            if !readable(read.state, read.user, &read.org, &project) {
                return refused(not_found(format!("no project {project} you may read")));
            }
            let row = project_row(read.state, &project, &read.org);
            let etag = project_etag(read.state, &project, &read.org);
            let mut answer = by_key(
                read,
                PROJECTS,
                vec![row.clone()],
                name,
                &plain,
                PROJECTS,
                "project",
            )
            .await;
            if answer.status() == StatusCode::OK && !sets.is_empty() {
                let bytes = axum::body::to_bytes(answer.into_body(), usize::MAX)
                    .await
                    .unwrap_or_default();
                let mut body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                if let Err(r) = expand(read, std::slice::from_mut(&mut body), &[row], &sets).await {
                    return *r;
                }
                answer = ok_json(&body);
            }
            // The project's ETag, which a Change-proposing action on it sends as If-Match.
            if answer.status() == StatusCode::OK {
                let bytes = axum::body::to_bytes(answer.into_body(), usize::MAX)
                    .await
                    .unwrap_or_default();
                let mut body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                if let Value::Object(members) = &mut body {
                    members.insert(ETAG.into(), Value::String(etag.clone()));
                }
                answer = ok_json(&body);
                if let Ok(tag) = HeaderValue::from_str(&etag) {
                    answer.headers_mut().insert(header::ETAG, tag);
                }
            }
            answer
        }
        [Segment::EntitySet(s), Segment::Key(key), Segment::Navigation(nav), rest @ ..]
            if s == PROJECTS =>
        {
            if !sets.is_empty() {
                return refused(bad("$expand goes from a project; a kind has no navigation"));
            }
            let project = match single_key(key) {
                Ok(Value::String(p)) => p,
                Ok(_) => return refused(bad("a project is keyed by its name")),
                Err(r) => return *r,
            };
            let Some(set) = kind(nav) else {
                return refused(not_found(format!("a project has no {nav}")));
            };
            if !readable(read.state, read.user, &read.org, &project) {
                return refused(not_found(format!("no project {project} you may read")));
            }
            let rows = match kind_rows(set, &read.caller, read.state, &project).await {
                Ok(rows) => rows,
                Err(r) => return *r,
            };
            let context = format!("{PROJECTS}('{project}')/{nav}");
            match rest {
                [] => collection_answer(read, nav, &context, &path, &plain, rows).await,
                [Segment::Count] => {
                    match read
                        .collection(nav, &context, &path, &counting(&plain), rows)
                        .await
                    {
                        Ok((answer, _)) => count_text(answer.count.unwrap_or(0)),
                        Err(r) => *r,
                    }
                }
                [Segment::Key(Key::Single(KeyValue::Literal(literal)))] => {
                    let Ok(name) = key_value(literal) else {
                        return refused(bad("a key is a string, e.g. ('counters')"));
                    };
                    let key = json!(format!(
                        "{project}/{}",
                        name.as_str()
                            .map_or_else(|| name.to_string(), str::to_owned)
                    ));
                    by_key(read, nav, rows, key, &plain, nav, set.type_name.as_str()).await
                }
                _ => refused(not_implemented("this resource path")),
            }
        }
        [Segment::EntitySet(s), rest @ ..] if kind(s).is_some() => {
            if !sets.is_empty() {
                return refused(bad("$expand goes from a project; a kind has no navigation"));
            }
            let Some(set) = kind(s) else {
                return refused(not_found(format!("no entity set {s}")));
            };
            match rest {
                [] | [Segment::Count] => {
                    let projects: Vec<String> = read
                        .projects()
                        .iter()
                        .filter_map(|p| p.get("name").and_then(Value::as_str).map(str::to_owned))
                        .collect();
                    let rows = match read.rows(set, &projects).await {
                        Ok(rows) => rows,
                        Err(r) => return *r,
                    };
                    if rest.is_empty() {
                        collection_answer(read, s, s, &path, &plain, rows).await
                    } else {
                        match read.collection(s, s, &path, &counting(&plain), rows).await {
                            Ok((answer, _)) => count_text(answer.count.unwrap_or(0)),
                            Err(r) => *r,
                        }
                    }
                }
                [Segment::Key(Key::Compound(pairs))] => {
                    let (Some(Value::String(project)), Some(name)) =
                        (compound(pairs, PROJECT), compound(pairs, set.key))
                    else {
                        return refused(bad(format!(
                            "{s} is keyed by (project,{}), e.g. {s}(project='air',{}='x')",
                            set.key, set.key
                        )));
                    };
                    if !readable(read.state, read.user, &read.org, &project) {
                        return refused(not_found(format!("no {s} row you may read")));
                    }
                    let rows = match kind_rows(set, &read.caller, read.state, &project).await {
                        Ok(rows) => rows,
                        Err(r) => return *r,
                    };
                    let key = json!(format!(
                        "{project}/{}",
                        name.as_str()
                            .map_or_else(|| name.to_string(), str::to_owned)
                    ));
                    by_key(read, s, rows, key, &plain, s, set.type_name.as_str()).await
                }
                [Segment::Key(_)] => refused(bad(format!(
                    "{s} is keyed by (project,{}), e.g. {s}(project='air',{}='x')",
                    set.key, set.key
                ))),
                _ => refused(not_implemented("this resource path")),
            }
        }
        [Segment::EntitySet(s), rest @ ..] if org_set(s).is_some() => {
            if !sets.is_empty() {
                return refused(bad("$expand goes from a project; a kind has no navigation"));
            }
            let rows = match org_rows(read, s) {
                Ok(rows) => rows,
                Err(r) => return *r,
            };
            match rest {
                [] => collection_answer(read, s, s, &path, &plain, rows).await,
                [Segment::Count] => {
                    match read.collection(s, s, &path, &counting(&plain), rows).await {
                        Ok((answer, _)) => count_text(answer.count.unwrap_or(0)),
                        Err(r) => *r,
                    }
                }
                [Segment::Key(key)] => {
                    let name = match single_key(key) {
                        Ok(Value::String(name)) => name,
                        Ok(_) | Err(_) => {
                            return refused(bad(format!("{s} is keyed by name, e.g. {s}('x')")))
                        }
                    };
                    by_key(read, s, rows, json!(name), &plain, s, s).await
                }
                _ => refused(not_implemented("this resource path")),
            }
        }
        _ => refused(not_implemented("this resource path")),
    }
}

/// The caller's rows of the Organization set `name`: the Organization's own manifests of its
/// kind in the `org` namespace, each read under the caller's right at the Organization (a
/// constrained grant per row), shaped as the REST list's rows and keyed by name.
fn org_rows(read: &Read<'_>, name: &str) -> Result<Vec<Value>, Refused> {
    let Some(set) = org_set(name) else {
        return Err(no(not_found(format!("no entity set {name}"))));
    };
    if !org_set_readable(read.state, read.user, &read.org, set) {
        return Err(no(not_found(format!("no {name} you may read"))));
    }
    let (state, kind) = (read.state, set.type_name.as_str());
    let rows: Vec<Value> = state
        .mirror
        .list(ORG_NAMESPACE, kind, &crate::store::ListOptions::default())
        .items
        .into_iter()
        .chain(state.mirror.refused_of(ORG_NAMESPACE, kind))
        .filter(|env| owned_by(state, &read.org, kind, &env.metadata.name))
        .filter(|env| {
            crate::permissions::holds_at_organization(
                state,
                &read.caller.identity,
                &read.org,
                kind,
                jc_core::kinds::Verb::Read,
                serde_json::to_value(env).ok().as_ref(),
            )
        })
        .map(|env| {
            let mut row = crate::api::odata::flat(crate::ops::resources::row(&env));
            row[ROW_KEY] = Value::String(env.metadata.name.clone());
            row[ETAG] = Value::String(env.etag());
            row
        })
        .collect();
    if rows.len() > MAX_ROWS {
        return Err(too_many(name));
    }
    Ok(rows)
}

async fn collection_answer(
    read: &Read<'_>,
    set: &str,
    context: &str,
    path: &[Segment],
    options: &QueryOptions,
    rows: Vec<Value>,
) -> Response {
    match read.collection(set, context, path, options, rows).await {
        Ok((answer, mut body)) => {
            body["value"] = Value::Array(answer.rows);
            ok_json(&body)
        }
        Err(r) => *r,
    }
}

/// The page size: `Prefer: odata.maxpagesize=n`, at most [`MAX_TOP`].
fn page_size(headers: &HeaderMap) -> (u64, Option<u64>) {
    let asked = headers
        .get_all("prefer")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .find_map(|p| p.trim().strip_prefix("odata.maxpagesize="))
        .and_then(|n| n.trim().parse::<u64>().ok())
        .filter(|n| *n >= 1);
    match asked {
        Some(n) => (n.min(MAX_TOP), Some(n.min(MAX_TOP))),
        None => (MAX_TOP, None),
    }
}

/// Whether the client takes JSON (`metadata`: XML too).
fn accepts(headers: &HeaderMap, xml: bool) -> bool {
    let Some(accept) = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    accept.split(',').any(|range| {
        let media = range.split(';').next().unwrap_or_default().trim();
        matches!(media, "*/*" | "application/*" | "application/json")
            || (xml && matches!(media, "application/xml" | "text/xml" | "text/*"))
    })
}

/// The query's options by name, `%24` read as `$`.
fn option_names(query: &str) -> Vec<String> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let name = p.split('=').next().unwrap_or_default();
            name.replacen("%24", "$", 1).to_ascii_lowercase()
        })
        .collect()
}

fn top_of(query: &str) -> Option<&str> {
    query.split('&').find_map(|p| {
        let (n, v) = p.split_once('=')?;
        n.replacen("%24", "$", 1)
            .eq_ignore_ascii_case("$top")
            .then_some(v)
    })
}

/// A refusal of the query's options before anything is read: the unserved ones 501, a custom one
/// 400, a format other than JSON 406.
fn refuse_options(query: &str, metadata: bool) -> Option<Response> {
    for name in option_names(query) {
        if REFUSED.contains(&name.as_str()) {
            return Some(refused(not_implemented(&name)));
        }
        if name == "$top"
            && top_of(query).is_some_and(|top| top.parse::<u64>().map_or(true, |n| n > MAX_TOP))
        {
            return Some(refused(bad(format!(
                "$top is at most {MAX_TOP}; the next rows follow with @odata.nextLink"
            ))));
        }
        if !name.starts_with('$') && !name.starts_with('@') {
            return Some(refused(bad(format!(
                "this service takes no option {name}; it answers $filter, $select, $orderby, \
                 $top, $skip, $count and $expand"
            ))));
        }
    }
    let format = query
        .split('&')
        .find_map(|p| {
            let (n, v) = p.split_once('=')?;
            (n.replacen("%24", "$", 1).eq_ignore_ascii_case("$format")).then_some(v)
        })
        .map(|v| {
            v.replace("%2F", "/")
                .replace("%2f", "/")
                .to_ascii_lowercase()
        });
    match format.as_deref() {
        None => None,
        Some(f) if f == "json" || f.starts_with("application/json") => None,
        Some(f) if metadata && (f == "xml" || f.starts_with("application/xml")) => None,
        Some(_) => Some(error(
            StatusCode::NOT_ACCEPTABLE,
            "invalid-request",
            "this service answers JSON ($metadata also XML); ask $format=json",
        )),
    }
}

/// The service document: the sets and singletons.
fn service_document(state: &AppState) -> Response {
    let mut value = vec![json!({ "name": PROJECTS, "kind": "EntitySet", "url": PROJECTS })];
    value
        .extend(kinds().map(|set| json!({ "name": set.set, "kind": "EntitySet", "url": set.set })));
    value.extend(
        org_kinds().map(|(set, ..)| json!({ "name": set, "kind": "EntitySet", "url": set })),
    );
    for singleton in [ME, ORGANIZATION] {
        value.push(json!({ "name": singleton, "kind": "Singleton", "url": singleton }));
    }
    ok_json(&json!({
        "@odata.context": format!("{}$metadata", service_root(state)),
        "value": value,
    }))
}

/// What the caller may do anywhere in the Organization: what their `$metadata` offers.
#[derive(Clone, Copy, Debug, Default)]
struct Rights {
    propose: bool,
    approve: bool,
}

fn rights(state: &AppState, user: &CurrentUser, org: &str) -> Rights {
    let mut rights = Rights::default();
    for project in state.mirror.namespaces() {
        if readable(state, user, org, &project) {
            let effective = crate::permissions::for_request(state, &user.0.identity, &project);
            rights.propose |= effective.may_propose_anything();
            rights.approve |= effective.may_anything(jc_core::kinds::Verb::Approve);
        }
    }
    rights
}

const CHANGES: &str = "changes";

/// An action, its binding type and its parameters.
type BoundAction = (
    &'static str,
    String,
    &'static [(&'static str, &'static str)],
);

/// What an action is bound to: its project, and the kind and key of an entity in it.
type Binding = (String, Option<(&'static ListSet, String)>);

/// The bound actions `rights` allow: `propose_create` on a project, `propose_update` and
/// `propose_delete` on each manifest kind, `approve` and `reject` on a Change. Each is
/// (name, binding type, parameters).
fn bound_actions(rights: Rights, visible: &BTreeSet<String>) -> Vec<BoundAction> {
    let mut out = Vec::new();
    if rights.propose {
        out.push((
            "propose_create",
            format!("{NAMESPACE}.Project"),
            &[("manifest", "Edm.Untyped"), ("dryRun", "Edm.Boolean")][..],
        ));
        for set in kinds().filter(|set| manifest_kind(set) && visible.contains(&set.set)) {
            let ty = format!("{NAMESPACE}.{}", set.type_name);
            out.push((
                "propose_update",
                ty.clone(),
                &[
                    ("manifest", "Edm.Untyped"),
                    ("patch", "Edm.Untyped"),
                    ("dryRun", "Edm.Boolean"),
                ][..],
            ));
            out.push(("propose_delete", ty, &[("dryRun", "Edm.Boolean")][..]));
        }
    }
    if rights.approve {
        if let Some(change) = kind(CHANGES).filter(|_| visible.contains(CHANGES)) {
            let ty = format!("{NAMESPACE}.{}", change.type_name);
            out.push(("approve", ty.clone(), &[("confirm", "Edm.String")][..]));
            out.push(("reject", ty, &[("reason", "Edm.String")][..]));
        }
    }
    out
}

fn manifest_kind(set: &ListSet) -> bool {
    !set.endpoint_list && crate::resource::by_kind(&set.type_name).is_some()
}

/// The CSDL XML of the caller: the bound actions they may call inside the schema.
fn xml_for(
    base: &str,
    rights: Rights,
    visible: &BTreeSet<String>,
    offered: &[&'static crate::ops::Operation],
) -> String {
    let mut actions = odata_ops::xml(offered);
    for (name, binding, parameters) in bound_actions(rights, visible) {
        actions.push_str(&format!(
            r#"<Action Name="{name}" IsBound="true"><Parameter Name="bindingParameter" Type="{binding}" Nullable="false"/>"#
        ));
        for (parameter, ty) in parameters {
            actions.push_str(&format!(r#"<Parameter Name="{parameter}" Type="{ty}"/>"#));
        }
        actions.push_str(r#"<ReturnType Type="Edm.Untyped"/></Action>"#);
    }
    match base.rfind("<EntityContainer") {
        Some(at) => {
            let mut xml = base.to_owned();
            xml.insert_str(at, &actions);
            xml
        }
        None => base.to_owned(),
    }
}

/// The CSDL JSON of the caller: each action's overloads under its name.
fn json_for(
    base: &str,
    rights: Rights,
    visible: &BTreeSet<String>,
    offered: &[&'static crate::ops::Operation],
) -> String {
    let Ok(mut doc) = serde_json::from_str::<Value>(base) else {
        return base.to_owned();
    };
    if let Some(schema) = doc.get_mut(NAMESPACE).and_then(Value::as_object_mut) {
        for (name, overload) in odata_ops::json(offered) {
            schema.insert(name.to_owned(), Value::Array(vec![overload]));
        }
        for (name, binding, parameters) in bound_actions(rights, visible) {
            let mut params =
                vec![json!({ "$Name": "bindingParameter", "$Type": binding, "$Nullable": false })];
            params.extend(
                parameters
                    .iter()
                    .map(|(p, ty)| json!({ "$Name": p, "$Type": ty, "$Nullable": true })),
            );
            let overload = json!({ "$Kind": "Action", "$IsBound": true, "$Parameter": params,
                                   "$ReturnType": { "$Type": "Edm.Untyped" } });
            match schema.get_mut(name).and_then(Value::as_array_mut) {
                Some(overloads) => overloads.push(overload),
                None => {
                    schema.insert(name.to_owned(), Value::Array(vec![overload]));
                }
            }
        }
    }
    doc.to_string()
}

fn metadata(
    headers: &HeaderMap,
    query: &str,
    rights: Rights,
    visible: &BTreeSet<String>,
    offered: &[&'static crate::ops::Operation],
) -> Response {
    let json_asked = query.split('&').any(|p| {
        p.split_once('=').is_some_and(|(n, v)| {
            n.replacen("%24", "$", 1).eq_ignore_ascii_case("$format")
                && (v == "json"
                    || v.to_ascii_lowercase().starts_with("application%2fjson")
                    || v.starts_with("application/json"))
        })
    }) || headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.starts_with("application/json"));
    let model = visible_model(visible);
    let (body, content_type) = if json_asked {
        (metadata_json(&model), "application/json")
    } else {
        (metadata_xml(&model), "application/xml")
    };
    match body {
        Ok(text) => {
            let text = if json_asked {
                json_for(&text, rights, visible, offered)
            } else {
                xml_for(&text, rights, visible, offered)
            };
            (StatusCode::OK, headers_for(content_type), text).into_response()
        }
        Err(e) => {
            tracing::warn!("the Organization OData metadata did not write: {e}");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal-error",
                "the service model could not be written",
            )
        }
    }
}

fn headers_for(content_type: &'static str) -> [(header::HeaderName, HeaderValue); 3] {
    headers(content_type)
}

fn singleton(state: &AppState, user: &CurrentUser, org: &str, name: &str) -> Response {
    let root = service_root(state);
    let body = if name == ME {
        let id = &user.0.identity;
        json!({
            "@odata.context": format!("{root}$metadata#{ME}"),
            "subject": id.subject,
            "username": id.username,
            "name": id.name,
            "email": id.email,
        })
    } else {
        let manifest = state
            .mirror
            .get(crate::permissions::ORG_NAMESPACE, ORGANIZATION, org);
        json!({
            "@odata.context": format!("{root}$metadata#{ORGANIZATION}"),
            "name": org,
            "title": manifest.and_then(|m| m.metadata.title),
        })
    };
    ok_json(&body)
}

/// Serves one OData request below `/api/v1/` (`rel` without that prefix and the leading `/`).
async fn serve(state: AppState, mut parts: axum::http::request::Parts, body: Bytes) -> Response {
    let rel = parts
        .uri
        .path()
        .trim_start_matches("/api/v1")
        .trim_start_matches('/')
        .to_owned();
    let query = parts.uri.query().unwrap_or_default().to_owned();
    if rel == "$batch" {
        return refused(not_implemented("$batch"));
    }
    // `{entity}/JoinedContext.Portal.{name}`: split at the namespace, so a `/` inside a
    // function's quoted literal stays in its call.
    let action = rel.find(&format!("/{ACTION_PREFIX}")).map(|at| {
        (
            rel[..at].to_owned(),
            rel[at + 1 + ACTION_PREFIX.len()..].to_owned(),
        )
    });
    let (action, called) = match (&parts.method, action) {
        (&Method::POST, Some(action)) => (Some(action), None),
        (&Method::GET, Some(function)) => (None, Some(function)),
        (&Method::GET | &Method::HEAD, None) => (None, None),
        _ => {
            let mut r = error(
                StatusCode::METHOD_NOT_ALLOWED,
                "invalid-request",
                "a write is a reviewed Change: POST the action JoinedContext.Portal.propose_create \
                 on its project, propose_update or propose_delete on the entity (with If-Match), \
                 approve or reject on the Change; $metadata lists the ones you may call",
            );
            r.headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
            return r;
        }
    };
    // The API router's own guard, which this layer stands in front of.
    if !crate::auth::csrf::is_allowed(&parts.method, &parts.headers) {
        return as_odata_error(crate::error::ApiError::Forbidden.into_response()).await;
    }
    let user = match CurrentUser::from_request_parts(&mut parts, &state).await {
        Ok(user) => user,
        Err(rejected) => return as_odata_error(rejected.into_response()).await,
    };
    let front = match Front::from_request_parts(&mut parts, &state).await {
        Ok(front) => front,
        Err(rejected) => return as_odata_error(rejected.into_response()).await,
    };
    let Some(org) = organization(&state) else {
        return refused(not_found(
            "this address serves several Organizations; read one at /{org}/api/v1/",
        ));
    };
    if let Some((entity, call)) = called {
        return function(&state, user, front, &org, &entity, &call, &query).await;
    }
    if let Some((entity, name)) = action {
        let call = Call {
            state: &state,
            user,
            front,
            org: &org,
            headers: &parts.headers,
            body,
        };
        return if odata_ops::form(&name).is_some() {
            operation_action(call, &entity, &name).await
        } else {
            act(call, &entity, &name).await
        };
    }
    let metadata_asked = rel == "$metadata";
    if let Some(r) = refuse_options(&query, metadata_asked) {
        return r;
    }
    if !accepts(&parts.headers, metadata_asked) {
        return error(
            StatusCode::NOT_ACCEPTABLE,
            "invalid-request",
            "this service answers application/json ($metadata also application/xml)",
        );
    }
    if metadata_asked {
        let projects = readable_projects(&state, &user, &org);
        let offered = odata_ops::offered(&state, &user, front, &projects);
        return metadata(
            &parts.headers,
            &query,
            rights(&state, &user, &org),
            &visible_sets(&state, &user, &org, &projects),
            &offered,
        );
    }
    if rel.is_empty() {
        return service_document(&state);
    }
    if rel == ME || rel == ORGANIZATION {
        if option_names(&query).iter().any(|n| n != "$format") {
            return refused(bad(format!("{rel} takes no query option")));
        }
        return singleton(&state, &user, &org, &rel);
    }
    let (page, preferred) = page_size(&parts.headers);
    let read = Read {
        state: &state,
        user: &user,
        caller: caller_of(user.clone(), front),
        org,
        page,
    };
    let target = if query.is_empty() {
        rel
    } else {
        format!("{rel}?{query}")
    };
    let mut response = resource(&read, &target).await;
    if let (Some(n), true) = (preferred, response.status().is_success()) {
        if let Ok(v) = HeaderValue::from_str(&format!("odata.maxpagesize={n}")) {
            response.headers_mut().insert("preference-applied", v);
        }
    }
    response
}

/// Whether a request below `/api/v1/` is OData (API/13 §2a compat rule): `OData-Version` or
/// `OData-MaxVersion`, a `$` option, `$metadata`, `$batch`, the service document or a key
/// predicate. The per-project service under `/projects/{project}/odata/` keeps its own routes.
pub fn is_odata(path: &str, query: Option<&str>, headers: &HeaderMap) -> bool {
    let Some(rel) = path.strip_prefix("/api/v1/") else {
        return false;
    };
    // The per-project service keeps its routes; signing in and out stays REST.
    if rel.starts_with("auth/") || rel.split('/').any(|segment| segment == "odata") {
        return false;
    }
    let rel = rel.trim_start_matches('/');
    rel.is_empty()
        || rel.starts_with("$metadata")
        || rel == "$batch"
        || rel.split('/').enumerate().any(|(i, segment)| {
            segment.starts_with('$')
                || (i == 0 && segment.contains('('))
                || segment.starts_with(ACTION_PREFIX)
        })
        || headers.contains_key("odata-version")
        || headers.contains_key("odata-maxversion")
        || query.is_some_and(|q| {
            q.split('&')
                .any(|p| p.starts_with('$') || p.to_ascii_lowercase().starts_with("%24"))
        })
}

/// The bound actions' namespace prefix: `.../JoinedContext.Portal.propose_update`.
const ACTION_PREFIX: &str = "JoinedContext.Portal.";

/// The most an action's parameters may weigh.
const MAX_BODY: usize = 1 << 20;

/// An action's parameters: a JSON object holding only `allowed` members.
fn parameters(body: &[u8], allowed: &[&str]) -> Result<Map<String, Value>, Refused> {
    if body.is_empty() {
        return Ok(Map::new());
    }
    let Ok(Value::Object(params)) = serde_json::from_slice::<Value>(body) else {
        return Err(no(bad(
            "an action's parameters are a JSON object, e.g. {\"dryRun\": true}",
        )));
    };
    if let Some(unknown) = params.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(no(bad(format!(
            "this action takes no parameter {unknown}; it takes {}",
            allowed.join(", ")
        ))));
    }
    Ok(params)
}

fn dry_run(params: &Map<String, Value>) -> Result<DryRunQuery, Refused> {
    let dry = match params.get("dryRun") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(_) => return Err(no(bad("dryRun is true or false"))),
    };
    Ok(DryRunQuery {
        dry_run: dry.then(|| "All".to_owned()),
        ..DryRunQuery::default()
    })
}

/// The `If-Match` a bound write needs: absent 428; not the entity's `tag` 412 with the current
/// one. Asked only once the entity is known readable, so neither tells an unreadable one exists.
fn precondition(headers: &HeaderMap, current: &str) -> Result<(), Refused> {
    let Some(sent) = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) else {
        return Err(Box::new(error(
            StatusCode::PRECONDITION_REQUIRED,
            "precondition-required",
            "a bound write sends If-Match with the entity's @odata.etag, e.g. If-Match: \"3f2a…\"",
        )));
    };
    let matches = sent
        .split(',')
        .map(|t| t.trim().trim_start_matches("W/"))
        .any(|t| t == "*" || t == current);
    if matches {
        return Ok(());
    }
    let mut stale = error(
        StatusCode::PRECONDITION_FAILED,
        "precondition-failed",
        "the entity changed since you read it; read it again and send its new @odata.etag",
    );
    if let Ok(tag) = HeaderValue::from_str(current) {
        stale.headers_mut().insert(header::ETAG, tag);
    }
    Err(Box::new(stale))
}

/// The entity an action is bound to: its project, kind and key, from either path form.
fn bound_to(entity: &str) -> Result<Binding, Refused> {
    let entity = key_as_segment(entity)?;
    let parsed = odata_url::parse(&entity, &EdmModel(&ORG.public), &limits()).map_err(|e| {
        no(not_found(format!(
            "{e}: no such entity; $metadata lists the actions"
        )))
    })?;
    let ODataUri::Resource { path, .. } = parsed else {
        return Err(no(not_found("no such entity")));
    };
    let text = |v: Value| v.as_str().map_or_else(|| v.to_string(), str::to_owned);
    match path.as_slice() {
        [Segment::EntitySet(s), Segment::Key(key)] if s == PROJECTS => {
            Ok((text(single_key(key)?), None))
        }
        [Segment::EntitySet(s), Segment::Key(project), Segment::Navigation(nav), Segment::Key(Key::Single(KeyValue::Literal(name)))]
            if s == PROJECTS =>
        {
            let set = kind(nav).ok_or_else(|| no(not_found(format!("a project has no {nav}"))))?;
            let name = key_value(name).map_err(no)?;
            Ok((text(single_key(project)?), Some((set, text(name)))))
        }
        [Segment::EntitySet(s), Segment::Key(Key::Compound(pairs))] => {
            let set = kind(s).ok_or_else(|| no(not_found(format!("no entity set {s}"))))?;
            match (compound(pairs, PROJECT), compound(pairs, set.key)) {
                (Some(project), Some(name)) => Ok((text(project), Some((set, text(name))))),
                _ => Err(no(bad(format!("{s} is keyed by (project,{})", set.key)))),
            }
        }
        _ => Err(no(not_found(
            "an action is bound to a project, an entity or a Change",
        ))),
    }
}

/// Runs a bound action through the REST door of the same write, so its permission check, lane
/// and four-eyes rule are that door's (PF-52, PF-56, PF-58): `propose_create` on a project,
/// `propose_update` and `propose_delete` on a manifest, `approve` and `reject` on a Change.
/// One action call: who, where, what.
struct Call<'a> {
    state: &'a AppState,
    user: CurrentUser,
    front: Front,
    org: &'a str,
    headers: &'a HeaderMap,
    body: Bytes,
}

async fn act(call: Call<'_>, entity: &str, name: &str) -> Response {
    match act_inner(call, entity, name).await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => as_odata_error(response).await,
        Err(refusal) => *refusal,
    }
}

async fn act_inner(call: Call<'_>, entity: &str, name: &str) -> Result<Response, Refused> {
    let Call {
        state,
        user,
        front,
        org,
        headers,
        body,
    } = call;
    let (project, bound) = bound_to(entity)?;
    if !readable(state, &user, org, &project) {
        return Err(no(not_found(format!("no project {project} you may read"))));
    }
    let json = |value: &Value| {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        (h, Bytes::from(value.to_string()))
    };
    let door =
        |r: Result<Response, crate::error::ApiError>| r.unwrap_or_else(IntoResponse::into_response);
    let Some((set, key)) = bound else {
        if name != "propose_create" {
            return Err(no(not_found(format!(
                "a project has no action {name}; it has propose_create"
            ))));
        }
        let params = parameters(&body, &["manifest", "dryRun"])?;
        let manifest = params
            .get("manifest")
            .filter(|m| m.is_object())
            .ok_or_else(|| no(bad("propose_create takes the manifest, e.g. {\"manifest\": {\"kind\": \"ContextSpace\", ...}}")))?;
        let plural = manifest
            .get("kind")
            .and_then(Value::as_str)
            .and_then(crate::resource::by_kind)
            .filter(|info| kind(info.plural).is_some())
            .map(|info| info.plural.to_owned())
            .ok_or_else(|| {
                no(bad(
                    "manifest.kind is a kind of a project, as $metadata lists its sets",
                ))
            })?;
        let (h, bytes) = json(manifest);
        return Ok(door(
            crate::api::mutate::create(
                user,
                front,
                State(state.clone()),
                Path((project, plural)),
                Query(dry_run(&params)?),
                h,
                bytes,
            )
            .await,
        ));
    };
    // The entity first: one the caller may not read answers 404 whatever the precondition.
    let caller = caller_of(user.clone(), front);
    let rows = kind_rows(set, &caller, state, &project).await?;
    let row_key = format!("{project}/{key}");
    let row = rows
        .iter()
        .find(|row| row.get(ROW_KEY).and_then(Value::as_str) == Some(row_key.as_str()))
        .ok_or_else(|| {
            no(not_found(format!(
                "no {} {key} you may read",
                set.type_name
            )))
        })?;
    let current = row
        .get(ETAG)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let plural = set.set.clone();
    match name {
        "propose_update" | "propose_delete" if manifest_kind(set) => {
            precondition(headers, &current)?;
            let mut forwarded = HeaderMap::new();
            if let Some(tag) = headers.get(header::IF_MATCH) {
                forwarded.insert(header::IF_MATCH, tag.clone());
            }
            if name == "propose_delete" {
                let params = parameters(&body, &["dryRun"])?;
                return Ok(door(
                    crate::api::delete::delete_resource(
                        user,
                        front,
                        State(state.clone()),
                        Path((project, plural, key)),
                        Query(dry_run(&params)?),
                        forwarded,
                    )
                    .await,
                ));
            }
            let params = parameters(&body, &["manifest", "patch", "dryRun"])?;
            let query = dry_run(&params)?;
            match (params.get("manifest"), params.get("patch")) {
                (Some(manifest), None) if manifest.is_object() => {
                    let (h, bytes) = json(manifest);
                    forwarded.extend(h);
                    Ok(door(crate::api::mutate::replace(user, front, State(state.clone()), Path((project, plural, key)), Query(query), forwarded, bytes).await))
                }
                (None, Some(patch)) if patch.is_object() => {
                    forwarded.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/merge-patch+json"));
                    let bytes = Bytes::from(patch.to_string());
                    Ok(door(crate::api::mutate::patch(user, front, State(state.clone()), Path((project, plural, key)), Query(query), forwarded, bytes).await))
                }
                _ => Err(no(bad("propose_update takes the whole manifest or a merge patch of it, one of them: {\"manifest\": {...}} or {\"patch\": {\"spec\": {...}}}"))),
            }
        }
        "approve" | "reject" if set.set == CHANGES => {
            precondition(headers, &current)?;
            let params = if name == "approve" {
                parameters(&body, &["confirm"])?
            } else {
                parameters(&body, &["reason"])?
            };
            let bytes = Bytes::from(Value::Object(params).to_string());
            let path = Path((project, key));
            Ok(door(if name == "approve" {
                crate::api::changes::approve_change(user, State(state.clone()), path, bytes).await
            } else {
                crate::api::changes::reject_change(user, State(state.clone()), path, bytes).await
            }))
        }
        _ => Err(no(not_found(format!(
            "{} has no action {name}; $metadata lists the actions you may call",
            set.type_name
        )))),
    }
}

/// The Organization's projects the caller may read.
fn readable_projects(state: &AppState, user: &CurrentUser, org: &str) -> Vec<String> {
    state
        .mirror
        .namespaces()
        .into_iter()
        .filter(|project| readable(state, user, org, project))
        .collect()
}

/// A project's `ETag`: its manifest's (API/13 §3), else a hash of the row the caller reads.
fn project_etag(state: &AppState, project: &str, org: &str) -> String {
    match state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Project", project)
    {
        Some(manifest) => manifest.etag(),
        None => {
            let Value::Object(row) = project_row(state, project, org) else {
                return String::new();
            };
            let set = &SERVED.sets[0];
            etag_of(state, set, project, project, &row)
        }
    }
}

/// The project an operation is bound to: `projects('p')` or `projects/p`, readable in `org`.
fn operation_project(
    state: &AppState,
    user: &CurrentUser,
    org: &str,
    entity: &str,
) -> Result<String, Refused> {
    match bound_to(entity)? {
        (project, None) if readable(state, user, org, &project) => Ok(project),
        (project, None) => Err(no(not_found(format!("no project {project} you may read")))),
        (_, Some(_)) => Err(no(not_found(
            "the registry's functions and actions are bound to a project, e.g. \
             projects('air')/JoinedContext.Portal.jc_schema_get(kind='Endpoint')",
        ))),
    }
}

/// `GET {project}/JoinedContext.Portal.{name}(...)`: a read-only operation of the registry.
async fn function(
    state: &AppState,
    user: CurrentUser,
    front: Front,
    org: &str,
    entity: &str,
    call: &str,
    query: &str,
) -> Response {
    let answer = async {
        let project = operation_project(state, &user, org, entity)?;
        let (name, params) = odata_ops::function_call(call, query)
            .map_err(|(status, slug, message)| Box::new(error(status, slug, &message)))?;
        let op = crate::ops::find(&name)
            .filter(|_| odata_ops::form(&name) == Some(odata_ops::Form::Function))
            .ok_or_else(|| {
                no(not_found(format!(
                    "a project has no function {name}; $metadata lists the ones you may call"
                )))
            })?;
        let root = service_root(state);
        Ok::<_, Refused>(
            odata_ops::answer(
                state,
                user,
                front,
                &project,
                op,
                Value::Object(params),
                &root,
            )
            .await,
        )
    };
    match answer.await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => as_odata_error(response).await,
        Err(refusal) => *refusal,
    }
}

/// `POST {project}/JoinedContext.Portal.{name}`: a writing operation of the registry, its input
/// the body; a Change-proposing one after `If-Match` with the project's `ETag`.
async fn operation_action(call: Call<'_>, entity: &str, name: &str) -> Response {
    let Call {
        state,
        user,
        front,
        org,
        headers,
        body,
    } = call;
    let answer = async {
        let project = operation_project(state, &user, org, entity)?;
        let op =
            crate::ops::find(name).ok_or_else(|| no(not_found(format!("no action {name}"))))?;
        let mut input = if body.is_empty() {
            Value::Object(Map::new())
        } else {
            match serde_json::from_slice::<Value>(&body) {
                Ok(input @ Value::Object(_)) => input,
                _ => {
                    return Err(no(bad(
                        "an action's parameters are a JSON object, as $metadata names them",
                    )))
                }
            }
        };
        if name == "jc_project_create" {
            // A project opens in the Organization whose service was called, never another.
            match input.get("organization") {
                None | Some(Value::Null) => {
                    input["organization"] = Value::String(org.to_owned());
                }
                Some(Value::String(named)) if named == org => {}
                Some(_) => {
                    return Err(no(bad(format!(
                        "organization is {org}, the Organization of this service, or left out"
                    ))))
                }
            }
        } else if odata_ops::form(name) == Some(odata_ops::Form::ProposingAction) {
            precondition(headers, &project_etag(state, &project, org))?;
        }
        let root = service_root(state);
        Ok::<_, Refused>(odata_ops::answer(state, user, front, &project, op, input, &root).await)
    };
    match answer.await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => as_odata_error(response).await,
        Err(refusal) => *refusal,
    }
}

/// The compat negotiation in front of `/api/v1` (API/13 §2a).
pub async fn negotiate(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let odata = is_odata(
        request.uri().path(),
        request.uri().query(),
        request.headers(),
    );
    let mut response = if odata {
        let (parts, body): (_, Body) = request.into_parts();
        let none_match = matches!(parts.method, Method::GET | Method::HEAD)
            .then(|| parts.headers.get(header::IF_NONE_MATCH).cloned())
            .flatten();
        match axum::body::to_bytes(body, MAX_BODY).await {
            Ok(body) => not_modified(serve(state, parts, body).await, none_match.as_ref()),
            Err(_) => error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload-too-large",
                "an action's parameters are at most 1 MiB",
            ),
        }
    } else {
        next.run(request).await
    };
    response.headers_mut().append(
        header::VARY,
        HeaderValue::from_static("OData-Version, OData-MaxVersion"),
    );
    if odata {
        // An answer is the caller's own (API/13 §2a): no shared cache keeps it for another.
        response.headers_mut().append(
            header::VARY,
            HeaderValue::from_static("Authorization, Cookie"),
        );
    }
    response
}

/// `304` for an entity whose `ETag` is the one `If-None-Match` names: decided on the answer
/// [`serve`] built, so only after its authorization ran (a refusal keeps its status, API/13 §2a).
fn not_modified(response: Response, none_match: Option<&HeaderValue>) -> Response {
    let (Some(sent), Some(current)) = (
        none_match.and_then(|v| v.to_str().ok()),
        response.headers().get(header::ETAG).cloned(),
    ) else {
        return response;
    };
    let matches = sent
        .split(',')
        .map(|t| t.trim().trim_start_matches("W/"))
        .any(|t| t == "*" || current.to_str().is_ok_and(|c| c == t));
    if response.status() != StatusCode::OK || !matches {
        return response;
    }
    let mut unchanged = StatusCode::NOT_MODIFIED.into_response();
    let headers = unchanged.headers_mut();
    headers.insert(header::ETAG, current);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private"));
    unchanged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odata_requests_are_told_from_rest_ones() {
        let none = HeaderMap::new();
        let mut versioned = HeaderMap::new();
        versioned.insert("odata-version", HeaderValue::from_static("4.01"));
        assert!(is_odata("/api/v1/", None, &none));
        assert!(is_odata("/api/v1/$metadata", None, &none));
        assert!(is_odata("/api/v1/projects", Some("$top=5"), &none));
        assert!(is_odata("/api/v1/projects", Some("%24top=5"), &none));
        assert!(is_odata("/api/v1/projects('air')", None, &none));
        assert!(is_odata("/api/v1/projects", None, &versioned));
        assert!(!is_odata("/api/v1/projects", None, &none));
        assert!(!is_odata(
            "/api/v1/projects/air/endpoints",
            Some("limit=5"),
            &none
        ));
        // The per-project service keeps its routes.
        assert!(!is_odata(
            "/api/v1/projects/air/odata/endpoints",
            Some("$top=1"),
            &none
        ));
    }

    #[test]
    fn key_as_segment_reads_as_a_key_predicate() {
        assert_eq!(
            key_as_segment("projects/air/endpoints?$top=1")
                .ok()
                .as_deref(),
            Some("projects('air')/endpoints?$top=1")
        );
        assert_eq!(
            key_as_segment("projects/air").ok().as_deref(),
            Some("projects('air')")
        );
        assert_eq!(
            key_as_segment("projects('air')").ok().as_deref(),
            Some("projects('air')")
        );
        assert!(key_as_segment("projects/Not_A_Name/endpoints").is_err());
    }

    #[test]
    fn a_skiptoken_holds_only_for_its_organization_caller_and_read() {
        let token = sign("hel", "ari", "endpoints?$top=5", "50");
        assert_eq!(
            verify("hel", "ari", "endpoints?$top=5", &token).as_deref(),
            Some("50")
        );
        assert_eq!(verify("ba", "ari", "endpoints?$top=5", &token), None);
        assert_eq!(verify("hel", "bea", "endpoints?$top=5", &token), None);
        assert_eq!(verify("hel", "ari", "endpoints?$top=6", &token), None);
        let tampered = token.replacen("50.", "500.", 1);
        assert_eq!(verify("hel", "ari", "endpoints?$top=5", &tampered), None);
    }

    #[test]
    fn the_public_model_keys_kinds_by_project_and_name_and_validates() {
        let model = &ORG.public;
        let ty = model.schemas[0]
            .entity_types
            .iter()
            .find(|t| t.name == "ContextSpace")
            .expect("ContextSpace");
        assert_eq!(ty.key, vec!["project".to_owned(), "name".to_owned()]);
        let project = model.schemas[0]
            .entity_types
            .iter()
            .find(|t| t.name == "Project")
            .expect("Project");
        assert!(project
            .navigation_properties
            .iter()
            .any(|n| n.name == "endpoints"));
        let xml = metadata_xml(model);
        assert!(
            xml.as_ref()
                .is_ok_and(|xml| xml.contains("<Singleton Name=\"me\"")),
            "{xml:?}"
        );
        assert!(metadata_json(model).is_ok());
    }
}
