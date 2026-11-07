//! One matrix of who may do what (T-0751, PF-50, PF-58, AG-11, CC-34, AG-77): the roles of the
//! taxonomy (Architecture/12 §2a) and an agent run against every resource route and operation,
//! for one kind of each family. The expected table is data; a cell that differs is reported with
//! the answer it got, and every refusal must carry the words of its reason.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{encode, envelope, person, Answer, REPO};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::ops::{self, Caller, OpError, Via};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Who {
    /// The seeded `read` on every project kind: reads the project, proposes nothing (PF-59, PF-61).
    Viewer,
    /// `propose` on every kind of the matrix, like the editor roles.
    Editor,
    /// `propose` and `approve`: approves the yellow lane, never their own change.
    Steward,
    /// `propose`, `approve` and `delete`: may approve their own change (PF-58).
    Admin,
}

const PEOPLE: [Who; 4] = [Who::Viewer, Who::Editor, Who::Steward, Who::Admin];

impl Who {
    fn name(self) -> &'static str {
        match self {
            Who::Viewer => "viewer",
            Who::Editor => "editor",
            Who::Steward => "steward",
            Who::Admin => "admin",
        }
    }

    fn identity(self) -> Identity {
        person(self.name())
    }

    fn verbs(self) -> &'static [&'static str] {
        match self {
            Who::Viewer => &["read"],
            Who::Editor => &["propose"],
            Who::Steward => &["propose", "approve"],
            Who::Admin => &["propose", "approve", "delete"],
        }
    }

    fn may(self, verb: &str) -> bool {
        self.verbs().contains(&verb)
    }
}

/// One kind of each family: where it lives, and a manifest the platform accepts.
struct Family {
    kind: &'static str,
    plural: &'static str,
    /// The project segment of its routes: `org` for an organization-scoped kind.
    project: &'static str,
    spec: fn() -> Value,
}

const FAMILIES: [Family; 7] = [
    Family {
        kind: "Endpoint",
        plural: "endpoints",
        project: PROJECT,
        spec: || {
            json!({
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                "audience": "organization",
                "enabledRepresentations": ["ngsi-ld", "ogc-features"]
            })
        },
    },
    Family {
        kind: "Pipeline",
        plural: "pipelines",
        project: PROJECT,
        spec: || {
            json!({
                "class": "resident",
                "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all",
                "quotas": { "maxMemoryMb": 128, "cpuMillicores": 250 }
            })
        },
    },
    Family {
        kind: "DataSource",
        plural: "datasources",
        project: PROJECT,
        spec: || {
            json!({
                "type": "mqtt",
                "mqtt": {
                    "urls": ["tls://mqtt.hel.fi:8883"],
                    "topics": ["sensors/+/reading"],
                    "passwordRef": { "name": "mqtt-hel", "key": "password" }
                }
            })
        },
    },
    Family {
        kind: "DataModel",
        plural: "datamodels",
        project: PROJECT,
        spec: || {
            json!({
                "version": "1.0.0",
                "lifecycle": "draft",
                "contextSpaceRef": "helsinki",
                "linkml": "./bikes.linkml.yaml",
                "classes": ["BikeHireDockingStation"]
            })
        },
    },
    Family {
        kind: "RoleBinding",
        plural: "rolebindings",
        project: ORG_NAMESPACE,
        spec: || {
            json!({
                "subjects": [{ "group": "bikes-team" }],
                "role": "editor",
                "scope": { "project": PROJECT }
            })
        },
    },
    Family {
        kind: "Dashboard",
        plural: "dashboards",
        project: PROJECT,
        spec: || {
            json!({
                "title": { "en": "Bikes" },
                "visibility": "project",
                "pages": [{ "layout": "full-map", "layers": ["bikes"] }]
            })
        },
    },
    Family {
        kind: "App",
        plural: "apps",
        project: PROJECT,
        spec: || {
            json!({
                "kind": "fullstack",
                "source": { "path": "./src" },
                "build": { "rust": "1.90", "node": "22" },
                "visibility": "project",
                "lifecycle": "published",
                "dataNeeds": [{
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                    "types": ["BikeHireDockingStation"],
                    "attrs": ["availableBikeNumber"],
                    "operations": ["queryEntity"],
                    "representations": ["ngsi-ld"]
                }],
                "limits": { "requestsPerMinute": 600, "maxFileRows": 20000 }
            })
        },
    },
];

