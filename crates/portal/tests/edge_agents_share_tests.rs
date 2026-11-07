//! The edge cases of `agents/share.rs`: what the assistant may draft from a sentence
//! (T-2086 `render`, T-2087 `prose_of`, T-2088 `edit`).
//!
//! The contract of the file in one sentence: **nothing here writes, and nothing the model or the
//! person types reaches a manifest unchecked** — the slug is minted here, the project is the
//! caller's own, and every name, audience, representation and attribute is refused unless it is
//! one the catalogue knows (EP-72, PF-51, PF-57, AG-59).
//!
//! Red cases found while writing these and carried by their own tasks, never by `main`:
//! T-2349 (`edit` panics on a manifest whose `spec` is not an object) and T-2350 (`render`
//! duplicates Policy and Group manifests, can mint a Policy name longer than a DNS-1123 label,
//! and bounds `requestsPerMinute` differently from `edit`). Both are fixed and their cases are
//! the last two sections of this file.

use serde_json::{json, Value};

use joinedcontext_portal::agents::share::{
    self, known_representations, prose_of, render, EditEndpoint, ProposeEndpoint, RateLimits,
    MAX_REQUESTS_PER_MINUTE,
};
use joinedcontext_portal::resource::{is_dns1123, API_VERSION};

const PROJECT: &str = "helsinki";

/// An edit in these cases adds no calendar, so it reads no served type.
fn edit(endpoints: &[Value], params: &EditEndpoint) -> Result<share::Edit, String> {
    share::edit(endpoints, params, |_| Vec::new())
}

fn share() -> ProposeEndpoint {
    ProposeEndpoint {
        context_space: "bikes".to_owned(),
        name: "bike-share".to_owned(),
        audience: Some("public".to_owned()),
        ..Default::default()
    }
}

/// An `Endpoint` manifest as the mirror holds one.
fn endpoint(name: &str, spec: Value) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": { "name": name, "namespace": PROJECT },
        "spec": spec,
    })
}

fn bikes() -> Vec<Value> {
    vec![
        endpoint(
            "bike-share",
            json!({
                "contextSpaceRef": "bikes",
                "slug": "abcdefghijklmnopqrstuvwxyz",
                "audience": "project-list",
                "allowedProjects": ["transport"],
                "enabledRepresentations": ["ngsi-ld", "ogc-features"],
            }),
        ),
        endpoint(
            "air-quality",
            json!({
                "contextSpaceRef": "air",
                "slug": "zyxwvutsrqponmlkjihgfedcba",
                "audience": "public",
                "enabledRepresentations": ["ngsi-ld"],
            }),
        ),
    ]
}

fn edit_of(name: &str) -> EditEndpoint {
    EditEndpoint {
        name: name.to_owned(),
        ..Default::default()
    }
}

/// The spellings that are not a DNS-1123 label, whatever the field is. Spaces around a name are
/// not here: `render` and `edit` both trim first, which is the one forgiveness they grant.
const NOT_A_LABEL: [&str; 14] = [
    "",
    " ",
    "Bikes",
    "BIKES",
    "bike_share",
    "bike share",
    "bike-share-",
    "-bike-share",
    "bike-share/",
    "../bike-share",
    "%2e%2e",
    "bike-share\u{0000}",
    "bike-share\r\nX-Injected: 1",
    "bikes\u{0308}",
];

// ---------------------------------------------------------------------------------------
// T-2086 `render`: the draft the person is shown before anything is proposed.
// ---------------------------------------------------------------------------------------

/// EP-02: the slug is minted here and nowhere else. `ProposeEndpoint` carries no slug field, so
/// no model answer can choose one, and two renderings of the same request never share one.
#[test]
fn the_slug_is_minted_here_and_two_renderings_never_share_one() {
    let params = share();
    let first = render(PROJECT, &params, &[]).expect("a valid share");
    let second = render(PROJECT, &params, &[]).expect("a valid share");
    assert_ne!(first.slug, second.slug, "a slug was reused");
    for slug in [&first.slug, &second.slug] {
        assert_eq!(slug.len(), 26, "{slug}");
        assert!(
            slug.bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)),
            "{slug} is not base32"
        );
    }
    assert_eq!(first.endpoint["spec"]["slug"], json!(first.slug));
    assert_eq!(first.prefill["slug"], json!(first.slug));
}

/// PF-57: the endpoint name and the context space are labels, and a rendering refuses every
/// spelling that is not one — a traversal in either would name a file outside the project.
#[test]
fn a_name_or_context_space_that_is_not_a_label_is_refused() {
    for bad in NOT_A_LABEL {
        let mut params = share();
        params.name = bad.to_owned();
        let refused = render(PROJECT, &params, &[]).expect_err("name");
        assert!(refused.contains("DNS-1123"), "name {bad:?}: {refused}");

        let mut params = share();
        params.context_space = bad.to_owned();
        let refused = render(PROJECT, &params, &[]).expect_err("contextSpace");
        assert!(
            refused.contains("DNS-1123"),
            "contextSpace {bad:?}: {refused}"
        );
    }
    let sixty_four = "a".repeat(64);
    let mut params = share();
    params.name = sixty_four;
    assert!(render(PROJECT, &params, &[]).is_err(), "64 characters");

    // Spaces around the name are trimmed and the label underneath is the one that is checked:
    // a model that pads its answer renders, and the manifest carries the trimmed name.
    let mut params = share();
    params.name = "  bike-share  ".to_owned();
    params.context_space = "\tbikes\n".to_owned();
    let rendered = render(PROJECT, &params, &[]).expect("a padded but valid name");
    assert_eq!(rendered.endpoint["metadata"]["name"], json!("bike-share"));
    assert_eq!(rendered.endpoint["spec"]["contextSpaceRef"], json!("bikes"));
}

