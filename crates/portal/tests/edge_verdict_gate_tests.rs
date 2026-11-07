//! Edge cases of the verdict gate and the listing it feeds (T-2119, T-2121 – T-2127; PF-57, AG-62).
//!
//! **The contract, in one sentence:** a proposal passes only on a verdict that is green, recorded in
//! this project, and taken of **this exact manifest** — and on a strict installation nothing else
//! passes, whoever asks and however the manifest is spelled.
//!
//! The gate is the one place where "somebody checked this" is remembered
//! (`src/ops/mod.rs:1130-1300`). A verdict accepted for the wrong manifest is a manifest that reaches
//! a Change without anyone having seen what it does; a verdict readable across people is a check one
//! person can do for another's import. So the cases below aim at the binding — manifest, project,
//! person — and at what the refusal says, which travels to a client through the MCP door as well as
//! the REST one.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops::verdict::{Finding, Level, Verdict};
use joinedcontext_portal::ops::{self, Caller, OpError, Via};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

const PROJECT: &str = "ovzdusie";
/// A value that must never appear in anything the gate answers.
const SECRET_IN_THE_MANIFEST: &str = "hunter2-not-for-an-answer";

fn identity(username: &str, groups: &[&str]) -> Identity {
    Identity {
        client: None,
        subject: format!("f:1:{username}"),
        username: username.to_owned(),
        email: Some(format!("{username}@banskabystrica.sk")),
        name: Some(username.to_owned()),
        roles: Vec::new(),
        groups: groups.iter().map(|group| (*group).to_owned()).collect(),
        second_factor: false,
    }
}

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

/// A project with one role that may propose and one that may only look.
fn state_with_roles(config: Config) -> AppState {
    let state = AppState::new(config, None);
    state.mirror.upsert(org(
        "Role",
        "developer",
        json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace", "DataSource", "Pipeline"],
                            "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "dev-binding",
        json!({ "subjects": [{ "group": "devs" }], "role": "developer",
                "scope": { "project": PROJECT } }),
    ));
    state.mirror.upsert(org(
        "Role",
        "read-only-role",
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": [] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "viewer-binding",
        json!({ "subjects": [{ "group": "city-viewers" }], "role": "read-only-role",
                "scope": { "project": PROJECT } }),
    ));
    state
}

/// A manifest that carries something nobody may read back out of an answer.
fn manifest(name: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": name, "namespace": PROJECT },
        "spec": { "isSandbox": false, "description": SECRET_IN_THE_MANIFEST },
    })
}

/// A branding file that says `validation: lax`, and the config that names it.
fn lax_config() -> Config {
    let path = std::env::temp_dir().join(format!("jc-lax-branding-{}.yaml", std::process::id()));
    std::fs::write(&path, "validation: lax\n").expect("write the branding file");
    let mut config = Config::for_tests();
    config.branding_file = Some(path.to_string_lossy().into_owned());
    config
}

/// Records a verdict of `judged` under the kind and name of `held`, the way a check does.
async fn recorded(state: &AppState, project: &str, held: &Value, verdict: Verdict) {
    let kind = held["kind"].as_str().expect("a kind");
    let name = held["metadata"]["name"].as_str().expect("a name");
    let store = state.drafts.clone();
    store
        .put(project, kind, name, held.clone(), Some(0), "jana", "person")
        .await
        .expect("the draft is written");
    store
        .set_verdict(project, kind, name, verdict)
        .await
        .expect("the verdict is written");
}

/// The gate's own words for what it answered, so a case can compare without `PartialEq`.
fn passed(outcome: Result<bool, OpError>) -> bool {
    match outcome {
        Ok(false) => true,
        Ok(true) => panic!("the gate let an unchecked manifest through"),
        Err(err) => panic!("the gate refused: {err}"),
    }
}

