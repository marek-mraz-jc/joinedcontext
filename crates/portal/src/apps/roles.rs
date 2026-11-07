//! A person's roles in one application, and the page that refuses them (ADR-N-030, AP-92,
//! AP-93, AP-95).
//!
//! Each App logs people in with its own Keycloak client, `app-{name}`, whose tokens carry the
//! person's roles in `resource_access.app-{name}.roles`; the reconciler writes those roles and
//! their mappings from the manifest (AP-113). A person is believed only on such a token, verified
//! here: issued for `app-{name}` and obtained by it (`azp`). Nothing else counts: no other
//! client's roles, no realm role, no cookie, no query, nothing in the bundle (AP-92).

use std::collections::BTreeMap;

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use jc_core::kinds::{AppSpec, AppVisibility};

use crate::auth::session::{Identity, EDGE_TOKEN_HEADER};
use crate::error::ApiError;
use crate::state::AppState;

/// The locales the refusal page speaks, the Portal's own (UI-12); the first is the fallback.
const LOCALES: [&str; 4] = ["en", "sk", "cs", "de"];

/// A person as one App knows them: who they are, and the App's roles they hold in the order
/// `spec.roles` declares them. A role the manifest no longer declares is not one.
#[derive(Debug, Clone)]
pub struct AppPerson {
    pub identity: Identity,
    pub roles: Vec<String>,
}

/// The token a request presents: `Authorization: Bearer` (an App's backend asking about its
/// caller, AP-109) or, where the edge is trusted, the edge's `X-Access-Token`.
pub fn presented<'a>(state: &AppState, headers: &'a HeaderMap) -> Option<&'a str> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let edge = || {
        state
            .config
            .trust_edge_token
            .then(|| headers.get(&EDGE_TOKEN_HEADER))
            .flatten()
            .and_then(|value| value.to_str().ok())
    };
    bearer
        .or_else(edge)
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

/// The caller of App `app`, verified on a token of its own client (AP-92). `401` without one, or
/// with one that is expired, signed out, for another client or obtained by one.
pub async fn verified(
    state: &AppState,
    headers: &HeaderMap,
    app: &str,
) -> Result<(Identity, Vec<String>), ApiError> {
    let token = presented(state, headers).ok_or(ApiError::Unauthorized)?;
    let verifier = state.bearer.as_ref().ok_or(ApiError::Unauthorized)?;
    let client = crate::reconciler::app_clients::client_id(app);
    let (session, roles) = verifier.verify_app_token(token, &client).await?;
    if state.is_revoked(&session) {
        return Err(ApiError::Unauthorized);
    }
    Ok((session.identity, roles))
}

impl AppPerson {
    /// The verified caller with the roles the token holds that `spec` declares.
    pub fn of(spec: &AppSpec, (identity, held): (Identity, Vec<String>)) -> Self {
        let roles = spec
            .roles
            .iter()
            .filter(|role| held.contains(&role.name))
            .map(|role| role.name.clone())
            .collect();
        Self { identity, roles }
    }
}

/// The caller of the published App `app` if a valid token of its client says who they are;
/// `None` for an anonymous visitor and for a token that is not the App's.
pub async fn person(
    state: &AppState,
    headers: &HeaderMap,
    spec: &AppSpec,
    app: &str,
) -> Option<AppPerson> {
    verified(state, headers, app)
        .await
        .ok()
        .map(|verified| AppPerson::of(spec, verified))
}

/// `#jc-config.user`, a function's `request.user` and `GET …/apps/{name}/me`:
/// `{id, name, email, roles}` for a person, `null` for an anonymous visitor, never a token
/// (AP-95, AP-109, SDK-35, SDK-37).
pub fn app_user(person: Option<&AppPerson>) -> serde_json::Value {
    person.map_or(serde_json::Value::Null, |person| {
        let identity = &person.identity;
        serde_json::json!({
            "id": identity.subject,
            "name": identity.name.clone().unwrap_or_else(|| identity.username.clone()),
            "email": identity.email,
            "roles": person.roles,
        })
    })
}