/// PF-57: the audience is one of three names, matched exactly.
#[test]
fn an_audience_that_is_not_one_of_the_three_is_refused() {
    for bad in [
        "",
        " ",
        "PUBLIC",
        "Public",
        "internal",
        "project-list/",
        "public\u{0000}",
        "\u{0440}ublic",
        "project list",
    ] {
        let mut params = share();
        params.audience = Some(bad.to_owned());
        let refused = render(PROJECT, &params, &[]).expect_err("audience");
        assert!(refused.contains("audience"), "{bad:?}: {refused}");
    }
}

/// EP-72: a share to a list of projects that names no project shares with nobody, and is refused
/// rather than rendered as an endpoint no one can reach.
#[test]
fn a_project_list_audience_without_a_project_is_refused() {
    let mut params = share();
    params.audience = Some("project-list".to_owned());
    params.allowed_projects = vec![" ".to_owned(), String::new()];
    let refused = render(PROJECT, &params, &[]).expect_err("no project");
    assert!(refused.contains("allowedProjects"), "{refused}");

    // The default audience is the same list, so an omitted audience is refused the same way.
    let mut params = share();
    params.audience = None;
    let refused = render(PROJECT, &params, &[]).expect_err("no audience, no project");
    assert!(refused.contains("allowedProjects"), "{refused}");
}

/// PF-57: a consumer project is a label too; a traversal there would name another organization's
/// group in the drafted Policy.
#[test]
fn a_consumer_project_that_is_not_a_label_is_refused() {
    for bad in NOT_A_LABEL.iter().filter(|b| !b.trim().is_empty()) {
        let mut params = share();
        params.audience = Some("project-list".to_owned());
        params.allowed_projects = vec!["transport".to_owned(), (*bad).to_owned()];
        let refused = render(PROJECT, &params, &[]).expect_err("allowedProjects");
        assert!(refused.contains("DNS-1123"), "{bad:?}: {refused}");
    }

    // A padded project is trimmed to the label it names, and a blank entry is dropped before the
    // list is counted — the audience that needs a project still needs a real one.
    let mut params = share();
    params.audience = Some("project-list".to_owned());
    params.allowed_projects = vec!["  transport  ".to_owned(), "   ".to_owned()];
    let rendered = render(PROJECT, &params, &[]).expect("a padded but valid project");
    assert_eq!(
        rendered.endpoint["spec"]["allowedProjects"],
        json!(["transport"])
    );
}

/// EP-72: an endpoint serves the representations the platform implements and no others.
#[test]
fn a_representation_the_catalogue_does_not_know_is_refused() {
    for bad in [
        "",
        " ",
        "NGSI-LD",
        "Ngsi-Ld",
        "xml",
        "ngsi-ld/",
        "../ngsi-ld",
        "ngsi-ld\u{0000}",
        "ngsi\u{2010}ld",
    ] {
        let mut params = share();
        params.representations = vec![bad.to_owned()];
        let refused = render(PROJECT, &params, &[]).expect_err("representation");
        assert!(refused.contains("representation"), "{bad:?}: {refused}");
    }
    // Every name the catalogue does know renders.
    for good in known_representations() {
        let mut params = share();
        params.representations = vec![format!("  {good}  ")];
        let rendered = render(PROJECT, &params, &[]).expect("a known representation");
        assert_eq!(
            rendered.endpoint["spec"]["enabledRepresentations"],
            json!([good]),
            "{good} was not trimmed"
        );
    }
}

/// EP-72: an empty list is the sensible default rather than an endpoint that serves nothing.
#[test]
fn an_empty_representation_list_defaults_to_ngsi_ld_and_csv() {
    let rendered = render(PROJECT, &share(), &[]).expect("a valid share");
    assert_eq!(
        rendered.endpoint["spec"]["enabledRepresentations"],
        json!(["ngsi-ld", "csv"])
    );
}

/// T-4600: CSV is the one file. A share that names a retired one is refused with the names it
/// may use, and an edit of an Endpoint that still lists one drops it.
#[test]
fn a_retired_file_is_refused_on_a_share_and_dropped_by_an_edit() {
    let mut params = share();
    params.representations = vec!["ngsi-ld".to_owned(), "xlsx".to_owned()];
    let refused = render(PROJECT, &params, &[]).expect_err("xlsx is retired");
    assert!(refused.contains("'xlsx'"), "{refused}");
    assert!(refused.contains("ngsi-ld, mcp, csv"), "{refused}");

    let mut stored = bikes();
    stored[0]["spec"]["enabledRepresentations"] = json!(["ngsi-ld", "geojson", "zip"]);
    let mut edit_params = edit_of("bike-share");
    edit_params.add_representations = vec!["csv".to_owned()];
    let edited = edit(&stored, &edit_params).expect("a valid edit");
    assert_eq!(
        edited.endpoint["spec"]["enabledRepresentations"],
        json!(["ngsi-ld", "csv"])
    );
}

/// PF-57: a hidden attribute and an entity type are identifiers. A name with a slash, a space or
/// a control character in it would reach the broker's projection as something else.
#[test]
fn an_attribute_or_type_that_is_not_an_identifier_is_refused() {
    let bad_identifiers = [
        "",
        " ",
        "a b",
        "a/b",
        "../maintenanceNotes",
        "maintenance notes",
        "maintenance\u{0000}Notes",
        "maintenance\r\nNotes",
        "maintenance\"Notes",
        "maintenanceNotes ",
        "maintenanceNötes",
    ];
    for bad in bad_identifiers {
        let mut params = share();
        params.hidden_attributes = vec![bad.to_owned()];
        let refused = render(PROJECT, &params, &[]).expect_err("hiddenAttributes");
        assert!(refused.contains("hiddenAttributes"), "{bad:?}: {refused}");

        let mut params = share();
        params.entity_types = vec![bad.to_owned()];
        let refused = render(PROJECT, &params, &[]).expect_err("entityTypes");
        assert!(refused.contains("entityTypes"), "{bad:?}: {refused}");
    }
    // 128 characters is the bound and 129 is past it.
    let mut params = share();
    params.hidden_attributes = vec!["a".repeat(128)];
    assert!(render(PROJECT, &params, &[]).is_ok(), "the bound");
    params.hidden_attributes = vec!["a".repeat(129)];
    assert!(render(PROJECT, &params, &[]).is_err(), "the bound plus one");
}

