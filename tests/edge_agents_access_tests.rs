//! The edge cases of `agents/access.rs`: the profile's half of what an agent may call
//! (T-2081 `from_spec`, T-2082 `names`, T-2083 `proposes`, T-2084 `grants_endpoint`,
//! T-2085 `check`).
//!
//! The contract of the file in one sentence: **a profile narrows what the person who started
//! the run may already do and never widens it**, so every unreadable, misspelled, duplicated,
//! wrongly typed or look-alike entry in `spec.access` must grant less than the profile's author
//! wrote, never more, and a decision (approve, delete, update) is never an agent's however the
//! profile is written (AG-70, AG-11, PF-51, PF-57, AG-59).

mod common;

use jc_core::kinds::Verb;
use serde_json::{json, Value};

use joinedcontext_portal::agents::access::Access;
use joinedcontext_portal::ops;

/// Every operation the registry knows, as `names` sees it.
fn registry() -> &'static [ops::Operation] {
    ops::registry()
}

fn op(name: &str) -> &'static ops::Operation {
    ops::find(name).expect("a registered operation")
}

/// A profile that names every operation there is and grants every verb on every kind it can
/// name: the widest access block an author could write. What this still cannot reach is what no
/// profile can reach.
fn the_widest_profile() -> Access {
    let operations: Vec<&str> = registry().iter().map(|op| op.name).collect();
    let mut kinds: Vec<&str> = registry().iter().map(|op| op.kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    let kinds: Vec<Value> = kinds
        .into_iter()
        .map(|kind| json!({ "kind": kind, "verbs": ["read", "propose", "approve", "delete", "update", "*"] }))
        .collect();
    Access::from_spec(&json!({ "access": { "operations": operations, "kinds": kinds } }))
}

// ---------------------------------------------------------------------------------------
// T-2081 `from_spec`: reading an untyped spec must fail closed on every shape.
// ---------------------------------------------------------------------------------------

/// PF-51, AG-70: an `access` that is not an object is not an access block, and the profile falls
/// back to the read-only operations — never to "everything".
#[test]
fn an_access_block_that_is_not_an_object_grants_only_the_read_only_operations() {
    for shape in [
        json!("*"),
        json!(["jc_change_approve"]),
        json!(7),
        json!(true),
        json!(null),
    ] {
        let access = Access::from_spec(&json!({ "access": shape.clone() }));
        assert_eq!(
            access,
            Access::default(),
            "access: {shape} was read as a declaration"
        );
        assert!(
            !access.names(op("jc_change_approve")),
            "access: {shape} reached a decision"
        );
    }
}

/// PF-51: the spec itself may be anything the profile loader hands over; none of it declares
/// access.
#[test]
fn a_spec_that_is_not_an_object_declares_nothing() {
    for spec in [json!(null), json!([]), json!("access"), json!(3)] {
        assert_eq!(Access::from_spec(&spec), Access::default(), "{spec}");
    }
}

/// AG-70: an empty `access: {}` is a declaration — of nothing. It is stricter than no block at
/// all, which is the honest reading: an author who opened the block and named nothing named
/// nothing.
#[test]
fn an_empty_access_block_grants_nothing_at_all_not_even_a_read() {
    let access = Access::from_spec(&json!({ "access": {} }));
    assert_ne!(access, Access::default());
    let offered: Vec<&str> = registry()
        .iter()
        .filter(|op| access.names(op))
        .map(|op| op.name)
        .collect();
    assert!(offered.is_empty(), "an empty block offered {offered:?}");
}

/// PF-51: `operations` that is not an array is not a list of operations. A string is the shape a
/// model gets wrong most often, and reading it as one entry would grant it.
#[test]
fn an_operations_list_that_is_not_an_array_grants_no_operation() {
    for shape in [
        json!("jc_change_approve"),
        json!({ "0": "jc_change_approve" }),
        json!(7),
        json!(null),
    ] {
        let access = Access::from_spec(&json!({ "access": { "operations": shape.clone() } }));
        let offered: Vec<&str> = registry()
            .iter()
            .filter(|op| access.names(op))
            .map(|op| op.name)
            .collect();
        assert!(
            offered.is_empty(),
            "operations: {shape} offered {offered:?}"
        );
    }
}

/// PF-51: a non-string entry is dropped, never stringified — `7` must not become the operation
/// named `7`, and `null` must not become the empty name.
#[test]
fn non_string_entries_in_the_operations_list_are_dropped_and_the_rest_still_reads() {
    let access = Access::from_spec(&json!({ "access": {
        "operations": [7, null, {}, ["jc_change_approve"], "jc_catalog_search", "jc_catalog_search"],
        "kinds": [{ "kind": "*", "verbs": ["read"] }],
    }}));
    assert!(
        access.names(op("jc_catalog_search")),
        "the one string entry"
    );
    assert!(!access.names(op("jc_change_approve")));
}

/// PF-51: a grant entry without its name key, or with a name that is not a string, is not a
/// grant. An entry without `verbs` is a grant of nothing, not a grant of everything.
#[test]
fn a_grant_without_a_name_or_without_verbs_grants_nothing() {
    let access = Access::from_spec(&json!({ "access": {
        "operations": ["jc_endpoint_propose", "jc_catalog_search"],
        "kinds": [
            { "verbs": ["read", "propose"] },
            { "kind": 7, "verbs": ["read", "propose"] },
            { "kind": "Endpoint" },
            { "kind": "ContextSpace", "verbs": "read" },
        ],
        "endpoints": [{ "verbs": ["read"] }, { "name": null, "verbs": ["read"] }],
    }}));
    assert!(
        !access.names(op("jc_endpoint_propose")),
        "Endpoint had no verbs"
    );
    assert!(!access.proposes("jc_endpoint_propose", "Endpoint"));
    assert!(!access.proposes("jc_endpoint_propose", "ContextSpace"));
    // Nothing named an endpoint, so the endpoint list is empty and the person's Policy decides.
    assert!(access.grants_endpoint("helsinki-bikes", true));
}

/// PF-51: a kind listed twice collapses to one entry, and the last one written wins. Documented
/// here because the author's intent is ambiguous and the reader of a profile must know which
/// line is in force; both entries are the author's own, so neither widens anyone.
#[test]
fn a_kind_or_endpoint_listed_twice_keeps_the_entry_written_last() {
    let narrowing = Access::from_spec(&json!({ "access": {
        "operations": ["jc_endpoint_propose"],
        "kinds": [
            { "kind": "Endpoint", "verbs": ["read", "propose"] },
            { "kind": "Endpoint", "verbs": ["read"] },
        ],
        "endpoints": [
            { "name": "helsinki-bikes", "verbs": ["read", "write"] },
            { "name": "helsinki-bikes", "verbs": ["read"] },
        ],
    }}));
    assert!(!narrowing.proposes("jc_endpoint_propose", "Endpoint"));
    assert!(!narrowing.grants_endpoint("helsinki-bikes", true));
    assert!(narrowing.grants_endpoint("helsinki-bikes", false));
}

/// PF-51, PF-57: names are matched byte for byte. A profile that means `Endpoint` and writes
/// ` Endpoint`, `endpoint` or the Cyrillic look-alike grants nothing, because guessing what the
/// author meant is how a grant on one kind serves another.
#[test]
fn kind_operation_and_verb_names_are_matched_exactly() {
    let near_misses = [
        ("Endpoint ", "read"),
        (" Endpoint", "read"),
        ("endpoint", "read"),
        ("ENDPOINT", "read"),
        ("Endpoint/", "read"),
        ("Endpoint\n", "read"),
        ("Endpoint\0", "read"),
        // U+0415 CYRILLIC CAPITAL LETTER IE in place of the ASCII E.
        ("\u{0415}ndpoint", "read"),
        ("Endpoint", "Read"),
        ("Endpoint", "read "),
        ("Endpoint", "propose\u{200b}"),
    ];
    for (kind, verb) in near_misses {
        let access = Access::from_spec(&json!({ "access": {
            // `jc_kpi_compute` is verbless and its kind is `Endpoint`, so it asks the kind
            // grant for `read`; `jc_endpoint_propose` asks it for `propose`.
            "operations": ["jc_endpoint_propose", "jc_kpi_compute"],
            "kinds": [{ "kind": kind, "verbs": [verb, "propose"] }],
        }}));
        assert!(
            !access.names(op("jc_kpi_compute")) || (kind == "Endpoint" && verb == "read"),
            "kind {kind:?} verb {verb:?} reached a read on Endpoint"
        );
        assert!(
            !access.proposes("jc_endpoint_propose", "Endpoint") || kind == "Endpoint",
            "kind {kind:?} verb {verb:?} reached a propose on Endpoint"
        );
    }
    for spelling in [
        "jc_catalog_search ",
        " jc_catalog_search",
        "JC_CATALOG_SEARCH",
        "jc_catalog_search\n",
        "../jc_catalog_search",
    ] {
        let access = Access::from_spec(&json!({ "access": {
            "operations": [spelling],
            "kinds": [{ "kind": "*", "verbs": ["read", "propose"] }],
        }}));
        assert!(
            !access.names(op("jc_catalog_search")),
            "{spelling:?} was read as jc_catalog_search"
        );
    }
}

/// PF-51: a profile is a document a person or a model wrote, so it may be long, deep or full of
/// entries that name nothing. Reading it must terminate and grant only what it spells correctly.
#[test]
fn a_huge_or_deeply_nested_access_block_is_read_without_panic_and_grants_only_what_it_spells() {
    let mut deep = json!("bottom");
    for _ in 0..512 {
        deep = json!({ "nested": deep });
    }
    let many: Vec<String> = (0..20_000).map(|i| format!("jc_made_up_{i}")).collect();
    let access = Access::from_spec(&json!({ "access": {
        "operations": many,
        "kinds": [{ "kind": "Endpoint", "verbs": ["read"], "extra": deep }],
        "unknown": deep_enough(),
    }}));
    let offered: Vec<&str> = registry()
        .iter()
        .filter(|op| access.names(op))
        .map(|op| op.name)
        .collect();
    assert!(offered.is_empty(), "made-up names offered {offered:?}");
}

fn deep_enough() -> Value {
    let mut deep = json!([]);
    for _ in 0..256 {
        deep = json!([deep]);
    }
    deep
}

// ---------------------------------------------------------------------------------------
// T-2082 `names`: the profile's half of an operation offered to the model.
// ---------------------------------------------------------------------------------------

/// AG-70, AG-11: `names` returns early on `op.kind == "*"` and never looks at the verb, so an
/// operation that carries a verb must never carry the wildcard kind — a profile naming it would
/// otherwise be offered a write, an approval or a delete with no kind grant behind it.
#[test]
fn no_operation_with_the_wildcard_kind_carries_a_verb() {
    let unguarded: Vec<&str> = registry()
        .iter()
        .filter(|op| op.kind == "*" && op.verb.is_some())
        .map(|op| op.name)
        .collect();
    assert!(
        unguarded.is_empty(),
        "the wildcard kind skips the verb check: {unguarded:?}"
    );
}

/// AG-11: approving, deleting and updating are the person's. The widest profile an author could
/// write must still not reach one of them. `read` is the profile's to give; `jc_data_write` carries
/// it because the Endpoint's Policy decides the write, and a run reaches no write there: it holds no
/// bearer to exchange (`Caller::for_run`) and the model is never offered the tool (`MCP_ONLY`).
#[test]
fn the_widest_profile_reaches_no_verb_other_than_propose() {
    let access = the_widest_profile();
    let reached: Vec<&str> = registry()
        .iter()
        .filter(|op| !matches!(op.verb, None | Some(Verb::Propose | Verb::Read)))
        .filter(|op| access.names(op))
        .map(|op| op.name)
        .collect();
    assert!(reached.is_empty(), "a profile reached {reached:?}");
}

/// AG-70: without an access block the profile gets exactly the operations the registry annotates
/// read-only, and the set is not empty (an agent with no block is still useful).
#[test]
fn without_an_access_block_exactly_the_read_only_operations_are_offered() {
    let access = Access::default();
    for op in registry() {
        assert_eq!(
            access.names(op),
            op.annotations.read_only_hint,
            "{} disagrees with its read-only annotation",
            op.name
        );
    }
    assert!(registry().iter().any(|op| access.names(op)));
}

/// AG-70: naming the operation is necessary. A profile that grants every verb on every kind but
/// names no operation is offered nothing.
#[test]
fn a_kind_grant_without_the_operation_named_offers_nothing() {
    let kinds: Vec<Value> = registry()
        .iter()
        .map(|op| json!({ "kind": op.kind, "verbs": ["read", "propose"] }))
        .collect();
    let access = Access::from_spec(&json!({ "access": { "operations": [], "kinds": kinds } }));
    let offered: Vec<&str> = registry()
        .iter()
        .filter(|op| access.names(op))
        .map(|op| op.name)
        .collect();
    assert!(
        offered.is_empty(),
        "offered without being named: {offered:?}"
    );
}

/// AG-70: naming the operation is not sufficient either — the kind verb is the second half, and
/// `read` is not `propose`.
#[test]
fn naming_an_operation_without_the_kind_verb_offers_nothing_and_read_is_not_propose() {
    let named_only = Access::from_spec(&json!({ "access": {
        "operations": ["jc_endpoint_propose"],
    }}));
    assert!(!named_only.names(op("jc_endpoint_propose")));

    let read_only = Access::from_spec(&json!({ "access": {
        "operations": ["jc_endpoint_propose"],
        "kinds": [{ "kind": "Endpoint", "verbs": ["read"] }],
    }}));
    assert!(!read_only.names(op("jc_endpoint_propose")));

    let both = Access::from_spec(&json!({ "access": {
        "operations": ["jc_endpoint_propose"],
        "kinds": [{ "kind": "Endpoint", "verbs": ["read", "propose"] }],
    }}));
    assert!(both.names(op("jc_endpoint_propose")));
}

/// AG-70: an empty `verbs` list on the right kind is a grant of nothing, and a list of verbs the
/// catalogue does not know grants nothing either.
#[test]
fn an_empty_or_unknown_verb_list_on_the_right_kind_offers_nothing() {
    for verbs in [json!([]), json!(["*"]), json!(["admin"]), json!([null, 7])] {
        let access = Access::from_spec(&json!({ "access": {
            "operations": ["jc_endpoint_propose"],
            "kinds": [{ "kind": "Endpoint", "verbs": verbs.clone() }],
        }}));
        assert!(
            !access.names(op("jc_endpoint_propose")),
            "verbs {verbs} offered a propose"
        );
    }
}

/// AG-70: a verbless operation asks for `read` on its own kind, and the wildcard kind still asks
/// for the operation to be named.
#[test]
fn a_verbless_operation_needs_read_on_its_own_kind_and_the_wildcard_still_needs_the_name() {
    let wildcard_ops: Vec<&str> = registry()
        .iter()
        .filter(|op| op.kind == "*")
        .map(|op| op.name)
        .collect();
    assert!(
        !wildcard_ops.is_empty(),
        "no wildcard operation to prove it on"
    );

    let names_one = Access::from_spec(&json!({ "access": { "operations": [wildcard_ops[0]] } }));
    assert!(
        names_one.names(op(wildcard_ops[0])),
        "no kind grant is needed"
    );
    for other in &wildcard_ops[1..] {
        assert!(!names_one.names(op(other)), "{other} was not named");
    }
}

/// PF-57: a profile naming operations that do not exist changes nothing about the ones that do.
#[test]
fn made_up_operation_names_in_a_profile_change_nothing() {
    let access = Access::from_spec(&json!({ "access": {
        "operations": ["jc_catalog_search", "jc_delete_everything", "", "*", "../jc_change_approve"],
        "kinds": [{ "kind": "*", "verbs": ["read"] }],
    }}));
    assert!(access.names(op("jc_catalog_search")));
    assert!(!access.names(op("jc_change_approve")));
}

/// PF-57: reading the same profile twice gives the same answer — `names` holds no state that a
/// first call could widen for a second.
#[test]
fn asking_twice_gives_the_same_answer() {
    let access = the_widest_profile();
    let first: Vec<bool> = registry().iter().map(|op| access.names(op)).collect();
    let second: Vec<bool> = registry().iter().map(|op| access.names(op)).collect();
    assert_eq!(first, second);
}

// ---------------------------------------------------------------------------------------
// T-2083 `proposes`: the kind a tool names only when it is called.
// ---------------------------------------------------------------------------------------

/// AG-70: `proposes` looks the kind up by its exact name and knows no wildcard. A profile that
/// granted `propose` on `*` must not thereby propose on every kind — `names` treats `*` as the
/// operation's own kind, never as the author's grant.
#[test]
fn a_propose_grant_on_the_wildcard_kind_proposes_on_no_real_kind() {
    let access = Access::from_spec(&json!({ "access": {
        "operations": ["jc_resource_propose"],
        "kinds": [{ "kind": "*", "verbs": ["read", "propose"] }],
    }}));
    for kind in ["Endpoint", "Pipeline", "ContextSpace", "Policy", "Change"] {
        assert!(
            !access.proposes("jc_resource_propose", kind),
            "the wildcard reached {kind}"
        );
    }
}

/// AG-70: both halves, always — the operation named and `propose` on the kind that the call
/// names.
#[test]
fn proposes_needs_the_operation_named_and_propose_on_that_kind() {
    let access = Access::from_spec(&json!({ "access": {
        "operations": ["jc_resource_propose"],
        "kinds": [
            { "kind": "Pipeline", "verbs": ["read", "propose"] },
            { "kind": "ContextSpace", "verbs": ["read"] },
        ],
    }}));
    assert!(access.proposes("jc_resource_propose", "Pipeline"));
    assert!(
        !access.proposes("jc_resource_propose", "ContextSpace"),
        "read is not propose"
    );
    assert!(
        !access.proposes("jc_resource_propose", "Endpoint"),
        "not granted"
    );
    assert!(
        !access.proposes("jc_endpoint_propose", "Pipeline"),
        "not named"
    );
}

/// AG-70: no access block and an empty one both propose nothing.
#[test]
fn a_profile_without_a_declaration_proposes_nothing() {
    assert!(!Access::default().proposes("jc_resource_propose", "Pipeline"));
    let empty = Access::from_spec(&json!({ "access": {} }));
    assert!(!empty.proposes("jc_resource_propose", "Pipeline"));
}

/// PF-57: the kind arrives from the tool call, so it is caller input. Every spelling that is not
/// the granted kind is refused, including the empty one, a traversal and a look-alike.
#[test]
fn a_kind_that_is_not_the_granted_one_never_proposes() {
    let access = Access::from_spec(&json!({ "access": {
        "operations": ["jc_resource_propose"],
        "kinds": [{ "kind": "Pipeline", "verbs": ["read", "propose"] }],
    }}));
    for kind in [
        "",
        " ",
        "pipeline",
        "PIPELINE",
        "Pipeline ",
        " Pipeline",
        "Pipeline/",
        "Pipeline\0",
        "Pipeline\r\nX-Injected: 1",
        "../Pipeline",
        "%2e%2e/Pipeline",
        "/Pipeline",
        "\u{0420}ipeline",
        "Pipeline\u{200b}",
    ] {
        assert!(
            !access.proposes("jc_resource_propose", kind),
            "{kind:?} proposed as Pipeline"
        );
    }
    assert!(access.proposes("jc_resource_propose", "Pipeline"));
}

/// PF-57: the operation name is caller input too, and the same exactness applies to it.
#[test]
fn an_operation_name_that_is_not_the_named_one_never_proposes() {
    let access = Access::from_spec(&json!({ "access": {
        "operations": ["jc_resource_propose"],
        "kinds": [{ "kind": "Pipeline", "verbs": ["read", "propose"] }],
    }}));
    for operation in [
        "",
        "jc_resource_propose ",
        "JC_RESOURCE_PROPOSE",
        "jc_resource_propose\0",
        "../jc_resource_propose",
        "jc_resource_proposex",
    ] {
        assert!(
            !access.proposes(operation, "Pipeline"),
            "{operation:?} was read as jc_resource_propose"
        );
    }
}

/// AG-11: `propose` is the only verb `proposes` reads; granting `approve` or `delete` on a kind
/// proposes nothing and reaches no decision.
#[test]
fn granting_a_decision_verb_on_a_kind_proposes_nothing() {
    let access = Access::from_spec(&json!({ "access": {
        "operations": ["jc_resource_propose"],
        "kinds": [{ "kind": "Pipeline", "verbs": ["approve", "delete", "update", "write", "*"] }],
    }}));
    assert!(!access.proposes("jc_resource_propose", "Pipeline"));
}

/// AG-70: the endpoint list is a different question; it never stands in for a kind grant.
#[test]
fn the_endpoint_list_does_not_propose_on_a_kind() {
    let access = Access::from_spec(&json!({ "access": {
        "operations": ["jc_resource_propose"],
        "endpoints": [{ "name": "helsinki-bikes", "verbs": ["read", "write"] }],
    }}));
    assert!(!access.proposes("jc_resource_propose", "Endpoint"));
}

// ---------------------------------------------------------------------------------------
// T-2084 `grants_endpoint`: which endpoint a run may build on.
// ---------------------------------------------------------------------------------------

/// PF-51: `write` asks for both verbs. An entry that grants `write` without `read` grants
/// nothing at all — an author who wrote one verb does not get the other for free.
#[test]
fn write_without_read_grants_neither_a_read_nor_a_write() {
    let access = Access::from_spec(&json!({ "access": { "operations": [], "endpoints": [
        { "name": "helsinki-kpi", "verbs": ["write"] },
    ]}}));
    assert!(!access.grants_endpoint("helsinki-kpi", false));
    assert!(!access.grants_endpoint("helsinki-kpi", true));
}

/// PF-51: read and write are separate, and a read grant never writes.
#[test]
fn a_read_grant_reads_and_does_not_write() {
    let access = Access::from_spec(&json!({ "access": { "operations": [], "endpoints": [
        { "name": "helsinki-bikes", "verbs": ["read"] },
        { "name": "helsinki-kpi", "verbs": ["read", "write"] },
    ]}}));
    assert!(access.grants_endpoint("helsinki-bikes", false));
    assert!(!access.grants_endpoint("helsinki-bikes", true));
    assert!(access.grants_endpoint("helsinki-kpi", false));
    assert!(access.grants_endpoint("helsinki-kpi", true));
}

/// PF-51: once the profile lists any endpoint, everything it does not list is refused — the list
/// is a narrowing, not a hint.
#[test]
fn an_endpoint_the_list_does_not_name_is_refused() {
    let access = Access::from_spec(&json!({ "access": { "operations": [], "endpoints": [
        { "name": "helsinki-bikes", "verbs": ["read", "write"] },
    ]}}));
    for name in ["", " ", "helsinki-air", "helsinki-bikes2"] {
        assert!(!access.grants_endpoint(name, false), "{name:?} was granted");
    }
}

/// PF-57: the endpoint name reaches this function from the run, so near misses are refused —
/// case, spaces, a trailing slash, a traversal, a null byte, a look-alike.
#[test]
fn an_endpoint_name_is_matched_exactly() {
    let access = Access::from_spec(&json!({ "access": { "operations": [], "endpoints": [
        { "name": "helsinki-bikes", "verbs": ["read", "write"] },
    ]}}));
    for name in [
        "helsinki-bikes ",
        " helsinki-bikes",
        "Helsinki-Bikes",
        "HELSINKI-BIKES",
        "helsinki-bikes/",
        "/helsinki-bikes",
        "helsinki-bikes\0",
        "helsinki-bikes\r\nX-Injected: 1",
        "../helsinki-bikes",
        "%2e%2e/helsinki-bikes",
        "helsinki%2dbikes",
        "helsinki\u{2010}bikes",
        "helsinki-bikes\u{200b}",
    ] {
        assert!(!access.grants_endpoint(name, false), "{name:?} was granted");
    }
    assert!(access.grants_endpoint("helsinki-bikes", true));
}

/// PF-51: a verb list that names neither `read` nor `write`, or names them in another spelling,
/// grants nothing.
#[test]
fn an_unknown_or_misspelt_verb_grants_no_endpoint() {
    for verbs in [
        json!([]),
        json!(["*"]),
        json!(["Read"]),
        json!(["read "]),
        json!(["propose"]),
        json!([null, 7]),
    ] {
        let access = Access::from_spec(&json!({ "access": { "operations": [], "endpoints": [
            { "name": "helsinki-bikes", "verbs": verbs.clone() },
        ]}}));
        assert!(
            !access.grants_endpoint("helsinki-bikes", false),
            "verbs {verbs} granted a read"
        );
    }
}

/// PF-51, T-2084: a profile with no endpoint list — and a profile with no access block at all —
/// leaves the endpoint to the person and to the Endpoint's own Policy, which is the documented
/// contract of this function. It is a grant of everything *here* only because the two checks
/// behind it are not skipped; asserted so a caller that starts relying on this alone is noticed.
#[test]
fn an_empty_endpoint_list_defers_to_the_person_and_the_policy() {
    for spec in [
        json!({ "access": { "operations": [] } }),
        json!({ "access": { "operations": [], "endpoints": [] } }),
        json!({ "role": "builder" }),
    ] {
        let access = Access::from_spec(&spec);
        assert!(access.grants_endpoint("helsinki-air", true), "{spec}");
    }
    assert!(Access::default().grants_endpoint("helsinki-air", true));
}

/// PF-51: an endpoint entry that is not an object, or whose name is not a string, is dropped —
/// and dropping every entry leaves the list empty, which defers rather than refuses. Written
/// down because that is the one shape where a malformed profile is wider than a correct one.
#[test]
fn endpoint_entries_that_are_not_objects_are_dropped_and_an_all_malformed_list_defers() {
    let partly = Access::from_spec(&json!({ "access": { "operations": [], "endpoints": [
        "helsinki-bikes",
        7,
        { "name": 7, "verbs": ["read"] },
        { "name": "helsinki-kpi", "verbs": ["read"] },
    ]}}));
    assert!(partly.grants_endpoint("helsinki-kpi", false));
    assert!(
        !partly.grants_endpoint("helsinki-bikes", false),
        "a bare string is no grant"
    );

    let all_malformed = Access::from_spec(&json!({ "access": { "operations": [], "endpoints": [
        "helsinki-bikes", 7, null,
    ]}}));
    assert!(
        all_malformed.grants_endpoint("helsinki-bikes", true),
        "an unreadable list is an empty list, and an empty list defers to the Policy"
    );
}

/// PF-51: `endpoints` that is not an array is no list, and defers the same way an absent one
/// does.
#[test]
fn an_endpoint_list_that_is_not_an_array_defers() {
    for shape in [
        json!("helsinki-bikes"),
        json!({ "helsinki-bikes": ["read"] }),
        json!(7),
    ] {
        let access = Access::from_spec(
            &json!({ "access": { "operations": [], "endpoints": shape.clone() } }),
        );
        assert!(access.grants_endpoint("helsinki-air", true), "{shape}");
    }
}

/// PF-51: a long list is read to the end, and the entry that is there is found.
#[test]
fn a_long_endpoint_list_is_read_to_its_end() {
    let mut endpoints: Vec<Value> = (0..1024)
        .map(|i| json!({ "name": format!("endpoint-{i}"), "verbs": ["read"] }))
        .collect();
    endpoints.push(json!({ "name": "helsinki-kpi", "verbs": ["read", "write"] }));
    let access =
        Access::from_spec(&json!({ "access": { "operations": [], "endpoints": endpoints } }));
    assert!(access.grants_endpoint("helsinki-kpi", true));
    assert!(access.grants_endpoint("endpoint-1023", false));
    assert!(!access.grants_endpoint("endpoint-1024", false));
}

// ---------------------------------------------------------------------------------------
// T-2085 `check`: both halves at the moment of the call.
// ---------------------------------------------------------------------------------------

/// PF-50, AG-70: an operation the registry does not know is refused, whatever the profile says
/// and whoever asks — and the reason names only what the caller sent.
#[tokio::test]
async fn an_operation_that_is_not_registered_is_refused_for_everyone() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    let access = the_widest_profile();
    for name in [
        "",
        " ",
        "jc_delete_everything",
        "../jc_catalog_search",
        "jc_catalog_search ",
        "JC_CATALOG_SEARCH",
        "jc_catalog_search\0",
    ] {
        let refused = access.check(name, &common::person("jana"), &state, "helsinki");
        assert_eq!(
            refused,
            Err(format!("operation '{name}' is not registered")),
            "{name:?}"
        );
    }
}

