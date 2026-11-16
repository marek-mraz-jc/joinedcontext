//! The Portal doors of the Organization's OData service (API/13 §2b, ADR-N-070 item 11, T-7100):
//! the REST doors no tool may answer (route_coverage_tests.rs names each reason: a temporary
//! password, a secret, a consent, a person's connector or access request, an agent's grant, a
//! person's record about a space's data: comments, kept copies, erasures, alerts; T-7105),
//! the Portal's own per-person readings, the open data pages editor's (T-7041) and the `org`
//! namespace's own Changes, which the service's `changes` set (the projects' Changes) leaves out.
//! None is a registry operation, so no MCP `tools/list` and no assistant names one; each
//! `portal_*` function or action runs its REST door's handler through the API router, for a
//! person in a browser session only.

use axum::http::{Method, StatusCode};
use serde_json::{json, Map, Value};

use crate::api::odata::NAMESPACE;
use crate::api::odata_ops::Refusal;
use crate::auth::session::{CurrentUser, Front};
use crate::permissions::{self, ORG_NAMESPACE};
use crate::state::AppState;

/// The name every Portal door starts with, and no registry operation does.
pub const PREFIX: &str = "portal_";

/// What a door is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    /// The `Organization` singleton of the installation's Organization.
    Organization,
    /// A project the person reads: its name fills `{project}`.
    Project,
}

/// Who `$metadata` lists a door for; the REST handler still decides every call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// Any person.
    Person,
    /// A holder of `Person` with this verb at the Organization (a people administrator).
    People(jc_core::kinds::Verb),
    /// The Organization's administrator.
    Administrator,
    /// A person who reads a project.
    ProjectReader,
}

/// Where a parameter goes in the REST request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// A `{member}` of the REST path.
    Path,
    /// A query member of the REST door, its EDM type.
    Query(&'static str),
    /// The REST door's JSON body.
    Body,
}

/// One Portal door: its OData name, its binding and the REST door whose handler answers it.
#[derive(Debug)]
pub struct Door {
    pub name: &'static str,
    pub bound: Bound,
    pub method: Method,
    /// The REST path below `/api/v1`, `{project}` from the binding and each other `{member}` a
    /// [`Place::Path`] parameter.
    pub path: &'static str,
    pub parameters: &'static [(&'static str, Place)],
    pub who: Who,
}

const BODY: (&str, Place) = ("body", Place::Body);
const PERSON: (&str, Place) = ("person", Place::Path);
const ENDPOINT: (&str, Place) = ("endpoint", Place::Path);
const ACCOUNT: (&str, Place) = ("account", Place::Path);
const PAGE: (&str, Place) = ("page", Place::Path);
const LANG: (&str, Place) = ("lang", Place::Path);
const CHANGE: (&str, Place) = ("id", Place::Path);
const SPACE: (&str, Place) = ("space", Place::Path);
const PEOPLE_WRITE: Who = Who::People(jc_core::kinds::Verb::Update);

const fn door(
    name: &'static str,
    bound: Bound,
    method: Method,
    path: &'static str,
    parameters: &'static [(&'static str, Place)],
    who: Who,
) -> Door {
    Door {
        name,
        bound,
        method,
        path,
        parameters,
        who,
    }
}

use Bound::{Organization as Org, Project};