/// EP-72: a rate limit of zero would serve nobody; it is refused rather than rendered.
#[test]
fn a_rate_limit_of_zero_is_refused() {
    let mut params = share();
    params.rate_limits = Some(RateLimits {
        requests_per_minute: 0,
        burst: None,
    });
    let refused = render(PROJECT, &params, &[]).expect_err("zero");
    assert!(refused.contains("requestsPerMinute"), "{refused}");
}

/// EP-72, PF-51: the manifest is namespaced to the project of the run. `ProposeEndpoint` carries
/// no namespace, so no model answer can draft into another project.
#[test]
fn the_draft_is_namespaced_to_the_project_of_the_run() {
    let mut params = share();
    params.audience = Some("project-list".to_owned());
    params.allowed_projects = vec!["transport".to_owned()];
    let rendered = render(PROJECT, &params, &[]).expect("a valid share");
    assert_eq!(rendered.endpoint["metadata"]["namespace"], json!(PROJECT));
    for policy in &rendered.policies {
        assert_eq!(policy["metadata"]["namespace"], json!(PROJECT));
        assert_eq!(policy["kind"], json!("Policy"));
    }
    for group in &rendered.groups {
        assert_eq!(group["metadata"]["namespace"], json!(PROJECT));
        assert_eq!(group["kind"], json!("Group"));
    }
}

/// EP-72: a public or organization share lists no consumer project — neither in the manifest,
/// nor in the form, nor as a drafted group — however many the request named.
#[test]
fn a_public_or_organization_share_lists_no_consumer_project() {
    for audience in ["public", "organization"] {
        let mut params = share();
        params.audience = Some(audience.to_owned());
        params.allowed_projects = vec!["transport".to_owned(), "energy".to_owned()];
        let rendered = render(PROJECT, &params, &[]).expect("a valid share");
        assert!(
            rendered.endpoint["spec"].get("allowedProjects").is_none(),
            "{audience} kept the project list"
        );
        assert_eq!(rendered.prefill["allowedProjects"], json!([]), "{audience}");
        assert!(rendered.groups.is_empty(), "{audience} drafted a group");
        // The Organization, and the owning project's own group: neither is a consumer (T-6464).
        let grants = if audience == "public" { 1 } else { 2 };
        assert_eq!(rendered.policies.len(), grants, "{audience}");
    }
}

/// EP-72, PF-62: the draft Policy grants a read to the audience the request named and nothing
/// else — no write, no assignee the request did not ask for.
#[test]
fn a_draft_policy_grants_a_read_to_the_named_audience_only() {
    let mut params = share();
    params.audience = Some("organization".to_owned());
    let rendered = render(PROJECT, &params, &[]).expect("a valid share");
    let policy = &rendered.policies[0];
    assert_eq!(policy["spec"]["operations"], json!(["retrieveOps"]));
    assert_eq!(
        policy["spec"]["assignee"],
        json!({ "kind": "organization" })
    );
    assert_eq!(
        policy["spec"]["contextSpaceRef"],
        json!({ "kind": "ContextSpace", "name": "bikes" })
    );

    let mut params = share();
    params.audience = Some("public".to_owned());
    let rendered = render(PROJECT, &params, &[]).expect("a valid share");
    assert_eq!(
        rendered.policies[0]["spec"]["assignee"],
        json!({ "kind": "role", "id": "public" })
    );
}

