//! People as operations of the registry (T-2732; PF-90…PF-94, AG-77, API/01 §24): the assistant
//! and an MCP client reach the same routes the Organization's People page calls, each operation
//! calling the route's own function, so the check is the route's — `Person` at organization
//! scope, and never a person who holds a right the caller lacks (PF-93).
//!
//! What stays a route of a person: resetting a password and removing a second factor, which hand
//! over or take away a way in, and deleting a person, which is a Change of its own. What no
//! operation ever answers: the temporary password a creation makes when the realm sends no
//! e-mail. The route answers it once, to the person at the Portal; a tool's answer reaches a
//! model, so here it is dropped and the answer says who hands one over instead.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::bounds::{text, ID, TITLE};
use super::runs::as_user;
use super::{annotations, parse_input, Operation};
use crate::api::people::{
    disable_person, edit_person, enable_person, get_person, list_people, sign_out_person,
};
use crate::change::Lane;
use crate::permissions::{binding_project, ORG_NAMESPACE};
use crate::state::AppState;
use crate::store::Mirror;
use jc_core::kinds::Verb;

/// The project's members, for an access connector's RoleBinding (T-6039).
pub const MEMBERS: &str = "jc_member_list";
/// Members one `jc_member_list` answer carries, and realm name lookups in flight at once.
const MEMBERS_MAX: usize = 200;
const NAMES_AT_ONCE: usize = 8;

/// One RoleBinding reaching a member: what `jc_change_plan` needs to name it, and what deleting
/// it takes from others, since a binding goes whole (T-6485).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Grant {
    binding: String,
    role: String,
    /// The subjects the binding names: above one, deleting it removes them too.
    subjects: usize,
    /// The Group that carries the member, where the binding names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberInput {
    #[serde(default)]
    search: Option<String>,
}

/// The people the RoleBindings of `project` reach, by the subject the binding names (a user id,
/// or an e-mail where a binding names one), each with the bindings it holds there: directly, or
/// through an organization Group the binding names (T-6039, T-6485).
fn members_of(mirror: &Mirror, project: &str) -> BTreeMap<String, Vec<Grant>> {
    let groups: HashMap<String, Vec<String>> = mirror
        .matching(|env| {
            env.kind == "Group" && env.metadata.namespace.as_deref() == Some(ORG_NAMESPACE)
        })
        .into_iter()
        .map(|group| {
            let users = users_in(group.spec.get("members"));
            (group.metadata.name, users)
        })
        .collect();
    let mut members: BTreeMap<String, Vec<Grant>> = BTreeMap::new();
    let bindings = mirror.matching(|env| {
        env.kind == "RoleBinding" && binding_project(mirror, &env.spec).as_deref() == Some(project)
    });
    for binding in bindings {
        let role = binding.spec["role"].as_str().unwrap_or_default().to_owned();
        let subjects = binding.spec.get("subjects").and_then(Value::as_array);
        let subjects = subjects.map_or(&[][..], Vec::as_slice);
        for subject in subjects {
            let group = subject["group"].as_str();
            let users = match (subject["user"].as_str(), group) {
                (Some(user), _) => vec![user.to_owned()],
                (None, Some(group)) => groups.get(group).cloned().unwrap_or_default(),
                (None, None) => Vec::new(),
            };
            for user in users {
                members.entry(user).or_default().push(Grant {
                    binding: binding.metadata.name.clone(),
                    role: role.clone(),
                    subjects: subjects.len(),
                    group: group.map(str::to_owned),
                });
            }
        }
    }
    for grants in members.values_mut() {
        grants.sort_by(|a, b| a.binding.cmp(&b.binding));
    }
    members
}

/// A member as `jc_member_list` answers it.
fn member_answer(user: &str, grants: &[Grant], name: Option<String>) -> Value {
    let roles: BTreeSet<&str> = grants.iter().map(|grant| grant.role.as_str()).collect();
    json!({ "user": user, "name": name, "roles": roles, "bindings": grants })
}

fn users_in(list: Option<&Value>) -> Vec<String> {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|member| member["user"].as_str().map(str::to_owned))
        .collect()
}

/// The realm's first and last name of each subject that is a user id, `None` where the realm has
/// none or cannot answer: the subjects still answer without it. Never the realm's e-mail.
async fn names_of(state: &AppState, subjects: &[&str]) -> Vec<Option<String>> {
    let admin = match state.people.as_deref() {
        Some(people) => people.admin().await.ok(),
        None => None,
    };
    let Some(admin) = admin else {
        return vec![None; subjects.len()];
    };
    let lookups: Vec<_> = subjects
        .iter()
        .map(|subject| name_of(&admin, subject))
        .collect();
    futures_util::stream::iter(lookups)
        .buffered(NAMES_AT_ONCE)
        .collect()
        .await
}