/// The annotation naming the Keycloak subject who created an App: who a `private` App opens for
/// (Architecture/16 §1). The Portal writes it on the create from the proposer's session and keeps
/// it on every update, so no manifest chooses its own owner.
pub const OWNER_ANNOTATION: &str = "joinedcontext.com/owner";

/// Sets [`OWNER_ANNOTATION`] on an App about to be proposed: the proposer on a create, the stored
/// value on an update, whatever the proposal wrote.
pub fn stamp_owner(
    annotations: &mut BTreeMap<String, String>,
    stored: Option<&BTreeMap<String, String>>,
    proposer: &Identity,
) {
    let owner = match stored {
        Some(stored) => stored.get(OWNER_ANNOTATION).cloned(),
        None => Some(proposer.subject.clone()),
    };
    match owner {
        Some(owner) => annotations.insert(OWNER_ANNOTATION.to_owned(), owner),
        None => annotations.remove(OWNER_ANNOTATION),
    };
}

/// Where a signed-in person stands toward one App's project, the facts its visibility asks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Standing {
    /// The subject [`OWNER_ANNOTATION`] names.
    pub owner: bool,
    /// The App carries no [`OWNER_ANNOTATION`] (written before T-4910) and the person may approve
    /// an App in its project: its administrator, who can give it an owner (F-BNT3-009). Who may
    /// only propose one is no owner of it.
    pub admin: bool,
    /// A binding in force covers the App's project, or the bootstrap group (PF-59).
    pub project_member: bool,
    /// A binding in force anywhere in the organization (DM-75).
    pub organization_member: bool,
}

/// Whether a person with `standing` opens an App of `visibility`; deny by default. `public` is
/// anyone, `roles` a role holder (AP-93), `organization` an organization member, `project` a
/// member of its project, `private` its owner (Architecture/16 §1).
pub fn admits(visibility: AppVisibility, person: Option<&AppPerson>, standing: Standing) -> bool {
    match (visibility, person) {
        (AppVisibility::Public, _) => true,
        (_, None) => false,
        (AppVisibility::Roles, Some(person)) => !person.roles.is_empty(),
        (AppVisibility::Organization, Some(_)) => standing.organization_member,
        (AppVisibility::Project, Some(_)) => standing.project_member,
        (AppVisibility::Private, Some(_)) => standing.owner || standing.admin,
    }
}

/// Whether the person may open App `name` of `project` at all: the one check of the static host,
/// functions, services and `…/apps/{name}/me` (T-4910). Each answers a refusal as the same `404` as
/// a name never created, except the `403` page of a `roles` App (AP-18, AP-93).
pub fn may_open(
    state: &AppState,
    project: &str,
    name: &str,
    spec: &AppSpec,
    person: Option<&AppPerson>,
) -> bool {
    let standing = match (spec.visibility, person) {
        (AppVisibility::Public | AppVisibility::Roles, _) | (_, None) => Standing::default(),
        (_, Some(person)) => standing(state, project, name, &person.identity),
    };
    admits(spec.visibility, person, standing)
}

fn standing(state: &AppState, project: &str, name: &str, identity: &Identity) -> Standing {
    let rights = crate::permissions::for_request(state, identity, project);
    let owner = state
        .mirror
        .get(project, "App", name)
        .and_then(|app| app.metadata.annotations.get(OWNER_ANNOTATION).cloned());
    Standing {
        owner: owner.as_deref() == Some(identity.subject.as_str()),
        admin: owner.is_none() && rights.may("App", jc_core::kinds::Verb::Approve),
        project_member: rights.may_read_project(),
        organization_member: crate::permissions::is_organization_member(state, identity),
    }
}