/// T-1042, PF-62: a group is drafted only for a consumer project the repository does not declare
/// yet, and never for one it already has.
#[test]
fn a_group_is_drafted_only_for_a_project_that_has_none() {
    let mut params = share();
    params.audience = Some("project-list".to_owned());
    params.allowed_projects = vec!["transport".to_owned(), "energy".to_owned()];
    let rendered = render(PROJECT, &params, &["transport".to_owned()]).expect("a valid share");
    let drafted: Vec<&str> = rendered
        .groups
        .iter()
        .map(|g| g["metadata"]["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(drafted, ["energy"]);
    for group in &rendered.groups {
        assert_eq!(
            group["spec"]["members"],
            json!([]),
            "a group drafted members"
        );
    }
}

/// UI-50, T-1221: the form takes the title as one string, so an empty or blank title is not
/// written at all and a title with spaces around it is trimmed.
#[test]
fn a_blank_title_is_not_written_and_a_title_is_trimmed() {
    for blank in [Some(String::new()), Some("   ".to_owned()), None] {
        let mut params = share();
        params.title = blank.clone();
        let rendered = render(PROJECT, &params, &[]).expect("a valid share");
        assert!(
            rendered.endpoint["metadata"].get("title").is_none(),
            "{blank:?} wrote a title"
        );
        assert!(rendered.prefill.get("title").is_none(), "{blank:?}");
    }
    let mut params = share();
    params.title = Some("  Bike stations  ".to_owned());
    let rendered = render(PROJECT, &params, &[]).expect("a valid share");
    assert_eq!(rendered.prefill["title"], json!("Bike stations"));
}

/// PF-57: a title is free text, so it may carry a newline; it reaches a JSON string and never a
/// header, and it comes back escaped rather than breaking the document.
#[test]
fn a_newline_in_a_title_stays_inside_the_json_string() {
    let mut params = share();
    params.title = Some("Bikes\r\nX-Injected: 1".to_owned());
    let rendered = render(PROJECT, &params, &[]).expect("a valid share");
    let text = serde_json::to_string(&rendered.endpoint).expect("serialisable");
    assert!(text.contains("Bikes\\r\\nX-Injected: 1"), "{text}");
    assert!(
        !text.contains("Bikes\r\n"),
        "a raw CRLF reached the document"
    );
}

/// EP-72: the rendering is a draft. It answers with manifests and the form values, and the
/// caller — not this function — is what writes; the function takes no state to write to.
#[test]
fn the_rendering_is_a_draft_that_matches_the_form_it_prefills() {
    let mut params = share();
    params.audience = Some("project-list".to_owned());
    params.allowed_projects = vec!["transport".to_owned()];
    params.hidden_attributes = vec!["maintenanceNotes".to_owned()];
    params.entity_types = vec!["BikeHireDockingStation".to_owned()];
    let rendered = render(PROJECT, &params, &[]).expect("a valid share");
    assert_eq!(rendered.endpoint["kind"], json!("Endpoint"));
    assert_eq!(rendered.endpoint["apiVersion"], json!(API_VERSION));
    assert_eq!(rendered.prefill["name"], json!("bike-share"));
    assert_eq!(rendered.prefill["contextSpaceRef"], json!("bikes"));
    assert_eq!(rendered.prefill["audience"], json!("project-list"));
    assert_eq!(
        rendered.prefill["enabledRepresentations"],
        rendered.endpoint["spec"]["enabledRepresentations"]
    );
    assert_eq!(
        rendered.prefill["allowedProjects"],
        rendered.endpoint["spec"]["allowedProjects"]
    );
    assert_eq!(
        rendered.endpoint["spec"]["projection"]["hiddenAttributes"],
        json!(["maintenanceNotes"])
    );
    assert_eq!(
        rendered.policies[0]["spec"]["information"],
        json!([{ "entities": [{ "type": "BikeHireDockingStation" }] }])
    );
}

/// T-2775, EP-20: a limit is the person's choice. A share that asked for none drafts an
/// Endpoint without `rateLimits` and opens the form with the limit fields empty.
#[test]
fn a_share_that_asks_for_no_limit_drafts_none() {
    let rendered = render(PROJECT, &share(), &[]).expect("a valid share");
    assert!(
        rendered.endpoint["spec"].get("rateLimits").is_none(),
        "{}",
        rendered.endpoint
    );
    assert!(
        rendered.prefill.get("rateLimits").is_none(),
        "{}",
        rendered.prefill
    );
}

// ---------------------------------------------------------------------------------------
// T-2087 `prose_of`: what the person reads when the model answered with a tool call.
// ---------------------------------------------------------------------------------------

/// AG-56: the call itself is machinery. Whatever the model fenced, the person reads the prose
/// around it and never the JSON.
#[test]
fn the_fenced_call_never_reaches_the_person() {
    let answer = "Here is the share.\n\n```json\n{\"tool\":\"propose_endpoint\",\"name\":\"bike-share\"}\n```\n\nSubmit it from the form.";
    let prose = prose_of(answer);
    assert!(!prose.contains("propose_endpoint"), "{prose}");
    assert!(!prose.contains('{'), "{prose}");
    assert_eq!(prose, "Here is the share.\n\n\n\nSubmit it from the form.");
}

/// AG-56: every fence is stripped, not only the first — a model that repeats its call must not
/// show the second copy.
#[test]
fn every_fenced_object_is_stripped_not_only_the_first() {
    let answer = "One.\n```json\n{\"tool\":\"propose_endpoint\"}\n```\nTwo.\n```\n{\"tool\":\"edit_endpoint\"}\n```\nThree.";
    let prose = prose_of(answer);
    assert!(!prose.contains("propose_endpoint"), "{prose}");
    assert!(!prose.contains("edit_endpoint"), "{prose}");
    assert!(prose.contains("One.") && prose.contains("Two.") && prose.contains("Three."));
}

/// AG-56: a fence holding a nested object is stripped whole; the inner braces do not end it.
#[test]
fn a_nested_object_in_a_fence_is_stripped_whole() {
    let answer = "Before.\n```json\n{\"tool\":\"propose_endpoint\",\"rateLimits\":{\"requestsPerMinute\":60}}\n```\nAfter.";
    let prose = prose_of(answer);
    assert!(!prose.contains("requestsPerMinute"), "{prose}");
    assert!(!prose.contains('}'), "{prose}");
    assert!(prose.starts_with("Before.") && prose.ends_with("After."));
}

/// AG-56: an answer with no fence is the person's whole answer, unchanged but for the trim.
#[test]
fn an_answer_without_a_fence_comes_back_whole() {
    for answer in [
        "The bike stations are already shared with the transport team.",
        "A brace { on its own is prose.",
        "```\nnot json at all\n```",
        "Inline `{\"tool\":\"propose_endpoint\"}` in a single backtick stays.",
    ] {
        assert_eq!(prose_of(answer), answer.trim(), "{answer:?}");
    }
}

/// AG-56: a fence the model never closed is left in the prose rather than swallowing the rest of
/// the answer. Written down because the person then sees the raw call — ugly, and honest: the
/// alternative is an answer that ends in the middle.
#[test]
fn an_unclosed_fence_is_left_alone() {
    let answer = "Here it is.\n```json\n{\"tool\":\"propose_endpoint\"}";
    assert_eq!(prose_of(answer), answer.trim());
}

/// AG-56: an answer that is nothing but the call leaves no prose, which is the empty string the
/// callers replace with a sentence of their own (`src/agents/oneshot/tools_space.rs:61`).
#[test]
fn an_answer_that_is_only_a_call_leaves_no_prose() {
    for answer in [
        "```json\n{\"tool\":\"propose_endpoint\"}\n```",
        "   ```json\n{\"tool\":\"propose_endpoint\"}\n```   ",
        "",
        "   \n\t  ",
    ] {
        assert!(prose_of(answer).is_empty(), "{answer:?} left prose");
    }
}

/// PF-57: the answer is the model's, so it may hold anything. Unicode, a null byte and a CRLF in
/// the prose survive as text, and nothing panics on them.
#[test]
fn unicode_control_characters_and_a_very_long_answer_survive_as_text() {
    let odd = "Hotovo — hľadá sa \u{0000} a \r\n koniec.";
    assert_eq!(prose_of(odd), odd);

    let long = format!(
        "{}\n```json\n{{\"tool\":\"propose_endpoint\"}}\n```\n{}",
        "a".repeat(200_000),
        "b".repeat(200_000)
    );
    let prose = prose_of(&long);
    assert!(!prose.contains("propose_endpoint"));
    assert_eq!(prose.len(), 400_002, "the prose lost text");
}

/// AG-56: a fenced block that is not a tool call is stripped too, because the caller only asks
/// for the prose once a call was found; documented so a future caller that runs `prose_of` over
/// an ordinary answer knows it would lose the model's JSON example.
#[test]
fn a_fenced_object_that_is_not_a_call_is_stripped_as_well() {
    let answer = "An example:\n```json\n{\"type\":\"Feature\"}\n```\nThat is the shape.";
    let prose = prose_of(answer);
    assert!(!prose.contains("Feature"), "{prose}");
}

// ---------------------------------------------------------------------------------------
// T-2088 `edit`: changing an endpoint that exists, and only the fields the call names.
// ---------------------------------------------------------------------------------------

/// PF-59, PF-57: an endpoint the project does not have is refused, and the sentence lists the
/// project's own endpoints and nothing from anywhere else.
#[test]
fn an_endpoint_that_is_not_in_the_project_is_refused_and_names_only_this_projects_endpoints() {
    let refused = edit(&bikes(), &edit_of("water-quality")).expect_err("not here");
    assert!(
        refused.contains("there is no endpoint 'water-quality'"),
        "{refused}"
    );
    assert!(
        refused.contains("air-quality") && refused.contains("bike-share"),
        "{refused}"
    );

    let empty = edit(&[], &edit_of("water-quality")).expect_err("no endpoints");
    assert!(empty.contains("this project has no endpoints"), "{empty}");
    assert!(
        !empty.contains(','),
        "an empty project listed something: {empty}"
    );
}

/// PF-57: the endpoint name is matched exactly; a near miss finds nothing rather than the
/// endpoint next to it.
#[test]
fn an_endpoint_name_is_matched_exactly() {
    for near in [
        "Bike-Share",
        "BIKE-SHARE",
        "bike-share2",
        "bike-shar",
        "../bike-share",
        "bike-share/",
        "bike-share\u{0000}",
        "bike\u{2010}share",
        "",
    ] {
        let refused = edit(&bikes(), &edit_of(near)).expect_err("near miss");
        assert!(
            refused.starts_with("there is no endpoint"),
            "{near:?}: {refused}"
        );
    }
    // Spaces around the name are trimmed, which is the one forgiveness the function grants.
    let mut params = edit_of("  bike-share  ");
    params.requests_per_minute = Some(60);
    assert!(
        edit(&bikes(), &params).is_ok(),
        "a trimmed name is the same name"
    );
}

/// EP-72: the audience is one of three names here as well.
#[test]
fn an_edit_to_an_audience_that_is_not_one_of_the_three_is_refused() {
    for bad in [
        "",
        " ",
        "PUBLIC",
        "internal",
        "project list",
        "public\u{0000}",
    ] {
        let mut params = edit_of("bike-share");
        params.audience = Some(bad.to_owned());
        let refused = edit(&bikes(), &params).expect_err("audience");
        assert!(refused.contains("audience"), "{bad:?}: {refused}");
    }
}

/// EP-72: narrowing a public endpoint to a list of projects is refused with what to do instead,
/// rather than silently leaving it public.
#[test]
fn allowed_projects_on_a_public_endpoint_is_refused_with_what_to_do_instead() {
    let mut params = edit_of("air-quality");
    params.allowed_projects = Some(vec!["transport".to_owned()]);
    let refused = edit(&bikes(), &params).expect_err("public");
    assert!(
        refused.contains("set audience to project-list"),
        "{refused}"
    );

    // Naming the audience with the projects is the path the sentence points at.
    let mut params = edit_of("air-quality");
    params.audience = Some("project-list".to_owned());
    params.allowed_projects = Some(vec!["transport".to_owned()]);
    let edited = edit(&bikes(), &params).expect("audience and projects together");
    assert_eq!(edited.endpoint["spec"]["audience"], json!("project-list"));
    assert_eq!(
        edited.endpoint["spec"]["allowedProjects"],
        json!(["transport"])
    );
}

/// PF-57: a consumer project is a label, and an edit that names none leaves the endpoint serving
/// nobody, so both are refused.
#[test]
fn a_consumer_project_that_is_not_a_label_or_an_empty_list_is_refused() {
    for bad in [
        "",
        " ",
        "Transport",
        "../transport",
        "transport/",
        "transport\u{0000}",
    ] {
        let mut params = edit_of("bike-share");
        params.allowed_projects = Some(vec![bad.to_owned()]);
        let refused = edit(&bikes(), &params).expect_err("allowedProjects");
        assert!(refused.contains("DNS-1123"), "{bad:?}: {refused}");
    }
    let mut params = edit_of("bike-share");
    params.allowed_projects = Some(Vec::new());
    let refused = edit(&bikes(), &params).expect_err("empty list");
    assert!(refused.contains("at least one project"), "{refused}");
}

/// EP-72: a representation must be one the platform implements, whether it is being added or
/// removed, and an endpoint must keep at least one.
#[test]
fn a_representation_the_catalogue_does_not_know_and_an_endpoint_that_serves_none_are_refused() {
    for bad in [
        "",
        " ",
        "XML",
        "ngsi-ld/",
        "../geojson",
        "geojson\u{0000}",
        "geojson",
        "xlsx",
        "zip",
    ] {
        let mut params = edit_of("bike-share");
        params.add_representations = vec![bad.to_owned()];
        let refused = edit(&bikes(), &params).expect_err("add");
        assert!(refused.contains("representation"), "add {bad:?}: {refused}");

        let mut params = edit_of("bike-share");
        params.remove_representations = vec![bad.to_owned()];
        let refused = edit(&bikes(), &params).expect_err("remove");
        assert!(
            refused.contains("representation"),
            "remove {bad:?}: {refused}"
        );
    }

    let mut params = edit_of("bike-share");
    params.remove_representations = vec!["ngsi-ld".to_owned(), "ogc-features".to_owned()];
    let refused = edit(&bikes(), &params).expect_err("all of them");
    assert!(
        refused.contains("would serve no representation"),
        "{refused}"
    );
}

/// EP-72: adding what is already there and removing what is not there change nothing, and a
/// request that changes nothing is refused rather than opening an empty Change.
#[test]
fn a_request_that_changes_nothing_is_refused() {
    for params in [
        edit_of("bike-share"),
        EditEndpoint {
            name: "bike-share".to_owned(),
            add_representations: vec!["ngsi-ld".to_owned(), "ogc-features".to_owned()],
            remove_representations: vec!["csv".to_owned()],
            ..Default::default()
        },
        EditEndpoint {
            name: "bike-share".to_owned(),
            audience: Some("project-list".to_owned()),
            allowed_projects: Some(vec!["transport".to_owned()]),
            ..Default::default()
        },
    ] {
        let refused = edit(&bikes(), &params).expect_err("nothing changes");
        assert!(refused.contains("changes nothing"), "{refused}");
    }
}

/// PF-57: the rate limit is bounded at both ends, and the bound plus one is refused.
#[test]
fn a_rate_limit_outside_the_bound_is_refused_and_the_bound_itself_is_taken() {
    for bad in [0, 100_001, u32::MAX] {
        let mut params = edit_of("bike-share");
        params.requests_per_minute = Some(bad);
        let refused = edit(&bikes(), &params).expect_err("out of bounds");
        assert!(refused.contains("requestsPerMinute"), "{bad}: {refused}");
    }
    for good in [1, 100_000] {
        let mut params = edit_of("bike-share");
        params.requests_per_minute = Some(good);
        let edited = edit(&bikes(), &params).expect("inside the bound");
        assert_eq!(
            edited.endpoint["spec"]["rateLimits"]["requestsPerMinute"],
            json!(good)
        );
    }
}

/// PF-57: a hidden attribute is an identifier here as well, and an empty list is the way to show
/// every attribute again — it removes the projection rather than writing an empty one.
#[test]
fn a_hidden_attribute_is_an_identifier_and_an_empty_list_removes_the_projection() {
    for bad in [
        "",
        " ",
        "a b",
        "../secret",
        "secret\u{0000}",
        "secret\r\nX: 1",
    ] {
        let mut params = edit_of("bike-share");
        params.hidden_attributes = Some(vec![bad.to_owned()]);
        let refused = edit(&bikes(), &params).expect_err("hiddenAttributes");
        assert!(refused.contains("hiddenAttributes"), "{bad:?}: {refused}");
    }

    let hidden = vec![endpoint(
        "bike-share",
        json!({
            "contextSpaceRef": "bikes", "slug": "s", "audience": "public",
            "enabledRepresentations": ["ngsi-ld"],
            "projection": { "hiddenAttributes": ["maintenanceNotes"] },
        }),
    )];
    let mut params = edit_of("bike-share");
    params.hidden_attributes = Some(Vec::new());
    let edited = edit(&hidden, &params).expect("an empty list");
    assert!(
        edited.endpoint["spec"].get("projection").is_none(),
        "an empty projection was left behind: {}",
        edited.endpoint["spec"]
    );
    assert_eq!(
        edited.changes,
        vec![joinedcontext_portal::agents::share::FieldChange {
            field: "hiddenAttributes".to_owned(),
            before: json!(["maintenanceNotes"]),
            after: json!([]),
        }]
    );
}

/// EP-02, EP-72: an edit changes what the call names and nothing else. The slug, the space, the
/// name and the namespace are the endpoint's identity and an edit never moves them.
#[test]
fn an_edit_never_moves_the_slug_the_space_the_name_or_the_project() {
    let before = bikes();
    let mut params = edit_of("bike-share");
    params.add_representations = vec!["csv".to_owned()];
    params.title = Some("Bike stations".to_owned());
    let edited = edit(&before, &params).expect("a valid edit");
    for field in ["slug", "contextSpaceRef"] {
        assert_eq!(
            edited.endpoint["spec"][field], before[0]["spec"][field],
            "{field} moved"
        );
    }
    assert_eq!(edited.endpoint["metadata"]["name"], json!("bike-share"));
    assert_eq!(edited.endpoint["metadata"]["namespace"], json!(PROJECT));
    assert_eq!(edited.endpoint["kind"], json!("Endpoint"));
    // The endpoint next to it is untouched, and the input is not mutated.
    assert_eq!(before, bikes(), "the manifests handed in were changed");
}

/// PF-51, AP-73: `status` is the platform's own computation. An edit proposes a spec, so the
/// status of the manifest it started from is dropped rather than proposed back.
#[test]
fn the_status_of_the_manifest_is_dropped_and_never_proposed() {
    let mut with_status = endpoint(
        "bike-share",
        json!({ "contextSpaceRef": "bikes", "slug": "s", "audience": "public", "enabledRepresentations": ["ngsi-ld"] }),
    );
    with_status["status"] = json!({ "phase": "Ready", "url": "https://internal.example.test/x" });
    let mut params = edit_of("bike-share");
    params.add_representations = vec!["csv".to_owned()];
    let edited = edit(&[with_status], &params).expect("a valid edit");
    assert!(
        edited.endpoint.get("status").is_none(),
        "the status came back: {}",
        edited.endpoint
    );
    let text = serde_json::to_string(&edited.endpoint).expect("serialisable");
    assert!(!text.contains("internal.example.test"), "{text}");
}

/// EP-72: the same edit twice gives the same manifest and the same list of changes — an edit is
/// a function of the manifest and the call, and a double submit cannot compound.
#[test]
fn the_same_edit_twice_gives_the_same_manifest() {
    let mut params = edit_of("bike-share");
    params.add_representations = vec!["csv".to_owned(), "csv".to_owned()];
    params.requests_per_minute = Some(120);
    let first = edit(&bikes(), &params).expect("a valid edit");
    let second = edit(&bikes(), &params).expect("a valid edit");
    assert_eq!(first.endpoint, second.endpoint);
    assert_eq!(first.changes, second.changes);
    assert_eq!(
        first.endpoint["spec"]["enabledRepresentations"],
        json!(["ngsi-ld", "ogc-features", "csv"]),
        "a repeated addition was added twice"
    );

    // Applying the result again changes nothing, which is what makes the edit idempotent.
    let again =
        edit(std::slice::from_ref(&first.endpoint), &params).expect_err("nothing left to change");
    assert!(again.contains("changes nothing"), "{again}");
}

/// EP-72: every field the call left out keeps what the manifest said, and the list of changes
/// names only what actually moved.
#[test]
fn only_the_fields_the_call_names_are_reported_as_changed() {
    let mut params = edit_of("bike-share");
    params.add_representations = vec!["csv".to_owned()];
    let edited = edit(&bikes(), &params).expect("a valid edit");
    let fields: Vec<&str> = edited.changes.iter().map(|c| c.field.as_str()).collect();
    assert_eq!(fields, ["enabledRepresentations"]);
    assert_eq!(edited.endpoint["spec"]["audience"], json!("project-list"));
    assert_eq!(
        edited.endpoint["spec"]["allowedProjects"],
        json!(["transport"])
    );
}

/// PF-57: the names the refusal prints come from the manifests handed in, so they are the
/// project's own and a label — nothing from another project can be echoed through this sentence.
#[test]
fn the_names_in_a_refusal_are_the_projects_own_labels() {
    let refused = edit(&bikes(), &edit_of("water-quality")).expect_err("not here");
    for name in refused
        .rsplit("its endpoints are ")
        .next()
        .expect("the list")
        .split(", ")
    {
        assert!(is_dns1123(name), "{name:?} is not a label: {refused}");
    }
}

// ---------------------------------------------------------------------------------------
// T-2349 `edit`: the mirror holds whatever the project's repository committed.
//
// `ResourceEnvelope::spec` is a bare `Value` and `serde_json` panics when a write indexes a
// value that is neither an object nor absent, so `spec: oops` in a repository anyone on the
// project may commit to crashed the assistant's request. The shapes `edit` writes into are
// checked before the first write and named in the refusal.
// ---------------------------------------------------------------------------------------

/// The `allowedProjects` edit: the shortest call that reaches a write into the spec.
fn widen(name: &str) -> EditEndpoint {
    EditEndpoint {
        allowed_projects: Some(vec!["transport".to_owned()]),
        ..edit_of(name)
    }
}

/// PF-57: a manifest whose spec is not an object is bad data, not a crash.
#[test]
fn an_endpoint_whose_spec_is_not_an_object_is_refused_and_does_not_panic() {
    for bad in [
        json!("not an object"),
        json!([1, 2, 3]),
        json!(7),
        json!(null),
    ] {
        let endpoints = vec![endpoint("bike-share", bad.clone())];
        let refused =
            edit(&endpoints, &widen("bike-share")).expect_err("bad data is an error, not a panic");
        assert!(refused.contains("bike-share"), "{bad}: {refused}");
        assert!(refused.contains("spec"), "{bad}: {refused}");
        assert!(refused.contains("repository"), "{bad}: {refused}");
    }
}

/// PF-57: a manifest with no spec at all is refused rather than given one. `IndexMut` on an
/// absent key *creates* an object, so this shape used not to panic; it invented a spec instead.
#[test]
fn an_endpoint_with_no_spec_is_refused_rather_than_given_one() {
    let endpoints = vec![json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": { "name": "bike-share", "namespace": PROJECT },
    })];
    let refused =
        edit(&endpoints, &widen("bike-share")).expect_err("a manifest with no spec is not edited");
    assert!(refused.contains("no spec"), "{refused}");
}

