//! Static apps served under the platform host at `/apps/{name}/` (AP-14).
//!
//! A `static` app is a built bundle in the app artifact root, one directory per app. Three
//! things decide what leaves this module: the app's manifest must say `lifecycle: published`
//! (AP-18), every file must match the Subresource Integrity digest its build lane recorded in
//! `integrity.json` (AP-12), and every response carries the app's own Content Security Policy
//! instead of the Portal's (AP-12).

use std::path::{Path as FsPath, PathBuf};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::Engine;
use jc_core::kinds::{AppLifecycle, AppSpec, AppVisibility};
use sha2::{Digest, Sha384};

use crate::error::ApiError;
use crate::state::AppState;

/// Digest map written by the build lane beside the bundle: bundle-relative path →
/// `sha384-<base64>`. An app directory without it serves nothing; the manifest is what makes a
/// bundle publishable, not an optional extra (AP-12).
pub const INTEGRITY_MANIFEST: &str = "integrity.json";

/// Serves `/apps/{name}/` — the app's own index.
///
/// A signed-in person's first index also brings the double-submit CSRF cookie of the apps origin:
/// the Portal's own is host-only on the Portal host, and a function call that carries the edge's
/// token is refused without one (AP-84, [`super::functions`]).
async fn serve_index(
    state: State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    name: Path<String>,
) -> Response {
    let needs_csrf = super::roles::presented(&state, &headers).is_some()
        && axum_extra::extract::cookie::CookieJar::from_headers(&headers)
            .get(crate::auth::csrf::CSRF_COOKIE)
            .is_none();
    let mut response = serve(
        state,
        headers,
        uri,
        Path((name.0, "index.html".to_string())),
    )
    .await;
    if needs_csrf && response.status().is_success() {
        let token = crate::auth::csrf::Token::mint();
        for cookie in [
            crate::auth::csrf::cookie(&token),
            crate::auth::csrf::legacy_app_cookie(&token),
        ] {
            if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
        }
    }
    response
}

/// The `404` of an App that is not there, as a page in the person's language for a browser
/// (T-4200).
fn missing(state: &AppState, headers: &HeaderMap, name: &str) -> Response {
    let start = if state.config.apps_url.is_some() {
        "/".to_owned()
    } else {
        format!("/apps/{name}/")
    };
    super::roles::missing_page(headers, &start, portal_origin(state).as_deref())
        .unwrap_or_else(|| ApiError::NotFound(format!("app '{name}' not found")).into_response())
}

/// The answer for a caller the visibility of published App `name` keeps out, `None` for one it
/// admits: the `404` of a name never created, or the `403` page of a `roles` App (T-4910).
fn kept_out(
    state: &AppState,
    headers: &HeaderMap,
    project: &str,
    name: &str,
    spec: &AppSpec,
    person: Option<&super::roles::AppPerson>,
) -> Option<Response> {
    if super::roles::may_open(state, project, name, spec, person) {
        return None;
    }
    if spec.visibility != AppVisibility::Roles {
        return Some(missing(state, headers, name));
    }
    // A published app with roles says who grants them instead of pretending it is not there,
    // and names none of its members (AP-93).
    let title = state
        .mirror
        .get(project, "App", name)
        .and_then(|env| env.metadata.title);
    Some(super::roles::refusal(
        spec,
        name,
        title.as_ref(),
        project,
        headers,
        portal_origin(state).as_deref(),
    ))
}

/// Serves `/edge/apps/{name}/admit`, the edge's `forward-auth` in front of a pod App's server and
/// a wasm App's shard, which never run the visibility check themselves (T-4922): `204` for a
/// caller the App admits, else the static host's own refusal, which the edge hands on.
async fn admit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let Some((project, spec, _)) = published_app(&state, &name) else {
        return missing(&state, &headers, &name);
    };
    let person = super::roles::person(&state, &headers, &spec, &name).await;
    kept_out(&state, &headers, &project, &name, &spec, person.as_ref())
        .unwrap_or_else(|| StatusCode::NO_CONTENT.into_response())
}