/// The `403` a person without a role of a `visibility: roles` app gets (AP-93): the app's title,
/// its roles with their titles, and who grants them, in the person's language. It names no
/// member, so nobody learns who holds a role by being refused.
pub fn refusal(
    spec: &AppSpec,
    name: &str,
    title: Option<&jc_core::i18n::Text>,
    project: &str,
    headers: &HeaderMap,
    portal_origin: Option<&str>,
) -> Response {
    let locale = locale(headers);
    let pick = |map: &BTreeMap<String, String>, fallback: &str| {
        map.get(locale)
            .or_else(|| map.get(LOCALES[0]))
            .or_else(|| map.values().next())
            .cloned()
            .unwrap_or_else(|| fallback.to_owned())
    };
    let app = escape(title.map_or(name, |title| title.resolve(&[locale.to_owned()], name)));
    let roles: String = spec
        .roles
        .iter()
        .map(|role| format!("<li>{}</li>", escape(&pick(&role.title, &role.name))))
        .collect();
    let (heading, ask) = sentences(locale);
    let page = format!(
        "<!doctype html><html lang=\"{locale}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{app}</title></head><body><main><h1>{app}</h1><p>{heading}</p><ul>{roles}</ul>\
         <p>{}</p></main></body></html>",
        ask.replace("{project}", &escape(project)),
    );
    let mut response = (StatusCode::FORBIDDEN, page).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    // The Portal frames the page when it opens the App (AP-122), so the refusal shows there.
    if let Ok(csp) = HeaderValue::from_str(&format!(
        "default-src 'none'; frame-ancestors {}",
        portal_origin.unwrap_or("'none'")
    )) {
        headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    response
}

/// The 404 a person's browser gets on an App's host (T-4200): a sentence in their language and the
/// way to the App's start, instead of the problem body every other caller still gets. It names no
/// App, so an unpublished one, one the person may not see and a name never created answer the
/// same page as a missing file (AP-18). `None` when the request does not ask for a page.
pub fn missing_page(
    headers: &HeaderMap,
    start: &str,
    portal_origin: Option<&str>,
) -> Option<Response> {
    let wants_page = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));
    if !wants_page {
        return None;
    }
    let locale = locale(headers);
    let (heading, link) = match locale {
        "sk" => (
            "Táto stránka v aplikácii neexistuje.",
            "Prejsť na úvod aplikácie",
        ),
        "cs" => (
            "Tato stránka v aplikaci neexistuje.",
            "Přejít na úvod aplikace",
        ),
        "de" => (
            "Diese Seite gibt es in der Anwendung nicht.",
            "Zum Anfang der Anwendung",
        ),
        _ => (
            "This page does not exist in the application.",
            "Go to the application's start",
        ),
    };
    let page = format!(
        "<!doctype html><html lang=\"{locale}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{heading}</title></head><body><main><h1>{heading}</h1>\
         <p><a href=\"{}\">{link}</a></p></main></body></html>",
        escape(start),
    );
    let mut response = (StatusCode::NOT_FOUND, page).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Ok(csp) = HeaderValue::from_str(&format!(
        "default-src 'none'; frame-ancestors {}",
        portal_origin.unwrap_or("'none'")
    )) {
        headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    Some(response)
}

/// The two sentences of the refusal page per locale.
fn sentences(locale: &str) -> (&'static str, &'static str) {
    match locale {
        "sk" => (
            "Túto aplikáciu môže otvoriť iba osoba s jednou z týchto rolí:",
            "Požiadajte správcu projektu {project}, aby vás pridal do jednej z týchto rolí.",
        ),
        "cs" => (
            "Tuto aplikaci může otevřít jen osoba s jednou z těchto rolí:",
            "Požádejte správce projektu {project}, aby vás přidal do jedné z těchto rolí.",
        ),
        "de" => (
            "Diese Anwendung öffnet nur, wer eine dieser Rollen hat:",
            "Bitten Sie eine verantwortliche Person des Projekts {project}, Sie einer dieser Rollen hinzuzufügen.",
        ),
        _ => (
            "Only a person holding one of these roles can open this application:",
            "Ask a steward of the project {project} to add you to one of these roles.",
        ),
    }
}