/// PF-57: `spec.projection` is written into by an edit that hides an attribute.
#[test]
fn an_endpoint_whose_projection_is_not_an_object_is_refused_and_does_not_panic() {
    let endpoints = vec![endpoint(
        "bike-share",
        json!({
            "audience": "project-list",
            "allowedProjects": ["transport"],
            "enabledRepresentations": ["ngsi-ld"],
            "projection": "hide everything",
        }),
    )];
    let mut params = edit_of("bike-share");
    params.hidden_attributes = Some(vec!["maintenanceNotes".to_owned()]);
    let refused = edit(&endpoints, &params).expect_err("bad data is an error, not a panic");
    assert!(refused.contains("spec.projection"), "{refused}");
}

/// PF-57: and `spec.rateLimits` by an edit that sets a rate.
#[test]
fn an_endpoint_whose_rate_limits_are_not_an_object_is_refused_and_does_not_panic() {
    let endpoints = vec![endpoint(
        "bike-share",
        json!({
            "audience": "project-list",
            "allowedProjects": ["transport"],
            "enabledRepresentations": ["ngsi-ld"],
            "rateLimits": 60,
        }),
    )];
    let mut params = edit_of("bike-share");
    params.requests_per_minute = Some(120);
    let refused = edit(&endpoints, &params).expect_err("bad data is an error, not a panic");
    assert!(refused.contains("spec.rateLimits"), "{refused}");
}

