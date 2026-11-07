//! Edge cases of the approval lane a change is put in (T-2109; CC-19, CC-39, CC-63, PF-52, PF-62).
//!
//! **The contract, in one sentence:** `change::classify` may err towards the stricter lane and never
//! towards the looser one — a deletion is Red whatever it deletes, a kind that can hand out access or
//! reach another organisation is Red whatever its spec holds, and anything the function has not been
//! taught is Yellow rather than Green.
//!
//! The lane is not cosmetic: it decides how many approvals a change needs and who may give them
//! (`src/api/changes.rs`), so a manifest that lands one lane lower is a manifest that merges with
//! less review than the architecture requires. Every caller hands the function a kind from the
//! catalogue and that manifest's own `spec`
//! (`src/api/mutate.rs:857`, `src/api/delete.rs:264`, `src/api/import.rs:1494`,
//! `src/api/blueprints.rs:314`, `src/agents/share.rs:207`); the one caller that passes `Null`
//! (`src/api/changes.rs:594`) is a lower bound by design, combined with the headline manifest's own
//! lane through `riskiest`, so a spec-dependent lane is never decided from an absent spec.
//!
//! Tests only (the family's rule). Every case is green; a red one becomes its own task.

use joinedcontext_portal::change::{classify, Lane, Operation};
use serde_json::{json, Value};

/// Red whatever the spec holds: access, federation, and the file that decides an environment.
const RED_WHATEVER_THEY_HOLD: &[&str] = &[
    "ContextSourceRegistration",
    "SharedSpaceReference",
    "DataSpaceParticipant",
    "DataOffer",
    "DataAgreement",
    "ServiceAccount",
    "Role",
    "RoleBinding",
    "Group",
    "ContextBroker", // PF-110, ADR-N-047
    "MqttBroker",    // access: its roles grant topics (ADR-N-048)
    "Environment",
    "Policy",
    "ScopeDefinition",
    "Organization",
    "Project",
    "DelegationPolicy", // the connector ceiling: access, like a Role (AG-151)
];

/// Green: a change nobody needs to review, because undoing it costs nothing.
const GREEN: &[&str] = &["Dashboard", "Layer"];

fn lane(kind: &str, op: Operation, spec: Value) -> Lane {
    classify(kind, op, &spec)
}

// -------------------------------------------------------------------------------------------------
// A deletion
// -------------------------------------------------------------------------------------------------

/// CC-19, CC-39: a deletion is Red before anything else is looked at — the kind, the spec and even
/// whether the kind exists are beside the point.
#[test]
fn a_deletion_is_red_whatever_it_deletes() {
    let specs = [
        json!({}),
        json!({ "isSandbox": true }),
        json!({ "audience": "internal" }),
        Value::Null,
        json!("not an object"),
        json!([1, 2, 3]),
    ];
    for info in joinedcontext_portal::resource::kinds() {
        for spec in &specs {
            assert_eq!(
                lane(info.kind, Operation::Delete, spec.clone()),
                Lane::Red,
                "deleting a {} with spec {spec} was not Red",
                info.kind,
            );
        }
    }
    // Including a kind the catalogue does not have: the lane is decided before the kind is read.
    for unknown in ["", "Dashboard ", "dashboard", "../Role", "Layer\n"] {
        assert_eq!(lane(unknown, Operation::Delete, json!({})), Lane::Red);
    }
}

// -------------------------------------------------------------------------------------------------
// A public endpoint
// -------------------------------------------------------------------------------------------------

/// EP-14, CC-63: publishing to the open internet is Red on create and on update alike.
#[test]
fn an_endpoint_that_says_public_is_red_on_every_write() {
    for op in [Operation::Create, Operation::Update] {
        assert_eq!(
            lane("Endpoint", op, json!({ "audience": "public" })),
            Lane::Red,
            "{op:?}",
        );
        // The rest of the spec is irrelevant, and so is the order of its keys.
        assert_eq!(
            lane(
                "Endpoint",
                op,
                json!({ "enabledRepresentations": ["ngsi-ld"], "audience": "public",
                        "contextSpaceRef": "ovzdusie" }),
            ),
            Lane::Red,
        );
    }
}