/// Every Portal door (API/13 §2b's table).
pub static DOORS: [Door; 57] = [
    door(
        "portal_people_remove",
        Org,
        Method::POST,
        "/organization/people/remove",
        &[BODY],
        PEOPLE_WRITE,
    ),
    door(
        "portal_people_name_by_id",
        Org,
        Method::POST,
        "/organization/people/name-by-id",
        &[("dryRun", Place::Query("Edm.Boolean"))],
        PEOPLE_WRITE,
    ),
    door(
        "portal_person_names",
        Org,
        Method::GET,
        "/organization/people/names",
        &[("ids", Place::Query("Edm.String"))],
        Who::Person,
    ),
    door(
        "portal_person_reset_password",
        Org,
        Method::POST,
        "/organization/people/{person}/reset-password",
        &[PERSON],
        PEOPLE_WRITE,
    ),
    door(
        "portal_person_resend_invitation",
        Org,
        Method::POST,
        "/organization/people/{person}/resend-invitation",
        &[PERSON],
        PEOPLE_WRITE,
    ),
    door(
        "portal_person_remove_second_factor",
        Org,
        Method::POST,
        "/organization/people/{person}/remove-second-factor",
        &[PERSON],
        PEOPLE_WRITE,
    ),
    door(
        "portal_person_delete",
        Org,
        Method::DELETE,
        "/organization/people/{person}",
        &[PERSON],
        Who::People(jc_core::kinds::Verb::Delete),
    ),
    door(
        "portal_person_consent_revoke",
        Org,
        Method::DELETE,
        "/organization/people/{person}/consents/{client}",
        &[PERSON, ("client", Place::Path)],
        PEOPLE_WRITE,
    ),
    door(
        "portal_connector_list",
        Org,
        Method::GET,
        "/organization/connectors",
        &[],
        Who::Administrator,
    ),
    door(
        "portal_connector_revoke",
        Org,
        Method::DELETE,
        "/organization/connectors/{connector}",
        &[("connector", Place::Path)],
        Who::Administrator,
    ),
    door(
        "portal_notification_list",
        Org,
        Method::GET,
        "/notifications",
        &[],
        Who::Person,
    ),
    door(
        "portal_notification_read",
        Org,
        Method::POST,
        "/notifications/{notification}/read",
        &[("notification", Place::Path)],
        Who::Person,
    ),
    door(
        "portal_preferences_get",
        Org,
        Method::GET,
        "/preferences",
        &[],
        Who::Person,
    ),
    door(
        "portal_preferences_set",
        Org,
        Method::PUT,
        "/preferences",
        &[BODY],
        Who::Person,
    ),
    door(
        "portal_preferences_recent",
        Org,
        Method::POST,
        "/preferences/recent",
        &[BODY],
        Who::Person,
    ),
    door(
        "portal_visualization_template",
        Org,
        Method::GET,
        "/visualization-templates/{template}",
        &[
            ("template", Place::Path),
            ("x", Place::Query("Edm.String")),
            ("y", Place::Query("Edm.String")),
            ("color", Place::Query("Edm.String")),
            ("facet", Place::Query("Edm.String")),
        ],
        Who::Person,
    ),
    door(
        "portal_opendata_pages",
        Org,
        Method::GET,
        "/organization/opendata/pages",
        &[],
        Who::Person,
    ),
    door(
        "portal_opendata_page_preview",
        Org,
        Method::POST,
        "/organization/opendata/pages/preview",
        &[BODY],
        Who::Person,
    ),
    door(
        "portal_opendata_page_set",
        Org,
        Method::PUT,
        "/organization/opendata/pages/{page}/{lang}",
        &[PAGE, LANG, BODY],
        Who::Person,
    ),
    door(
        "portal_opendata_page_revert",
        Org,
        Method::DELETE,
        "/organization/opendata/pages/{page}/{lang}",
        &[PAGE, LANG],
        Who::Person,
    ),
    door(
        "portal_organization_change_list",
        Org,
        Method::GET,
        "/projects/org/changes",
        &[],
        Who::Person,
    ),
    door(
        "portal_organization_change_history",
        Org,
        Method::GET,
        "/projects/org/changes/history",
        &[
            ("page", Place::Query("Edm.Int32")),
            ("kind", Place::Query("Edm.String")),
            ("name", Place::Query("Edm.String")),
        ],
        Who::Person,
    ),
    door(
        "portal_organization_change_get",
        Org,
        Method::GET,
        "/projects/org/changes/{id}",
        &[CHANGE],
        Who::Person,
    ),
    door(
        "portal_organization_change_manifests",
        Org,
        Method::GET,
        "/projects/org/changes/{id}/manifests",
        &[CHANGE],
        Who::Person,
    ),
    door(
        "portal_secret_list",
        Project,
        Method::GET,
        "/projects/{project}/secrets",
        &[],
        Who::ProjectReader,
    ),
    door(
        "portal_secret_set",
        Project,
        Method::POST,
        "/projects/{project}/secrets",
        &[BODY],
        Who::ProjectReader,
    ),
    door(
        "portal_serviceaccount_secret_issue",
        Project,
        Method::POST,
        "/projects/{project}/serviceaccounts/{account}/secret",
        &[ACCOUNT],
        Who::ProjectReader,
    ),
    door(
        "portal_serviceaccount_revoke",
        Project,
        Method::POST,
        "/projects/{project}/serviceaccounts/{account}/revoke",
        &[ACCOUNT, BODY],
        Who::ProjectReader,
    ),
    door(
        "portal_access_request_list",
        Project,
        Method::GET,
        "/projects/{project}/endpoints/{endpoint}/access-requests",
        &[ENDPOINT],
        Who::ProjectReader,
    ),
    door(
        "portal_access_request_file",
        Project,
        Method::POST,
        "/projects/{project}/endpoints/{endpoint}/access-requests",
        &[ENDPOINT, BODY],
        Who::ProjectReader,
    ),
    door(
        "portal_endpoint_quality",
        Project,
        Method::GET,
        "/projects/{project}/endpoints/{endpoint}/quality",
        &[ENDPOINT],
        Who::ProjectReader,
    ),
    door(
        "portal_space_quality",
        Project,
        Method::GET,
        "/projects/{project}/spaces/{space}/quality",
        &[SPACE],
        Who::ProjectReader,
    ),
    door(
        "portal_space_quality_checks",
        Project,
        Method::GET,
        "/projects/{project}/spaces/{space}/quality/checks",
        &[SPACE],
        Who::ProjectReader,
    ),
    door(
        "portal_space_temporal_audit",
        Project,
        Method::GET,
        "/projects/{project}/spaces/{space}/temporal-audit",
        &[SPACE, ("pipeline", Place::Query("Edm.String"))],
        Who::ProjectReader,
    ),
    door(
        "portal_space_retention_deleted",
        Project,
        Method::GET,
        "/projects/{project}/spaces/{space}/retention/deleted",
        &[SPACE],
        Who::ProjectReader,
    ),
    door(
        "portal_space_retention_preview",
        Project,
        Method::POST,
        "/projects/{project}/spaces/{space}/retention/preview",
        &[SPACE, BODY],
        Who::ProjectReader,
    ),
    door(
        "portal_space_subject_export",
        Project,
        Method::POST,
        "/projects/{project}/spaces/{space}/subjects/export",
        &[SPACE, BODY],
        Who::ProjectReader,
    ),
    door(
        "portal_erasure_list",
        Project,
        Method::GET,
        "/projects/{project}/spaces/{space}/erasures",
        &[SPACE],
        Who::ProjectReader,
    ),
    door(
        "portal_erasure_file",
        Project,
        Method::POST,
        "/projects/{project}/spaces/{space}/erasures",
        &[SPACE, ("dryRun", Place::Query("Edm.String")), BODY],
        Who::ProjectReader,
    ),
    door(
        "portal_erasure_get",
        Project,
        Method::GET,
        "/projects/{project}/spaces/{space}/erasures/{id}",
        &[SPACE, CHANGE],
        Who::ProjectReader,
    ),
    door(
        "portal_erasure_approve",
        Project,
        Method::POST,
        "/projects/{project}/spaces/{space}/erasures/{id}/approve",
        &[SPACE, CHANGE, BODY],
        Who::ProjectReader,
    ),
    door(
        "portal_erasure_decline",
        Project,
        Method::POST,
        "/projects/{project}/spaces/{space}/erasures/{id}/decline",
        &[SPACE, CHANGE, BODY],
        Who::ProjectReader,
    ),
    door(
        "portal_comment_list",
        Project,
        Method::GET,
        "/projects/{project}/spaces/{space}/comments",
        &[SPACE, ("urn", Place::Query("Edm.String"))],
        Who::ProjectReader,
    ),
    door(
        "portal_comment_add",
        Project,
        Method::POST,
        "/projects/{project}/spaces/{space}/comments",
        &[SPACE, BODY],
        Who::ProjectReader,
    ),
    door(
        "portal_comment_remove",
        Project,
        Method::DELETE,
        "/projects/{project}/spaces/{space}/comments/{id}",
        &[SPACE, CHANGE],
        Who::ProjectReader,
    ),
    door(
        "portal_trash_list",
        Project,
        Method::GET,
        "/projects/{project}/spaces/{space}/trash",
        &[SPACE],
        Who::ProjectReader,
    ),
    door(
        "portal_trash_keep",
        Project,
        Method::POST,
        "/projects/{project}/spaces/{space}/trash",
        &[SPACE, BODY],
        Who::ProjectReader,
    ),
    door(
        "portal_trash_forget",
        Project,
        Method::DELETE,
        "/projects/{project}/spaces/{space}/trash/{id}",
        &[SPACE, CHANGE],
        Who::ProjectReader,
    ),
    door(
        "portal_drift_list",
        Project,
        Method::GET,
        "/projects/{project}/drift",
        &[],
        Who::ProjectReader,
    ),
    door(
        "portal_drift_revert",
        Project,
        Method::POST,
        "/projects/{project}/drift/{space}/{id}/revert",
        &[SPACE, CHANGE],
        Who::ProjectReader,
    ),
    door(
        "portal_drift_adopt",
        Project,
        Method::POST,
        "/projects/{project}/drift/{space}/{id}/adopt",
        &[SPACE, CHANGE],
        Who::ProjectReader,
    ),
    door(
        "portal_alert_list",
        Org,
        Method::GET,
        "/alerts",
        &[],
        Who::Person,
    ),
    door(
        "portal_alert_subscribe",
        Org,
        Method::PUT,
        "/alerts",
        &[BODY],
        Who::Person,
    ),
    door(
        "portal_alert_unsubscribe",
        Org,
        Method::DELETE,
        "/alerts/{id}",
        &[CHANGE],
        Who::Person,
    ),
    door(
        "portal_alert_mute",
        Org,
        Method::POST,
        "/alerts/{id}/mute",
        &[CHANGE, BODY],
        Who::Person,
    ),
    door(
        "portal_alert_notice_list",
        Org,
        Method::GET,
        "/alerts/notices",
        &[],
        Who::Person,
    ),
    door(
        "portal_alert_notice_read",
        Org,
        Method::POST,
        "/alerts/notices/{id}/read",
        &[CHANGE],
        Who::Person,
    ),
];