/// PF-57: `metadata` needs no guard and has none. `edit` finds its endpoint by
/// `endpoint["metadata"]["name"]`, and the immutable `Index` answers `Null` rather than
/// panicking, so a manifest whose metadata is not an object is never the one that is found.
#[test]
fn an_endpoint_whose_metadata_is_not_an_object_is_never_found_and_does_not_panic() {
    let endpoints = vec![json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": "helsinki/bike-share",
        "spec": { "audience": "project-list", "allowedProjects": ["water"] },
    })];
    let refused = edit(&endpoints, &widen("bike-share")).expect_err("no endpoint of that name");
    assert!(refused.contains("bike-share"), "{refused}");
}

// ---------------------------------------------------------------------------------------
// T-2350 `render`: what the person dictates twice is one thing, and a name they cannot fix
// from the chat is refused before it is drafted.
// ---------------------------------------------------------------------------------------

/// EP-72: one assignee, one Policy. A project named twice is one project, and a Change carrying
/// two files of one name is a conflict the person did not ask for.
#[test]
fn a_repeated_project_renders_one_policy_and_one_group() {
    let mut params = share();
    params.audience = Some("project-list".to_owned());
    params.allowed_projects = vec!["transport".to_owned(), "transport".to_owned()];
    let rendered = render(PROJECT, &params, &[]).expect("a valid share");
    assert_eq!(rendered.policies.len(), 1, "{:?}", rendered.policies);
    assert_eq!(rendered.groups.len(), 1, "{:?}", rendered.groups);
    assert_eq!(
        rendered.endpoint["spec"]["allowedProjects"],
        json!(["transport"])
    );
    assert_eq!(rendered.prefill["allowedProjects"], json!(["transport"]));
}