/// True when the gate let an unchecked manifest through, which only a lax installation does.
fn skipped(outcome: Result<bool, OpError>) -> bool {
    match outcome {
        Ok(true) => true,
        Ok(false) => false,
        Err(err) => panic!("the gate refused instead of skipping: {err}"),
    }
}

fn refusal_of(err: OpError) -> Value {
    match err {
        OpError::Conflict(body) => body,
        other => panic!("the gate answered {other} instead of a refusal"),
    }
}

// -------------------------------------------------------------------------------------------------
// T-2121 `verdict_for_manifest`
// -------------------------------------------------------------------------------------------------

/// PF-57: the verdict has to be of this manifest. One changed byte, one changed key, a verdict of
/// another resource, of another project, or a red one: each is a refusal that names the check to run.
#[tokio::test]
async fn only_a_green_verdict_of_this_exact_manifest_in_this_project_opens_the_gate() {
    let state = state_with_roles(Config::for_tests());
    let subject = manifest("ovzdusie-main");
    recorded(&state, PROJECT, &subject, Verdict::green(&subject, None)).await;

    // The manifest that was checked passes, and the key order it is written in does not matter.
    assert!(
        passed(ops::verdict_for_manifest(&state, PROJECT, &subject).await),
        "a checked manifest was refused"
    );
    let reordered = json!({
        "spec": { "description": SECRET_IN_THE_MANIFEST, "isSandbox": false },
        "metadata": { "namespace": PROJECT, "name": "ovzdusie-main" },
        "kind": "ContextSpace",
        "apiVersion": API_VERSION,
    });
    assert!(
        passed(ops::verdict_for_manifest(&state, PROJECT, &reordered).await),
        "the same manifest written in another key order was refused"
    );

    // One changed byte anywhere in the spec is a different manifest: stale.
    let mut changed = subject.clone();
    changed["spec"]["isSandbox"] = json!(true);
    let body = refusal_of(
        ops::verdict_for_manifest(&state, PROJECT, &changed)
            .await
            .expect_err("a changed manifest passed on the old verdict"),
    );
    assert_eq!(body["error"], json!("verdict_required"));
    assert_eq!(body["reason"], json!("stale"));

    // A resource nobody checked, and the same manifest in a project where nothing was recorded.
    for (project, candidate) in [
        (PROJECT, manifest("ovzdusie-other")),
        ("doprava", subject.clone()),
    ] {
        let body = refusal_of(
            ops::verdict_for_manifest(&state, project, &candidate)
                .await
                .expect_err("an unchecked manifest passed"),
        );
        assert_eq!(body["reason"], json!("verdict_absent"), "{project}");
    }

    // A check that found problems is not a check that passed.
    let red = manifest("ovzdusie-red");
    recorded(
        &state,
        PROJECT,
        &red,
        Verdict::red(
            &red,
            vec![Finding {
                level: Level::Error,
                path: "spec.isSandbox".into(),
                message: "the dry run refused it".into(),
            }],
            None,
        ),
    )
    .await;
    let body = refusal_of(
        ops::verdict_for_manifest(&state, PROJECT, &red)
            .await
            .expect_err("a red verdict opened the gate"),
    );
    assert_eq!(body["reason"], json!("verdict_failed"));
}

/// PF-57, PF-51: the refusal is a sentence a person can act on — the check to run and why — and it
/// carries nothing of the manifest it judged. The same document travels to an MCP client, so a value
/// typed into a manifest may not ride out on it.
#[tokio::test]
async fn the_refusal_names_the_check_to_run_and_carries_nothing_of_the_manifest() {
    let state = state_with_roles(Config::for_tests());
    // Each kind has its own check, and the refusal names that one (`check_for_kind`).
    for (kind, check) in [
        ("ContextSpace", "jc_manifest_dry_run"),
        ("DataSource", "jc_datasource_check"),
        ("Pipeline", "jc_pipeline_test"),
    ] {
        let candidate = json!({
            "apiVersion": API_VERSION,
            "kind": kind,
            "metadata": { "name": "unchecked", "namespace": PROJECT },
            "spec": { "token": SECRET_IN_THE_MANIFEST },
        });
        let body = refusal_of(
            ops::verdict_for_manifest(&state, PROJECT, &candidate)
                .await
                .expect_err("an unchecked manifest passed"),
        );
        assert_eq!(body["check"], json!(check), "{kind}");
        assert!(
            body["detail"]
                .as_str()
                .is_some_and(|text| text.contains("check")),
            "{kind} was refused without telling anybody what to do: {body}",
        );
        let text = body.to_string();
        for leaked in [SECRET_IN_THE_MANIFEST, "unchecked", "hunter2"] {
            assert!(
                !text.contains(leaked),
                "the refusal of a {kind} carried {leaked:?}: {text}",
            );
        }
    }
}

