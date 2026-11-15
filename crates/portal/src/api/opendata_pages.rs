//! The texts of an Organization's open data portal pages (T-7031): connector, privacy, terms,
//! support and accessibility, each per language in Markdown, held in the Organization's own
//! manifest as `spec.appearance.pages` (Git history, Changes review) and shown by jc's catalogue
//! over the built-in text (`jc_core::opendata_pages`).
//!
//! Every route is for the request's Organization's administrators (403 for anyone else, 401
//! without a session). A write proposes the Organization manifest with that one page text set or
//! removed, through the same door every manifest takes (propose: its checks, its Change, its
//! audit), so it never touches another page, language or Organization. The preview renders with
//! the catalogue's own renderer: raw HTML shows as text and a `javascript:` link loses its link.

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use jc_core::opendata_pages::{self, MAX_PAGE_MARKDOWN, PAGES};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::auth::session::CurrentUser;
use crate::change::Operation;
use crate::error::ApiError;
use crate::permissions::{self, ORG_NAMESPACE};
use crate::state::AppState;
use jc_core::kinds::Verb;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/organization/opendata/pages", get(list_pages))
        .route("/organization/opendata/pages/preview", post(preview))
        .route(
            "/organization/opendata/pages/{page}/{lang}",
            put(publish).delete(revert),
        )
}

/// One page in one language: the built-in Markdown and the Organization's own, if any.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenDataPageText {
    /// `connector`, `privacy`, `terms`, `support` or `accessibility`.
    pub page: String,
    /// A two-letter language code, e.g. `sk`.
    pub lang: String,
    /// The title the portal shows: the own text's first `# heading`, else the built-in one.
    pub title: String,
    /// The text the portal shows when the Organization has none of its own.
    pub built_in: String,
    /// The Organization's own text, as published; absent when the built-in one shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
}

/// The editable pages of the request's Organization's open data portal.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenDataPages {
    /// The Organization the texts belong to.
    pub organization: String,
    /// Its languages (`spec.locales`), the default first; `sk` and `en` when it names none.
    pub languages: Vec<String>,
    /// The largest Markdown one page in one language holds, in bytes.
    pub max_bytes: usize,
    /// Every page in every language, in menu order.
    pub pages: Vec<OpenDataPageText>,
}

/// Markdown to preview or publish.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PageMarkdown {
    /// The page in Markdown, its first `# heading` the title, e.g. `# Podpora\n\nNapíšte nám.`
    pub markdown: String,
}

/// A page as the portal would show it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PagePreview {
    /// The first `# heading`, absent when the Markdown has none (the built-in title shows).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The body as safe HTML: no raw HTML, no image, no script link.
    pub html: String,
}

/// The request's Organization and its manifest, for those who may propose that manifest: a
/// binding in force at that Organization itself (or the bootstrap group), never one of another
/// Organization or of one of its projects.
fn organization(
    state: &AppState,
    user: &CurrentUser,
) -> Result<(String, crate::resource::ResourceEnvelope), ApiError> {
    let name = crate::organization_host::organization()
        .or_else(|| permissions::installation_organization(&state.mirror));
    let allowed = name.as_deref().is_some_and(|name| {
        permissions::holds_at_organization(
            state,
            &user.0.identity,
            name,
            "Organization",
            Verb::Propose,
            None,
        )
    });
    let Some(name) = name.filter(|_| allowed) else {
        return Err(ApiError::Denied(
            "editing the open data portal's pages is for the Organization's administrators, on the Administration page (UI-87)".into(),
        ));
    };
    let manifest = state
        .mirror
        .get(ORG_NAMESPACE, "Organization", &name)
        .ok_or_else(|| ApiError::NotFound(format!("Organization {name} is not loaded")))?;
    Ok((name, manifest))
}

fn languages(manifest: &crate::resource::ResourceEnvelope) -> Vec<String> {
    let listed: Vec<String> = manifest
        .spec
        .get("locales")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|l| l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase())
        .filter(|l| l.len() == 2)
        .collect();
    let mut out: Vec<String> = Vec::new();
    let default = manifest.spec.get("defaultLocale").and_then(Value::as_str);
    for lang in default.map(str::to_owned).into_iter().chain(listed) {
        if !out.contains(&lang) {
            out.push(lang);
        }
    }
    if out.is_empty() {
        out = vec!["sk".into(), "en".into()];
    }
    out
}

