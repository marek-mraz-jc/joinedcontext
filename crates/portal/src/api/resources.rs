use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::api::list_query::{self, ListQuery};

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::filter::{Filter, OrderBy};
use crate::resource::selector::{FieldSelector, LabelSelector};
use crate::resource::{by_plural, ResourceEnvelope, API_VERSION};
use crate::state::AppState;
use crate::store::ListOptions;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceList {
    pub api_version: String,
    pub kind: String,
    pub metadata: ListMeta,
    pub items: Vec<ResourceEnvelope>,
    /// The `cursor` of the next page; absent on the last one (API/13 section 2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// How many rows the whole listing holds, every page together; only with `count=true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<usize>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListMeta {
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub continue_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_item_count: Option<usize>,
}

/// What a kind's list takes beyond the list contract of [`ListQuery`].
#[derive(Debug, Default, Deserialize)]
pub struct KindListQuery {
    /// AIP-160 subset over the manifest (API/13 section 2).
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default, rename = "labelSelector")]
    pub label_selector: Option<String>,
    #[serde(default, rename = "fieldSelector")]
    pub field_selector: Option<String>,
    #[serde(default, rename = "continue")]
    pub continue_token: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
    /// Read inside this workspace: its branch over `main` (CC-76).
    #[serde(default)]
    pub workspace: Option<String>,
}

/// The query of a single read.
#[derive(Debug, Default, Deserialize)]
pub struct GetQuery {
    /// Read inside this workspace: its branch over `main` (CC-76).
    #[serde(default)]
    pub workspace: Option<String>,
    /// Read the project as it stood at this commit id (MF-11).
    pub revision: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/{plural}",
    summary = "List Resources",
    description = "Lists the resources of one kind in the project, optionally of one context space.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("plural" = String, Path, description = "Resource kind plural"),
        ("filter" = Option<String>, Query, description = "AIP-160 subset over manifest fields, e.g. labels.team = \"air\" AND spec.schedule:*"),
        ("sort" = Option<String>, Query, description = "One field of the kind, `-` in front for descending, e.g. -metadata.name; default metadata.name"),
        ("orderBy" = Option<String>, Query, description = "Old name of sort: one field, optionally followed by desc"),
        ("q" = Option<String>, Query, description = "Text searched case-insensitively in each manifest's name, title, labels and spec, e.g. air"),
        ("count" = Option<bool>, Query, description = "true adds totalCount, the rows of every page together"),
        ("cursor" = Option<String>, Query, description = "The nextCursor of the previous page"),
        ("labelSelector" = Option<String>, Query, description = "Label selector"),
        ("fieldSelector" = Option<String>, Query, description = "Field selector"),
        ("limit" = Option<usize>, Query, description = "Rows per page, 1 to 1000; the whole list without it"),
        ("continue" = Option<String>, Query, description = "Pagination continue token"),
        ("revision" = Option<String>, Query, description = "Read the project as it stood at this commit id (MF-11)"),
        ("workspace" = Option<String>, Query, description = "Read inside this workspace (CC-76)"),
    ),
    responses(
        (status = 200, description = "List of resources", body = ResourceList),
        (status = 400, description = "A bad filter, sort, q, count, cursor, selector or limit, a revision that is not a commit id, or one beside a workspace", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Resource not found", body = ProblemDetails),
        (status = 422, description = "A sort field the kind does not have", body = ProblemDetails)
    )
)]
pub async fn list(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, plural)): Path<(String, String)>,
    paging: Result<ListQuery, ApiError>,
    Query(query): Query<KindListQuery>,
) -> Result<Json<ResourceList>, ApiError> {
    // One body for a plural that is not a kind, a kind no binding of the caller reads and a
    // project no binding covers: what the caller may not read is not there (PF-59, R20).
    let not_found = || {
        ApiError::NotFound(format!(
            "plural '{plural}' not found in project '{project}'"
        ))
    };
    let kind_info = by_plural(&plural).ok_or_else(not_found)?;
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !effective.may_read_some(kind_info.kind) {
        return Err(not_found());
    }
    // A bad list parameter is judged after the read check: a project the caller may not read
    // answers 404 whatever the query says.
    let paging = paging?;

    let label_selector = match query.label_selector.as_deref() {
        Some(raw) if !raw.trim().is_empty() => {
            Some(LabelSelector::parse(raw).map_err(|e| ApiError::BadRequest(e.to_string()))?)
        }
        _ => None,
    };

    let field_selector = match query.field_selector.as_deref() {
        Some(raw) if !raw.trim().is_empty() => {
            Some(FieldSelector::parse(raw).map_err(|e| ApiError::BadRequest(e.to_string()))?)
        }
        _ => None,
    };

    let listing = Listing::of(
        &paging,
        &query,
        kind_info.kind,
        OrderBy::default(),
        &[
            Some(user.0.identity.subject.as_str()),
            Some(project.as_str()),
            Some(plural.as_str()),
        ],
    )?;

    // The store answers every manifest of the kind; the read check, the filter and the page run
    // here in that order, so an object the caller may not read never shifts a page or shows in
    // remainingItemCount (API/13 section 2).
    let opts = ListOptions {
        label_selector,
        field_selector,
        limit: None,
        continue_token: None,
    };

    let page = match (query.workspace.as_deref(), query.revision.as_deref()) {
        (Some(_), Some(_)) => return Err(both()),
        (Some(name), None) => {
            crate::ops::workspaces::mirror_of(&state, &user.0.identity, name, &project)
                .await?
                .list(&project, kind_info.kind, &opts)
        }
        (None, Some(revision)) => mirror_at(&state, &project, revision, not_found)
            .await?
            .list(&project, kind_info.kind, &opts),
        (None, None) => state.mirror.list(&project, kind_info.kind, &opts),
    };
    // Item by item: a grant bound to one context space reads that space's manifests alone, and
    // one narrowed to an Organization's own settings its own (ADR-N-058 section 2.4, T-5698).
    let readable = page
        .items
        .into_iter()
        .filter(|env| effective.may_read_envelope(kind_info.kind, env));
    Ok(Json(listing.page(
        readable.map(|env| (env.metadata.name.clone(), env)),
        query.continue_token.as_deref(),
    )))
}