/// PF-57: a name a thousand characters long, with control characters in it, is refused like any
/// other unknown name and never panics or truncates into a name that exists.
#[tokio::test]
async fn an_enormous_or_control_laden_operation_name_is_refused_without_panic() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    let access = the_widest_profile();
    for name in [
        "a".repeat(1024 * 1024),
        format!("jc_catalog_search\r\n{}", "X".repeat(64)),
        "jc_catalog_search\u{0000}jc_change_approve".to_owned(),
    ] {
        let refused = access.check(&name, &common::person("jana"), &state, "helsinki");
        assert!(refused.is_err(), "{name:.40?} was accepted");
    }
}

/// AG-70, PF-59: the profile's half is answered first, so an operation the profile does not name
/// is refused with the same sentence whether or not the caller could have run it — the refusal
/// says nothing about the project.
#[tokio::test]
async fn the_profile_refusal_comes_first_and_reveals_nothing_about_the_project() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    let access = Access::from_spec(&json!({ "access": { "operations": [] } }));
    let expected = Err(joinedcontext_portal::agents::access::refusal(
        "jc_catalog_search",
    ));

    let nobody = common::person("jana");
    let mut admin = common::person("ada");
    admin.groups = vec!["portal-approver".to_owned()];

    for project in ["helsinki", "a-project-that-is-not-there"] {
        assert_eq!(
            access.check("jc_catalog_search", &nobody, &state, project),
            expected,
            "{project} for a person with no grant"
        );
        assert_eq!(
            access.check("jc_catalog_search", &admin, &state, project),
            expected,
            "{project} for an administrator"
        );
    }
}