fn own_texts(manifest: &crate::resource::ResourceEnvelope) -> Vec<Value> {
    manifest
        .spec
        .pointer("/appearance/pages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn is(text: &Value, page: &str, lang: &str) -> bool {
    text.get("page").and_then(Value::as_str) == Some(page)
        && text.get("lang").and_then(Value::as_str) == Some(lang)
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/opendata/pages",
    operation_id = "list_opendata_pages",
    summary = "List The Open Data Portal's Pages",
    description = "Every page of the request's Organization's open data portal in each of its languages: the built-in Markdown and the Organization's own text (spec.appearance.pages, T-7031). For the Organization's administrators.",
    tag = "system",
    responses(
        (status = 200, description = "The pages", body = OpenDataPages),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "Not an administrator of the Organization", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_pages(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<OpenDataPages>, ApiError> {
    let (organization, manifest) = organization(&state, &user)?;
    let own = own_texts(&manifest);
    let languages = languages(&manifest);
    let mut pages = Vec::new();
    for page in PAGES {
        for lang in &languages {
            let built_in = opendata_pages::default_markdown(page, lang).unwrap_or_default();
            let markdown = own
                .iter()
                .find(|text| is(text, page, lang))
                .and_then(|text| text.get("markdown").and_then(Value::as_str))
                .map(str::to_owned);
            let title = markdown
                .as_deref()
                .and_then(|md| opendata_pages::render(md, &[]).title)
                .or_else(|| opendata_pages::render(built_in, &[]).title)
                .unwrap_or_else(|| page.to_owned());
            pages.push(OpenDataPageText {
                page: page.to_owned(),
                lang: lang.clone(),
                title,
                built_in: built_in.to_owned(),
                markdown,
            });
        }
    }
    Ok(Json(OpenDataPages {
        organization,
        languages,
        max_bytes: MAX_PAGE_MARKDOWN,
        pages,
    }))
}

/// Refuses Markdown the manifest would refuse, naming the field.
fn checked(markdown: &str) -> Result<(), ApiError> {
    if markdown.trim().is_empty()
        || markdown.len() > MAX_PAGE_MARKDOWN
        || markdown
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        let line = format!(
            "markdown: must be Markdown of 1 to {MAX_PAGE_MARKDOWN} bytes without control characters"
        );
        return Err(ApiError::Unprocessable {
            detail: line.clone(),
            errors: vec![line],
        });
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/opendata/pages/preview",
    operation_id = "preview_opendata_page",
    summary = "Preview An Open Data Portal Page",
    description = "The Markdown as the open data portal would show it, rendered by the portal's own renderer: raw HTML as text, no image, javascript: and data: links without their link (T-7031). For the Organization's administrators; nothing is stored.",
    tag = "system",
    request_body(content = PageMarkdown, example = json!({ "markdown": "# Podpora\n\nNapíšte nám na **podpora@bbsk.sk**." })),
    responses(
        (status = 200, description = "The page as HTML", body = PagePreview),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "Not an administrator of the Organization", body = crate::error::ProblemDetails),
        (status = 422, description = "Empty, over the size cap or with control characters", body = crate::error::ProblemDetails),
    )
)]
pub async fn preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(body): Json<PageMarkdown>,
) -> Result<Json<PagePreview>, ApiError> {
    organization(&state, &user)?;
    checked(&body.markdown)?;
    let shown = opendata_pages::render(&body.markdown, &[]);
    Ok(Json(PagePreview {
        title: shown.title,
        html: shown.html,
    }))
}

/// The page and language of a path, or 404.
fn target(page: &str, lang: &str) -> Result<(), ApiError> {
    if !PAGES.contains(&page) {
        return Err(ApiError::NotFound(format!(
            "the open data portal has no page {page}; its pages are {}",
            PAGES.join(", ")
        )));
    }
    if lang.len() != 2 || !lang.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err(ApiError::NotFound(format!(
            "{lang} is not a two-letter language code such as sk"
        )));
    }
    Ok(())
}

