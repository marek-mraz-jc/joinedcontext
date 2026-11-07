//! The one move of the old personal connectors (UI-98, kept in a person's preferences and served
//! at `/api/mcp/me/{name}`) into the connector store (API/14, T-5970), so one store holds every
//! connector. Each old one becomes a `data-read` connector of the same name, read-only, for the
//! person's ceiling `maxExpiry`, its scope no wider than its selector: a selector of only
//! projects or spaces keeps them as items, any other is resolved today to the Endpoints it took.
//! The old list then leaves the preferences document; an old connector that did not move stays
//! in it under `connectorsNotMoved` with the reason, and a list this Portal cannot read stays where
//! it was, so a one-way move loses nothing a person made. No reader serves either key.
//!
//! The move runs for the signed-in person only (their preferences read or save, T-6362): what an
//! old selector took is cut to the Endpoints, spaces and projects that person reads today and
//! their DelegationPolicy ceiling admits, so a moved connector never names what its owner cannot
//! see. Neither the start-up nor the gateway's listener knows a person's groups, so neither moves.

use chrono::{Duration, Utc};
use jc_core::kinds::Audience;
use jc_core::kinds::EndpointSpec;
use jc_core::reach::EndpointFacts;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::auth::session::Identity;
use crate::connectors::{Ceiling, ItemAccess, NewConnector, ScopeItem};
use crate::error::ApiError;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::Mirror;

/// The preferences key the old connectors lived under.
const KEY: &str = "connectors";
/// The preferences key old connectors that did not move are kept under, `{name, select, reason}`.
const ASIDE: &str = "connectorsNotMoved";
/// Scope items a connector may carry (connectors::validate).
const MAX_SCOPE: usize = 50;

/// An old personal connector as preferences held it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Old {
    pub name: String,
    #[serde(default)]
    pub select: OldSelect,
}

/// The old selector: an Endpoint matched when it matched every field given.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OldSelect {
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub spaces: Vec<String>,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub themes: Vec<String>,
    #[serde(default)]
    pub access_level: Vec<String>,
    #[serde(default)]
    pub endpoints: Vec<String>,
}

/// What one person's move did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Moved {
    /// Old connectors now in the store.
    pub moved: usize,
    /// Old connectors refused as a conflict: their name is live already (a second Portal moved
    /// them first), or the person holds the most live connectors.
    pub present: usize,
    /// Old connectors whose selector takes no Endpoint the person reads today, or whose scope
    /// their ceiling refuses: nothing to reach.
    pub empty: usize,
    /// Old lists this Portal cannot read (an entry without a name, a string for a list): left in
    /// place, nothing of them moved.
    pub unreadable: usize,
}

/// The keys of a stored preferences row only the Portal keeps: the old list and the connectors
/// that did not move. `GET /api/v1/preferences` hides them and `PUT` carries them over, so a
/// whole-document save can never drop them.
pub fn held(stored: Option<&Value>) -> Map<String, Value> {
    let Some(row) = stored.and_then(Value::as_object) else {
        return Map::new();
    };
    [KEY, ASIDE]
        .into_iter()
        .filter_map(|key| row.get(key).map(|value| (key.to_owned(), value.clone())))
        .collect()
}

/// `row` without the keys [`held`] names, for the parse of the person's own preferences.
pub fn without_held(mut row: Value) -> Value {
    if let Some(object) = row.as_object_mut() {
        object.remove(KEY);
        object.remove(ASIDE);
    }
    row
}

/// Whether a stored row still holds old connectors to move.
pub fn holds_old(row: &Value) -> bool {
    row.get(KEY).is_some()
}

/// One old connector kept aside, with why it did not move.
fn aside(old: &Old, reason: &str) -> Value {
    json!({ "name": old.name, "select": old.select, "reason": reason })
}

/// Whether an Endpoint's audience passes the old `accessLevel`. A project Endpoint passes only
/// when both `project` and `grant` were picked: which of the two a person reads it by is known
/// per call only, so one of them alone keeps it out rather than letting the other in.
fn level_passes(levels: &[String], audience: Audience) -> bool {
    let has = |level: &str| levels.iter().any(|l| l == level);
    levels.is_empty()
        || match audience {
            Audience::Public => has("public"),
            Audience::Organization => has("organization"),
            Audience::ProjectList => has("project") && has("grant"),
        }
}