/// PF-59, R20: a read the profile does name, by a person no binding names, answers as a project
/// that is not there — not as a project they may not read.
#[tokio::test]
async fn a_read_by_a_person_with_no_binding_answers_not_found_and_not_denied() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    let access = the_widest_profile();
    let refused = access
        .check(
            "jc_catalog_search",
            &common::person("jana"),
            &state,
            "helsinki",
        )
        .expect_err("a person with no binding reads nothing");
    assert!(
        refused.contains("not found"),
        "a read told the caller the project exists: {refused}"
    );
    assert!(
        !refused.contains("403") && !refused.to_lowercase().contains("denied"),
        "{refused}"
    );
}

/// PF-50, PF-57: the refusal is a sentence for a person and a model, so it carries no stack, no
/// SQL, no forge or realm URL and no token.
#[tokio::test]
async fn a_refusal_carries_no_stack_no_sql_and_no_internal_url() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    let host = gitea.uri();
    for access in [
        Access::default(),
        the_widest_profile(),
        Access::from_spec(&json!({ "access": {} })),
    ] {
        for name in ["jc_catalog_search", "jc_change_approve", "jc_made_up"] {
            let Err(refused) = access.check(name, &common::person("jana"), &state, "helsinki")
            else {
                continue;
            };
            let lower = refused.to_lowercase();
            for leak in [
                "select ",
                "insert ",
                "panicked",
                "stack backtrace",
                "token",
                "http://",
                "https://",
                "keycloak",
                "gitea",
            ] {
                assert!(!lower.contains(leak), "{refused:?} carries {leak:?}");
            }
            assert!(
                !refused.contains(&host),
                "{refused:?} carries the forge address"
            );
        }
    }
}