/// Proposes the Organization manifest with its `spec.appearance.pages` replaced by `texts`.
async fn propose(
    state: &AppState,
    user: &CurrentUser,
    name: &str,
    manifest: crate::resource::ResourceEnvelope,
    texts: Vec<Value>,
) -> Result<Response, ApiError> {
    let mut manifest = serde_json::to_value(&manifest)
        .map_err(|e| ApiError::Internal(format!("the Organization manifest: {e}")))?;
    if let Some(object) = manifest.as_object_mut() {
        object.remove("status");
    }
    let spec = manifest
        .get_mut("spec")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ApiError::Internal("the Organization manifest has no spec".into()))?;
    let appearance = spec
        .entry("appearance")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(appearance) = appearance.as_object_mut() else {
        return Err(ApiError::Internal(
            "spec.appearance is not an object".into(),
        ));
    };
    if texts.is_empty() {
        appearance.remove("pages");
    } else {
        appearance.insert("pages".into(), Value::Array(texts));
    }
    if appearance.is_empty() {
        spec.remove("appearance");
    }
    crate::api::mutate::propose(
        user,
        state,
        ORG_NAMESPACE,
        "organizations",
        Some(name),
        Operation::Update,
        false,
        manifest,
    )
    .await
}

#[utoipa::path(
    put,
    path = "/api/v1/organization/opendata/pages/{page}/{lang}",
    operation_id = "publish_opendata_page",
    summary = "Publish An Open Data Portal Page",
    description = "Proposes the request's Organization manifest with its own Markdown of one page in one language (spec.appearance.pages, T-7031): a Change, reviewed and audited like every manifest change; the open data portal shows it once merged. For the Organization's administrators.",
    tag = "system",
    params(
        ("page" = String, Path, description = "connector, privacy, terms, support or accessibility"),
        ("lang" = String, Path, description = "A two-letter language code, e.g. sk"),
    ),
    request_body(content = PageMarkdown, example = json!({ "markdown": "# Podpora\n\nNapíšte nám na **podpora@bbsk.sk**." })),
    responses(
        (status = 202, description = "The Change proposing the text", body = crate::change::Change),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "Not an administrator of the Organization", body = crate::error::ProblemDetails),
        (status = 404, description = "No such page or language code", body = crate::error::ProblemDetails),
        (status = 409, description = "A Change on the Organization is open already; approve or reject it first", body = crate::error::ProblemDetails),
        (status = 422, description = "Empty, over the size cap or with control characters", body = crate::error::ProblemDetails),
    )
)]
pub async fn publish(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((page, lang)): Path<(String, String)>,
    Json(body): Json<PageMarkdown>,
) -> Result<Response, ApiError> {
    let (name, manifest) = organization(&state, &user)?;
    target(&page, &lang)?;
    checked(&body.markdown)?;
    let mut texts: Vec<Value> = own_texts(&manifest)
        .into_iter()
        .filter(|text| !is(text, &page, &lang))
        .collect();
    texts.push(serde_json::json!({ "page": page, "lang": lang, "markdown": body.markdown }));
    propose(&state, &user, &name, manifest, texts).await
}

#[utoipa::path(
    delete,
    path = "/api/v1/organization/opendata/pages/{page}/{lang}",
    operation_id = "revert_opendata_page",
    summary = "Revert An Open Data Portal Page",
    description = "Proposes the request's Organization manifest without its own text of one page in one language, so the built-in text shows again (T-7031). For the Organization's administrators.",
    tag = "system",
    params(
        ("page" = String, Path, description = "connector, privacy, terms, support or accessibility"),
        ("lang" = String, Path, description = "A two-letter language code, e.g. sk"),
    ),
    responses(
        (status = 202, description = "The Change removing the text", body = crate::change::Change),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "Not an administrator of the Organization", body = crate::error::ProblemDetails),
        (status = 404, description = "No such page, or the page shows its built-in text already", body = crate::error::ProblemDetails),
        (status = 409, description = "A Change on the Organization is open already; approve or reject it first", body = crate::error::ProblemDetails),
    )
)]
pub async fn revert(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((page, lang)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let (name, manifest) = organization(&state, &user)?;
    target(&page, &lang)?;
    let own = own_texts(&manifest);
    if !own.iter().any(|text| is(text, &page, &lang)) {
        return Err(ApiError::NotFound(format!(
            "{page} in {lang} shows the built-in text already"
        )));
    }
    let texts = own
        .into_iter()
        .filter(|text| !is(text, &page, &lang))
        .collect();
    propose(&state, &user, &name, manifest, texts).await
}
