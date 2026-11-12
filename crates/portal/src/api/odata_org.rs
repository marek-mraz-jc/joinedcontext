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

use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::extract::{FromRequestParts, Request, State};
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

use crate::api::odata::{
    bad, key_value, limits, metadata_xml, not_found, not_implemented, rows_of, ListSet, Refusal,
    MAX_ROWS, MAX_TOP, NAMESPACE, SERVED,
};
use crate::api::ops::caller_of;
use crate::auth::session::{CurrentUser, Front};
use crate::ops::Caller;
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
    metadata: Result<String, String>,
    metadata_json: Result<String, String>,
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
    let metadata = metadata_xml(&public).map(|xml| with_singletons_xml(&xml));
    let metadata_json = metadata_json(&public);
    OrgServed {
        public,
        service,
        evaluated,
        metadata,
        metadata_json,
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
    error(status, &slug, &message)
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
            row.insert(PROJECT.into(), Value::String(project.to_owned()));
            row.insert(ROW_KEY.into(), Value::String(format!("{project}/{key}")));
            Some(Value::Object(row))
        })
        .collect())
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
    body.extend(row);
    ok_json(&Value::Object(body))
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
                    if name != PROJECTS && kind(name).is_none() {
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
        _ => refused(not_implemented("this resource path")),
    }
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
    for singleton in [ME, ORGANIZATION] {
        value.push(json!({ "name": singleton, "kind": "Singleton", "url": singleton }));
    }
    ok_json(&json!({
        "@odata.context": format!("{}$metadata", service_root(state)),
        "value": value,
    }))
}

fn metadata(headers: &HeaderMap, query: &str) -> Response {
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
    let (body, content_type) = if json_asked {
        (&ORG.metadata_json, "application/json")
    } else {
        (&ORG.metadata, "application/xml")
    };
    match body {
        Ok(text) => (StatusCode::OK, headers_for(content_type), text.clone()).into_response(),
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
async fn serve(state: AppState, mut parts: axum::http::request::Parts) -> Response {
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
    if !matches!(parts.method, Method::GET | Method::HEAD) {
        let mut r = error(
            StatusCode::METHOD_NOT_ALLOWED,
            "invalid-request",
            "this service answers reads; a write is a reviewed Change the Portal's actions open",
        );
        r.headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return r;
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
        return metadata(&parts.headers, &query);
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
        || rel
            .split('/')
            .enumerate()
            .any(|(i, segment)| segment.starts_with('$') || (i == 0 && segment.contains('(')))
        || headers.contains_key("odata-version")
        || headers.contains_key("odata-maxversion")
        || query.is_some_and(|q| {
            q.split('&')
                .any(|p| p.starts_with('$') || p.to_ascii_lowercase().starts_with("%24"))
        })
}

/// The compat negotiation in front of `/api/v1` (API/13 §2a).
pub async fn negotiate(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let odata = is_odata(
        request.uri().path(),
        request.uri().query(),
        request.headers(),
    );
    let mut response = if odata {
        let (parts, _body): (_, Body) = request.into_parts();
        serve(state, parts).await
    } else {
        next.run(request).await
    };
    response.headers_mut().append(
        header::VARY,
        HeaderValue::from_static("OData-Version, OData-MaxVersion"),
    );
    response
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
        assert!(
            ORG.metadata
                .as_ref()
                .is_ok_and(|xml| xml.contains("<Singleton Name=\"me\"")),
            "{:?}",
            ORG.metadata
        );
        assert!(ORG.metadata_json.is_ok());
    }
}