/// What a list query asks beyond the store's selectors: its filter, search, order and where the
/// page starts, each checked against the kind before anything is read.
struct Listing {
    filter: Option<Filter>,
    paging: ListQuery,
    order: OrderBy,
    after: Option<(Value, String)>,
    listing_id: String,
}

impl Listing {
    /// `parts` names the listing (caller, project, plural) a cursor stays bound to.
    fn of(
        paging: &ListQuery,
        query: &KindListQuery,
        kind: &str,
        default: OrderBy,
        parts: &[Option<&str>],
    ) -> Result<Self, ApiError> {
        let schema = jc_core::registry::schema_of(kind);
        let filter = match query.filter.as_deref().map(str::trim) {
            Some(text) if !text.is_empty() => Some(
                Filter::parse(text, schema.as_ref())
                    .map_err(|e| ApiError::BadRequest(e.to_string()))?,
            ),
            _ => None,
        };
        let order = match &paging.sort {
            None => default,
            Some(sort) => OrderBy::parse(&sort.order_by(), schema.as_ref()).map_err(|e| {
                ApiError::Unprocessable {
                    detail: format!("sort names `{}`, which a {kind} does not have", sort.field),
                    errors: vec![format!("sort: {}", e.message)],
                }
            })?,
        };
        let mut id_parts = parts.to_vec();
        id_parts.extend([
            query.filter.as_deref().map(str::trim),
            query.label_selector.as_deref(),
            query.field_selector.as_deref(),
            query.workspace.as_deref(),
            query.revision.as_deref(),
        ]);
        let listing_id = paging.listing_id(&id_parts);
        let after = match (paging.cursor.as_deref(), query.continue_token.as_deref()) {
            (Some(_), Some(_)) => {
                return Err(ApiError::BadRequest(
                    "send cursor or continue, not both; cursor replaces continue".into(),
                ))
            }
            (None, Some(_)) if order != OrderBy::default() => {
                return Err(ApiError::BadRequest(
                    "continue pages by metadata.name only; page a sort with cursor".into(),
                ))
            }
            (Some(text), None) => Some(list_query::open(text, &listing_id)?),
            _ => None,
        };
        Ok(Self {
            filter,
            paging: paging.clone(),
            order,
            after,
            listing_id,
        })
    }

