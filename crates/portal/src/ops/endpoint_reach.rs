//! How a person reaches an Endpoint's data, as the registry's Endpoint read answers it (T-6027,
//! S-01, S-06, S-30, S-40): the entity types it serves, its address on the data host, one
//! address per representation its config serves, the embed code of a public one, and on request
//! who may write to it.

use serde_json::{json, Value};

use super::{Caller, OpError};
use crate::api::catalogue::{endpoint_url, served_representations};
use crate::auth::session::Identity;
use crate::permissions::{organization_of, ORG_NAMESPACE};
use crate::reconciler::groups::organization_group;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::ListOptions;

/// The frame height the viz host's embed code uses when a site names none.
const EMBED_HEIGHT: u32 = 640;
/// The viz host's frame sandbox (`embed.js`), so a pasted frame reads as the Embed button's.
const EMBED_SANDBOX: &str =
    "allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox";

/// The project a group speaks for: `{slug}-readers` and `{slug}-writers` are filled from its
/// bindings (PF-87, T-6461); any other name is its own.
fn project_of_group(group: &str) -> &str {
    group
        .strip_suffix("-readers")
        .or_else(|| group.strip_suffix("-writers"))
        .unwrap_or(group)
}

/// Why the gateway refuses `identity` at an Endpoint of `project` with this `spec`, by the rule of
/// its `admit_on` (EP-14, EP-15, ADR-N-067 2.3): a group of an admitted project, and for
/// `organization` also the Organization's own: a group inside it, a group named as one of its
/// projects, or the group of its Organization-scoped bindings. A sibling project's
/// `-readers` is not one: a vendor's binding reaches that project alone (T-6470). `None` when the
/// person is admitted or the Endpoint is public: only the Policies decide those. The caller reads
/// the Endpoint already, so this says nothing they could not read. The gateway answers such a
/// person 404 or 403, which alone reads as "not deployed yet" (PF-32).
pub(super) fn not_admitted(
    state: &AppState,
    identity: &Identity,
    project: &str,
    spec: &Value,
) -> Option<String> {
    let names = || identity.groups.iter().map(|g| g.trim_start_matches('/'));
    let listed: Vec<&str> = spec["allowedProjects"]
        .as_array()
        .map(|all| all.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let of_admitted_project = || {
        names()
            .flat_map(|g| [g, project_of_group(g)])
            .any(|g| g == project || listed.contains(&g))
    };
    let admits = match spec["audience"].as_str()? {
        "organization" => {
            let organization = organization_of(&state.mirror, project)?;
            let inside = format!("{organization}/");
            let bound = organization_group(&organization);
            let its_project = |g: &str| {
                state.mirror.get(ORG_NAMESPACE, "Project", g).is_some()
                    && organization_of(&state.mirror, g).as_deref() == Some(&organization)
            };
            if of_admitted_project()
                || names().any(|g| g.starts_with(&inside) || g == bound || its_project(g))
            {
                return None;
            }
            format!("audience organization admits members of Organization {organization}")
        }
        "project-list" => {
            if of_admitted_project() {
                return None;
            }
            format!("audience project-list admits the group of project {project} and of its allowedProjects")
        }
        _ => return None,
    };
    Some(format!(
        "you would not reach this Endpoint: {admits}, and your sign-in holds none of those groups, \
         so the gateway answers your reads and writes through it 404 or 403. A role in project \
         {project} puts you in its groups within minutes and counts from your next sign-in: ask \
         an organization administrator for one, or audience public leaves it to the Policies \
         alone who reads and writes"
    ))
}

/// A plan's warning when `manifest` is an Endpoint the planning person would not reach (T-6451):
/// three approvals later their first write would answer 404 or 403.
pub(super) fn warn_outside(
    answer: &mut Value,
    state: &AppState,
    identity: &Identity,
    project: &str,
    manifest: Option<&Value>,
) {
    let endpoint = manifest.filter(|m| m["kind"] == "Endpoint");
    let why = endpoint.and_then(|m| {
        not_admitted(state, identity, project, &m["spec"])
            .or_else(|| no_public_read(state, project, &m["spec"]))
    });
    let findings = answer
        .pointer_mut("/verdict/findings")
        .and_then(Value::as_array_mut);
    if let (Some(message), Some(findings)) = (why, findings) {
        findings.push(json!({ "level": "warning", "path": "spec.audience", "message": message }));
    }
}

/// The warning on a public Endpoint whose space grants the public role nothing yet: such an
/// Endpoint goes Live and every anonymous read answers 403 (T-6501).
fn no_public_read(state: &AppState, project: &str, spec: &Value) -> Option<String> {
    if spec["audience"] != "public" {
        return None;
    }
    let space = crate::resource::reference_name(&spec["contextSpaceRef"])?;
    let granted = state
        .mirror
        .list(project, "Policy", &crate::store::ListOptions::default())
        .items
        .iter()
        .any(|policy| {
            let spec = &policy.spec;
            crate::resource::reference_name(&spec["contextSpaceRef"]) == Some(space)
                && spec["assignee"]["kind"] == "role"
                && spec["assignee"]["id"] == "public"
        });
    (!granted).then(|| {
        format!(
            "no Policy of the space {space} grants the public role yet, so anonymous reads answer \
             403: add a Policy with `assignee: {{kind: role, id: public}}` and `operations: \
             [retrieveOps]` to this plan"
        )
    })
}

/// The types an Endpoint serves and its address: what a listing row carries.
pub(super) struct Reach {
    pub(super) types: Vec<String>,
    /// `https://api.{domain}/api/endpoint/{slug}/`; `None` before the Endpoint has a slug.
    pub(super) url: Option<String>,
}

/// The types and address of `endpoint`: the types its spec lists, else the classes of its
/// space's data model (`dataModelRef`, or the DataModels naming the space).
pub(super) fn reach(state: &AppState, project: &str, endpoint: &ResourceEnvelope) -> Reach {
    let spec = &endpoint.spec;
    let listed: Vec<String> = spec["entityTypes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str().map(str::to_owned))
        .collect();
    let types = if listed.is_empty() {
        crate::resource::reference_name(&spec["contextSpaceRef"])
            .map(|space| space_classes(state, project, space))
            .unwrap_or_default()
    } else {
        listed
    };
    Reach {
        types,
        url: spec["slug"]
            .as_str()
            .map(|slug| endpoint_url(&state.config.api_base_url(), slug)),
    }
}

/// The classes of `space`'s data model: the one its `dataModelRef` names, else every DataModel
/// of the project whose `contextSpaceRef` is the space.
pub(crate) fn space_classes(state: &AppState, project: &str, space: &str) -> Vec<String> {
    let named = crate::api::assistant_context::model_classes(&state.mirror, project, space);
    if !named.is_empty() {
        return named;
    }
    let mut classes: Vec<String> = state
        .mirror
        .list(project, "DataModel", &ListOptions::default())
        .items
        .into_iter()
        .filter(|model| {
            crate::resource::reference_name(&model.spec["contextSpaceRef"]) == Some(space)
        })
        .flat_map(|model| {
            model.spec["classes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|class| class.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .collect();
    classes.sort();
    classes.dedup();
    classes
}

impl Reach {
    /// The Endpoint read's `access` member: the address, the types, one address per served
    /// representation (a representation the config does not serve is not offered), and the
    /// embed code when the Endpoint is public.
    pub(super) fn detail(self, state: &AppState, endpoint: &ResourceEnvelope) -> Value {
        let Some(url) = self.url else {
            return json!({ "types": self.types, "surfaces": [] });
        };
        let surfaces = surfaces(&endpoint.spec, &url, &self.types);
        let public = endpoint.spec["audience"].as_str() == Some("public");
        let embed = public
            .then(|| embed(state, endpoint, self.types.first().map(String::as_str)))
            .flatten();
        let mut access = json!({ "url": url, "types": self.types, "surfaces": surfaces });
        if let Some(embed) = embed {
            access["embed"] = json!(embed);
        }
        access
    }
}

/// One `{name, url}` per address of each representation `spec` serves under `url`.
fn surfaces(spec: &Value, url: &str, types: &[String]) -> Vec<Value> {
    let surface = |name: &str, at: String| json!({ "name": name, "url": at });
    let mut out = Vec::new();
    for served in served_representations(spec) {
        match served.as_str() {
            "ngsi-ld" => out.push(surface("ngsi-ld", format!("{url}ngsi-ld/v1/"))),
            "ogc-features" => {
                out.push(surface("ogc-features", format!("{url}ogc/features/")));
                // Each type is an OGC collection, whose items are GeoJSON.
                out.extend(types.iter().map(|t| {
                    surface(
                        "geojson",
                        format!("{url}ogc/features/collections/{}/items", encoded(t)),
                    )
                }));
            }
            "odata" => out.push(surface("odata", format!("{url}odata/v4/"))),
            "sta" => out.push(surface("sta", format!("{url}sta/v1.1"))),
            "csv" => out.push(surface("csv", format!("{url}file.csv"))),
            "websocket" => out.push(surface(
                "websocket",
                format!("{}ws", url.replacen("http", "ws", 1)),
            )),
            "mcp" => out.push(surface("mcp", format!("{url}mcp"))),
            _ => {}
        }
    }
    if let Ok(endpoint) = serde_json::from_value::<jc_core::kinds::EndpointSpec>(spec.clone()) {
        out.extend(
            jc_core::kinds::ical_links(endpoint.ical_outputs())
                .into_iter()
                .map(|(_, path)| surface("ical", format!("{url}{path}"))),
        );
    }
    let feeds: Vec<jc_core::kinds::FeedOutput> =
        serde_json::from_value(spec["representations"]["feed"].clone()).unwrap_or_default();
    out.extend(
        jc_core::kinds::feed_links(&feeds)
            .into_iter()
            .map(|(_, path, _, _)| surface("feed", format!("{url}{path}"))),
    );
    out
}

fn encoded(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

/// The viz host's frame for a public Endpoint (`embed.js` `snippet`, T-5129): it reads the
/// Endpoint anonymously, so it shows what any visitor may read. `None` when the Portal has no
/// `api.{domain}` data host, so no `viz.{domain}` beside it.
fn embed(
    state: &AppState,
    endpoint: &ResourceEnvelope,
    entity_type: Option<&str>,
) -> Option<String> {
    let api = state.config.api_base_url();
    let domain = api.host_str()?.strip_prefix("api.")?;
    let slug = endpoint.spec["slug"].as_str()?;
    let mut hash = slug.to_owned();
    if let Some(entity_type) = entity_type {
        hash.push_str(&format!("&type={}", encoded(entity_type)));
    }
    let name = endpoint.metadata.name.as_str();
    let title = endpoint.metadata.title.as_ref();
    let title: String = title
        .map_or(name, |title| title.resolve(&[], name))
        .chars()
        .take(200)
        .collect();
    Some(format!(
        "<iframe src=\"https://viz.{domain}/embed.html#{}\" title=\"{}\" width=\"100%\" \
         height=\"{EMBED_HEIGHT}\" style=\"border:0;max-width:100%\" loading=\"lazy\" \
         referrerpolicy=\"no-referrer\" sandbox=\"{EMBED_SANDBOX}\"></iframe>",
        attribute(&hash),
        attribute(&title)
    ))
}

/// `text` safe inside a double-quoted HTML attribute.
fn attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Who may write to Endpoint `name`: each principal the gateway's PDP grants a write on one of
/// its types, with the types and the Policies that grant it. The access matrix decides who may
/// ask (an organization administrator or a reader of the space's Policies; anybody else is
/// refused) and records the read (EP-103), and its principals are only those the Policies of
/// this project's space name, so nobody outside the caller's project is listed.
pub(super) async fn writers(
    caller: &Caller,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<Value, OpError> {
    let axum::Json(matrix) = crate::api::access_matrix::access_matrix(
        super::runs::as_user(caller),
        axum::extract::State(state.clone()),
        axum::extract::Path((project.to_owned(), name.to_owned())),
        axum::extract::Query(crate::api::access_matrix::MatrixQuery { kind: None }),
    )
    .await?;
    let writers: Vec<Value> = matrix
        .principals
        .iter()
        .filter_map(|principal| {
            let id = format!("{}:{}", principal.kind, principal.name);
            let held = |effect: &str| -> Vec<&str> {
                matrix
                    .cells
                    .iter()
                    .filter(|c| c.principal == id && c.action == "write" && c.effect == effect)
                    .map(|c| c.entity_type.as_str())
                    .collect()
            };
            // A prohibition takes the write of its type away (ODRL, EP-103).
            let prohibited = held("prohibit");
            let types: Vec<&str> = held("permit")
                .into_iter()
                .filter(|t| !prohibited.contains(t) && !prohibited.contains(&"*"))
                .collect();
            (!types.is_empty()).then(|| {
                json!({
                    "kind": principal.kind,
                    "name": principal.name,
                    "types": types,
                    "via": principal.via.iter().map(|v| format!("{} {}", v.kind, v.name)).collect::<Vec<_>>(),
                })
            })
        })
        .collect();
    Ok(json!(writers))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `project` in `organization`, as the repository names it.
    fn with_project(state: &AppState, project: &str, organization: &str) {
        state.mirror.upsert(ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "Project".to_owned(),
            metadata: crate::resource::ObjectMeta::new(project, ORG_NAMESPACE),
            spec: json!({ "organizationRef": organization }),
            status: None,
        });
    }

    fn person(groups: &[&str]) -> Identity {
        serde_json::from_value(json!({ "subject": "s", "username": "u", "groups": groups }))
            .expect("identity")
    }

    #[test]
    fn a_public_endpoint_without_a_public_grant_is_warned_about() {
        let state = state();
        let spec = json!({ "audience": "public", "contextSpaceRef": "bikes" });
        let warned = no_public_read(&state, "city", &spec).expect("warned");
        assert!(warned.contains("kind: role, id: public"), "{warned}");
        assert_eq!(
            no_public_read(
                &state,
                "city",
                &json!({ "audience": "organization", "contextSpaceRef": "bikes" })
            ),
            None
        );

        let policy = |name: &str, space: &str, id: &str| ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "Policy".to_owned(),
            metadata: crate::resource::ObjectMeta::new(name, "city"),
            spec: json!({ "contextSpaceRef": space, "assignee": { "kind": "role", "id": id } }),
            status: None,
        };
        // Another space's grant and another role leave the warning; the space's public grant ends it.
        state.mirror.upsert(policy("a", "air", "public"));
        state.mirror.upsert(policy("b", "bikes", "steward"));
        assert!(no_public_read(&state, "city", &spec).is_some());
        state.mirror.upsert(policy("c", "bikes", "public"));
        assert_eq!(no_public_read(&state, "city", &spec), None);
    }

    #[test]
    fn a_person_outside_the_audience_is_told_before_the_gateway_answers_404() {
        let state = state();
        with_project(&state, "city", "hel");
        with_project(&state, "other", "hel");
        with_project(&state, "far", "espoo");
        let organization = json!({ "audience": "organization" });
        let refused =
            |groups: &[&str], spec: &Value| not_admitted(&state, &person(groups), "city", spec);
        // T-6451: an org-admin by RoleBinding alone, in a group that is no project.
        let text = refused(&["/platform-readers"], &organization).expect("refused");
        assert!(text.contains("Organization hel"), "{text}");
        assert!(text.contains("project city"), "{text}");
        assert!(text.contains("404 or 403"), "{text}");
        assert!(text.contains("audience public"), "{text}");
        // A project of another Organization admits nothing here, as at the gateway (PF-32).
        assert!(refused(&["far"], &organization).is_some());
        // Member of the Organization, or of one of its projects.
        assert_eq!(refused(&["/hel/members"], &organization), None);
        assert_eq!(refused(&["city"], &organization), None);
        // T-6461: the groups a binding fills, of this project or a sibling, never of a far one.
        // T-6470: the project's own bound group admits, a sibling's only with the Organization.
        assert_eq!(refused(&["/city-readers"], &organization), None);
        assert!(refused(&["other-writers"], &organization).is_some());
        assert_eq!(
            refused(&["other-writers", "hel_bound"], &organization),
            None
        );
        assert!(refused(&["other-writers", "espoo_bound"], &organization).is_some());
        assert!(refused(&["far-readers"], &organization).is_some());
        assert_eq!(refused(&["other"], &organization), None);
        let list = json!({ "audience": "project-list", "allowedProjects": ["other"] });
        assert!(refused(&["hel/members"], &list).is_some());
        assert_eq!(refused(&["other"], &list), None);
        assert_eq!(refused(&["city"], &list), None);
        assert_eq!(refused(&[], &json!({ "audience": "public" })), None);
    }

    fn state() -> AppState {
        let mut config = crate::config::Config::for_tests();
        config.public_base_url = "https://portal.city.sk".parse().expect("url");
        AppState::new(config, None)
    }

    fn endpoint(spec: Value, title: Option<&str>) -> ResourceEnvelope {
        let mut envelope = ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: crate::resource::ObjectMeta::new("bikes", "doprava"),
            spec,
            status: None,
        };
        envelope.metadata.title = title.map(|t| jc_core::i18n::Text::Plain(t.to_owned()));
        envelope
    }

    #[test]
    fn an_endpoint_offers_only_the_representations_it_serves() {
        let state = state();
        let open = endpoint(
            json!({ "slug": "bikesdopravazzzzzzzzzzzzzzz", "entityTypes": ["Bike Counter"],
                    "audience": "public", "enabledRepresentations": ["csv", "ogc-features"] }),
            None,
        );
        let access = reach(&state, "doprava", &open).detail(&state, &open);
        let base = "https://api.city.sk/api/endpoint/bikesdopravazzzzzzzzzzzzzzz/";
        assert_eq!(access["url"], json!(base));
        let urls: Vec<&str> = access["surfaces"]
            .as_array()
            .expect("surfaces")
            .iter()
            .filter_map(|s| s["url"].as_str())
            .collect();
        assert!(
            urls.contains(&format!("{base}file.csv").as_str()),
            "{urls:?}"
        );
        assert!(
            urls.contains(&format!("{base}ogc/features/collections/Bike+Counter/items").as_str())
        );
        assert!(urls.contains(&format!("{base}ngsi-ld/v1/").as_str()));
        assert!(!urls.iter().any(|u| u.contains("odata")), "{urls:?}");
    }

    #[test]
    fn the_embed_code_escapes_the_title_and_is_only_for_a_public_endpoint() {
        let state = state();
        let spec = json!({ "slug": "bikesdopravazzzzzzzzzzzzzzz", "audience": "public",
                           "entityTypes": ["BikeCounter"] });
        let public = endpoint(spec.clone(), Some("\"><script>alert(1)</script>"));
        let access = reach(&state, "doprava", &public).detail(&state, &public);
        let embed = access["embed"].as_str().expect("embed");
        assert!(embed.starts_with(
            "<iframe src=\"https://viz.city.sk/embed.html#bikesdopravazzzzzzzzzzzzzzz&amp;type=BikeCounter\""
        ));
        assert!(
            embed.contains("title=\"&quot;&gt;&lt;script&gt;"),
            "{embed}"
        );
        assert!(!embed.contains("<script>"), "{embed}");
        let mut restricted = spec;
        restricted["audience"] = json!("restricted");
        let hidden = endpoint(restricted, None);
        let access = reach(&state, "doprava", &hidden).detail(&state, &hidden);
        assert!(access.get("embed").is_none(), "{access}");
    }

    #[tokio::test]
    async fn writers_are_refused_to_a_person_who_reads_no_policy() {
        let state = state();
        let caller = Caller {
            identity: crate::auth::session::Identity {
                client: None,
                subject: "sub-jana".into(),
                username: "jana".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: false,
            },
            via: super::super::Via::Mcp,
            access: None,
            bearer: None,
        };
        let refused = writers(&caller, &state, "doprava", "bikes")
            .await
            .expect_err("no Policy read");
        assert_eq!(
            refused.into_api_error().into_document().0,
            axum::http::StatusCode::FORBIDDEN
        );
    }
}