async fn name_of(admin: &crate::people::Admin<'_>, subject: &str) -> Option<String> {
    if subject.contains('@') {
        return None;
    }
    let user = admin.get(subject).await.ok()?;
    let name = [user.first_name, user.last_name]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!name.is_empty()).then_some(name)
}

fn member_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "search": text("Part of a name or user id, e.g. jana", TITLE)
        },
        "additionalProperties": false
    })
}

fn members_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "members": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "user": { "type": "string", "description": "The subject a RoleBinding names: put it in a RoleBinding's subjects as { user: ... }" },
                        "name": { "type": ["string", "null"] },
                        "roles": { "type": "array", "items": { "type": "string" } },
                        "bindings": {
                            "type": "array",
                            "description": "The RoleBindings behind the roles. To remove the member, delete a binding by name with jc_change_plan where subjects is 1 and no group is named; else take the member out of its subjects, or out of the Group",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "binding": { "type": "string", "description": "The RoleBinding's metadata.name" },
                                    "role": { "type": "string" },
                                    "subjects": { "type": "integer", "description": "Subjects the binding names: deleting it removes them all" },
                                    "group": { "type": "string", "description": "The Group that carries this member, where the binding names one" }
                                },
                                "required": ["binding", "role", "subjects"]
                            }
                        }
                    },
                    "required": ["user", "name", "roles", "bindings"]
                }
            },
            "truncated": { "type": "boolean" }
        },
        "required": ["members", "truncated"]
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ListInput {
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    first: Option<u32>,
    #[serde(default)]
    max: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdInput {
    id: String,
}

/// The route's `CreatePerson`, as the operation reads it and sends it on.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateInput {
    email: String,
    first_name: String,
    last_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    locale: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EditInput {
    id: String,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    locale: Option<String>,
}

/// What the answer says in place of a temporary password (PF-92).
const HAND_OVER: &str = "The realm sent no e-mail, so the person has no way in yet. A person \
     with update on Person gives them a password with Reset password on their page in the Portal; \
     a password is never answered to a tool.";

fn id_schema(what: &str) -> Value {
    json!({
        "type": "object",
        "properties": { "id": text(what, ID) },
        "required": ["id"],
        "additionalProperties": false
    })
}

fn list_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "search": text("A part of the name or the e-mail", TITLE),
            "first": { "type": "integer", "minimum": 0, "description": "Offset, default 0" },
            "max": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Page size, default 50" }
        },
        "additionalProperties": false
    })
}

fn create_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "email": text("E-mail: sign-in name and where the invitation goes", 254),
            "firstName": text("Given name", 255),
            "lastName": text("Family name", 255),
            "locale": text("Portal language: en, sk, cs or de", 8)
        },
        "required": ["email", "firstName", "lastName"],
        "additionalProperties": false
    })
}

fn edit_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": text("Keycloak user id", ID),
            "firstName": text("Given name", 255),
            "lastName": text("Family name", 255),
            "email": text("A new e-mail address, verified again", 254),
            "locale": text("The Portal language: en, sk, cs or de", 8)
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

fn person_schema() -> Value {
    json!({ "type": "object", "description": "One person as the People page shows them" })
}

fn page_schema() -> Value {
    json!({ "type": "object", "description": "One page of people, with the total" })
}

fn created_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "person": person_schema(),
            "emailSent": { "type": "boolean" },
            "handOver": { "type": "string", "description": "Present when no e-mail went out: who gives the person a way in" }
        },
        "required": ["person", "emailSent"]
    })
}