/// EP-72: the order the person wrote survives the dedup, so the draft reads back as dictated.
#[test]
fn deduplicating_projects_keeps_the_order_the_person_wrote() {
    let mut params = share();
    params.audience = Some("project-list".to_owned());
    params.allowed_projects = vec![
        "water".to_owned(),
        "transport".to_owned(),
        "water".to_owned(),
    ];
    let rendered = render(PROJECT, &params, &[]).expect("a valid share");
    assert_eq!(
        rendered.endpoint["spec"]["allowedProjects"],
        json!(["water", "transport"])
    );
    let names: Vec<&str> = rendered
        .policies
        .iter()
        .filter_map(|p| p["metadata"]["name"].as_str())
        .collect();
    assert_eq!(names, ["bike-share-water", "bike-share-transport"]);
}

/// EP-72: a representation named twice is served once.
#[test]
fn a_repeated_representation_is_rendered_once() {
    let mut params = share();
    params.representations = vec![
        "ngsi-ld".to_owned(),
        "ogc-features".to_owned(),
        "ngsi-ld".to_owned(),
    ];
    let rendered = render(PROJECT, &params, &[]).expect("a valid share");
    assert_eq!(
        rendered.endpoint["spec"]["enabledRepresentations"],
        json!(["ngsi-ld", "ogc-features"])
    );
}