/// PF-57: a manifest with no kind or no name has no check of its own to look for, so the gate says
/// nothing and the proposal refuses the shape instead — `jc_resource_propose`'s input requires both
/// (`src/ops/resources.rs`, `kind_named`), and a manifest that reaches the forge without them is
/// refused there. The case is here so that "the gate is silent" stays a decision and not a surprise.
#[tokio::test]
async fn a_manifest_that_names_nothing_is_left_to_the_proposal_to_refuse() {
    let state = state_with_roles(Config::for_tests());
    for shapeless in [
        json!({}),
        json!({ "kind": "ContextSpace" }),
        json!({ "metadata": { "name": "no-kind" } }),
        json!({ "kind": 7, "metadata": { "name": "not-a-string" } }),
        json!({ "kind": "ContextSpace", "metadata": { "name": 7 } }),
        json!("not a manifest"),
        Value::Null,
    ] {
        assert!(
            passed(ops::verdict_for_manifest(&state, PROJECT, &shapeless).await),
            "{shapeless} was answered by the gate rather than by the proposal"
        );
    }
}

/// PF-57: a lax installation lets an unchecked manifest through and the answer says so (`Ok(true)`
/// is what the caller logs as "nobody checked this"). The binding is untouched: lax changes who may
/// skip the check, never what a verdict is a verdict of.
#[tokio::test]
async fn a_lax_installation_lets_an_unchecked_manifest_through_and_marks_it() {
    let state = state_with_roles(lax_config());
    let subject = manifest("ovzdusie-main");
    assert!(
        skipped(ops::verdict_for_manifest(&state, PROJECT, &subject).await),
        "a lax installation refused an unchecked manifest"
    );
    // A recorded green verdict of this manifest is still the answer that nothing was skipped.
    recorded(&state, PROJECT, &subject, Verdict::green(&subject, None)).await;
    assert!(passed(
        ops::verdict_for_manifest(&state, PROJECT, &subject).await
    ));
    // And a stale one is still skipped rather than accepted as fresh.
    let mut changed = subject.clone();
    changed["spec"]["isSandbox"] = json!(true);
    assert!(skipped(
        ops::verdict_for_manifest(&state, PROJECT, &changed).await
    ));
}

// -------------------------------------------------------------------------------------------------
// T-2122 `forget_check`, T-2123 `record_check`
// -------------------------------------------------------------------------------------------------