fn signed_out_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "id": { "type": "string" }, "signedOut": { "type": "boolean" } },
        "required": ["id", "signedOut"]
    })
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: MEMBERS,
            title: "List Project Members",
            description: "Project members: user id, name, roles and the RoleBinding of each role",
            input: member_schema,
            output: members_schema,
            annotations: annotations(true, false, true),
            // The project's bindings are what its RoleBindings page shows: read on RoleBinding
            // in this project, checked in the body (a read verb here would hide it from every
            // profile, `Access::names`), never the organization's people list (T-6039).
            kind: "RoleBinding",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<MemberInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: MemberInput = parse_input(val)?;
                    crate::permissions::for_request(state, &caller.identity, project).check(
                        "RoleBinding",
                        Verb::Read,
                        None,
                    )?;
                    let members = members_of(&state.mirror, project);
                    let truncated = members.len() > MEMBERS_MAX;
                    let members: Vec<(String, Vec<Grant>)> =
                        members.into_iter().take(MEMBERS_MAX).collect();
                    let subjects: Vec<&str> = members.iter().map(|(user, _)| user.as_str()).collect();
                    let names = names_of(state, &subjects).await;
                    let search = input.search.map(|s| s.to_lowercase());
                    let found: Vec<Value> = members
                        .iter()
                        .zip(names)
                        .filter(|((user, _), name)| {
                            search.as_deref().is_none_or(|search| {
                                user.to_lowercase().contains(search)
                                    || name.as_deref().is_some_and(|n| n.to_lowercase().contains(search))
                            })
                        })
                        .map(|((user, grants), name)| member_answer(user, grants, name))
                        .collect();
                    Ok(json!({ "members": found, "truncated": truncated }))
                })
            },
        },
        Operation {
            name: "jc_person_list",
            title: "List People",
            description: "Searches the organization's people, a page at a time",
            input: list_schema,
            output: page_schema,
            annotations: annotations(true, false, true),
            // People live in the realm, not in a project, so the route's own check at
            // organization scope decides (PF-91); the registry does not pre-judge it.
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ListInput>(val.clone()).map(|_| ()),
            run: |caller, state, _project, val| {
                Box::pin(async move {
                    let input: ListInput = parse_input(val)?;
                    let mut query = HashMap::new();
                    if let Some(search) = input.search {
                        query.insert("search".to_owned(), search);
                    }
                    if let Some(first) = input.first {
                        query.insert("first".to_owned(), first.to_string());
                    }
                    if let Some(max) = input.max {
                        query.insert("max".to_owned(), max.to_string());
                    }
                    let axum::Json(page) =
                        list_people(as_user(caller), State(state.clone()), Query(query)).await?;
                    Ok(serde_json::to_value(page)?)
                })
            },
        },
        Operation {
            name: "jc_person_get",
            title: "Get Person",
            description: "One person with their groups and roles",
            input: || id_schema("Keycloak user id"),
            output: person_schema,
            annotations: annotations(true, false, true),
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<IdInput>(val.clone()).map(|_| ()),
            run: |caller, state, _project, val| {
                Box::pin(async move {
                    let input: IdInput = parse_input(val)?;
                    let axum::Json(person) =
                        get_person(as_user(caller), State(state.clone()), Path(input.id)).await?;
                    Ok(serde_json::to_value(person)?)
                })
            },
        },
        Operation {
            name: "jc_person_create",
            title: "Create Person",
            description: "Creates a person and sends the sign-up e-mail",
            input: create_schema,
            output: created_schema,
            annotations: annotations(false, false, false),
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<CreateInput>(val.clone()).map(|_| ()),
            run: |caller, state, _project, val| {
                Box::pin(async move {
                    let input: CreateInput = parse_input(val)?;
                    let body = serde_json::to_vec(&input)?;
                    let created =
                        crate::api::people::create(state, &caller.identity, &body).await?;
                    let mut answer =
                        json!({ "person": created.person, "emailSent": created.email_sent });
                    if created.temporary_password.is_some() {
                        answer["handOver"] = json!(HAND_OVER);
                    }
                    Ok(answer)
                })
            },
        },
        Operation {
            name: "jc_person_edit",
            title: "Edit Person",
            description: "Edits a person's name, e-mail or language",
            input: edit_schema,
            output: person_schema,
            annotations: annotations(false, false, true),
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<EditInput>(val.clone()).map(|_| ()),
            run: |caller, state, _project, val| {
                Box::pin(async move {
                    let input: EditInput = parse_input(val)?;
                    let mut body = serde_json::Map::new();
                    for (key, value) in [
                        ("firstName", input.first_name),
                        ("lastName", input.last_name),
                        ("email", input.email),
                        ("locale", input.locale),
                    ] {
                        if let Some(value) = value {
                            body.insert(key.to_owned(), Value::String(value));
                        }
                    }
                    let body = Bytes::from(serde_json::to_vec(&body)?);
                    let axum::Json(person) =
                        edit_person(as_user(caller), State(state.clone()), Path(input.id), body)
                            .await?;
                    Ok(serde_json::to_value(person)?)
                })
            },
        },
        Operation {
            name: "jc_person_disable",
            title: "Disable Person",
            description: "Disables a person and ends their sessions; never the caller or the last Organization Administrator",
            input: || id_schema("Keycloak user id"),
            output: person_schema,
            annotations: annotations(false, true, true),
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<IdInput>(val.clone()).map(|_| ()),
            run: |caller, state, _project, val| {
                Box::pin(async move {
                    let input: IdInput = parse_input(val)?;
                    let axum::Json(person) =
                        disable_person(as_user(caller), State(state.clone()), Path(input.id))
                            .await?;
                    Ok(serde_json::to_value(person)?)
                })
            },
        },
        Operation {
            name: "jc_person_enable",
            title: "Enable Person",
            description: "Enables a disabled person",
            input: || id_schema("Keycloak user id"),
            output: person_schema,
            annotations: annotations(false, false, true),
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<IdInput>(val.clone()).map(|_| ()),
            run: |caller, state, _project, val| {
                Box::pin(async move {
                    let input: IdInput = parse_input(val)?;
                    let axum::Json(person) =
                        enable_person(as_user(caller), State(state.clone()), Path(input.id))
                            .await?;
                    Ok(serde_json::to_value(person)?)
                })
            },
        },
        Operation {
            name: "jc_person_sign_out",
            title: "Sign Person Out",
            description: "Ends every session of a person",
            input: || id_schema("Keycloak user id"),
            output: signed_out_schema,
            annotations: annotations(false, true, true),
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<IdInput>(val.clone()).map(|_| ()),
            run: |caller, state, _project, val| {
                Box::pin(async move {
                    let input: IdInput = parse_input(val)?;
                    sign_out_person(as_user(caller), State(state.clone()), Path(input.id.clone()))
                        .await?;
                    Ok(json!({ "id": input.id, "signedOut": true }))
                })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    fn envelope(kind: &str, namespace: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: kind.into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some(namespace.into()),
                ..Default::default()
            },
            spec,
            status: None,
        }
    }

    fn binding(name: &str, subjects: Value, role: &str, scope: Value) -> ResourceEnvelope {
        let spec = json!({ "subjects": subjects, "role": role, "scope": scope });
        envelope("RoleBinding", ORG_NAMESPACE, name, spec)
    }

    /// T-6039: the members of a project are the people its bindings reach, through a Group or a
    /// space of the project too; another project's and the organization's bindings stay out.
    #[test]
    fn the_members_are_whom_the_projects_bindings_reach() {
        let mirror = Mirror::new();
        mirror.upsert(envelope("ContextSpace", "doprava", "bikes", json!({})));
        let members = json!({ "members": [{ "user": "id-group-1" }, { "user": "id-jana" }] });
        mirror.upsert(envelope("Group", ORG_NAMESPACE, "pipeliners", members));
        for item in [
            binding(
                "a",
                json!([{ "user": "id-jana" }]),
                "viewer",
                json!({ "project": "doprava" }),
            ),
            binding(
                "b",
                json!([{ "group": "pipeliners" }]),
                "steward",
                json!({ "project": "doprava" }),
            ),
            binding(
                "c",
                json!([{ "user": "id-space" }]),
                "viewer",
                json!({ "contextSpace": "bikes" }),
            ),
            binding(
                "d",
                json!([{ "user": "id-other" }]),
                "viewer",
                json!({ "project": "other" }),
            ),
            binding(
                "e",
                json!([{ "user": "id-org" }]),
                "admin",
                json!({ "organization": "org" }),
            ),
        ] {
            mirror.upsert(item);
        }
        let found = members_of(&mirror, "doprava");
        let users: Vec<&str> = found.keys().map(String::as_str).collect();
        assert_eq!(users, ["id-group-1", "id-jana", "id-space"]);
        let jana: Vec<(&str, &str)> = found["id-jana"]
            .iter()
            .map(|grant| (grant.binding.as_str(), grant.role.as_str()))
            .collect();
        assert_eq!(jana, [("a", "viewer"), ("b", "steward")]);
        // What deleting each binding takes: the person alone, or the Group that carries them.
        assert_eq!(
            (found["id-jana"][0].subjects, &found["id-jana"][0].group),
            (1, &None)
        );
        assert_eq!(found["id-jana"][1].group.as_deref(), Some("pipeliners"));
        assert!(members_of(&mirror, "elsewhere").is_empty());
    }

    /// Without a realm to ask, the members still answer, with no name.
    #[tokio::test]
    async fn names_are_none_without_a_realm() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let names = names_of(&state, &["id-jana", "scn.jana@hel.fi"]).await;
        assert_eq!(names, [None, None]);
    }

    #[test]
    fn the_member_list_reads_bindings_of_its_project() {
        let op = crate::ops::find("jc_member_list").expect("registered");
        assert_eq!((op.kind, op.verb), ("RoleBinding", None));
        assert!(op.annotations.read_only_hint);
    }
}