/// The first locale of `Accept-Language` the page speaks, by primary subtag; English otherwise.
/// Quality values are read in the order written, which is the order every browser sends.
fn locale(headers: &HeaderMap) -> &'static str {
    headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .into_iter()
        .flat_map(|v| v.split(','))
        .filter_map(|tag| tag.split(';').next()?.trim().split('-').next())
        .find_map(|primary| {
            LOCALES
                .iter()
                .find(|l| l.eq_ignore_ascii_case(primary))
                .copied()
        })
        .unwrap_or(LOCALES[0])
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> AppSpec {
        serde_json::from_value(serde_json::json!({
            "kind": "static",
            "source": { "path": "." },
            "build": { "node": "22" },
            "visibility": "roles",
            "lifecycle": "published",
            "roles": [
                { "name": "viewer", "title": { "en": "Viewer", "sk": "Čitateľ" } },
                { "name": "steward", "title": { "en": "Steward <b>" } }
            ],
            "access": [
                { "role": "viewer", "subjects": [{ "group": "helsinki-operations" }] },
                { "role": "steward", "subjects": [{ "user": "Jana.Kovacova@hel.fi" }] }
            ],
            "dataNeeds": []
        }))
        .expect("an app with roles")
    }

    fn person(roles: &[&str]) -> AppPerson {
        AppPerson::of(
            &spec(),
            (
                Identity {
                    client: None,
                    subject: "5f0c".into(),
                    username: "x@hel.fi".into(),
                    email: Some("x@hel.fi".into()),
                    name: None,
                    roles: vec!["platform-admin".into()],
                    groups: vec!["helsinki-operations".into()],
                    second_factor: false,
                },
                roles.iter().map(|role| (*role).to_owned()).collect(),
            ),
        )
    }

    /// AP-92: the token's roles count in the order `spec.roles` declares them, and a role the
    /// manifest does not declare, a realm role or a group counts for nothing.
    #[test]
    fn a_person_holds_the_declared_roles_their_token_carries() {
        assert_eq!(person(&["steward", "viewer"]).roles, ["viewer", "steward"]);
        assert_eq!(person(&["viewer", "retired-role"]).roles, ["viewer"]);
        assert!(
            person(&[]).roles.is_empty(),
            "no group or realm role counts"
        );
    }

    /// AP-95, SDK-35: the application learns its own roles, never the platform's, and no token.
    #[test]
    fn the_served_user_carries_the_app_roles_and_null_for_nobody() {
        let user = app_user(Some(&person(&["viewer"])));
        assert_eq!(
            user,
            serde_json::json!({ "id": "5f0c", "name": "x@hel.fi", "email": "x@hel.fi", "roles": ["viewer"] })
        );
        assert_eq!(app_user(Some(&person(&[])))["roles"], serde_json::json!([]));
        assert_eq!(app_user(None), serde_json::Value::Null);
    }

    /// T-4910: each visibility against each caller; deny by default. A role holder with no binding
    /// opens only `roles` and `public`, an owner outside the project only their `private` App.
    #[test]
    fn who_may_open_follows_the_visibility_and_the_callers_standing() {
        use AppVisibility::{Organization, Private, Project, Public, Roles};
        let holder = person(&["viewer"]);
        let nobody = person(&[]);
        let realm = Standing::default();
        let org = Standing {
            organization_member: true,
            ..realm
        };
        let member = Standing {
            project_member: true,
            ..org
        };
        let owner = Standing {
            owner: true,
            ..realm
        };
        let admin = Standing {
            admin: true,
            ..member
        };
        // (caller, standing, [public, roles, organization, project, private])
        let table: [(&str, Option<&AppPerson>, Standing, [bool; 5]); 7] = [
            ("anonymous", None, realm, [true, false, false, false, false]),
            (
                "realm user of another organization",
                Some(&nobody),
                realm,
                [true, false, false, false, false],
            ),
            (
                "same organization, other project",
                Some(&nobody),
                org,
                [true, false, true, false, false],
            ),
            (
                "project member",
                Some(&nobody),
                member,
                [true, false, true, true, false],
            ),
            (
                "owner",
                Some(&nobody),
                owner,
                [true, false, false, false, true],
            ),
            (
                "administrator of an App without an owner",
                Some(&nobody),
                admin,
                [true, false, true, true, true],
            ),
            (
                "role holder",
                Some(&holder),
                realm,
                [true, true, false, false, false],
            ),
        ];
        for (caller, who, standing, expected) in table {
            for (visibility, expected) in [Public, Roles, Organization, Project, Private]
                .into_iter()
                .zip(expected)
            {
                assert_eq!(
                    admits(visibility, who, standing),
                    expected,
                    "{caller} opening {visibility:?}"
                );
            }
        }
    }

    /// T-4910: the owner is the proposer of the create and the stored owner after, whatever the
    /// manifest wrote; an App stored without one stays without one.
    #[test]
    fn the_owner_is_the_creator_and_no_manifest_picks_it() {
        let olga = person(&[]).identity;
        let forged = BTreeMap::from([(OWNER_ANNOTATION.to_owned(), "sub-mallory".to_owned())]);
        let mut created = forged.clone();
        stamp_owner(&mut created, None, &olga);
        assert_eq!(created[OWNER_ANNOTATION], olga.subject);
        let stored = BTreeMap::from([(OWNER_ANNOTATION.to_owned(), "sub-olga".to_owned())]);
        let mut updated = forged.clone();
        stamp_owner(&mut updated, Some(&stored), &olga);
        assert_eq!(updated[OWNER_ANNOTATION], "sub-olga");
        let mut legacy = forged;
        stamp_owner(&mut legacy, Some(&BTreeMap::new()), &olga);
        assert!(!legacy.contains_key(OWNER_ANNOTATION));
    }

    /// AP-93: the refusal names the app and its roles in the person's language, escapes what the
    /// manifest wrote, is never cached and names no member.
    #[test]
    fn the_refusal_page_names_the_roles_and_no_member() {
        let spec = spec();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("sk-SK,sk;q=0.9,en;q=0.8"),
        );
        let title = jc_core::i18n::Text::from("Alerts");
        let response = refusal(
            &spec,
            "alerts",
            Some(&title),
            "helsinki",
            &headers,
            Some("https://portal.hel.fi"),
        );
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        // Shown inside the Portal's frame when the App is opened there (AP-122), nowhere else.
        assert_eq!(
            response.headers()[header::CONTENT_SECURITY_POLICY],
            "default-src 'none'; frame-ancestors https://portal.hel.fi"
        );
        let body = futures_util::FutureExt::now_or_never(axum::body::to_bytes(
            response.into_body(),
            usize::MAX,
        ))
        .expect("a ready body")
        .expect("the page");
        let page = String::from_utf8(body.to_vec()).expect("utf-8");
        assert!(page.contains("<h1>Alerts</h1>"), "{page}");
        assert!(page.contains("<li>Čitateľ</li>"), "{page}");
        assert!(page.contains("<li>Steward &lt;b&gt;</li>"), "{page}");
        assert!(page.contains("správcu projektu helsinki"), "{page}");
        assert!(
            !page.to_lowercase().contains("jana"),
            "no member is named: {page}"
        );
        assert!(
            !page.contains("helsinki-operations"),
            "no group is named: {page}"
        );
    }

    #[test]
    fn the_locale_falls_back_to_english() {
        let mut headers = HeaderMap::new();
        assert_eq!(locale(&headers), "en");
        headers.insert(
            header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("fi-FI, de;q=0.5"),
        );
        assert_eq!(locale(&headers), "de");
    }
}