fn manifest(family: &Family, name: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": family.kind,
        "metadata": { "name": name, "namespace": family.project },
        "spec": (family.spec)(),
    })
}

/// A resource a cell adds. A space has one model (DM-61): a new DataModel takes a space of its own.
fn fresh(family: &Family, name: &str) -> Value {
    let mut fresh = manifest(family, name);
    if family.kind == "DataModel" {
        fresh["spec"]["contextSpaceRef"] = json!("helsinki-archive");
    }
    fresh
}

/// The resource of a family every cell reads, changes or removes.
fn existing(family: &Family) -> String {
    format!("{}-existing", family.kind.to_ascii_lowercase())
}

/// A merge request of the forge, numbered by family and author so each cell has its own.
fn pull(family_index: usize, author: Option<Who>) -> u64 {
    let by = author.map_or(0, |who| {
        1 + PEOPLE.iter().position(|p| *p == who).unwrap_or(0)
    });
    100 + (family_index as u64) * 10 + by as u64
}

fn branch(family: &Family, number: u64) -> String {
    format!(
        "portal/create-{}-proposed-{number:08x}",
        family.kind.to_ascii_lowercase()
    )
}

async fn forge() -> MockServer {
    let gitea = common::forge().await;

    // Every existing resource has its file; a new one has none.
    for family in &FAMILIES {
        let file = serde_norway::to_string(&manifest(family, &existing(family))).expect("yaml");
        Mock::given(method("GET"))
            .and(path_regex(format!(
                "^{REPO}/contents/.*/{}(/[a-z-]+)?\\.yaml$",
                existing(family)
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "sha-existing", "content": encode(&file)
            })))
            .with_priority(2)
            .mount(&gitea)
            .await;
    }
    // The merge requests the approval cells decide: one by somebody else and one by each person.
    for (index, family) in FAMILIES.iter().enumerate() {
        let authors = std::iter::once(None).chain(PEOPLE.iter().copied().map(Some));
        for author in authors {
            let number = pull(index, author);
            let head = branch(family, number);
            let (login, email) = match author {
                Some(who) => (who.name().to_owned(), format!("{}@hel.fi", who.name())),
                None => ("someone".to_owned(), "someone@hel.fi".to_owned()),
            };
            Mock::given(method("GET"))
                .and(path(format!("{REPO}/pulls/{number}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "number": number,
                    "html_url": format!("https://gitea.example/pulls/{number}"),
                    "state": "open",
                    "title": format!("create {} proposed", family.kind),
                    "head": { "ref": head },
                    "base": { "ref": "main" },
                    "created_at": "2026-09-14T09:00:00Z",
                    "user": { "login": login, "full_name": login, "email": email },
                    "mergeable": true,
                    "merged": false
                })))
                .mount(&gitea)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("{REPO}/pulls/{number}/files")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                    "filename": format!("projects/helsinki/{}/proposed.yaml", family.plural),
                    "status": "added"
                }])))
                .mount(&gitea)
                .await;
            let file = serde_norway::to_string(&manifest(family, "proposed")).expect("yaml");
            Mock::given(method("GET"))
                .and(path_regex(format!("^{REPO}/contents/.*")))
                .and(query_param("ref", head.as_str()))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "sha": "sha-proposed", "content": encode(&file)
                })))
                .with_priority(1)
                .mount(&gitea)
                .await;
        }
    }
    gitea
}

