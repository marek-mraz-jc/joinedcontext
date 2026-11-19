//! The signed-in catalogue (ADR-N-067 §2.4, T-6411).
//!
//! Every public Endpoint of the installation, and for a signed-in person every other Endpoint the
//! gateway lets them read, each marked by what grants it: `public`, `organization`, a `group` or
//! a grant to them alone (`granted`). The Portal decides nothing: it asks the gateway's
//! `access/simulate` with no action once per non-public Endpoint, for the caller exactly as their
//! own token names them, and keeps an Endpoint the gateway admits them to and grants something
//! on (the Access explorer's question, API/01 §41). Rows point at the Endpoint, read live with
//! the person's own session; nothing is copied (owner 2026-10-22).

use std::future::Future;

use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::api::access_explore::EXPLORE_AT_ONCE;
use crate::api::access_simulate::ask_gateway;
use crate::auth::session::{Front, Session, EDGE_TOKEN_HEADER};
use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::ListOptions;

/// One dataset the caller may read.
#[derive(Debug, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReadableDataset {
    pub project: String,
    /// The Endpoint's manifest name.
    pub name: String,
    pub slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The surfaces it serves; the page builds each URL under `/api/endpoint/{slug}`.
    pub representations: Vec<String>,
    /// `public`, `organization` (every member of its Organization), `group` (see `group`) or
    /// `granted` (a grant to the caller, a role of theirs or their account).
    pub access: String,
    /// The group whose members it is granted to, when `access` is `group`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadableCatalogue {
    pub datasets: Vec<ReadableDataset>,
    /// Non-public Endpoints the gateway did not answer about; they are left out, never guessed.
    pub unanswered: usize,
}

