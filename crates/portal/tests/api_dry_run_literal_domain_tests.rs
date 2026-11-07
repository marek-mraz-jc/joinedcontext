//! `literal_domain_findings` (T-2518; CC-74, CC-83, T-1484): every string of a manifest's spec
//! that writes the organization's own domain out instead of `{orgDomain}`, named by its JSON path.
//!
//! The finding is a warning of the dry run, so a copy of the manifest renders the organization it
//! lands in. A match is a whole host or a subdomain's end: the characters around it are not a DNS
//! label's (`[A-Za-z0-9-]`), compared byte for byte.

use joinedcontext_portal::api::dry_run::literal_domain_findings;
use serde_json::{json, Value};

const DOMAIN: &str = "banskabystrica.sk";

/// The paths the findings name, in the order they come.
fn paths(spec: &Value, domain: &str) -> Vec<String> {
    literal_domain_findings(spec, domain)
        .iter()
        .filter_map(|finding| finding.split(' ').next().map(str::to_owned))
        .collect()
}

fn found(text: &str) -> bool {
    !literal_domain_findings(&json!({ "value": text }), DOMAIN).is_empty()
}

/// T-2518, T-1484: a host that only ends like the domain is another organization's; one that
/// continues with a dot, a port, a path or a query still writes it out.
#[test]
fn a_domain_that_is_a_substring_of_a_longer_hostname_is_reported_the_way_the_code_decides() {
    for (text, reported) in [
        ("notbanskabystrica.sk", false),
        ("my-banskabystrica.sk", false),
        ("banskabystrica.sky", false),
        ("banskabystrica.sk-mirror.example", false),
        ("banskabystrica.sk", true),
        ("mesto.banskabystrica.sk", true),
        ("https://banskabystrica.sk:8443/feed", true),
        ("https://banskabystrica.sk/feed?x=1", true),
        ("banskabystrica.sk.evil.example", true),
        ("jana@banskabystrica.sk", true),
        // A longer host first and the domain itself later in one string: the second counts.
        ("notbanskabystrica.sk and banskabystrica.sk", true),
    ] {
        assert_eq!(found(text), reported, "{text}");
    }
}

/// T-2518, CC-83: an entity id carries the domain between colons, and the finding names the path
/// and the id with `{orgDomain}` in its place.
#[test]
fn a_domain_embedded_inside_a_urn_is_found_with_its_json_path() {
    let spec = json!({
        "targetEntity": { "id": "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:s1" },
    });
    let found = literal_domain_findings(&spec, DOMAIN);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].starts_with("spec.targetEntity.id writes the organization's domain out"));
    assert!(
        found[0].contains("urn:ngsi-ld:AirQualityObserved:{orgDomain}:ovzdusie:s1"),
        "{}",
        found[0]
    );
}

/// T-2518: a domain without a dot (a test realm, `localhost`) is never matched, whatever the spec
/// holds, and neither is an empty one.
#[test]
fn a_domain_without_a_dot_never_matches() {
    let spec =
        json!({ "a": "localhost", "b": ["http://localhost/x", "localhost.localdomain"], "c": "" });
    for domain in ["localhost", "", "sk"] {
        assert!(
            literal_domain_findings(&spec, domain).is_empty(),
            "{domain:?}"
        );
    }
}

/// T-2518: arrays add `[i]` and objects `.key`, at every depth and in any mix.
#[test]
fn nested_arrays_and_objects_both_get_indexed_paths() {
    let spec = json!({
        "pages": [
            { "layers": [["x", "banskabystrica.sk"]] },
            { "title": "no domain" },
            { "links": { "home": "https://banskabystrica.sk" } },
        ],
    });
    assert_eq!(
        paths(&spec, DOMAIN),
        ["spec.pages[0].layers[0][1]", "spec.pages[2].links.home"]
    );
}

/// T-2518: one string is one finding, however often it writes the domain, and the suggested
/// text replaces every occurrence.
#[test]
fn a_domain_appearing_twice_in_one_string_is_reported_once() {
    let spec = json!({ "note": "banskabystrica.sk mirrors mesto.banskabystrica.sk" });
    let found = literal_domain_findings(&spec, DOMAIN);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].contains("write `{orgDomain} mirrors mesto.{orgDomain}`"),
        "{}",
        found[0]
    );
}

/// T-2518: an object key is a name the schema gives, not a value a copy renders: not reported.
#[test]
fn the_domain_appearing_only_as_a_key_is_not_reported() {
    let spec = json!({ "banskabystrica.sk": "value", "hosts": { "mesto.banskabystrica.sk": 1 } });
    assert!(literal_domain_findings(&spec, DOMAIN).is_empty());
}

/// T-2518: the comparison is byte for byte, so a domain written in another case is not found.
/// Hosts are case-insensitive, so such a string still hard-codes the organization; noted in
/// chyby.md, and pinned here as the code decides today.
#[test]
fn a_domain_in_different_case_is_handled_the_way_the_code_decides() {
    for text in [
        "BanskaBystrica.sk",
        "BANSKABYSTRICA.SK",
        "https://Mesto.BanskaBystrica.SK/feed",
    ] {
        assert!(!found(text), "{text}");
    }
    // The configured domain in upper case finds only the upper-case spelling.
    let spec = json!({ "a": "BANSKABYSTRICA.SK", "b": "banskabystrica.sk" });
    assert_eq!(paths(&spec, "BANSKABYSTRICA.SK"), ["spec.a"]);
}

/// T-2518: nothing written out, nothing found: numbers, booleans, null and `{orgDomain}`
/// itself included.
#[test]
fn no_domain_anywhere_returns_an_empty_list() {
    let spec = json!({
        "n": 3, "b": true, "z": null,
        "id": "urn:ngsi-ld:Station:{orgDomain}:bikes:1",
        "list": ["a", { "b": "c" }],
    });
    assert!(literal_domain_findings(&spec, DOMAIN).is_empty());
}

#[test]
fn an_empty_spec_object_returns_an_empty_list() {
    for spec in [
        json!({}),
        json!([]),
        json!(null),
        json!(""),
        json!(DOMAIN.len()),
    ] {
        assert!(literal_domain_findings(&spec, DOMAIN).is_empty(), "{spec}");
    }
    // A spec that is itself the string is its own path.
    assert_eq!(paths(&json!(DOMAIN), DOMAIN), ["spec"]);
}

/// T-2518: depth costs one stack frame per level. A manifest arrives parsed from YAML or JSON,
/// whose parsers stop at 128 levels, so a depth of a hundred is past anything a manifest holds
/// and still ends with the one finding at the bottom.
#[test]
fn a_structure_nested_deeper_than_ten_levels_still_terminates() {
    let mut spec = json!({ "leaf": "https://banskabystrica.sk" });
    for level in 0..100 {
        spec = if level % 2 == 0 {
            json!([spec])
        } else {
            json!({ "n": spec })
        };
    }
    let found = paths(&spec, DOMAIN);
    assert_eq!(found.len(), 1);
    assert!(found[0].starts_with("spec.n[0].n[0].n[0]"), "{}", found[0]);
    assert!(found[0].ends_with(".leaf"), "{}", found[0]);
}