/// Whether the old selector took `e`, `accessLevel` judged by [`level_passes`].
fn takes(select: &OldSelect, e: &EndpointFacts) -> bool {
    fn any(wanted: &[String], has: impl Fn(&String) -> bool) -> bool {
        wanted.is_empty() || wanted.iter().any(has)
    }
    let space = format!("{}/{}", e.project, e.space);
    any(&select.projects, |p| *p == e.project)
        && any(&select.spaces, |s| *s == space)
        && any(&select.endpoints, |s| *s == e.slug)
        && any(&select.domains, |d| e.domains.contains(d))
        && any(&select.themes, |t| {
            e.themes.iter().any(|theme| {
                serde_json::to_value(theme)
                    .ok()
                    .as_ref()
                    .and_then(Value::as_str)
                    == Some(t)
            })
        })
        && level_passes(&select.access_level, e.audience)
}

fn item(project: &str) -> ScopeItem {
    ScopeItem {
        project: project.to_owned(),
        space: None,
        endpoint: None,
        pipeline: None,
        access: ItemAccess::Read,
        // Old connectors had no personal-data switch: answers keep those attributes out (AG-158).
        personal_data: false,
    }
}

/// Every Endpoint of the mirror `keep` passes, as the facts a selector reads.
fn facts(
    mirror: &Mirror,
    mut keep: impl FnMut(&str, &ResourceEnvelope) -> bool,
) -> Vec<EndpointFacts> {
    let mut facts: Vec<EndpointFacts> = mirror
        .matching(|env| env.kind == "Endpoint")
        .into_iter()
        .filter_map(|env| {
            let project = env.metadata.namespace.clone()?;
            if !keep(&project, &env) {
                return None;
            }
            let spec: EndpointSpec = serde_json::from_value(env.spec).ok()?;
            let types = crate::api::assistant_context::model_classes(
                mirror,
                &project,
                spec.context_space_ref.name(),
            );
            Some(EndpointFacts::of(
                &project,
                &env.metadata.name,
                &spec,
                types,
            ))
        })
        .collect();
    facts.sort_by(|a, b| a.slug.cmp(&b.slug));
    facts
}

/// The scope reaching no more than `select` did over `endpoints`. Projects and spaces alone stay
/// items (an Endpoint added there later is reached, as it was); anything finer is the Endpoints
/// it takes today. Empty when it takes none; at most 50 items (`MAX_SCOPE`), the rest left out.
pub fn scope_of(select: &OldSelect, endpoints: &[EndpointFacts]) -> Vec<ScopeItem> {
    let coarse = select.domains.is_empty()
        && select.themes.is_empty()
        && select.access_level.is_empty()
        && select.endpoints.is_empty();
    let mut scope: Vec<ScopeItem> = if coarse && !select.spaces.is_empty() {
        select
            .spaces
            .iter()
            .filter_map(|written| written.split_once('/'))
            .filter(|(p, _)| select.projects.is_empty() || select.projects.iter().any(|q| q == p))
            .map(|(p, space)| ScopeItem {
                space: Some(space.to_owned()),
                ..item(p)
            })
            .collect()
    } else if coarse {
        let mut projects: Vec<String> = if select.projects.is_empty() {
            endpoints.iter().map(|e| e.project.clone()).collect()
        } else {
            select.projects.clone()
        };
        projects.sort();
        projects.dedup();
        projects.iter().map(|p| item(p)).collect()
    } else {
        endpoints
            .iter()
            .filter(|e| takes(select, e))
            .map(|e| ScopeItem {
                endpoint: Some(e.name.clone()),
                ..item(&e.project)
            })
            .collect()
    };
    scope.truncate(MAX_SCOPE);
    scope
}

/// The new connector an old one becomes, its scope cut to what `ceiling` admits and its life to
/// the ceiling's days; `None` when nothing is left.
pub fn adopted(old: &Old, endpoints: &[EndpointFacts], ceiling: &Ceiling) -> Option<NewConnector> {
    let scope: Vec<ScopeItem> = scope_of(&old.select, endpoints)
        .into_iter()
        .filter(|i| {
            ceiling.allows_project_name(&i.project)
                && ceiling.allows_endpoint(&i.project, i.endpoint.as_deref())
        })
        .collect();
    (!scope.is_empty()).then(|| NewConnector {
        name: old.name.clone(),
        segments: vec!["data-read".to_owned()],
        scope,
        kinds: None,
        writes: false,
        autonomous: false,
        expires_at: Utc::now() + Duration::days(ceiling.max_days()),
        allowed_clients: Vec::new(),
    })
}