#[utoipa::path(
    get,
    path = "/api/v1/catalogue/readable",
    summary = "Datasets You May Read",
    description = "Every public Endpoint and, signed in, every other Endpoint the gateway lets the caller read, each marked public, organization, the granting group or granted (ADR-N-067 §2.4). The gateway decides each Endpoint for the caller as their own token names them; an Endpoint it does not answer about is counted in `unanswered` and left out. Anonymous callers get the public Endpoints. Changes no data.",
    tag = "ckan",
    responses(
        (status = 200, description = "The datasets the caller may read", body = ReadableCatalogue),
    )
)]
pub async fn get_readable(
    user: Result<CurrentUser, ApiError>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<ReadableCatalogue>, ApiError> {
    // No session, or a token that does not verify: the caller is the public, who reads only the
    // public Endpoints, so a broken token never widens anything.
    let subject = match user {
        Ok(user) => Some(subject_of(&state, &user.0, &headers)),
        Err(ApiError::Unauthorized) => None,
        Err(other) => return Err(other),
    };
    let ask = |slug: String, question: Value| {
        let state = state.clone();
        async move { ask_gateway(&state, &slug, &question).await }
    };
    Ok(Json(readable(&state, subject.as_ref(), ask).await))
}

/// The catalogue of a signed-in person, as the gateway answers for them: what a member connector
/// may name (ADR-N-067 §2.4, T-6413).
pub(crate) async fn readable_by(
    state: &AppState,
    session: &Session,
    headers: &HeaderMap,
) -> ReadableCatalogue {
    let subject = subject_of(state, session, headers);
    let ask = |slug: String, question: Value| {
        let state = state.clone();
        async move { ask_gateway(&state, &slug, &question).await }
    };
    readable(state, Some(&subject), ask).await
}

/// The organization claims of the token this request was verified with: `jc_org` and
/// `jc_groups` (ADR-N-067 2.2), which the session's identity does not keep.
#[derive(Debug, Default, Deserialize, PartialEq)]
struct OrganizationClaims {
    #[serde(default)]
    jc_org: Vec<String>,
    #[serde(default)]
    jc_groups: Option<Vec<String>>,
}

/// The caller as the gateway's simulator takes them: their Keycloak id, groups and realm roles
/// from the verified session, and `jc_org`/`jc_groups` from the token the extractor verified in
/// this same request (a bearer or the edge's header), else the cookie session's ID token, whose
/// signature the login checked. A cookie never carries that token: the Portal keeps it beside
/// the session ([`AppState::session_id_token`], T-6549), and without it (a restart, another
/// replica) the person is named with no Organization standing, as a token without the claim.
pub(crate) fn subject_of(state: &AppState, session: &Session, headers: &HeaderMap) -> Value {
    let kept;
    let token = match Front::of(headers, state.config.trust_edge_token) {
        Front::Bearer => headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer ")),
        Front::Edge => headers
            .get(&EDGE_TOKEN_HEADER)
            .and_then(|v| v.to_str().ok()),
        Front::Portal => {
            kept = state
                .session_id_token(session)
                .unwrap_or_else(|| session.id_token.clone());
            Some(kept.as_str())
        }
    };
    let claims: OrganizationClaims = token
        .and_then(|token| crate::auth::oidc::jwt_payload(token.trim()))
        .unwrap_or_default();
    let identity = &session.identity;
    let mut subject = json!({
        "user": identity.subject,
        "groups": identity.groups,
        "roles": identity.roles,
    });
    if !claims.jc_org.is_empty() {
        subject["organizations"] = json!(claims.jc_org);
    }
    if let Some(groups) = claims.jc_groups {
        subject["organizationGroups"] = json!(groups);
    }
    subject
}

/// The catalogue for `subject` (`None` is the public): the public Endpoints of every project,
/// then each other Endpoint `ask` (the gateway's `access/simulate`) admits the subject to and
/// grants something on, at most [`EXPLORE_AT_ONCE`] questions in flight. Sorted by title.
///
/// ponytail: one gateway question per non-public Endpoint per page load; a per-person cache
/// keyed by the token's groups is the upgrade once an installation holds thousands.
pub(crate) async fn readable<F, Fut>(
    state: &AppState,
    subject: Option<&Value>,
    ask: F,
) -> ReadableCatalogue
where
    F: Fn(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, ApiError>>,
{
    let mut datasets = Vec::new();
    let mut closed = Vec::new();
    for project in state.mirror.namespaces() {
        for endpoint in state
            .mirror
            .list(&project, "Endpoint", &ListOptions::default())
            .items
        {
            let Some(slug) = endpoint.spec["slug"].as_str().map(str::to_owned) else {
                continue;
            };
            if endpoint.spec["audience"].as_str() == Some("public") {
                datasets.push(row(&project, &endpoint, slug, "public", None));
            } else if subject.is_some() {
                closed.push((project.clone(), endpoint, slug));
            }
        }
    }
    let mut unanswered = 0;
    if let Some(subject) = subject {
        let question = json!({ "subject": subject });
        let answers: Vec<_> = futures_util::stream::iter(closed)
            .map(|(project, endpoint, slug)| {
                let answer = ask(slug.clone(), question.clone());
                async move { (project, endpoint, slug, answer.await) }
            })
            .buffered(EXPLORE_AT_ONCE)
            .collect()
            .await;
        for (project, endpoint, slug, answer) in answers {
            match answer {
                Ok(answer) if granted(&answer) => {
                    let (access, group) = mark(state, &project, &answer);
                    datasets.push(row(&project, &endpoint, slug, access, group));
                }
                // Not served by the gateway yet: nobody reads it.
                Ok(_) | Err(ApiError::NotFound(_)) => {}
                Err(err) => {
                    tracing::warn!(%slug, error = %err, "the gateway did not answer for the catalogue");
                    unanswered += 1;
                }
            }
        }
    }
    datasets.sort_by(|a, b| {
        let title = |d: &ReadableDataset| d.title.clone().unwrap_or_else(|| d.name.clone());
        title(a).to_lowercase().cmp(&title(b).to_lowercase())
    });
    ReadableCatalogue {
        datasets,
        unanswered,
    }
}

/// The internal datasets of `catalogue` as rows of the catalogue list (T-7400), each
/// `(slug, dataset)` in the shape the list reads: the public ones are left out (CKAN lists them
/// already), `project` keeps that project's alone, and every word of `q` must be in the name,
/// title or description, as CKAN narrows the public rows. The publisher is the Organization that
/// owns the project, under its name and title, as its public datasets are filed (T-7317), and
/// the second value is the Endpoints the gateway did not answer about.
pub(crate) fn internal_rows(
    state: &AppState,
    catalogue: ReadableCatalogue,
    q: &str,
    project: Option<&str>,
) -> (Vec<(String, Value)>, usize) {
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    let rows = catalogue
        .datasets
        .into_iter()
        .filter(|d| d.access != "public" && project.is_none_or(|p| p == d.project))
        .filter(|d| {
            let text = [Some(&d.name), d.title.as_ref(), d.description.as_ref()]
                .into_iter()
                .flatten()
                .map(|s| s.to_lowercase())
                .collect::<Vec<_>>()
                .join(" ");
            words.iter().all(|word| text.contains(word.as_str()))
        })
        .map(|d| {
            let organization =
                crate::permissions::organization_of(&state.mirror, &d.project).map(|name| {
                    let title = crate::api::organization_addresses::title(state, &name);
                    json!({ "name": name, "title": title })
                });
            let dataset = json!({
                "name": d.slug,
                "title": d.title.unwrap_or(d.name),
                "notes": d.description,
                "organization": organization,
            });
            (d.slug, dataset)
        })
        .collect();
    (rows, catalogue.unanswered)
}

/// The gateway admits the subject and grants at least one permission (a prohibition alone
/// reads nothing).
fn granted(answer: &Value) -> bool {
    answer["admitted"] == json!(true)
        && answer["permissions"]
            .as_array()
            .is_some_and(|permissions| !permissions.is_empty())
}

/// What grants the subject this Endpoint, from the assignees of the permitting Policies the
/// gateway named: an Organization grant first, then a group, else a grant to them alone.
fn mark(state: &AppState, project: &str, answer: &Value) -> (&'static str, Option<String>) {
    let mut assignees = Vec::new();
    for name in answer["policies"].as_array().into_iter().flatten() {
        let Some(policy) = name
            .as_str()
            .and_then(|n| state.mirror.get(project, "Policy", n))
        else {
            continue;
        };
        let spec = &policy.spec;
        let rules = spec["rules"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        for part in std::iter::once(spec).chain(rules) {
            let effect = part["effect"].as_str().or(spec["effect"].as_str());
            if effect != Some("prohibition") && part["assignee"].is_object() {
                assignees.push(part["assignee"].clone());
            }
        }
    }
    let of = |kind: &str| assignees.iter().find(|a| a["kind"] == kind);
    if of("organization").is_some() {
        ("organization", None)
    } else if let Some(group) = of("group") {
        ("group", group["id"].as_str().map(str::to_owned))
    } else {
        ("granted", None)
    }
}

fn row(
    project: &str,
    endpoint: &ResourceEnvelope,
    slug: String,
    access: &str,
    group: Option<String>,
) -> ReadableDataset {
    let spec = &endpoint.spec;
    let text = |key: &str| spec[key].as_str().map(str::to_owned);
    ReadableDataset {
        project: project.to_owned(),
        name: endpoint.metadata.name.clone(),
        slug,
        title: text("title"),
        description: text("description"),
        representations: crate::api::catalogue::served_representations(spec),
        access: access.to_owned(),
        group,
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/catalogue/readable", get(get_readable))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::session::Identity;
    use std::collections::BTreeMap;

    fn endpoint(state: &AppState, project: &str, name: &str, audience: &str) {
        state.mirror.upsert(ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: crate::resource::ObjectMeta::new(name, project),
            spec: json!({ "slug": format!("{name}zzzzzzzzzzzzzzzzzzzzzz"), "audience": audience,
                          "title": name, "contextSpaceRef": "traffic" }),
            status: None,
        });
    }

    fn policy(state: &AppState, project: &str, name: &str, assignee: Value) {
        state.mirror.upsert(ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "Policy".to_owned(),
            metadata: crate::resource::ObjectMeta::new(name, project),
            spec: json!({ "contextSpaceRef": "traffic", "assignee": assignee,
                          "operations": ["queryEntity"] }),
            status: None,
        });
    }

    /// helsinki: a public Endpoint, one granted to the Organization, one to traffic-analysts;
    /// espoo: one granted to espoo's Organization.
    fn world() -> AppState {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        endpoint(&state, "helsinki", "bikes", "public");
        endpoint(&state, "helsinki", "counts", "organization");
        endpoint(&state, "helsinki", "incidents", "organization");
        endpoint(&state, "espoo", "parking", "organization");
        policy(
            &state,
            "helsinki",
            "counts-org",
            json!({ "kind": "organization" }),
        );
        policy(
            &state,
            "helsinki",
            "incidents-analysts",
            json!({ "kind": "group", "id": "traffic-analysts" }),
        );
        policy(
            &state,
            "espoo",
            "parking-org",
            json!({ "kind": "organization" }),
        );
        state
    }

    /// A stub of the gateway's evaluation: the Policy granting each slug, and whom it grants.
    fn gateway(subject: &Value, slug: &str) -> Result<Value, ApiError> {
        let orgs = subject["organizations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let groups = subject["organizationGroups"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let grants: BTreeMap<&str, (&str, bool)> = [
            ("counts", ("counts-org", orgs.contains(&json!("helsinki")))),
            (
                "incidents",
                (
                    "incidents-analysts",
                    groups.contains(&json!("/helsinki/traffic-analysts")),
                ),
            ),
            ("parking", ("parking-org", orgs.contains(&json!("espoo")))),
        ]
        .into();
        let name = slug.trim_end_matches('z');
        let (policy, yes) = grants.get(name).copied().unwrap_or(("", false));
        Ok(if yes {
            json!({ "admitted": true, "policies": [policy], "permissions": [{ "types": ["*"] }] })
        } else {
            json!({ "admitted": true, "policies": [], "permissions": [] })
        })
    }

    async fn catalogue_of(
        state: &AppState,
        subject: Option<Value>,
    ) -> Vec<(String, String, Option<String>)> {
        let ask =
            |slug: String, question: Value| async move { gateway(&question["subject"], &slug) };
        readable(state, subject.as_ref(), ask)
            .await
            .datasets
            .into_iter()
            .map(|d| (d.name, d.access, d.group))
            .collect()
    }

    fn person(orgs: &[&str], groups: &[&str]) -> Value {
        json!({ "user": "u1", "groups": [], "roles": [],
                "organizations": orgs, "organizationGroups": groups })
    }

    /// ADR-N-067 §2.4: visitor < member < analyst, each row marked by what grants it.
    #[tokio::test]
    async fn the_public_a_member_and_an_analyst_each_read_their_own_catalogue() {
        let state = world();
        let public = |n: &str| (n.to_owned(), "public".to_owned(), None);
        assert_eq!(catalogue_of(&state, None).await, [public("bikes")]);
        let member = person(&["helsinki"], &["/helsinki/members"]);
        assert_eq!(
            catalogue_of(&state, Some(member)).await,
            [
                public("bikes"),
                ("counts".to_owned(), "organization".to_owned(), None)
            ]
        );
        let analyst = person(
            &["helsinki"],
            &["/helsinki/members", "/helsinki/traffic-analysts"],
        );
        assert_eq!(
            catalogue_of(&state, Some(analyst)).await,
            [
                public("bikes"),
                ("counts".to_owned(), "organization".to_owned(), None),
                (
                    "incidents".to_owned(),
                    "group".to_owned(),
                    Some("traffic-analysts".to_owned())
                ),
            ]
        );
    }

    /// T-7400: the catalogue list takes the internal datasets alone, filed under the owning
    /// Organization's name and title, narrowed by the scope's project and by every search word;
    /// the public's catalogue gives it none.
    #[tokio::test]
    async fn internal_rows_carry_their_organization_and_narrow_by_project_and_words() {
        let state = world();
        let put = |kind: &str, name: &str, metadata: Value, spec: Value| {
            let mut meta = crate::resource::ObjectMeta::new(name, "org");
            if let Some(title) = metadata.get("title") {
                meta.title = serde_json::from_value(title.clone()).ok();
            }
            state.mirror.upsert(ResourceEnvelope {
                api_version: crate::resource::API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: meta,
                spec,
                status: None,
            });
        };
        put(
            "Organization",
            "helsinki",
            json!({ "title": "Helsinki Region" }),
            json!({}),
        );
        put(
            "Project",
            "helsinki",
            json!({}),
            json!({ "organizationRef": "helsinki" }),
        );
        let ask =
            |slug: String, question: Value| async move { gateway(&question["subject"], &slug) };
        let analyst = person(&["helsinki"], &["/helsinki/traffic-analysts"]);
        let rows = async |q: &str, project: Option<&str>| {
            let catalogue = readable(&state, Some(&analyst), ask).await;
            internal_rows(&state, catalogue, q, project).0
        };
        let all = rows("", None).await;
        let slugs: Vec<_> = all
            .iter()
            .map(|(slug, _)| slug.trim_end_matches('z'))
            .collect();
        assert_eq!(slugs, ["counts", "incidents"]);
        assert_eq!(
            all[0].1["organization"],
            json!({ "name": "helsinki", "title": "Helsinki Region" })
        );
        assert_eq!(rows("INCIDENTS", None).await.len(), 1);
        assert_eq!(rows("incidents nothing", None).await.len(), 0);
        assert_eq!(rows("", Some("espoo")).await.len(), 0);
        assert_eq!(rows("", Some("helsinki")).await.len(), 2);
        let public = readable(&state, None, ask).await;
        assert!(internal_rows(&state, public, "", None).0.is_empty());
    }

    /// A signed-in person the gateway grants nothing reads the public Endpoints alone, and the
    /// public is never asked about: the gateway decides, the Portal adds nothing.
    #[tokio::test]
    async fn a_member_without_a_grant_sees_no_closed_endpoint() {
        let state = world();
        let nobody = json!({ "user": "u2", "groups": [], "roles": [] });
        assert_eq!(
            catalogue_of(&state, Some(nobody)).await,
            [("bikes".to_owned(), "public".to_owned(), None)]
        );
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let ask = |_: String, _: Value| {
            asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { Ok(json!({ "admitted": true, "permissions": [{}] })) }
        };
        readable(&state, None, ask).await;
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// A gateway that does not answer, refuses at the door or only prohibits grants nothing; the
    /// unanswered are counted, never listed.
    #[tokio::test]
    async fn an_unanswered_or_prohibited_endpoint_is_left_out() {
        let state = world();
        let ask = |slug: String, _: Value| async move {
            match slug.trim_end_matches('z') {
                "counts" => Err(ApiError::Unavailable("down".into())),
                "incidents" => Ok(json!({ "admitted": false, "permissions": [{}] })),
                _ => Ok(json!({ "admitted": true, "permissions": [], "prohibitions": [{}] })),
            }
        };
        let answer = readable(&state, Some(&json!({ "user": "u3" })), ask).await;
        assert_eq!(answer.unanswered, 1);
        assert_eq!(answer.datasets.len(), 1, "only the public one");
    }

    fn session(id_token: &str) -> Session {
        Session {
            identity: Identity {
                subject: "u1".into(),
                username: "demo.analyst".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec!["helsinki/members".into()],
                client: None,
                second_factor: false,
            },
            expires_at: 0,
            issued_at: 0,
            signed_in_at: None,
            stepped_up_at: None,
            id_token: id_token.into(),
            access_expires_at: 0,
            refresh_token: None,
        }
    }

    fn jwt(payload: Value) -> String {
        use base64::Engine;
        let part = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("e30.{part}.sig")
    }

    /// The organization claims come from the token the request was verified with: the bearer
    /// header when there is one, else the cookie session's ID token; a token without them names
    /// none, so the gateway reads it as a token without the mapper.
    #[test]
    fn the_subject_carries_the_organization_claims_of_the_verified_token() {
        let state = world();
        let claims = json!({ "jc_org": ["helsinki"], "jc_groups": ["/helsinki/members"] });
        let subject = subject_of(&state, &session(&jwt(claims.clone())), &HeaderMap::new());
        assert_eq!(subject["organizations"], json!(["helsinki"]));
        assert_eq!(subject["organizationGroups"], json!(["/helsinki/members"]));
        assert_eq!(subject["groups"], json!(["helsinki/members"]));

        let mut headers = HeaderMap::new();
        let bearer = jwt(json!({ "jc_org": ["espoo"] }));
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {bearer}").parse().expect("a header"),
        );
        let subject = subject_of(&state, &session(&jwt(claims)), &headers);
        assert_eq!(
            subject["organizations"],
            json!(["espoo"]),
            "the bearer, not the cookie"
        );
        assert!(subject.get("organizationGroups").is_none());

        let none = subject_of(&state, &session(""), &HeaderMap::new());
        assert!(none.get("organizations").is_none() && none.get("organizationGroups").is_none());
    }

    /// T-6997: a session read back from its cookie has no ID token (T-6549); the catalogue takes
    /// the one the Portal kept for it, so a member whose only grant is a `kind: organization`
    /// rule finds that Endpoint, as the gateway serves it to them. A member of another
    /// Organization, or a session whose token this Portal did not keep, finds no closed one.
    #[tokio::test]
    async fn a_cookie_session_is_named_by_the_id_token_the_portal_kept() {
        let state = world();
        let signed_in = |orgs: &[&str]| {
            let mut login = session(&jwt(json!({ "jc_org": orgs, "jc_groups": [] })));
            login.identity.subject = format!("member-of-{}", orgs.join("-"));
            login.expires_at = crate::auth::session::now_unix() + 600;
            state.keep_session_claims(&login);
            Session {
                id_token: String::new(),
                ..login
            }
        };
        let catalogue = |cookie: Session| {
            let subject = subject_of(&state, &cookie, &HeaderMap::new());
            let state = state.clone();
            async move { catalogue_of(&state, Some(subject)).await }
        };
        let public = ("bikes".to_owned(), "public".to_owned(), None);
        let counts = ("counts".to_owned(), "organization".to_owned(), None);
        let parking = ("parking".to_owned(), "organization".to_owned(), None);
        assert_eq!(
            catalogue(signed_in(&["helsinki"])).await,
            [public.clone(), counts]
        );
        assert_eq!(
            catalogue(signed_in(&["espoo"])).await,
            [public.clone(), parking]
        );
        let mut not_kept = session("");
        not_kept.identity.subject = "after-a-restart".into();
        assert_eq!(catalogue(not_kept).await, [public]);
    }

    /// T-6411 Security: a member (no RoleBinding) who types a project page's address reads
    /// nothing of it: the project's lists answer 404, as for a project that does not exist.
    #[tokio::test]
    async fn a_member_reaching_a_project_page_by_its_address_finds_nothing() {
        let state = world();
        let member = CurrentUser(session(""));
        for plural in ["endpoints", "contextspaces", "pipelines"] {
            let answer = crate::api::resources::list(
                member.clone(),
                State(state.clone()),
                axum::extract::Path(("helsinki".to_owned(), plural.to_owned())),
                Ok(crate::api::list_query::ListQuery::default()),
                axum::extract::Query(crate::api::resources::KindListQuery::default()),
            )
            .await;
            assert!(
                matches!(answer, Err(ApiError::NotFound(_))),
                "{plural}: {answer:?}"
            );
        }
    }
}