/// T-0956: after a proposal, the draft its check created goes — but only while it still holds that
/// manifest. A person's own draft of the same resource with other content is theirs, and forgetting
/// somebody else's proposal may not take it.
#[tokio::test]
async fn forgetting_a_check_takes_only_the_draft_that_still_holds_that_manifest() {
    let state = state_with_roles(Config::for_tests());
    let mine = manifest("ovzdusie-main");
    recorded(&state, PROJECT, &mine, Verdict::green(&mine, None)).await;

    // Forgetting a different manifest of the same kind and name leaves the draft alone.
    let mut theirs = mine.clone();
    theirs["spec"]["isSandbox"] = json!(true);
    ops::forget_check(&state, PROJECT, &theirs).await;
    assert!(
        passed(ops::verdict_for_manifest(&state, PROJECT, &mine).await),
        "forgetting another manifest took this one's check"
    );

    // Forgetting in another project takes nothing either.
    ops::forget_check(&state, "doprava", &mine).await;
    assert!(passed(
        ops::verdict_for_manifest(&state, PROJECT, &mine).await
    ));

    // Forgetting this manifest takes it, and the gate refuses it again.
    ops::forget_check(&state, PROJECT, &mine).await;
    let body = refusal_of(
        ops::verdict_for_manifest(&state, PROJECT, &mine)
            .await
            .expect_err("a forgotten check still opened the gate"),
    );
    assert_eq!(body["reason"], json!("verdict_absent"));

    // Forgetting a shapeless manifest, or one that was never there, is not an error and takes
    // nothing: a second proposal of the same change must not be able to clear somebody's draft.
    for shapeless in [json!({}), json!("not a manifest"), Value::Null] {
        ops::forget_check(&state, PROJECT, &shapeless).await;
    }
}

/// T-0956, PF-50: a check is recorded where the proposal of the same manifest looks for it — and only
/// for somebody who may propose it. A viewer's check writes no draft of theirs
/// (`record_verdict` asks `Verb::Propose` first), so nobody can seed a project they may only read.
#[tokio::test]
async fn a_check_is_recorded_for_whoever_may_propose_it_and_for_nobody_else() {
    let state = state_with_roles(Config::for_tests());
    let subject = manifest("ovzdusie-main");

    let viewer = Caller::new(identity("jana", &["city-viewers"]), Via::Session);
    ops::record_check(
        &viewer,
        &state,
        PROJECT,
        &subject,
        &Verdict::green(&subject, None),
    )
    .await;
    let body = refusal_of(
        ops::verdict_for_manifest(&state, PROJECT, &subject)
            .await
            .expect_err("a viewer's check opened the gate"),
    );
    assert_eq!(body["reason"], json!("verdict_absent"));

    // The developer's own check is recorded, and it is found by the manifest alone.
    let developer = Caller::new(identity("stefan", &["devs"]), Via::Session);
    ops::record_check(
        &developer,
        &state,
        PROJECT,
        &subject,
        &Verdict::green(&subject, None),
    )
    .await;
    assert!(passed(
        ops::verdict_for_manifest(&state, PROJECT, &subject).await
    ));

    // On a draft that exists, the developer's red check holds: a viewer's green one after it
    // replaces nothing (T-4840).
    ops::record_check(
        &developer,
        &state,
        PROJECT,
        &subject,
        &Verdict::red(&subject, Vec::new(), None),
    )
    .await;
    ops::record_check(
        &viewer,
        &state,
        PROJECT,
        &subject,
        &Verdict::green(&subject, None),
    )
    .await;
    let body = refusal_of(
        ops::verdict_for_manifest(&state, PROJECT, &subject)
            .await
            .expect_err("a viewer's check replaced the developer's red one"),
    );
    assert_eq!(body["reason"], json!("verdict_failed"));

    // A manifest that names nothing records nothing, and nothing panics.
    for shapeless in [json!({}), json!({ "kind": "ContextSpace" }), Value::Null] {
        ops::record_check(
            &developer,
            &state,
            PROJECT,
            &shapeless,
            &Verdict::green(&shapeless, None),
        )
        .await;
    }
}

// -------------------------------------------------------------------------------------------------
// T-2124 `record_import_check`, T-2125 `verdict_for_import`, T-2126 `forget_import_check`
// -------------------------------------------------------------------------------------------------