/// The organization's roles and a binding of each person over the whole organization, the
/// context space the manifests read, and one existing resource of each family.
fn state_with(gitea: &MockServer) -> AppState {
    let state = common::state_on(gitea);
    // Entity: a pipeline writes only what its proposer may write, the space behind its target
    // Endpoint (PL-94).
    let kinds: Vec<&str> = FAMILIES
        .iter()
        .map(|family| family.kind)
        .chain(["Entity"])
        .collect();
    for who in PEOPLE {
        state.mirror.upsert(envelope(
            "Role",
            who.name(),
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": kinds, "verbs": who.verbs() }] }),
        ));
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{}-binding", who.name()),
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": format!("{}@hel.fi", who.name()) }],
                "role": who.name(),
                "scope": { "organization": "hel" }
            }),
        ));
    }
    state.mirror.upsert(envelope(
        "ContextSpace",
        "helsinki",
        PROJECT,
        json!({ "isSandbox": false }),
    ));
    // The group the RoleBinding family names: a binding to a group no manifest declares is
    // refused before any role is read (PF-62), which would answer every cell of that family 400.
    state.mirror.upsert(envelope(
        "Group",
        "bikes-team",
        ORG_NAMESPACE,
        json!({ "displayName": "Bikes team" }),
    ));
    // The Endpoint the Pipeline family writes through: without it every Pipeline proposal
    // writes a space nobody can write and is refused (PL-111).
    state.mirror.upsert(envelope(
        "Endpoint",
        "helsinki-all",
        PROJECT,
        json!({ "contextSpaceRef": "helsinki", "slug": "hq4tm7ge2xdv6ksb3ncf5arw2y", "audience": "organization" }),
    ));
    state.mirror.upsert(envelope(
        "Layer",
        "bikes",
        PROJECT,
        json!({ "sourceEndpointRef": "endpoint-existing", "entityType": "BikeHireDockingStation", "style": "circle" }),
    ));
    for family in &FAMILIES {
        state.mirror.upsert(envelope(
            family.kind,
            &existing(family),
            family.project,
            (family.spec)(),
        ));
    }
    state
}

async fn send(state: &AppState, who: Who, http: &str, uri: &str, body: Option<Value>) -> Answer {
    common::checked_send(state, who.identity(), http, uri, body).await
}

/// What a cell expects: the status, and for a refusal the words its reason must carry.
struct Expect {
    status: StatusCode,
    reason: Option<String>,
}

fn allowed(status: StatusCode) -> Expect {
    Expect {
        status,
        reason: None,
    }
}

fn refused(reason: impl Into<String>) -> Expect {
    Expect {
        status: StatusCode::FORBIDDEN,
        reason: Some(reason.into()),
    }
}

/// The documented table (Architecture/12 §2a, API/01 §20): a verb the role lacks is refused
/// naming it; reading needs no binding.
fn when(may: bool, then: StatusCode, verb: &str, kind: &str) -> Expect {
    if may {
        allowed(then)
    } else {
        refused(format!("no role grants {verb} on {kind}"))
    }
}

fn check(misses: &mut Vec<String>, cell: &str, answer: &Answer, expect: &Expect) {
    let reason_ok = expect
        .reason
        .as_deref()
        .is_none_or(|words| answer.text.contains(words));
    if answer.status != expect.status || !reason_ok {
        misses.push(format!(
            "{cell}: expected {} {}, got {} {}",
            expect.status.as_u16(),
            expect.reason.as_deref().unwrap_or(""),
            answer.status.as_u16(),
            answer.text.chars().take(240).collect::<String>()
        ));
    }
}