/// EP-14, CC-63: everything that is not the word `public` in the `audience` field is a narrower
/// audience, and a narrower audience is the Yellow lane. Each of these near misses is stricter than
/// it looks, because the word is not the manifest's own — `Audience` is an enum of three kebab-case
/// values (`crates/jc-core/src/kinds/endpoint.rs:120`), so a manifest saying `Public`, `PUBLIC` or
/// `" public"` never reaches this function at all: it is refused when the spec is parsed. The last
/// assertion of this case is that refusal, so the two halves cannot drift apart.
#[test]
fn nothing_but_the_word_public_in_the_audience_makes_an_endpoint_red() {
    for audience in [
        json!("Public"),
        json!("PUBLIC"),
        json!(" public"),
        json!("public "),
        json!("public\n"),
        json!("publico"),
        json!("p\u{0430}blic"), // a Cyrillic а
        json!(["public"]),
        json!({ "value": "public" }),
        json!(true),
        json!(1),
        Value::Null,
    ] {
        assert_eq!(
            lane(
                "Endpoint",
                Operation::Create,
                json!({ "audience": audience })
            ),
            Lane::Yellow,
            "audience {audience} was read as the public one",
        );
    }
    // No audience at all, and the word one level down where it does not belong.
    assert_eq!(lane("Endpoint", Operation::Create, json!({})), Lane::Yellow);
    assert_eq!(
        lane(
            "Endpoint",
            Operation::Create,
            json!({ "spec": { "audience": "public" } })
        ),
        Lane::Yellow,
    );
    // And the manifest that spells it any other way is refused before a lane is ever asked for.
    for refused in ["Public", "PUBLIC", " public", "public "] {
        let parsed = serde_json::from_value::<jc_core::kinds::Audience>(json!(refused));
        assert!(
            parsed.is_err(),
            "{refused:?} parsed as an audience, so it can reach `classify` after all",
        );
    }
    assert!(serde_json::from_value::<jc_core::kinds::Audience>(json!("public")).is_ok());
}

// -------------------------------------------------------------------------------------------------
// A sandbox space
// -------------------------------------------------------------------------------------------------

/// CC-67: a sandbox is Green because it is thrown away. Only the boolean `true` says so; a string, a
/// number or a missing field leaves the space in the Yellow lane, and deleting one is still Red.
#[test]
fn only_a_boolean_sandbox_makes_a_space_green() {
    assert_eq!(
        lane(
            "ContextSpace",
            Operation::Create,
            json!({ "isSandbox": true })
        ),
        Lane::Green,
    );
    for value in [
        json!("true"),
        json!("yes"),
        json!(1),
        json!(false),
        Value::Null,
        json!({}),
        json!(["true"]),
    ] {
        assert_eq!(
            lane(
                "ContextSpace",
                Operation::Create,
                json!({ "isSandbox": value })
            ),
            Lane::Yellow,
            "isSandbox {value} was read as a sandbox",
        );
    }
    assert_eq!(
        lane("ContextSpace", Operation::Create, json!({})),
        Lane::Yellow
    );
    assert_eq!(
        lane(
            "ContextSpace",
            Operation::Delete,
            json!({ "isSandbox": true })
        ),
        Lane::Red,
        "a sandbox is still deleted in the Red lane (CC-19)",
    );
}

// -------------------------------------------------------------------------------------------------
// The kinds that are Red whatever they hold
// -------------------------------------------------------------------------------------------------

/// PF-52, PF-62, CC-73: access and federation are reviewed in the Red lane on their own account, so
/// no spec can talk one of these kinds down a lane.
#[test]
fn access_and_federation_are_red_whatever_their_spec_says() {
    let disguises = [
        json!({}),
        json!({ "isSandbox": true }),
        json!({ "audience": "internal" }),
        json!({ "kind": "Dashboard" }),
        json!({ "lane": "green" }),
        Value::Null,
        json!("Dashboard"),
    ];
    for kind in RED_WHATEVER_THEY_HOLD {
        for op in [Operation::Create, Operation::Update] {
            for spec in &disguises {
                assert_eq!(
                    lane(kind, op, spec.clone()),
                    Lane::Red,
                    "{kind} {op:?} with spec {spec} left the Red lane",
                );
            }
        }
    }
}