/// AG-11: a decision is refused by the profile's half for everybody, including the bootstrap
/// administrator who may approve everything by hand.
#[tokio::test]
async fn a_decision_is_refused_by_the_profile_half_even_for_an_administrator() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    let access = the_widest_profile();
    let mut admin = common::person("ada");
    admin.groups = vec!["portal-approver".to_owned()];
    for op in registry()
        .iter()
        .filter(|op| op.kind == "Change" && op.verb.is_some())
    {
        assert_eq!(
            access.check(op.name, &admin, &state, "helsinki"),
            Err(joinedcontext_portal::agents::access::refusal(op.name)),
            "{} reached an administrator's decision",
            op.name
        );
    }
}

/// AG-70, AG-82: `check` is the profile's and the person's half, and not the agent gate. An
/// operation no agent may run must still be refused by `Caller::may_run` after `check` passes,
/// which is what `ops::call` does behind every tool.
#[tokio::test]
async fn check_is_not_the_only_gate_an_agent_passes() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    let access = the_widest_profile();
    let mut admin = common::person("ada");
    admin.groups = vec!["portal-approver".to_owned()];
    let agent = ops::Caller {
        bearer: None,
        identity: admin.clone(),
        via: ops::Via::Agent,
        access: None,
    };
    for name in ["jc_workspace_propose", "jc_workspace_discard"] {
        let profile_and_person = access.check(name, &admin, &state, "helsinki");
        let agent_gate = agent.may_run(op(name));
        assert!(
            profile_and_person.is_err() || agent_gate.is_err(),
            "{name} passed both the profile and the agent gate"
        );
        assert!(agent_gate.is_err(), "{name} is not an agent's (AG-82)");
    }
}

/// PF-50: asking twice gives the same answer; `check` caches nothing that a first refusal could
/// turn into a second acceptance.
#[tokio::test]
async fn asking_check_twice_gives_the_same_answer() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    let access = the_widest_profile();
    let jana = common::person("jana");
    for name in ["jc_catalog_search", "jc_change_approve", "jc_made_up"] {
        let first = access.check(name, &jana, &state, "helsinki");
        let second = access.check(name, &jana, &state, "helsinki");
        assert_eq!(first, second, "{name}");
    }
}

/// PF-57: the project name is caller-shaped too. A traversal or a control character in it is not
/// a project the caller may read, and the answer is a refusal rather than a panic.
#[tokio::test]
async fn a_project_name_that_is_not_one_is_refused_without_panic() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    let access = the_widest_profile();
    for project in [
        "",
        " ",
        "../helsinki",
        "%2e%2e/helsinki",
        "helsinki\0",
        "helsinki\r\nX: 1",
        &"p".repeat(4096),
    ] {
        let refused = access.check(
            "jc_catalog_search",
            &common::person("jana"),
            &state,
            project,
        );
        assert!(refused.is_err(), "{project:?} was accepted");
    }
}