/// The door named `name`.
pub fn find(name: &str) -> Option<&'static Door> {
    DOORS.iter().find(|door| door.name == name)
}

impl Door {
    /// A `GET` door is a function, every other an action.
    pub fn is_function(&self) -> bool {
        self.method == Method::GET
    }

    fn edm(place: Place) -> &'static str {
        match place {
            Place::Path => "Edm.String",
            Place::Query(ty) => ty,
            Place::Body => "Edm.Untyped",
        }
    }

    fn binding(&self) -> String {
        let ty = match self.bound {
            Bound::Organization => "Organization",
            Bound::Project => "Project",
        };
        format!("{NAMESPACE}.{ty}")
    }
}

/// Whether the caller is a person in a browser session: the Portal's cookie or the edge's,
/// never a bearer token and never a service account's or an agent's identity.
pub fn is_person_session(user: &CurrentUser, front: Front) -> bool {
    front != Front::Bearer && user.0.identity.client.is_none()
}

/// The refusal of a door to anything but a person's browser session.
pub fn bearer_refusal() -> Refusal {
    (
        StatusCode::FORBIDDEN,
        "forbidden",
        "the Portal's own doors (portal_*) answer a person signed in to the Portal only, never a \
         bearer, service account or agent token; no tool calls them (API/13 §2b)"
            .to_owned(),
    )
}