/// Serves `/apps/{name}/{path}`.
async fn serve(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((name, path)): Path<(String, String)>,
) -> Response {
    if let Some(refused) = off_its_origin(&state, &headers, &name, &uri) {
        return refused;
    }

    // Every refusal below is the same 404. A draft app, a retired app, a name that was never
    // created and a caller who may not see it are indistinguishable from outside, so the host
    // never discloses what is being worked on (AP-18).
    let not_found = || missing(&state, &headers, &name);

    let Some((project, spec, build)) = published_app(&state, &name) else {
        return not_found();
    };
    // The person as this App's own client knows them (ADR-N-030, AP-92).
    let person = super::roles::person(&state, &headers, &spec, &name).await;
    if let Some(refused) = kept_out(&state, &headers, &project, &name, &spec, person.as_ref()) {
        return refused;
    }
    let Some(app_root) = app_root(
        state.config.apps_dir.as_deref().map(FsPath::new),
        state.config.apps_cache_dir.as_deref().map(FsPath::new),
        &name,
        build.as_ref(),
    ) else {
        return not_found();
    };
    let Some(file) = resolve(&app_root, &path) else {
        return not_found();
    };
    let Ok(bytes) = std::fs::read(&file) else {
        return not_found();
    };

    let Some(expected) = integrity_of(&app_root, &path) else {
        // No recorded digest means the file is not part of the published bundle, even if it
        // sits in the directory.
        return not_found();
    };
    if expected != sri_sha384(&bytes) {
        // A bundle that no longer matches what CI signed is never served under a published
        // app's name; the operator gets the log line, the caller gets a plain refusal.
        tracing::error!(app = %name, path = %path, "app asset fails its recorded integrity digest");
        return ApiError::Unavailable("the app bundle failed its integrity check".into())
            .into_response();
    }

    // The index is the one file that is not served as built: the SDK starts from the
    // `#jc-config` the host writes into it (SDK-02), after the digest was checked on the bytes
    // CI recorded (AP-12). An app that reads nothing is served as it was built.
    // It carries the person and their roles in this app, computed now, so no shared cache keeps
    // it and nobody else is served it (AP-95).
    let personal = path == "index.html" && person.is_some();
    let bytes = if path == "index.html" {
        let domain = crate::api::assistant::org_domain(&state, &project);
        match served_config(&state.mirror, &project, &name, &spec, &domain) {
            Some(mut config) => match String::from_utf8(bytes) {
                Ok(html) => {
                    config["user"] = super::roles::app_user(person.as_ref());
                    // The map's style comes from the platform, never a tile host the app names
                    // (AP-67); the policy below admits exactly its route.
                    if let Some(url) = crate::api::basemap::style_url(&state.config, &project) {
                        config["basemap"] = serde_json::Value::String(url);
                    }
                    // Where the entity panel links an entity the reader may not edit here (SDK-40).
                    if let Some(portal) = portal_of(&state.config.public_base_url, &project) {
                        config["portal"] = serde_json::Value::String(portal);
                    }
                    with_config(&html, &config).into_bytes()
                }
                Err(raw) => raw.into_bytes(),
            },
            None => bytes,
        }
    } else {
        bytes
    };

    let shared = spec.visibility == AppVisibility::Public;
    let mut response = file_response(&headers, &path, &expected, (personal, shared), bytes);

    // No `X-Frame-Options`: its `SAMEORIGIN` would refuse the Portal, whose host is not the apps
    // origin, and `frame-ancestors` says who may frame the App (AP-122).
    let basemap = crate::api::basemap::route_prefix(&state.config, &project);
    let policy =
        content_security_policy(&spec, portal_origin(&state).as_deref(), basemap.as_deref());
    if let Ok(csp) = HeaderValue::from_str(&with_store(
        &policy,
        &spec,
        state.config.apps_store_origin.as_deref(),
    )) {
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    response
}

/// One file of an App with its caching headers, or `304` when the browser holds this digest.
///
/// A non-public App's file is asked for again within a minute, so a reader whose access ended
/// keeps none of it longer than that; its digest makes the second ask a `304`, answered only
/// after the access check (T-4914). The index is rewritten per reader and never revalidated.
fn file_response(
    headers: &HeaderMap,
    path: &str,
    digest: &str,
    (personal, shared): (bool, bool),
    bytes: Vec<u8>,
) -> Response {
    let etag = (path != "index.html").then(|| format!("\"{digest}\""));
    let held = headers
        .get(header::IF_NONE_MATCH)
        .map(HeaderValue::as_bytes);
    let unchanged = etag.is_some() && held == etag.as_deref().map(str::as_bytes);
    let (status, body) = match unchanged {
        true => (StatusCode::NOT_MODIFIED, Body::empty()),
        false => (StatusCode::OK, Body::from(bytes)),
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, cache_control(personal, shared, path));
    if let Some(etag) = &etag {
        builder = builder.header(header::ETAG, etag.as_str());
    }
    if !shared {
        // The edge's session cookie says whom the answer was for.
        builder = builder.header(header::VARY, "Cookie");
    }
    builder
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// `Cache-Control` of an App's file (AP-142, AP-95, T-4914). `private` throughout: an App's files
/// are for its readers, never a shared cache. An index that carries a person is kept by nobody. A
/// republish reuses a path, so a file is revalidated; one whose name carries its own hash never
/// changes under that name and a public App's is kept, a WebAssembly module included. A
/// non-public App's hashed file is kept a minute, the longest a revoked reader still runs it.
fn cache_control(personal: bool, shared: bool, path: &str) -> &'static str {
    match (personal, shared, hashed(path)) {
        (true, _, _) => "private, no-store",
        (false, true, true) => "private, max-age=31536000, immutable",
        (false, true, false) => "no-cache",
        (false, false, true) => "private, max-age=60",
        (false, false, false) => "private, no-cache",
    }
}

/// The Portal's own origin, the one host that frames every App under its header (AP-122), when
/// Apps live on an origin of their own. Without one an App is served on the Portal's origin, and
/// a framed App would reach into the Portal's page with the viewer's session (T-2476): `None`.
pub(super) fn portal_origin(state: &AppState) -> Option<String> {
    let portal = state.config.public_base_url.origin();
    state
        .config
        .apps_url
        .as_ref()
        .filter(|apps| apps.origin() != portal)
        .map(|_| portal.ascii_serialization())
}

/// The origin one App is served on, `{name}.apps.{host}` under the configured apex with its
/// scheme and port (AP-133). `None` for a name no host could carry.
pub fn app_origin(apex: &url::Url, name: &str) -> Option<url::Url> {
    if !crate::resource::is_dns1123(name) {
        return None;
    }
    let host = apex.host_str()?;
    let mut origin = apex.clone();
    origin.set_host(Some(&format!("{name}.apps.{host}"))).ok()?;
    origin.set_path("/");
    origin.set_query(None);
    Some(origin)
}

/// An App asked for anywhere but its own origin answers a `308` there, above all on the
/// Portal's host or another App's (T-2476, AP-133); `None` when the request is on it, or when
/// no apps origin is configured and the App is served on every host.
pub(super) fn off_its_origin(
    state: &AppState,
    headers: &HeaderMap,
    name: &str,
    uri: &Uri,
) -> Option<Response> {
    let apex = state.config.apps_url.as_ref()?;
    let Some(origin) = app_origin(apex, name) else {
        return Some(StatusCode::NOT_FOUND.into_response());
    };
    (!is_origin(headers, &origin)).then(|| to_app_origin(&origin, name, uri))
}

/// Whether the request's `Host` names the apps origin: same host, ignoring case, and the same
/// port, the scheme's default when none is written. A request without a `Host` is not on it.
pub(super) fn is_origin(headers: &HeaderMap, origin: &url::Url) -> bool {
    let Some(authority) = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<axum::http::uri::Authority>().ok())
    else {
        return false;
    };
    origin
        .host_str()
        .is_some_and(|host| authority.host().eq_ignore_ascii_case(host))
        && authority.port_u16().or(origin.port_or_known_default()) == origin.port_or_known_default()
}