#[tokio::test]
async fn every_role_meets_every_route_and_operation_exactly_as_the_table_says() {
    let gitea = forge().await;
    let state = state_with(&gitea);
    let mut misses = Vec::new();

    for (index, family) in FAMILIES.iter().enumerate() {
        let (kind, plural, project) = (family.kind, family.plural, family.project);
        let resources = format!("/api/v1/projects/{project}/{plural}");
        let one = format!("{resources}/{}", existing(family));
        let changed = {
            let mut manifest = manifest(family, &existing(family));
            manifest["metadata"]["labels"] = json!({ "joinedcontext.com/matrix": "changed" });
            manifest
        };
        let ops = format!("/api/v1/projects/{project}/ops");

        for who in PEOPLE {
            let cell = |action: &str| format!("{} {kind} {action}", who.name());
            let other = format!("chg-{:08x}", pull(index, None));
            let own = format!("chg-{:08x}", pull(index, Some(who)));
            let decide = json!({ "confirm": "proposed" });

            let answer = send(&state, who, "GET", &resources, None).await;
            check(
                &mut misses,
                &cell("list"),
                &answer,
                &allowed(StatusCode::OK),
            );
            let answer = send(&state, who, "GET", &one, None).await;
            check(&mut misses, &cell("get"), &answer, &allowed(StatusCode::OK));

            let propose = when(who.may("propose"), StatusCode::ACCEPTED, "propose", kind);
            let fresh = fresh(family, &format!("new-{}", who.name()));
            let answer = send(&state, who, "POST", &resources, Some(fresh.clone())).await;
            check(&mut misses, &cell("POST"), &answer, &propose);
            let answer = send(&state, who, "PUT", &one, Some(changed.clone())).await;
            check(&mut misses, &cell("PUT"), &answer, &propose);
            let answer = send(
                &state,
                who,
                "PATCH",
                &one,
                Some(
                    json!({ "metadata": { "labels": { "joinedcontext.com/matrix": "patched" } } }),
                ),
            )
            .await;
            check(&mut misses, &cell("PATCH"), &answer, &propose);

            let remove = when(who.may("delete"), StatusCode::ACCEPTED, "delete", kind);
            let answer = send(&state, who, "DELETE", &one, None).await;
            check(&mut misses, &cell("DELETE"), &answer, &remove);

            let approve_other = if who.may("approve") {
                allowed(StatusCode::ACCEPTED)
            } else {
                refused("no role grants approve in project")
            };
            let answer = send(
                &state,
                who,
                "POST",
                &format!("/api/v1/projects/{project}/changes/{other}/approve"),
                Some(decide.clone()),
            )
            .await;
            check(
                &mut misses,
                &cell("approve another's change"),
                &answer,
                &approve_other,
            );
            // Access and a model are never approved by their proposer, an administrator's neither
            // (PF-58, T-4833, T-4830).
            let access = matches!(kind, "RoleBinding" | "DataModel");
            let approve_own = match who {
                Who::Admin | Who::Steward if access => refused("never by its proposer"),
                Who::Admin => allowed(StatusCode::ACCEPTED),
                Who::Steward => refused("cannot approve their own change"),
                _ => refused("no role grants approve in project"),
            };
            let answer = send(
                &state,
                who,
                "POST",
                &format!("/api/v1/projects/{project}/changes/{own}/approve"),
                Some(decide.clone()),
            )
            .await;
            check(
                &mut misses,
                &cell("approve own change"),
                &answer,
                &approve_own,
            );
            let answer = send(
                &state,
                who,
                "POST",
                &format!("/api/v1/projects/{project}/changes/{other}/reject"),
                Some(json!({ "reason": "not now" })),
            )
            .await;
            check(&mut misses, &cell("reject"), &answer, &approve_other);

            // The same actions through the operations registry answer the same.
            let answer = send(
                &state,
                who,
                "POST",
                &format!("{ops}/jc_resource_list"),
                Some(json!({ "kind": kind })),
            )
            .await;
            check(
                &mut misses,
                &cell("jc_resource_list"),
                &answer,
                &allowed(StatusCode::OK),
            );
            let answer = send(
                &state,
                who,
                "POST",
                &format!("{ops}/jc_resource_get"),
                Some(json!({ "kind": kind, "name": existing(family) })),
            )
            .await;
            check(
                &mut misses,
                &cell("jc_resource_get"),
                &answer,
                &allowed(StatusCode::OK),
            );
            let answer = send(
                &state,
                who,
                "POST",
                &format!("{ops}/jc_resource_propose"),
                Some(json!({ "manifest": fresh })),
            )
            .await;
            check(&mut misses, &cell("jc_resource_propose"), &answer, &propose);
            let answer = send(
                &state,
                who,
                "POST",
                &format!("{ops}/jc_resource_delete"),
                Some(
                    json!({ "kind": kind, "name": existing(family), "confirm": existing(family) }),
                ),
            )
            .await;
            check(&mut misses, &cell("jc_resource_delete"), &answer, &remove);
            let answer = send(
                &state,
                who,
                "POST",
                &format!("{ops}/jc_change_approve"),
                Some(json!({ "id": other, "confirm": "proposed" })),
            )
            .await;
            check(
                &mut misses,
                &cell("jc_change_approve another's"),
                &answer,
                &approve_other,
            );
            // An operation never approves its caller's own change, an administrator's neither (AG-11).
            let approve_own_by_operation = if who.may("approve") && access {
                refused("never by its proposer")
            } else if who.may("approve") {
                refused("cannot approve their own change")
            } else {
                refused("no role grants approve in project")
            };
            let answer = send(
                &state,
                who,
                "POST",
                &format!("{ops}/jc_change_approve"),
                Some(json!({ "id": own, "confirm": "proposed" })),
            )
            .await;
            check(
                &mut misses,
                &cell("jc_change_approve own"),
                &answer,
                &approve_own_by_operation,
            );
            let answer = send(
                &state,
                who,
                "POST",
                &format!("{ops}/jc_change_reject"),
                Some(json!({ "id": other, "reason": "not now" })),
            )
            .await;
            check(
                &mut misses,
                &cell("jc_change_reject"),
                &answer,
                &approve_other,
            );
        }

        // An agent run acts for the administrator who started it and still never decides (AG-11).
        let agent = Caller::new(Who::Admin.identity(), Via::Agent);
        let other = format!("chg-{:08x}", pull(index, None));
        for (name, input) in [
            (
                "jc_change_approve",
                json!({ "id": other, "confirm": "proposed" }),
            ),
            (
                "jc_change_reject",
                json!({ "id": other, "reason": "not now" }),
            ),
        ] {
            let op = ops::find(name).expect("registered");
            let result = ops::call(op, &agent, &state, project, input).await;
            let answer = match result {
                Err(OpError::Api(err)) => Answer {
                    status: StatusCode::FORBIDDEN,
                    text: err.to_string(),
                },
                other => Answer {
                    status: StatusCode::OK,
                    text: format!("{other:?}"),
                },
            };
            check(
                &mut misses,
                &format!("agent {kind} {name}"),
                &answer,
                &refused("an agent never approves or rejects a change"),
            );
        }
        // The agent checks what it proposes, as every door does (T-0956).
        let dry_run = ops::find("jc_manifest_dry_run").expect("registered");
        let _ = ops::call(
            dry_run,
            &agent,
            &state,
            project,
            json!({ "manifest": fresh(family, "new-agent") }),
        )
        .await;
        // A Pipeline's records are checked too (T-4840), as its workbench keeps them.
        common::records_checked(&state, project, "new-agent").await;
        let propose = ops::find("jc_resource_propose").expect("registered");
        let answer = match ops::call(
            propose,
            &agent,
            &state,
            project,
            json!({ "manifest": fresh(family, "new-agent") }),
        )
        .await
        {
            Ok(_) => Answer {
                status: StatusCode::ACCEPTED,
                text: String::new(),
            },
            Err(err) => Answer {
                status: StatusCode::FORBIDDEN,
                text: format!("{err:?}"),
            },
        };
        check(
            &mut misses,
            &format!("agent {kind} jc_resource_propose"),
            &answer,
            &allowed(StatusCode::ACCEPTED),
        );
    }

    assert!(
        misses.is_empty(),
        "{} cell(s) differ from the table:\n{}",
        misses.len(),
        misses.join("\n")
    );
}