    /// Filters, searches, orders and cuts the rows the caller may read into one page. Each row
    /// carries the name that breaks a tie in the order and that a cursor ends at.
    fn page(
        &self,
        readable: impl Iterator<Item = (String, ResourceEnvelope)>,
        continue_token: Option<&str>,
    ) -> ResourceList {
        let order = &self.order;
        let mut rows: Vec<(Value, String, ResourceEnvelope)> = readable
            .filter_map(|(tie, env)| {
                let doc = serde_json::to_value(&env).ok()?;
                (self.filter.as_ref().is_none_or(|f| f.matches(&doc)) && self.paging.finds(&doc))
                    .then(|| (order.key_of(&doc), tie, env))
            })
            .collect();
        let total_count = self.paging.count.then_some(rows.len());
        rows.sort_by(|a, b| order.cmp((&a.0, &a.1), (&b.0, &b.1)));
        if let Some((key, name)) = &self.after {
            rows.retain(|(k, tie, _)| {
                order.cmp((k, tie), (key, name)) == std::cmp::Ordering::Greater
            });
        }
        if let Some(token) = continue_token {
            rows.retain(|(_, _, env)| env.metadata.name.as_str() > token);
        }

        let total = rows.len();
        let limit = self.paging.limit.unwrap_or(total);
        rows.truncate(limit);
        let more = total > limit;
        let last = rows.last().filter(|_| more);
        let next_cursor = last.map(|(key, tie, _)| list_query::seal(&self.listing_id, key, tie));
        ResourceList {
            api_version: API_VERSION.to_string(),
            kind: "List".to_string(),
            metadata: ListMeta {
                continue_token: last.map(|(_, _, env)| env.metadata.name.clone()),
                remaining_item_count: more.then(|| total - limit),
            },
            items: rows.into_iter().map(|(_, _, env)| env).collect(),
            next_cursor,
            total_count,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/{plural}/{name}",
    summary = "Get Resource",
    description = "Reads one resource's manifest and status by kind and name.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("plural" = String, Path, description = "Resource kind plural"),
        ("name" = String, Path, description = "Resource name"),
        ("workspace" = Option<String>, Query, description = "Read inside this workspace (CC-76)"),
        ("revision" = Option<String>, Query, description = "Read the project as it stood at this commit id (MF-11)"),
        ("If-None-Match" = Option<String>, Header, description = "An ETag of an earlier read: 304 when the manifest is unchanged"),
    ),
    responses(
        (status = 200, description = "Resource envelope, with its strong ETag", body = ResourceEnvelope),
        (status = 304, description = "Unchanged since the ETag sent in If-None-Match"),
        (status = 400, description = "A revision that is not a commit id, or one beside a workspace", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Resource not found", body = ProblemDetails)
    )
)]
pub async fn get_resource(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, plural, name)): Path<(String, String, String)>,
    Query(query): Query<GetQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // The same 404 body for an unknown plural and for a resource that exists but is not
    // visible: existence is never disclosed (R20).
    let not_found = || {
        ApiError::NotFound(format!(
            "resource '{name}' not found in project '{project}'"
        ))
    };
    let kind_info = by_plural(&plural).ok_or_else(not_found)?;
    // An App's default group is read by whoever may read the App (AP-119).
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    let app_group = crate::groups::may_read_as_app_group(
        &state,
        &user.0.identity,
        &project,
        kind_info.kind,
        &name,
    );
    if !effective.may_read_some(kind_info.kind) && !app_group {
        return Err(not_found());
    }
    let envelope = match (query.workspace.as_deref(), query.revision.as_deref()) {
        (Some(_), Some(_)) => return Err(both()),
        (Some(workspace), None) => {
            crate::ops::workspaces::mirror_of(&state, &user.0.identity, workspace, &project)
                .await?
                .get(&project, kind_info.kind, &name)
        }
        (None, Some(revision)) => mirror_at(&state, &project, revision, not_found).await?.get(
            &project,
            kind_info.kind,
            &name,
        ),
        (None, None) => state.mirror.get(&project, kind_info.kind, &name),
    }
    .ok_or_else(not_found)?;
    // A grant bound to one context space, or narrowed to one Organization's own settings, reads
    // its own manifests alone (ADR-N-058 section 2.4, T-5698).
    if !app_group && !effective.may_read_envelope(kind_info.kind, &envelope) {
        return Err(not_found());
    }

    let etag = envelope.etag();
    let unchanged = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|sent| sent.split(',').any(|v| v.trim() == etag || v.trim() == "*"));
    if unchanged {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
    }
    Ok(([(header::ETAG, etag)], Json(envelope)).into_response())
}

fn both() -> ApiError {
    ApiError::BadRequest("read a workspace or a revision, not both".into())
}