/// CC-63: the catalogue and this function have to be read together. Every kind the Portal knows is
/// either Red whatever it holds, Green by name, or Yellow by decision — and a kind added to the
/// catalogue tomorrow lands in none of the three lists, which makes this case red until somebody
/// says which lane it belongs in. That is the point: the default of `classify` is Yellow, and a
/// kind that can hand out access must never reach production classified by a default.
#[test]
fn every_kind_the_catalogue_holds_has_a_lane_somebody_decided() {
    // Yellow by decision: a manifest of this kind changes what the platform serves, and one
    // approval by somebody who did not write it is the review it needs.
    //
    // Two of them are Yellow only because that is the lane they are in today, not because anybody
    // weighed them: `Subscription` is a standing egress of context data to an address in its own
    // spec, and `CkanInstance` publishes the catalogue outward. Both are filed as T-2292, which
    // moves them docs first; until it lands this list is the truth and this case holds it.
    const YELLOW_BY_DECISION: &[&str] = &[
        "AgentProfile",
        "AgentRun",
        "App",
        "AssistantDeployment", // internal; a public one is Red by its channel, below
        "Basemap",
        "Blueprint",
        "Bundle",
        "CkanInstance", // T-2292
        "ContextSpace",
        "DataModel",
        "DataSource",
        "Endpoint",
        "Entity",
        "KnowledgeSource",
        "Mapping",
        "McpServer", // organization or a project list; a public one is Red by its audience, below
        "ModelProjection",
        "Pipeline",
        "Subscription", // T-2292
        "SyncSource",
        "UiSchema",
        "View",
    ];

    let mut undecided = Vec::new();
    for info in joinedcontext_portal::resource::kinds() {
        let kind = info.kind;
        let listed = RED_WHATEVER_THEY_HOLD.contains(&kind)
            || GREEN.contains(&kind)
            || YELLOW_BY_DECISION.contains(&kind);
        if !listed {
            undecided.push(kind.to_owned());
            continue;
        }
        let expected = if RED_WHATEVER_THEY_HOLD.contains(&kind) {
            Lane::Red
        } else if GREEN.contains(&kind) {
            Lane::Green
        } else {
            Lane::Yellow
        };
        assert_eq!(
            lane(kind, Operation::Create, json!({})),
            expected,
            "{kind} is classified {:?}, not the {expected:?} this file says it is",
            lane(kind, Operation::Create, json!({})),
        );
    }
    assert!(
        undecided.is_empty(),
        "these kinds have no lane anybody decided, so `classify` gives them its Yellow default: \
         {undecided:?} — decide each one, in this file and in `src/change.rs`",
    );
}

/// CC-63: a kind that is not in the catalogue cannot be talked into the Green lane either. None of
/// these reaches `classify` in production — `resource::by_kind` (`src/resource/mod.rs:140`) matches
/// the kind exactly, so a manifest saying `role` is refused before a lane is asked for — and the
/// function is stricter than it needs to be anyway.
#[test]
fn a_kind_the_catalogue_does_not_have_is_never_green() {
    for forged in [
        "role",
        "ROLE",
        "Role ",
        " Role",
        "Role\u{0}",
        "R\u{0151}le",
        "dashboard",
        "Dashboard\n",
        "",
        "../../Dashboard",
        "Layer;Dashboard",
    ] {
        let decided = lane(forged, Operation::Create, json!({ "isSandbox": true }));
        assert_ne!(
            decided,
            Lane::Green,
            "{forged:?} was classified Green by a name the catalogue does not have",
        );
        assert!(
            joinedcontext_portal::resource::by_kind(forged).is_none(),
            "{forged:?} is a catalogue kind after all, so this case is about the wrong thing",
        );
    }
    // A very long name is a name like any other: no panic, no Green.
    let long = "R".repeat(100_000);
    assert_eq!(lane(&long, Operation::Create, json!({})), Lane::Yellow);
}

/// T-3057, MF-52: an assistant that answers anyone is public exposure and spends the model
/// budget, so its deployment is Red; one for signed-in staff, and a source, are Yellow.
#[test]
fn a_public_assistant_deployment_is_red_and_an_internal_one_yellow() {
    for channel in ["public", "ckan", "iframe"] {
        assert_eq!(
            lane(
                "AssistantDeployment",
                Operation::Create,
                json!({"channel": channel})
            ),
            Lane::Red,
            "{channel}"
        );
        assert_eq!(
            lane(
                "AssistantDeployment",
                Operation::Update,
                json!({"channel": channel})
            ),
            Lane::Red,
            "{channel}"
        );
    }
    assert_eq!(
        lane(
            "AssistantDeployment",
            Operation::Create,
            json!({"channel": "internal"})
        ),
        Lane::Yellow
    );
    assert_eq!(
        lane(
            "KnowledgeSource",
            Operation::Update,
            json!({"visibility": "public"})
        ),
        Lane::Yellow
    );
}

/// MF-53, ADR-N-043: declaring or widening a named MCP server to `public` exposes its members to
/// anyone and takes the Red lane; one for the organization or a list of projects is Yellow.
#[test]
fn a_public_mcp_server_is_red_and_a_narrower_one_yellow() {
    for operation in [Operation::Create, Operation::Update] {
        assert_eq!(
            lane("McpServer", operation, json!({"audience": "public"})),
            Lane::Red
        );
        for audience in ["organization", "project-list"] {
            assert_eq!(
                lane("McpServer", operation, json!({"audience": audience})),
                Lane::Yellow,
                "{audience}"
            );
        }
    }
    // An audience that is not a string is not public, and not Green either.
    assert_eq!(
        lane(
            "McpServer",
            Operation::Create,
            json!({"audience": ["public"]})
        ),
        Lane::Yellow
    );
}