/// The doors `$metadata` lists for the caller in `org`: none outside a person's session; the
/// Organization's on the installation's Organization (`people`) by [`Who`]; a project's when
/// the person reads one.
pub(crate) fn offered(
    state: &AppState,
    user: &CurrentUser,
    front: Front,
    org: &str,
    people: bool,
    reads_a_project: bool,
) -> Vec<&'static Door> {
    if !is_person_session(user, front) {
        return Vec::new();
    }
    let identity = &user.0.identity;
    DOORS
        .iter()
        .filter(|door| match (door.bound, door.who) {
            (Bound::Project, _) | (_, Who::ProjectReader) => reads_a_project,
            (Bound::Organization, _) if !people => false,
            (_, Who::Person) => true,
            (_, Who::People(verb)) => {
                permissions::holds_at_organization(state, identity, org, "Person", verb, None)
            }
            (_, Who::Administrator) => {
                permissions::for_request(state, identity, ORG_NAMESPACE).administers_organization()
            }
        })
        .collect()
}

/// The `<Function>` and `<Action>` elements of `doors`.
pub(crate) fn xml(doors: &[&'static Door]) -> String {
    let mut out = String::new();
    for door in doors {
        let element = if door.is_function() {
            "Function"
        } else {
            "Action"
        };
        out.push_str(&format!(
            r#"<{element} Name="{}" IsBound="true"><Parameter Name="bindingParameter" Type="{}" Nullable="false"/>"#,
            door.name,
            door.binding()
        ));
        for (member, place) in door.parameters {
            let nullable = if *place == Place::Path {
                r#" Nullable="false""#
            } else {
                ""
            };
            out.push_str(&format!(
                r#"<Parameter Name="{member}" Type="{}"{nullable}/>"#,
                Door::edm(*place)
            ));
        }
        out.push_str(&format!(r#"<ReturnType Type="Edm.Untyped"/></{element}>"#));
    }
    out
}

/// The CSDL JSON overload of each of `doors`, by name.
pub(crate) fn json(doors: &[&'static Door]) -> Vec<(&'static str, Value)> {
    doors
        .iter()
        .map(|door| {
            let mut params = vec![json!({ "$Name": "bindingParameter",
                "$Type": door.binding(), "$Nullable": false })];
            params.extend(door.parameters.iter().map(|(member, place)| {
                json!({ "$Name": member, "$Type": Door::edm(*place),
                        "$Nullable": *place != Place::Path })
            }));
            let kind = if door.is_function() {
                "Function"
            } else {
                "Action"
            };
            (
                door.name,
                json!({ "$Kind": kind, "$IsBound": true, "$Parameter": params,
                        "$ReturnType": { "$Type": "Edm.Untyped" } }),
            )
        })
        .collect()
}

/// The REST request a door call becomes: its path and query below `/api/v1`, and its JSON body.
#[derive(Debug, PartialEq)]
pub struct Rest {
    pub path_and_query: String,
    pub body: Option<Vec<u8>>,
}

fn bad(message: String) -> Refusal {
    (StatusCode::BAD_REQUEST, "invalid-request", message)
}

/// A parameter's value as the text of a path segment or a query member.
fn text(member: &str, value: &Value) -> Result<String, Refusal> {
    match value {
        Value::String(s) if !s.is_empty() => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(bad(format!("{member} is a non-empty text"))),
    }
}

fn encoded(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// The REST request of a call of `door` with `params`, `project` filling `{project}`: every
/// path member required, every member one the door declares.
pub fn rest(
    door: &Door,
    project: Option<&str>,
    params: &Map<String, Value>,
) -> Result<Rest, Refusal> {
    if let Some(unknown) = params
        .keys()
        .find(|name| !door.parameters.iter().any(|(member, _)| member == name))
    {
        return Err(bad(format!(
            "{} has no parameter {unknown}; $metadata names its parameters",
            door.name
        )));
    }
    let mut path = door
        .path
        .replace("{project}", &encoded(project.unwrap_or_default()));
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    let mut body = None;
    for (member, place) in door.parameters {
        let value = params.get(*member).filter(|v| !v.is_null());
        match (place, value) {
            (Place::Path, Some(value)) => {
                path = path.replace(&format!("{{{member}}}"), &encoded(&text(member, value)?));
            }
            (Place::Path, None) => return Err(bad(format!("{member} is required"))),
            (Place::Query(_), Some(value)) => {
                query.append_pair(member, &text(member, value)?);
            }
            (Place::Body, Some(value)) => {
                body = Some(serde_json::to_vec(value).unwrap_or_default());
            }
            (_, None) => {}
        }
    }
    let query = query.finish();
    let path_and_query = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    Ok(Rest {
        path_and_query,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_door_is_a_registry_operation_and_each_path_member_is_a_parameter() {
        let mut names = std::collections::BTreeSet::new();
        for door in &DOORS {
            assert!(names.insert(door.name), "{} twice", door.name);
            assert!(door.name.starts_with(PREFIX), "{}", door.name);
            assert!(
                crate::ops::find(door.name).is_none(),
                "{} is a registry operation, so an MCP and assistant tool",
                door.name
            );
            assert!(crate::api::odata::identifier(door.name));
            let members: Vec<&str> = door
                .path
                .split('/')
                .filter_map(|s| s.strip_prefix('{')?.strip_suffix('}'))
                .filter(|m| *m != "project")
                .collect();
            let paths: Vec<&str> = door
                .parameters
                .iter()
                .filter(|(_, p)| *p == Place::Path)
                .map(|(m, _)| *m)
                .collect();
            assert_eq!(members, paths, "{}", door.name);
            assert_eq!(
                door.path.contains("{project}"),
                door.bound == Bound::Project,
                "{}",
                door.name
            );
        }
    }

    fn user(client: Option<&str>) -> CurrentUser {
        use crate::auth::session::{Identity, Session};
        CurrentUser(Session {
            identity: Identity {
                client: client.map(str::to_owned),
                subject: "sub-ada".into(),
                username: "ada".into(),
                email: None,
                name: None,
                roles: Vec::new(),
                groups: vec!["platform-admins".into()],
                second_factor: true,
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            signed_in_at: None,
            id_token: "dummy".into(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        })
    }

    /// A bearer token, a service account's or an agent's identity finds no door, whatever its
    /// rights: a bootstrap administrator through an agent client included.
    #[test]
    fn only_a_persons_session_is_offered_a_door() {
        let state = AppState::new(
            crate::config::Config {
                bootstrap_admins: "platform-admins".into(),
                ..crate::config::Config::for_tests()
            },
            None,
        );
        let person = user(None);
        assert!(is_person_session(&person, Front::Portal));
        assert!(is_person_session(&person, Front::Edge));
        assert!(!is_person_session(&person, Front::Bearer));
        assert!(!is_person_session(&user(Some("assistant")), Front::Portal));
        let offered = |user: &CurrentUser, front| offered(&state, user, front, "hel", true, true);
        assert_eq!(offered(&person, Front::Portal).len(), DOORS.len());
        assert!(offered(&person, Front::Bearer).is_empty());
        assert!(offered(&user(Some("assistant")), Front::Portal).is_empty());
        assert_eq!(bearer_refusal().0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn a_call_becomes_its_rest_request() {
        let door = find("portal_person_consent_revoke").expect("door");
        let params = json!({ "person": "a/b c", "client": "hub" });
        let rest = super::rest(door, None, params.as_object().expect("object")).expect("rest");
        assert_eq!(
            rest.path_and_query,
            "/organization/people/a%2Fb%20c/consents/hub"
        );
        assert_eq!(rest.body, None);

        let door = find("portal_people_name_by_id").expect("door");
        let params = json!({ "dryRun": true });
        let rest = super::rest(door, None, params.as_object().expect("object")).expect("rest");
        assert_eq!(
            rest.path_and_query,
            "/organization/people/name-by-id?dryRun=true"
        );
        assert_eq!(rest.body, None);

        let door = find("portal_serviceaccount_revoke").expect("door");
        let params = json!({ "account": "bot", "body": { "index": 0 } });
        let rest =
            super::rest(door, Some("air"), params.as_object().expect("object")).expect("rest");
        assert_eq!(
            rest.path_and_query,
            "/projects/air/serviceaccounts/bot/revoke"
        );
        assert_eq!(rest.body.as_deref(), Some(br#"{"index":0}"#.as_slice()));

        let door = find("portal_secret_list").expect("door");
        let rest = super::rest(door, Some("air"), &Map::new()).expect("rest");
        assert_eq!(rest.path_and_query, "/projects/air/secrets");

        let door = find("portal_person_reset_password").expect("door");
        for params in [
            json!({}),
            json!({ "person": "" }),
            json!({ "person": "x", "extra": 1 }),
        ] {
            let refused =
                super::rest(door, None, params.as_object().expect("object")).expect_err("refused");
            assert_eq!(refused.0, StatusCode::BAD_REQUEST, "{params}");
        }
    }
}