/// The project as it stood at one commit (MF-11, MF-16): its subtree of the repository at
/// `revision`, loaded the way the live mirror is, with no status, because status is what the
/// Portal computes now and a past commit has none (MF-04).
///
/// Only a commit id is a revision. A branch name would read a workspace's unmerged edits past
/// the workspace's own door (CC-76). A commit the forge does not know answers the caller's own
/// 404, so a guessed sha tells nothing (R20); the read grants are the caller's current ones,
/// checked before this runs, so a binding that once allowed a read does not allow it now.
// ponytail: one forge read per file per request; cache by commit id if history reads get hot.
async fn mirror_at(
    state: &AppState,
    project: &str,
    revision: &str,
    not_found: impl Fn() -> ApiError,
) -> Result<crate::store::Mirror, ApiError> {
    if !crate::api::export::is_commit(revision) {
        return Err(ApiError::BadRequest(
            "revision is a commit id: 7 to 40 lowercase hex digits".into(),
        ));
    }
    let gitea = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("no repository is configured".into()))?;
    let gitea: &crate::git::GiteaClient = &gitea;
    let paths = match gitea.list_tree(revision).await {
        Ok(paths) => paths,
        Err(crate::git::GitError::NotFound) => return Err(not_found()),
        Err(crate::git::GitError::Api { status, .. }) if (400..500).contains(&status) => {
            return Err(not_found())
        }
        Err(err) => return Err(err.into()),
    };
    let prefix = format!("projects/{project}/");
    let mirror = crate::store::Mirror::new();
    for path in paths {
        if !path.starts_with(&prefix) || !(path.ends_with(".yaml") || path.ends_with(".yml")) {
            continue;
        }
        let Some(file) = gitea.get_file(&path, revision).await? else {
            continue;
        };
        let Some(mut envelope) = crate::store::envelope_of(&file.content) else {
            continue;
        };
        match envelope.metadata.namespace.as_deref() {
            None | Some("") => envelope.metadata.namespace = Some(project.to_owned()),
            Some(namespace) if namespace != project => continue,
            Some(_) => {}
        }
        envelope.strip_status();
        mirror.upsert(envelope);
    }
    Ok(mirror)
}

/// `GET /api/v1/endpoints` (PF-61, T-2877): the organization-level Endpoints page, an
/// administration view. An administrator of the organization (PF-03) reads every Endpoint of
/// every project, each carrying the project it lives in; anyone else is answered `404`, the
/// answer for a page that is not theirs (R20). A project's own Endpoints stay under its `read`.
#[utoipa::path(
    get,
    path = "/api/v1/endpoints",
    summary = "List Endpoints Everywhere",
    description = "Every Endpoint of every project with its project, for an organization administrator (PF-61, PF-03).",
    tag = "resources",
    params(
        ("sort" = Option<String>, Query, description = "One Endpoint field, `-` in front for descending, e.g. -metadata.name; default project, then name"),
        ("q" = Option<String>, Query, description = "Text searched case-insensitively in each Endpoint's name, title, labels and spec, e.g. air"),
        ("count" = Option<bool>, Query, description = "true adds totalCount, the rows of every page together"),
        ("cursor" = Option<String>, Query, description = "The nextCursor of the previous page"),
        ("limit" = Option<usize>, Query, description = "Rows per page, 1 to 1000; the whole list without it"),
    ),
    responses(
        (status = 200, description = "Every Endpoint of the organization, across projects", body = ResourceList),
        (status = 400, description = "A bad sort, q, count, cursor or limit", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Not an administrator of the organization", body = ProblemDetails),
        (status = 422, description = "A sort field an Endpoint does not have", body = ProblemDetails)
    )
)]
pub async fn list_endpoints_everywhere(
    user: CurrentUser,
    State(state): State<AppState>,
    paging: Result<ListQuery, ApiError>,
) -> Result<Json<ResourceList>, ApiError> {
    if !crate::permissions::for_request(&state, &user.0.identity, crate::permissions::ORG_NAMESPACE)
        .administers_organization()
    {
        return Err(ApiError::NotFound(
            "the organization-level Endpoints page is for administrators of the organization; \
             a project's own endpoints are under its Endpoints (PF-61)"
                .into(),
        ));
    }
    let paging = paging?;
    let listing = Listing::of(
        &paging,
        &KindListQuery::default(),
        "Endpoint",
        OrderBy::by_name_alone(),
        &[Some(user.0.identity.subject.as_str()), Some("endpoints")],
    )?;
    let mut items = Vec::new();
    for project in state.mirror.namespaces() {
        let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
        if !effective.may_read("Endpoint") {
            continue;
        }
        items.extend(
            state
                .mirror
                .list(&project, "Endpoint", &ListOptions::default())
                .items
                .into_iter()
                .filter(|env| {
                    effective.may_read_manifest(
                        "Endpoint",
                        &serde_json::to_value(env).unwrap_or(serde_json::Value::Null),
                    )
                })
                // Project, then name: two projects can hold an Endpoint of one name.
                .map(|env| (format!("{project}/{}", env.metadata.name), env)),
        );
    }
    Ok(Json(listing.page(items.into_iter(), None)))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/endpoints", get(list_endpoints_everywhere))
        .route(
            "/projects/{project}/{plural}",
            get(list).post(crate::api::mutate::create),
        )
        .route(
            "/projects/{project}/{plural}/{name}",
            get(get_resource)
                .put(crate::api::mutate::replace)
                .patch(crate::api::mutate::patch)
                .delete(crate::api::delete::delete_resource),
        )
}