/// An app asked for anywhere but its own origin, above all on the Portal's (T-2476): a `308`
/// to the same path and query on the App's host, `/apps/{name}` dropped, so every link keeps
/// working and no byte of the bundle is ever served where it could reach the Portal API, or
/// another App's storage, with the viewer's session. The target is the configured origin, never
/// the request's `Host`, so this is no open redirect.
pub(super) fn to_app_origin(origin: &url::Url, name: &str, uri: &Uri) -> Response {
    let path = uri.path();
    let rest = path
        .strip_prefix(&format!("/apps/{name}"))
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
        .unwrap_or(path)
        .trim_start_matches('/');
    let query = uri
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let location = format!("{origin}{rest}{query}");
    match HeaderValue::from_str(&location) {
        Ok(location) => (
            StatusCode::PERMANENT_REDIRECT,
            [(header::LOCATION, location)],
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The published App manifest of this name, whatever project owns it. App names are
/// DNS-1123 labels and the app URL has no project segment (AP-14), so the name is what
/// identifies it here.
pub(super) fn published_app(
    state: &AppState,
    name: &str,
) -> Option<(String, AppSpec, Option<jc_core::Build>)> {
    if !crate::resource::is_dns1123(name) {
        return None;
    }
    let envelope = state
        .mirror
        .find(|env| env.kind == "App" && env.metadata.name == name)?;
    let build = envelope.status.as_ref().and_then(|s| s.build.clone());
    let project = envelope.metadata.namespace.clone()?;
    let spec: AppSpec = serde_json::from_value(envelope.spec).ok()?;
    (spec.lifecycle == AppLifecycle::Published).then_some((project, spec, build))
}

/// The configuration a static app starts from (SDK-02), `None` when it reads no endpoint.
///
/// A static app has no pod (AP-14). Its endpoints are, for each context space its `dataNeeds`
/// name, the App's own generated `app-{name}` when its grants are committed, or else every
/// Endpoint of the project on that space; then every Endpoint the project's
/// `SharedSpaceReference`s point at (EP-15). The first is the
/// primary. Slugs and spaces only, never a token: the reader's own session is what reaches each
/// endpoint, and the gateway's Policy decides what it may read (AP-07).
pub(super) fn served_config(
    mirror: &crate::store::Mirror,
    project: &str,
    name: &str,
    spec: &AppSpec,
    org_domain: &str,
) -> Option<serde_json::Value> {
    let unique = served_endpoints(mirror, project, name, spec);
    let Some(primary) = unique.first().cloned() else {
        if !spec.data_needs.is_empty() {
            // The page renders the SDK's configuration error; this is the line that says why.
            tracing::warn!(app = %name, project = %project, "static app reads no endpoint the platform holds");
        }
        return None;
    };
    let needs = serde_json::to_value(&spec.data_needs).unwrap_or_default();
    Some(serde_json::json!({
        "slug": primary.slug,
        "orgDomain": org_domain,
        "space": primary.space,
        "transport": "origin",
        "appName": name,
        "endpointName": primary.name,
        "endpoints": crate::agents::endpoints::config(&unique, &needs),
    }))
}

/// The endpoints an App reads, in the order [`served_config`] offers them, the primary first:
/// also the audiences its Keycloak client puts in a token, so the gateway admits that token on
/// each of them and nowhere else (AP-113).
pub(crate) fn served_endpoints(
    mirror: &crate::store::Mirror,
    project: &str,
    name: &str,
    spec: &AppSpec,
) -> Vec<crate::agents::endpoints::RunEndpoint> {
    use crate::agents::endpoints::{RunEndpoint, MAX_ENDPOINTS};
    use crate::api::assistant::ref_name;

    let of = |env: &crate::resource::ResourceEnvelope| {
        let slug = env.spec.get("slug").and_then(serde_json::Value::as_str)?;
        (!slug.is_empty()).then(|| RunEndpoint {
            name: env.metadata.name.clone(),
            slug: slug.to_owned(),
            space: ref_name(&env.spec["contextSpaceRef"]).unwrap_or_default(),
        })
    };
    let sorted = |mut envs: Vec<crate::resource::ResourceEnvelope>| {
        envs.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
        envs
    };
    let in_project = |env: &crate::resource::ResourceEnvelope, kind: &str| {
        env.kind == kind && env.metadata.namespace.as_deref() == Some(project)
    };

    // The App's own endpoint, once its grants are committed (T-2632): the one it reads through
    // (AP-04). Every other endpoint of that space is left out; offering them made the SDK refuse a
    // type several of them serve, so the app read nothing (T-2667).
    let own = mirror
        .get(project, "Endpoint", &format!("app-{name}"))
        .filter(|env| {
            env.metadata
                .annotations
                .get(jc_core::annotations::GENERATED_BY)
                .map(String::as_str)
                == Some(crate::apps::reconciler::GENERATOR)
        });
    // The first need's space is the App's own; a further one is read through its public
    // Endpoints alone (AP-04), as jc-core `served_endpoints` has it for the gateway.
    let first = spec
        .data_needs
        .first()
        .map(|need| need.context_space_ref.name());
    let mut found: Vec<RunEndpoint> = Vec::new();
    for need in &spec.data_needs {
        let space = need.context_space_ref.name();
        let further = first != Some(space);
        if let Some(endpoint) = own
            .as_ref()
            .filter(|env| ref_name(&env.spec["contextSpaceRef"]).as_deref() == Some(space))
            .and_then(of)
        {
            found.push(endpoint);
            continue;
        }
        let serving = mirror.matching(|env| {
            in_project(env, "Endpoint")
                && ref_name(&env.spec["contextSpaceRef"]).as_deref() == Some(space)
                && (!further || env.spec["audience"] == "public")
        });
        found.extend(sorted(serving).iter().filter_map(of));
    }
    // A reference names its endpoint by project and name, or by the slug the loader resolved that
    // to (EP-77), the form dev holds (T-3599); the gateway's audience table reads both.
    for reference in sorted(mirror.matching(|env| in_project(env, "SharedSpaceReference"))) {
        let target = &reference.spec["endpointRef"];
        let shared = match (
            target["project"].as_str(),
            target["name"].as_str(),
            reference.spec["endpointSlug"].as_str(),
        ) {
            (Some(source), Some(endpoint), _) => mirror.get(source, "Endpoint", endpoint),
            (_, _, Some(slug)) if !slug.is_empty() => mirror
                .matching(|env| env.kind == "Endpoint" && env.spec["slug"] == slug)
                .into_iter()
                .next(),
            _ => None,
        };
        found.extend(shared.as_ref().and_then(of));
    }

    let mut unique: Vec<RunEndpoint> = Vec::new();
    for endpoint in found {
        if !unique.iter().any(|seen| seen.slug == endpoint.slug) {
            unique.push(endpoint);
        }
    }
    if unique.len() > MAX_ENDPOINTS {
        // ponytail: the first five by space order; an app that needs more is a manifest to split.
        tracing::warn!(app = %name, count = unique.len(), "static app resolves more endpoints than one app may read");
        unique.truncate(MAX_ENDPOINTS);
    }
    unique
}

/// The Portal's page of the App's project, which the SDK's entity panel links an entity to
/// (SDK-40): only over https, the one scheme the SDK takes for it, and never with a token or a
/// person in it.
pub(super) fn portal_of(base: &url::Url, project: &str) -> Option<String> {
    (base.scheme() == "https")
        .then(|| format!("{}/projects/{project}", base.as_str().trim_end_matches('/')))
}

/// The index with its `#jc-config` element, first thing in the head so it is the one
/// `getElementById` finds even if a bundle ships a stale one of its own.
fn with_config(html: &str, config: &serde_json::Value) -> String {
    let element = format!(
        "<script id=\"jc-config\" type=\"application/json\">{}</script>",
        crate::agents::preview::script_json(config)
    );
    // The template's index carries the empty element for the Portal to fill (SDK-06); the filled
    // one goes first in the head and the empty one goes, so the page holds one `#jc-config`.
    let html = html.replacen(
        "<script id=\"jc-config\" type=\"application/json\"></script>",
        "",
        1,
    );
    let lower = html.to_ascii_lowercase();
    let at = lower
        .find("<head>")
        .map(|at| at + "<head>".len())
        .or_else(|| lower.find("</head>"))
        .unwrap_or(0);
    format!("{}{element}{}", &html[..at], &html[at..])
}

/// The directory one app is served from (AP-72, AP-102).
///
/// `status.build` names the build the manifest deploys, fetched into
/// `{apps_cache_dir}/{name}/{hex}/` ([`crate::apps::fetch`]). An app that names no build, or
/// names one this host does not hold yet, keeps serving `{apps_dir}/{name}/`, the bundle the
/// image ships, and [`build_missing`] is what says so on the App itself.
pub(super) fn app_root(
    shipped: Option<&FsPath>,
    cache: Option<&FsPath>,
    name: &str,
    build: Option<&jc_core::Build>,
) -> Option<PathBuf> {
    let held = cache
        .zip(build)
        .and_then(|(cache, build)| crate::apps::fetch::build_dir(cache, name, &build.digest))
        .and_then(|dir| dir.canonicalize().ok())
        .filter(|dir| dir.is_dir());
    held.or_else(|| shipped?.join(name).canonicalize().ok())
}

/// Whether this Portal's image ships the bundle of the app `name`: `{apps_dir}/{name}/index.html`
/// (AP-87). Only a DNS-1123 name is looked up, so a name never walks out of the directory.
pub fn ships_bundle(apps_dir: Option<&str>, name: &str) -> bool {
    crate::resource::is_dns1123(name)
        && apps_dir.is_some_and(|root| FsPath::new(root).join(name).join("index.html").is_file())
}

/// The refusal of an `App` that says the Portal ships its bundle when this Portal holds none
/// (AP-87): the annotation is what lets a published static App name no repository, so it is
/// believed only where the bundle is. Names the App being written and nothing else (PF-59).
pub fn unshipped_claim(
    apps_dir: Option<&str>,
    envelope: &crate::resource::ResourceEnvelope,
) -> Option<String> {
    use jc_core::kinds::app::SHIPPED_WITH_ANNOTATION;
    let name = &envelope.metadata.name;
    (envelope.kind == "App"
        && envelope
            .metadata
            .annotations
            .contains_key(SHIPPED_WITH_ANNOTATION)
        && !ships_bundle(apps_dir, name))
    .then(|| {
        format!(
            "annotation '{SHIPPED_WITH_ANNOTATION}' marks an application whose bundle the Portal \
             image ships, and this Portal ships none for '{name}': remove the annotation and \
             publish the App from its own repository with spec.source.git (AP-87)"
        )
    })
}

/// Every published app whose `status.build` names a build this host does not hold (AP-72).
///
/// The host keeps serving what it has; this is the list the reconciler turns into a red `Ready`
/// condition, so an operator sees a build that never arrived instead of a stale app that looks
/// healthy.
pub fn build_missing(apps_cache_dir: Option<&str>, mirror: &crate::store::Mirror) -> Vec<String> {
    let Some(cache) = apps_cache_dir.map(FsPath::new) else {
        return Vec::new();
    };
    let mut missing: Vec<String> = mirror
        .matching(|env| env.kind == "App")
        .into_iter()
        .filter(|env| {
            let Some(build) = env.status.as_ref().and_then(|s| s.build.as_ref()) else {
                return false;
            };
            !crate::apps::fetch::build_dir(cache, &env.metadata.name, &build.digest)
                .is_some_and(|dir| dir.is_dir())
        })
        .map(|env| env.metadata.name)
        .collect();
    missing.sort();
    missing
}

/// Every published App the host has nothing to serve for: no `status.build` (AP-13a) and no
/// bundle this Portal's image ships (AP-87). Its host answers 404, so it reads `Pending` with a
/// red `Ready` saying why, never `Live` (T-2989); the condition keeps the time of the run that
/// first said it. Returns the names it marked.
pub fn report_unbuilt(
    apps_dir: Option<&str>,
    mirror: &crate::store::Mirror,
    transitions: &crate::reconciler::transitions::Transitions,
) -> Vec<String> {
    let published = jc_core::kinds::AppLifecycle::Published.as_str();
    let unbuilt = mirror.matching(|env| {
        env.kind == "App"
            && env
                .spec
                .get("lifecycle")
                .and_then(serde_json::Value::as_str)
                == Some(published)
            && env
                .status
                .as_ref()
                .is_some_and(|status| status.build.is_none())
            && !ships_bundle(apps_dir, &env.metadata.name)
    });
    let mut names = Vec::new();
    for mut envelope in unbuilt {
        if let Some(status) = envelope.status.as_mut() {
            status.phase = crate::resource::Phase::Pending;
            status.conditions = vec![crate::reconciler::streams::make_condition(
                "Ready",
                "False",
                "NoBuild",
                "published, but no build has been written back to status.build, so its host \
                 serves nothing; rebuild the App from its page (AP-13a)",
            )];
        }
        transitions.keep(&mut envelope);
        names.push(envelope.metadata.name.clone());
        mirror.upsert(envelope);
    }
    names.sort();
    names
}

/// The app's Content Security Policy (AP-12). `default-src` and `connect-src` stay on `'self'`
/// plus whatever the manifest adds; `frame-ancestors` is the Portal's origin, which opens every
/// App under its header (AP-122), plus the declared origins of an embeddable app. With no Portal
/// origin to name (see `portal_origin`) it is `'none'`, or `'self'` for an embeddable app.
/// `basemap` is the project's basemap route prefix when the platform configures one (AP-67): the
/// one source `connect-src` and `img-src` gain for the map's style and tiles.
pub fn content_security_policy(
    spec: &AppSpec,
    portal_origin: Option<&str>,
    basemap: Option<&str>,
) -> String {
    let csp = spec.csp.as_ref();
    let mut connect = vec!["'self'".to_string()];
    if let Some(csp) = csp {
        connect.extend(
            csp.connect_src
                .iter()
                .filter(|s| *s != "self")
                .filter_map(|s| quoted(s)),
        );
    }
    let mut img = "'self' data: blob:".to_string();
    if let Some(prefix) = basemap {
        connect.push(prefix.to_string());
        img = format!("{img} {prefix}");
    }

    let mut frame_ancestors: Vec<String> = portal_origin.map(str::to_owned).into_iter().collect();
    if spec.embeddable {
        if let Some(csp) = csp {
            frame_ancestors.extend(
                csp.frame_ancestors
                    .iter()
                    // `'none'` beside an origin would still admit the origin; the Portal's
                    // frame stays, whatever the manifest says.
                    .filter(|s| *s != "none" && Some(s.as_str()) != portal_origin)
                    .filter_map(|s| quoted(s)),
            );
        }
    }
    let frame_ancestors = match (frame_ancestors.is_empty(), spec.embeddable) {
        (false, _) => frame_ancestors.join(" "),
        (true, true) => "'self'".to_owned(),
        (true, false) => "'none'".to_owned(),
    };

    // The frames an App shows of its own: itself and the https origins its manifest names, never
    // more (AP-12). `default-src 'self'` alone refused every nested frame.
    let mut frame = vec!["'self'".to_string()];
    if let Some(csp) = csp {
        frame.extend(
            csp.frame_src
                .iter()
                .filter(|s| *s != "self")
                .filter_map(|s| quoted(s)),
        );
    }

    // WebAssembly for the statically served interfaces, `ui` and `wasm` (AP-142, AP-148): a pod
    // App's server sends its own page, and its policy stays the narrower one.
    let scripts = if matches!(
        spec.class,
        jc_core::kinds::AppClass::Ui | jc_core::kinds::AppClass::Wasm
    ) {
        "'self' 'wasm-unsafe-eval'"
    } else {
        "'self'"
    };
    format!(
        "default-src 'self'; base-uri 'self'; object-src 'none'; script-src {scripts}; \
         style-src 'self' 'unsafe-inline'; img-src {img}; font-src 'self' data:; \
         form-action 'self'; connect-src {}; frame-src {}; frame-ancestors {frame_ancestors}",
        connect.join(" "),
        frame.join(" ")
    )
}

/// `policy` with the object store's public origin added to `connect-src`, exactly that origin,
/// for a `wasm` App that declares blob storage: its browser sends presigned uploads and downloads
/// there itself (AP-145, T-3359); so does any App that lists the `files` service, whose
/// `files.url()` hands out the same URLs (AP-170). Any other App, or no configured origin, keeps
/// `policy` as it is.
pub fn with_store(policy: &str, spec: &AppSpec, store: Option<&str>) -> String {
    let stores_files = (spec.class == jc_core::kinds::AppClass::Wasm
        && spec
            .storage
            .as_ref()
            .is_some_and(|storage| storage.blob.is_some()))
        || spec.services.contains(&jc_core::kinds::AppService::Files);
    match store {
        Some(origin) if stores_files => policy.replacen(
            "connect-src 'self'",
            &format!("connect-src 'self' {origin}"),
            1,
        ),
        _ => policy.to_owned(),
    }
}

/// Whether a bundle path is a build output named by its content, `assets/{name}-{hash}.{ext}` as
/// Vite writes it: a hash of eight or more letters, digits or `_`.
fn hashed(path: &str) -> bool {
    let Some(file) = path.strip_prefix("assets/").filter(|f| !f.contains('/')) else {
        return false;
    };
    let stem = file.split_once('.').map_or(file, |(stem, _)| stem);
    stem.rsplit_once('-').is_some_and(|(name, hash)| {
        !name.is_empty()
            && hash.len() >= 8
            && hash.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}

/// CSP keywords are quoted, origins are not. A source is written into the header only when it
/// is one https source expression and nothing else: jc-core refuses `*` but not a `;` or a space,
/// and either would let a manifest value open a directive of its own
/// (`https://a.example; script-src 'unsafe-inline'`). Anything else is left out (AP-12).
fn quoted(source: &str) -> Option<String> {
    match source {
        "self" | "none" => Some(format!("'{source}'")),
        other => {
            let allowed = |b: u8| b.is_ascii_alphanumeric() || b"-._~:/%".contains(&b);
            let url = url::Url::parse(other).ok()?;
            let plain = other.bytes().all(allowed)
                && url.scheme() == "https"
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none();
            if plain {
                Some(other.to_string())
            } else {
                tracing::warn!(source = %other.chars().take(80).collect::<String>(), "a CSP source of an App is not one https source expression and is left out");
                None
            }
        }
    }
}

/// Resolves a bundle-relative path inside one app's directory, or `None` if it would leave it.
/// `canonicalize` is what decides: it resolves `..` and every symlink, so a link pointing out of
/// the root is caught as surely as a literal traversal.
pub(super) fn resolve(app_root: &FsPath, path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.ends_with('/') {
        return None;
    }
    // `.jc/` carries what the bundle holds for the platform, a wasm App's server component
    // among it, and is never served (AP-151).
    if path.split('/').any(|segment| segment == BUNDLE_PRIVATE) {
        return None;
    }
    let file = app_root.join(path).canonicalize().ok()?;
    (file.starts_with(app_root) && file.is_file()).then_some(file)
}

/// The folder of a bundle the static host never serves (AP-151).
pub const BUNDLE_PRIVATE: &str = ".jc";

/// The digest the build lane recorded for this file, if any.
pub(super) fn integrity_of(app_root: &FsPath, path: &str) -> Option<String> {
    let manifest = resolve(app_root, INTEGRITY_MANIFEST)?;
    let digests: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(manifest).ok()?).ok()?;
    digests.get(path).cloned()
}

/// The Subresource Integrity form of a file's digest, `sha384-<base64>` (AP-12).
pub fn sri_sha384(bytes: &[u8]) -> String {
    format!(
        "sha384-{}",
        base64::engine::general_purpose::STANDARD.encode(Sha384::digest(bytes))
    )
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/apps/{name}/", get(serve_index))
        .route(
            "/apps/{name}/api/functions/{fn}",
            axum::routing::post(super::functions::call).layer(
                axum::extract::DefaultBodyLimit::max(super::functions::MAX_BODY_BYTES),
            ),
        )
        .route(
            "/apps/{name}/api/services/email/send",
            axum::routing::post(super::email::send).layer(axum::extract::DefaultBodyLimit::max(
                super::email::MAX_BODY_BYTES,
            )),
        )
        .route("/apps/{name}/api/services/jobs", get(super::jobs::list))
        .route(
            "/apps/{name}/api/services/ai/complete",
            axum::routing::post(super::ai::complete).layer(axum::extract::DefaultBodyLimit::max(
                super::ai::MAX_BODY_BYTES,
            )),
        )
        .route("/apps/{name}/api/services/files", get(super::files::list))
        .route(
            "/apps/{name}/api/services/files/{*path}",
            get(super::files::get)
                .put(super::files::put)
                .delete(super::files::delete)
                .post(super::files::url)
                .layer(axum::extract::DefaultBodyLimit::max(
                    super::files::MAX_BODY_BYTES,
                )),
        )
        .route("/apps/{name}/{*path}", get(serve))
        .route("/edge/apps/{name}/admit", get(admit))
        // The signed link in every App message: on the Portal's host, outside `/api/v1`, since a
        // mail client's one-click carries no session and no CSRF token; the signature is the
        // credential (RFC 8058).
        .route(
            "/mail/unsubscribe",
            get(super::email::unsubscribe_page).post(super::email::unsubscribe),
        )
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_entity_panel_links_to_the_project_in_the_portal_over_https_only() {
        let base = url::Url::parse("https://portal.dev.example.org/").expect("url");
        assert_eq!(
            portal_of(&base, "helsinki").as_deref(),
            Some("https://portal.dev.example.org/projects/helsinki")
        );
        let prefixed = url::Url::parse("https://dev.example.org/portal/").expect("url");
        assert_eq!(
            portal_of(&prefixed, "helsinki").as_deref(),
            Some("https://dev.example.org/portal/projects/helsinki")
        );
        let plain = url::Url::parse("http://localhost:8080/").expect("url");
        assert_eq!(portal_of(&plain, "helsinki"), None);
    }
    use super::*;
    use jc_core::kinds::ContentSecurityPolicy;

    fn spec() -> AppSpec {
        serde_json::from_value(serde_json::json!({
            "kind": "static",
            "source": { "path": "." },
            "build": { "node": "22" },
            "visibility": "public",
            "lifecycle": "published",
            "dataNeeds": []
        }))
        .expect("a minimal static app")
    }

    const PORTAL: &str = "https://portal.example.sk";

    /// T-3359: a wasm App that stores files reaches the store's public origin, that origin
    /// alone; one without blob storage, any other App, or no configured origin gains nothing.
    #[test]
    fn a_storing_wasm_app_reaches_the_stores_origin_alone() {
        let wasm = |storage: serde_json::Value| -> AppSpec {
            serde_json::from_value(serde_json::json!({
                "kind": "wasm",
                "source": { "path": "." },
                "build": { "node": "22", "rust": "1.90" },
                "dataNeeds": [],
                "storage": storage,
            }))
            .expect("a wasm app")
        };
        const STORE: &str = "https://files.example.sk";
        let storing = wasm(serde_json::json!({ "blob": {} }));
        let policy = content_security_policy(&storing, Some(PORTAL), None);
        let added = with_store(&policy, &storing, Some(STORE));
        assert!(
            added.contains("connect-src 'self' https://files.example.sk;"),
            "{added}"
        );
        assert_eq!(
            added.matches(STORE).count(),
            1,
            "connect-src alone: {added}"
        );
        assert!(
            added.contains("script-src 'self' 'wasm-unsafe-eval'"),
            "its interface computes like a ui App's"
        );
        let sql_only = wasm(serde_json::json!({ "sql": {} }));
        let policy = content_security_policy(&sql_only, Some(PORTAL), None);
        assert_eq!(with_store(&policy, &sql_only, Some(STORE)), policy);
        let mut ui = spec();
        let policy = content_security_policy(&ui, Some(PORTAL), None);
        assert_eq!(with_store(&policy, &ui, Some(STORE)), policy);
        ui.services = vec![jc_core::kinds::AppService::Files];
        assert!(
            with_store(&policy, &ui, Some(STORE))
                .contains("connect-src 'self' https://files.example.sk;"),
            "an App with the files service (AP-170)"
        );
        let policy = content_security_policy(&storing, Some(PORTAL), None);
        assert_eq!(with_store(&policy, &storing, None), policy);
    }

    #[test]
    fn a_plain_app_is_framed_by_the_portal_alone_and_talks_only_to_the_platform() {
        let csp = content_security_policy(&spec(), Some(PORTAL), None);
        assert!(
            csp.ends_with("frame-ancestors https://portal.example.sk"),
            "{csp}"
        );
        assert!(csp.contains("connect-src 'self';"), "{csp}");
        assert!(
            !csp.contains('*'),
            "a wildcard never reaches the header: {csp}"
        );
    }

    #[test]
    fn an_app_frames_itself_and_only_the_https_origins_it_declares() {
        // Nothing declared: its own frames only, and still no third party (AP-12).
        assert!(content_security_policy(&spec(), Some(PORTAL), None)
            .contains("frame-src 'self'; frame-ancestors"));
        let mut spec = spec();
        spec.csp = Some(ContentSecurityPolicy {
            frame_src: vec![
                "https://www.openstreetmap.org".into(),
                "self".into(),
                "https://video.example.fi/embed/".into(),
            ],
            connect_src: Vec::new(),
            frame_ancestors: Vec::new(),
        });
        let csp = content_security_policy(&spec, Some(PORTAL), None);
        assert!(
            csp.contains(
                "frame-src 'self' https://www.openstreetmap.org https://video.example.fi/embed/;"
            ),
            "{csp}"
        );
        // The frame an App shows is not a frame that may show the App.
        assert!(
            csp.ends_with("frame-ancestors https://portal.example.sk"),
            "{csp}"
        );
    }

    /// The sources of the policy's `script-src`, token by token: `'wasm-unsafe-eval'` is not
    /// `'unsafe-eval'`, which a substring check could not tell apart.
    fn script_sources(csp: &str) -> Vec<&str> {
        csp.split(';')
            .map(str::trim)
            .find_map(|d| d.strip_prefix("script-src "))
            .map(|d| d.split_whitespace().collect())
            .unwrap_or_default()
    }

    /// AP-142: every ui App may compile WebAssembly, and no script may `eval` or run inline.
    #[test]
    fn scripts_are_the_apps_own_and_webassembly_alone_is_compiled() {
        let csp = content_security_policy(&spec(), Some(PORTAL), None);
        let scripts = script_sources(&csp);
        assert_eq!(scripts, ["'self'", "'wasm-unsafe-eval'"], "{csp}");
        assert!(!scripts.contains(&"'unsafe-eval'") && !scripts.contains(&"'unsafe-inline'"));
        assert_eq!(
            script_sources("default-src 'self'; script-src 'self' 'unsafe-eval'"),
            ["'self'", "'unsafe-eval'"]
        );
        // A pod App keeps the narrower policy: no WebAssembly compilation it never asked for.
        let mut pod = spec();
        pod.class = jc_core::kinds::AppClass::UiRust;
        let csp = content_security_policy(&pod, Some(PORTAL), None);
        assert_eq!(script_sources(&csp), ["'self'"], "{csp}");
    }

    #[test]
    fn only_a_file_named_by_its_hash_is_kept() {
        for path in [
            "assets/index-B4tq81Zr.js",
            "assets/compute_bg-Cx8f2kQ9.wasm",
            "assets/worker-a1b2c3d4_e5.js",
        ] {
            assert!(hashed(path), "{path}");
        }
        for path in [
            "index.html",
            "bundle.js",
            "assets/logo.svg",
            "assets/a-short.js",
            "assets/x/y-B4tq81Zr.js",
            "img/index-B4tq81Zr.js",
            "assets/-B4tq81Zr.js",
            "assets/index-B4tq8.1Zr",
        ] {
            assert!(!hashed(path), "{path}");
        }
    }

    #[test]
    fn a_manifest_value_never_opens_a_directive_of_its_own() {
        // jc-core refuses `*` but not `;` or a space: the header must not carry either (AP-12).
        let mut spec = spec();
        spec.embeddable = true;
        let hostile = [
            "https://a.example; script-src 'unsafe-inline' 'unsafe-eval'",
            "https://a.example 'unsafe-inline'",
            "https://a.example,https://b.example",
            "http://plain.example",
            "https://user:pw@a.example",
            "https://a.example/?q=1",
            "javascript:alert(1)",
            "data:",
            "",
        ];
        spec.csp = Some(ContentSecurityPolicy {
            frame_src: hostile.iter().map(|s| s.to_string()).collect(),
            connect_src: hostile.iter().map(|s| s.to_string()).collect(),
            frame_ancestors: hostile.iter().map(|s| s.to_string()).collect(),
        });
        let csp = content_security_policy(&spec, Some(PORTAL), None);
        assert_eq!(csp.matches("script-src").count(), 1, "{csp}");
        assert_eq!(
            script_sources(&csp),
            ["'self'", "'wasm-unsafe-eval'"],
            "{csp}"
        );
        assert!(!csp.contains("a.example"), "{csp}");
        assert!(
            !csp.contains("plain.example") && !csp.contains("javascript"),
            "{csp}"
        );
        assert!(csp.contains("frame-src 'self';"), "{csp}");
        assert!(csp.contains("connect-src 'self';"), "{csp}");
        assert!(
            csp.ends_with("frame-ancestors https://portal.example.sk"),
            "{csp}"
        );
    }

    #[test]
    fn an_embeddable_app_adds_only_its_declared_frame_ancestors() {
        let mut spec = spec();
        spec.embeddable = true;
        spec.csp = Some(ContentSecurityPolicy {
            frame_src: Vec::new(),
            connect_src: vec!["self".into()],
            frame_ancestors: vec![
                "https://city.example.sk".into(),
                "none".into(),
                PORTAL.into(),
            ],
        });
        let csp = content_security_policy(&spec, Some(PORTAL), None);
        assert!(
            csp.ends_with("frame-ancestors https://portal.example.sk https://city.example.sk"),
            "{csp}"
        );
        assert!(csp.contains("connect-src 'self';"), "{csp}");
    }

    #[test]
    fn a_configured_basemap_adds_its_route_and_nothing_else() {
        let prefix = "https://portal.example/api/v1/projects/bbsk/basemap/";
        let csp = content_security_policy(&spec(), Some(PORTAL), Some(prefix));
        assert!(
            csp.contains(&format!("connect-src 'self' {prefix};")),
            "{csp}"
        );
        assert!(
            csp.contains(&format!("img-src 'self' data: blob: {prefix};")),
            "{csp}"
        );
        // Only those two directives widen, and only by the one prefix.
        assert_eq!(csp.matches(prefix).count(), 2, "{csp}");
        assert!(csp.contains("default-src 'self';"), "{csp}");
    }

    #[test]
    fn a_declared_frame_ancestor_is_ignored_while_the_app_is_not_embeddable() {
        // spec.embeddable is the switch (AP-12); a frameAncestors list alone must not open it.
        let mut spec = spec();
        spec.csp = Some(ContentSecurityPolicy {
            frame_src: Vec::new(),
            connect_src: Vec::new(),
            frame_ancestors: vec!["https://elsewhere.example".into()],
        });
        assert!(content_security_policy(&spec, Some(PORTAL), None)
            .ends_with("frame-ancestors https://portal.example.sk"));
    }

    #[test]
    fn without_an_apps_origin_of_its_own_nothing_but_an_embedder_frames_an_app() {
        // The App would share the Portal's origin, so the Portal never frames it (T-2476).
        assert!(content_security_policy(&spec(), None, None).ends_with("frame-ancestors 'none'"));
        let mut spec = spec();
        spec.embeddable = true;
        assert!(content_security_policy(&spec, None, None).ends_with("frame-ancestors 'self'"));
        spec.csp = Some(ContentSecurityPolicy {
            frame_src: Vec::new(),
            connect_src: Vec::new(),
            frame_ancestors: vec!["https://city.example.sk".into()],
        });
        assert!(content_security_policy(&spec, None, None)
            .ends_with("frame-ancestors https://city.example.sk"));
    }

    #[test]
    fn the_digest_is_the_subresource_integrity_form() {
        // sha384 of the empty input, the value a browser would compute for an empty asset.
        assert_eq!(
            sri_sha384(b""),
            "sha384-OLBgp1GsljhM2TJ+sbHjaiH9txEUvgdDTAzHv2P24donTt6/529l+9Ua0vFImLlb"
        );
    }

    #[test]
    fn the_configuration_opens_the_head_and_cannot_close_its_element() {
        let config = serde_json::json!({ "appName": "</script><script>alert(1)</script>" });
        let html = with_config(
            "<!doctype html><HTML><Head><title>x</title></head></html>",
            &config,
        );
        assert!(
            html.starts_with("<!doctype html><HTML><Head><script id=\"jc-config\""),
            "{html}"
        );
        assert_eq!(html.matches("</script>").count(), 1, "{html}");

        // A document with no head at all still gets it, before anything else.
        let bare = with_config("<p>x</p>", &config);
        assert!(bare.starts_with("<script id=\"jc-config\""), "{bare}");
    }

    /// SDK-06: the template's index carries the empty element the Portal fills; the served page
    /// holds one `#jc-config`, the filled one, and never a second the app might read instead.
    #[test]
    fn the_templates_empty_configuration_is_replaced_not_doubled() {
        let config = serde_json::json!({ "appName": "alerts" });
        let html = with_config(
            "<html><head><title>x</title>\n    <script id=\"jc-config\" type=\"application/json\"></script>\n</head><body></body></html>",
            &config,
        );
        assert_eq!(html.matches("id=\"jc-config\"").count(), 1, "{html}");
        assert!(html.contains("{\"appName\":\"alerts\"}</script>"), "{html}");
    }

    #[test]
    fn a_path_leaving_the_app_root_does_not_resolve() {
        let dir = std::env::temp_dir().join(format!("jc-apps-{}", std::process::id()));
        let app = dir.join("demo");
        std::fs::create_dir_all(&app).expect("app dir");
        std::fs::write(dir.join("secret.txt"), b"not yours").expect("neighbour file");
        std::fs::write(app.join("index.html"), b"<!doctype html>").expect("index");

        let root = app_root(Some(&dir), None, "demo", None).expect("the app directory");
        assert!(resolve(&root, "index.html").is_some());
        assert!(resolve(&root, "../secret.txt").is_none());
        assert!(resolve(&root, "/etc/passwd").is_none());
        assert!(resolve(&root, "").is_none(), "a directory is not a file");
        // AP-151: a wasm App's server component travels in the bundle and is never served.
        std::fs::create_dir_all(app.join(".jc")).expect(".jc");
        std::fs::write(app.join(".jc/component.wasm"), b"\0asm").expect("component");
        assert!(resolve(&root, ".jc/component.wasm").is_none());
        assert!(resolve(&root, "assets/../.jc/component.wasm").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