/// T-1460, PF-57: an import's check is held under the caller's own name, so it is one checked import
/// per person and project. One person's check may never let another person's import through, and a
/// bundle that changed after the check is checked again.
#[tokio::test]
async fn one_persons_import_check_is_never_anothers_and_never_another_bundles() {
    let state = state_with_roles(Config::for_tests());
    let jana = identity("jana", &["devs"]);
    let stefan = identity("stefan", &["devs"]);
    let bundle = json!({ "files": ["projects/ovzdusie/spaces/main.yaml"], "count": 1 });

    ops::record_import_check(&state, &jana, PROJECT, &bundle).await;
    assert!(
        passed(ops::verdict_for_import(&state, &jana, PROJECT, &bundle).await),
        "the person who checked the bundle was refused"
    );

    // Somebody else's import of the very same files is unchecked as far as they are concerned.
    let body = refusal_of(
        ops::verdict_for_import(&state, &stefan, PROJECT, &bundle)
            .await
            .expect_err("another person's check let this import through"),
    );
    assert_eq!(body["error"], json!("verdict_required"));
    assert_eq!(body["check"], json!("jc_project_import"));

    // The same person in another project, and the same person with other files: both unchecked.
    assert!(ops::verdict_for_import(&state, &jana, "doprava", &bundle)
        .await
        .is_err());
    let changed = json!({ "files": ["projects/ovzdusie/spaces/main.yaml", "projects/ovzdusie/roles/admin.yaml"], "count": 2 });
    let body = refusal_of(
        ops::verdict_for_import(&state, &jana, PROJECT, &changed)
            .await
            .expect_err("a bundle that grew a file passed on the old check"),
    );
    assert_eq!(body["reason"], json!("stale"));

    // Forgetting takes this person's check and leaves the other's, and the next import is checked.
    ops::record_import_check(&state, &stefan, PROJECT, &bundle).await;
    ops::forget_import_check(&state, &jana, PROJECT).await;
    assert!(ops::verdict_for_import(&state, &jana, PROJECT, &bundle)
        .await
        .is_err());
    assert!(
        passed(ops::verdict_for_import(&state, &stefan, PROJECT, &bundle).await),
        "forgetting one person's check took another's"
    );
    // Forgetting twice, and forgetting what was never there, is not an error.
    ops::forget_import_check(&state, &jana, PROJECT).await;
    ops::forget_import_check(&state, &identity("nobody", &[]), PROJECT).await;
}

// -------------------------------------------------------------------------------------------------
// T-2127 `verdict_refusal`
// -------------------------------------------------------------------------------------------------

/// ADR-N-021, T-0947: the refusal a door carries before it offers an elicitation is the route's own,
/// word for word, and only a proposal has one. Every other operation of the registry answers `None`,
/// so no door invents a gate the route does not have.
#[tokio::test]
async fn only_a_proposal_carries_a_verdict_refusal_and_it_is_the_routes_own() {
    let state = state_with_roles(Config::for_tests());
    let draft = manifest("ovzdusie-main");
    recorded(&state, PROJECT, &draft, Verdict::green(&draft, None)).await;

    for op in ops::registry() {
        let input = json!({ "draft": { "kind": "ContextSpace", "name": "ovzdusie-main" } });
        let carried =
            ops::verdict_refusal(&state, &identity("stefan", &["devs"]), op, PROJECT, &input).await;
        if op.verb != Some(jc_core::kinds::Verb::Propose) {
            assert!(
                carried.is_none(),
                "{} carried a verdict refusal and is not a proposal",
                op.name,
            );
        } else {
            // This draft is checked, so a proposal of it carries nothing either.
            assert!(
                carried.is_none(),
                "{} refused a draft whose check is green and fresh",
                op.name,
            );
        }
    }

    // A draft nobody checked: the document is the one the route answers, and it says which check.
    let unchecked = manifest("ovzdusie-unchecked");
    let store = state.drafts.clone();
    store
        .put(
            PROJECT,
            "ContextSpace",
            "ovzdusie-unchecked",
            unchecked.clone(),
            Some(0),
            "jana",
            "person",
        )
        .await
        .expect("the draft is written");
    let propose = ops::find("jc_resource_propose").expect("a registered proposal");
    let input = json!({ "draft": { "kind": "ContextSpace", "name": "ovzdusie-unchecked" } });
    let carried = ops::verdict_refusal(
        &state,
        &identity("stefan", &["devs"]),
        propose,
        PROJECT,
        &input,
    )
    .await
    .expect("the refusal the route would answer");
    assert_eq!(carried["error"], json!("verdict_required"));
    assert_eq!(carried["check"], json!("jc_manifest_dry_run"));
    assert_eq!(carried["reason"], json!("verdict_absent"));
    let text = carried.to_string();
    assert!(
        !text.contains(SECRET_IN_THE_MANIFEST),
        "the refusal carried the draft's content: {text}",
    );

    // An input that names no draft, or a draft of another project or another person's kind, carries
    // nothing: the door asks about what the call named and never goes looking.
    for input in [
        json!({}),
        json!({ "draft": {} }),
        json!({ "draft": { "kind": "ContextSpace" } }),
        json!({ "draft": { "name": "ovzdusie-unchecked" } }),
        json!({ "draft": { "kind": "ContextSpace", "name": "nothing-like-it" } }),
        json!({ "draft": { "kind": "Role", "name": "ovzdusie-unchecked" } }),
        json!({ "draft": "ovzdusie-unchecked" }),
    ] {
        assert!(
            ops::verdict_refusal(
                &state,
                &identity("stefan", &["devs"]),
                propose,
                PROJECT,
                &input
            )
            .await
            .is_none(),
            "{input} produced a refusal about a draft it did not name",
        );
    }
    assert!(
        ops::verdict_refusal(
            &state,
            &identity("stefan", &["devs"]),
            propose,
            "doprava",
            &input
        )
        .await
        .is_none(),
        "a draft of another project answered this project's door",
    );
}