/// The Endpoints `identity` reads today, as the facts a selector reads: an old selector named no
/// right, so only these may become a connector's scope (T-6362).
fn readable_endpoints(state: &AppState, identity: &Identity) -> Vec<EndpointFacts> {
    let mut rights = std::collections::BTreeMap::new();
    facts(&state.mirror, |project, env| {
        rights
            .entry(project.to_owned())
            .or_insert_with(|| crate::permissions::for_request(state, identity, project))
            .may_read_envelope("Endpoint", env)
    })
}

/// `new` without the scope items its owner holds no read in (a project or space an old selector
/// named), `None` when nothing is left.
fn readable(
    state: &AppState,
    identity: &Identity,
    new: Option<NewConnector>,
) -> Option<NewConnector> {
    let mut new = new?;
    new.scope = std::mem::take(&mut new.scope)
        .into_iter()
        .enumerate()
        .filter(|(i, item)| {
            crate::api::connectors::reachable_item(state, identity, *i, item).is_ok()
        })
        .map(|(_, item)| item)
        .collect();
    (!new.scope.is_empty()).then_some(new)
}

/// Why `ceiling` would not let `new` go live at once, `None` when it does.
fn refused(new: &NewConnector, ceiling: &Ceiling) -> Option<String> {
    match crate::connectors::validate(new, ceiling, Utc::now()) {
        Ok(above) if above.is_empty() => None,
        Ok(above) => Some(above.join("; ")),
        Err(refusal) => Some(refusal.to_string()),
    }
}

fn db_error(err: sqlx::Error) -> ApiError {
    tracing::error!(error = %err, "moving personal connectors: database call failed");
    ApiError::from_db(&err, "preferences")
}