/// EP-72: the same normalisation in `edit`, which writes the list it is handed.
#[test]
fn an_edit_that_repeats_a_project_widens_to_it_once() {
    let mut params = edit_of("bike-share");
    params.allowed_projects = Some(vec![
        "transport".to_owned(),
        "water".to_owned(),
        "transport".to_owned(),
    ]);
    let edited = edit(&bikes(), &params).expect("a valid edit");
    assert_eq!(
        edited.endpoint["spec"]["allowedProjects"],
        json!(["transport", "water"])
    );
}

/// EP-72: two labels do not make a third. The endpoint name and the project may each be 63
/// characters and the Policy is named after both, so the refusal names both halves and their
/// lengths — the person can act on it from the chat, which a validation failure on a name they
/// never typed is not (T-1042).
#[test]
fn a_policy_name_longer_than_a_label_is_refused_with_both_halves() {
    let name = "b".repeat(63);
    let project = "t".repeat(63);
    let mut params = share();
    params.name = name.clone();
    params.audience = Some("project-list".to_owned());
    params.allowed_projects = vec![project.clone()];
    let refused = render(PROJECT, &params, &[]).expect_err("127 characters is not a label");
    assert!(refused.contains(&name), "{refused}");
    assert!(refused.contains(&project), "{refused}");
    assert!(refused.contains("63"), "{refused}");
    assert!(!is_dns1123(&format!("{name}-{project}")));
}

/// EP-72: the audiences whose Policy is named after a word rather than a project overflow the
/// same way, and are refused the same way.
#[test]
fn a_public_or_organization_policy_name_is_bounded_too() {
    for audience in ["public", "organization"] {
        let mut params = share();
        params.name = "b".repeat(63);
        params.audience = Some(audience.to_owned());
        let refused = render(PROJECT, &params, &[]).expect_err("63 + the suffix is not a label");
        assert!(refused.contains(audience), "{audience}: {refused}");
    }
}

/// EP-72: one field, one bound. `render` used to take `u32::MAX` where `edit` refuses anything
/// above `MAX_REQUESTS_PER_MINUTE`, and the share is the surface a person reaches first.
#[test]
fn a_share_refuses_the_rate_edit_refuses_and_takes_the_bound_itself() {
    for bad in [0, MAX_REQUESTS_PER_MINUTE + 1, u32::MAX] {
        let mut params = share();
        params.rate_limits = Some(RateLimits {
            requests_per_minute: bad,
            burst: None,
        });
        let refused = render(PROJECT, &params, &[]).expect_err("outside the bound");
        assert!(refused.contains("requestsPerMinute"), "{bad}: {refused}");
    }
    for good in [1, MAX_REQUESTS_PER_MINUTE] {
        let mut params = share();
        params.rate_limits = Some(RateLimits {
            requests_per_minute: good,
            burst: None,
        });
        let rendered = render(PROJECT, &params, &[]).expect("inside the bound");
        assert_eq!(
            rendered.endpoint["spec"]["rateLimits"]["requestsPerMinute"],
            json!(good)
        );
    }
}