// -------------------------------------------------------------------------------------------------
// T-2119 `listing`
// -------------------------------------------------------------------------------------------------

/// AG-59, PF-50: what a caller is offered is what they may take. The listing is the registry filtered
/// by the two halves `call` uses and the cheap half of each writing route's own check (AG-63,
/// T-5608), so no door can offer an operation that would then be refused —
/// and a stranger is offered nothing at all.
#[test]
fn nobody_is_offered_an_operation_the_gate_would_refuse_them() {
    let state = state_with_roles(Config::for_tests());
    let waved_through = joinedcontext_portal::ops::resources::CHECKED_BY_THE_ROUTE;

    for (who, groups) in [
        ("nobody", &[][..]),
        ("jana", &["city-viewers"][..]),
        ("stefan", &["devs"][..]),
    ] {
        let caller = Caller::new(identity(who, groups), Via::Session);
        let offered = ops::listing(&caller, &state, PROJECT);
        let names: Vec<&str> = offered.iter().map(|op| op.name.as_str()).collect();

        for op in ops::registry() {
            let allowed = caller.may_run(op).is_ok()
                && ops::may_try(op, &caller.identity, &state, PROJECT, None).is_ok();
            assert_eq!(
                names.contains(&op.name),
                allowed,
                "{who} was {} {}",
                if allowed { "not offered" } else { "offered" },
                op.name,
            );
        }
        // A caller with no binding is offered only what the routes check for themselves, and
        // the data read, which the Endpoint's own Policy decides (ADR-N-067).
        if who == "nobody" {
            for name in &names {
                assert!(
                    waved_through.contains(name)
                        || *name == joinedcontext_portal::ops::data_query::NAME,
                    "a stranger was offered {name}, which the registry itself gates",
                );
            }
        }
        // Every row is a real operation and carries the published fields, nothing more.
        for summary in &offered {
            let op = ops::find(&summary.name).expect("a registered operation");
            assert_eq!(summary.title, op.title);
            assert_eq!(summary.description, op.description);
            assert_eq!(summary.annotations, op.annotations);
            assert_eq!(summary.lane, op.lane);
            assert!(
                summary.input_schema.is_object(),
                "{} published no input schema",
                summary.name,
            );
        }
    }
}