/// Moves the signed-in person's old connectors into the store and drops them from their
/// preferences, each cut to what `identity` reads today and its ceiling admits. Answers 503 while
/// the mirror holds no Endpoints yet (a selector would resolve to nothing); does nothing without a
/// database or old connectors, or for a client's token (AG-151: a client manages no connector).
pub async fn move_one(state: &AppState, identity: &Identity) -> Result<Moved, ApiError> {
    let Some(pool) = state.db.as_ref() else {
        return Ok(Moved::default());
    };
    if identity.client.is_some() {
        return Ok(Moved::default());
    }
    let subject = identity.subject.as_str();
    let Some(mut doc) = crate::db::load_preferences(pool, subject)
        .await
        .map_err(db_error)?
    else {
        return Ok(Moved::default());
    };
    if doc.get(KEY).is_none() {
        return Ok(Moved::default());
    }
    if state.mirror.is_empty() {
        // Before the first mirror a selector resolves to nothing: wait rather than lose it.
        return Err(ApiError::Unavailable(
            "your connectors are being moved to the connector store; try again in a minute".into(),
        ));
    }
    let Ok(old) = serde_json::from_value::<Vec<Old>>(doc[KEY].clone()) else {
        // The list stays where it is: a later Portal that reads it moves it then.
        tracing::warn!(%subject, "old personal connectors are not readable; left in place, none moved");
        return Ok(Moved {
            unreadable: 1,
            ..Moved::default()
        });
    };
    let endpoints = readable_endpoints(state, identity);
    let ceiling = Ceiling::of(&state.mirror, identity);
    let base = crate::api::connectors::base(state);
    let mut moved = Moved::default();
    let mut kept = Vec::new();
    for one in &old {
        let Some(new) = readable(state, identity, adopted(one, &endpoints, &ceiling)) else {
            tracing::info!(connector = %one.name, "an old personal connector takes no Endpoint its owner may read today; kept aside, not moved");
            moved.empty += 1;
            kept.push(aside(one, "takes no Endpoint you may read today"));
            continue;
        };
        // What the ceiling would queue or refuse (a client list it demands) stays aside: a move
        // asks no approval on the person's behalf.
        if let Some(why) = refused(&new, &ceiling) {
            tracing::info!(connector = %one.name, %why, "an old personal connector is above its owner's ceiling; kept aside");
            moved.empty += 1;
            kept.push(aside(one, &why));
            continue;
        }
        match state
            .connectors
            .create(subject, new, Vec::new(), &base)
            .await
        {
            Ok(_) => moved.moved += 1,
            // Its name is live already (another Portal moved it), or the person holds the most
            // live connectors: kept aside with the store's reason.
            Err(ApiError::Conflict(why)) => {
                tracing::info!(connector = %one.name, %why, "an old personal connector was not moved; kept aside");
                moved.present += 1;
                kept.push(aside(one, &why));
            }
            Err(other) => return Err(other),
        }
    }
    if let Some(object) = doc.as_object_mut() {
        object.remove(KEY);
        if !kept.is_empty() {
            // Appended to what an earlier move kept, whatever shape that holds.
            let mut list = match object.remove(ASIDE) {
                Some(Value::Array(earlier)) => earlier,
                Some(other) => vec![other],
                None => Vec::new(),
            };
            list.extend(kept);
            object.insert(ASIDE.to_owned(), Value::Array(list));
        }
    }
    crate::db::save_preferences(pool, subject, &doc)
        .await
        .map_err(db_error)?;
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jc_core::kinds::catalog::DataTheme;
    use std::collections::BTreeSet;

    fn endpoint(
        project: &str,
        name: &str,
        space: &str,
        audience: Audience,
        theme: Option<DataTheme>,
    ) -> EndpointFacts {
        EndpointFacts {
            project: project.into(),
            name: name.into(),
            slug: format!("slug-{name}"),
            space: space.into(),
            audience,
            types: BTreeSet::new(),
            domains: BTreeSet::new(),
            themes: theme.into_iter().collect(),
        }
    }

    fn catalogue() -> Vec<EndpointFacts> {
        vec![
            endpoint(
                "doprava",
                "buses",
                "transit",
                Audience::Public,
                Some(DataTheme::Tran),
            ),
            endpoint(
                "doprava",
                "counts",
                "transit",
                Audience::ProjectList,
                Some(DataTheme::Tran),
            ),
            endpoint(
                "ovzdusie",
                "air",
                "air",
                Audience::Organization,
                Some(DataTheme::Envi),
            ),
        ]
    }

    fn select(value: Value) -> OldSelect {
        serde_json::from_value(value).expect("an old selector")
    }

    fn named(scope: &[ScopeItem]) -> Vec<String> {
        scope
            .iter()
            .map(|i| {
                let narrowed = i
                    .space
                    .as_deref()
                    .map(|s| format!("space {s}"))
                    .or_else(|| i.endpoint.as_deref().map(|e| format!("endpoint {e}")))
                    .unwrap_or_else(|| "all".into());
                format!("{} {narrowed}", i.project)
            })
            .collect()
    }

    #[test]
    fn projects_and_spaces_alone_stay_items() {
        let c = catalogue();
        assert_eq!(
            named(&scope_of(&select(serde_json::json!({})), &c)),
            ["doprava all", "ovzdusie all"]
        );
        assert_eq!(
            named(&scope_of(
                &select(serde_json::json!({"projects": ["doprava"]})),
                &c
            )),
            ["doprava all"]
        );
        assert_eq!(
            named(&scope_of(
                &select(
                    serde_json::json!({"projects": ["ovzdusie"], "spaces": ["doprava/transit", "ovzdusie/air"]})
                ),
                &c
            )),
            ["ovzdusie space air"]
        );
    }

    #[test]
    fn a_finer_selector_becomes_the_endpoints_it_takes_today() {
        let c = catalogue();
        assert_eq!(
            named(&scope_of(
                &select(serde_json::json!({"themes": ["TRAN"]})),
                &c
            )),
            ["doprava endpoint buses", "doprava endpoint counts"]
        );
        assert_eq!(
            named(&scope_of(
                &select(serde_json::json!({"endpoints": ["slug-air"], "projects": ["doprava"]})),
                &c
            )),
            Vec::<String>::new()
        );
        assert_eq!(
            named(&scope_of(
                &select(serde_json::json!({"endpoints": ["slug-air"]})),
                &c
            )),
            ["ovzdusie endpoint air"]
        );
    }

    #[test]
    fn an_access_level_never_lets_more_in() {
        let c = catalogue();
        let only = |levels: Value| {
            named(&scope_of(
                &select(serde_json::json!({"accessLevel": levels})),
                &c,
            ))
        };
        assert_eq!(
            only(serde_json::json!(["public"])),
            ["doprava endpoint buses"]
        );
        assert_eq!(only(serde_json::json!(["project"])), Vec::<String>::new());
        assert_eq!(
            only(serde_json::json!(["project", "grant"])),
            ["doprava endpoint counts"]
        );
        assert_eq!(
            only(serde_json::json!(["organization"])),
            ["ovzdusie endpoint air"]
        );
    }

    #[test]
    fn a_moved_connector_reads_only_for_the_default_days() {
        let old: Old = serde_json::from_value(
            serde_json::json!({"name": "my-air", "select": {"themes": ["ENVI"]}}),
        )
        .expect("an old connector");
        let new = adopted(&old, &catalogue(), &Ceiling::of_policies(Vec::new()))
            .expect("it takes one Endpoint");
        assert_eq!(new.name, "my-air");
        assert_eq!(new.segments, ["data-read"]);
        assert!(!new.writes);
        assert!(new.scope.iter().all(|i| i.access == ItemAccess::Read));
        let within =
            crate::connectors::validate(&new, &Ceiling::of_policies(Vec::new()), Utc::now())
                .expect("a valid request");
        assert!(within.is_empty(), "within the default ceiling: {within:?}");
        let none: Old = serde_json::from_value(
            serde_json::json!({"name": "x", "select": {"themes": ["AGRI"]}}),
        )
        .expect("an old connector");
        assert!(adopted(&none, &catalogue(), &Ceiling::of_policies(Vec::new())).is_none());
    }

    #[test]
    fn a_selector_wider_than_the_cap_is_cut_never_widened() {
        let many: Vec<EndpointFacts> = (0..60)
            .map(|i| endpoint("p", &format!("e{i:02}"), "s", Audience::Public, None))
            .collect();
        let scope = scope_of(
            &select(serde_json::json!({"accessLevel": ["public"]})),
            &many,
        );
        assert_eq!(scope.len(), MAX_SCOPE);
        assert!(scope.iter().all(|i| i.endpoint.is_some()));
    }

    /// A reader of project `helsinki` alone; `espoo` holds a private Endpoint beside it.
    fn reader_world() -> (AppState, Identity) {
        use crate::config::Config;
        use crate::permissions::ORG_NAMESPACE;
        use crate::resource::ObjectMeta;
        let state = AppState::new(Config::for_tests(), None);
        let put = |kind: &str, name: &str, namespace: &str, spec: Value| {
            state.mirror.upsert(ResourceEnvelope {
                api_version: crate::resource::API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: ObjectMeta::new(name, namespace),
                spec,
                status: None,
            })
        };
        put(
            "Role",
            "reader",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace"], "verbs": ["read"] }] }),
        );
        put(
            "RoleBinding",
            "reader",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "reader@hel.fi" }], "role": "reader", "scope": { "project": "helsinki" } }),
        );
        for (project, audience) in [("helsinki", "organization"), ("espoo", "organization")] {
            put("ContextSpace", "air", project, json!({}));
            put(
                "Endpoint",
                &format!("{project}-air"),
                project,
                json!({ "contextSpaceRef": "air", "slug": format!("{project}airaaaaaaaaaaaaaaaaaaaaaa"), "audience": audience }),
            );
        }
        let reader = Identity {
            client: None,
            subject: "sub-reader".into(),
            username: "reader@hel.fi".into(),
            email: Some("reader@hel.fi".into()),
            name: None,
            roles: vec![],
            groups: vec![],
            second_factor: false,
        };
        (state, reader)
    }

    #[test]
    fn a_moved_connector_never_names_an_endpoint_or_project_its_owner_cannot_read() {
        let (state, reader) = reader_world();
        let endpoints = readable_endpoints(&state, &reader);
        assert_eq!(
            endpoints
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["helsinki-air"],
            "the other project's private Endpoint is not a candidate"
        );
        let ceiling = Ceiling::of(&state.mirror, &reader);
        let move_of = |select: Value| {
            let old = Old {
                name: "fav".into(),
                select: serde_json::from_value(select).expect("an old selector"),
            };
            readable(&state, &reader, adopted(&old, &endpoints, &ceiling)).map(|n| named(&n.scope))
        };
        assert_eq!(
            move_of(json!({})),
            Some(vec!["helsinki all".to_owned()]),
            "an empty selector reaches the readable project alone"
        );
        assert_eq!(
            move_of(json!({ "endpoints": ["espooairaaaaaaaaaaaaaaaaaaaaaa"] })),
            None,
            "a favourite of a private Endpoint moves nowhere"
        );
        assert_eq!(
            move_of(json!({ "projects": ["espoo", "helsinki"] })),
            Some(vec!["helsinki all".to_owned()]),
            "a project the owner holds no right in leaves the scope"
        );
        assert_eq!(
            move_of(json!({ "spaces": ["espoo/air"] })),
            None,
            "a space of such a project too"
        );
    }
}
