//! jc-core's instants and dates read and write as the chrono build did (T-7252, T-7233 plan).
//! Every expected string was pasted from a run of the chrono build over the same inputs.

use crate::envelope::ResourceEnvelope;
use crate::kinds::catalog::Temporal;
use crate::kinds::pipeline_lifetime::{End, LifetimeMode};
use crate::kinds::service_account::BindingValidity;
use crate::kinds::{QualityRule, SubscriptionSpec};
use jiff::Timestamp;
use serde_json::{json, Map, Value};

fn now() -> Timestamp {
    "2026-10-25T01:30:00.987654321Z"
        .parse()
        .expect("an instant")
}

/// A validity bound: offsets folded to `Z`, the fraction in 0, 3, 6 or 9 digits, chrono's
/// `FromStr` leniency (space or `t` separator, `+0100`, outer spaces), the DST nights of
/// Bratislava (`+01:00`/`+02:00`) and Helsinki (`+02:00`/`+03:00`).
#[test]
fn a_validity_bound_round_trips_as_chrono_wrote_it() {
    let golden = [
        ("2026-03-29T01:30:00Z", Some("2026-03-29T01:30:00Z")),
        (
            "2026-03-29T03:30:00.5+02:00",
            Some("2026-03-29T01:30:00.500Z"),
        ),
        (
            "2026-10-25T02:30:00.123456+01:00",
            Some("2026-10-25T01:30:00.123456Z"),
        ),
        (
            "2026-10-25T02:30:00.123456+03:00",
            Some("2026-10-24T23:30:00.123456Z"),
        ),
        (
            "2026-01-01T00:00:00.123456789Z",
            Some("2026-01-01T00:00:00.123456789Z"),
        ),
        (
            "2026-01-01T00:00:00.1234567891Z",
            Some("2026-01-01T00:00:00.123456789Z"),
        ),
        ("2026-01-01 00:00:00Z", Some("2026-01-01T00:00:00Z")),
        ("2026-01-01t00:00:00z", Some("2026-01-01T00:00:00Z")),
        // chrono kept a leap second as `23:59:60Z`; jiff has none and reads it as the next
        // second, the one instant later. No writer of a validity sends one.
        ("2026-06-30T23:59:60Z", Some("2026-07-01T00:00:00Z")),
        ("2026-02-30T00:00:00Z", None),
        ("2026-01-01T00:00:00", None),
        ("2026-01-01T00:00:00+0100", Some("2025-12-31T23:00:00Z")),
        (" 2026-01-01T00:00:00Z", Some("2026-01-01T00:00:00Z")),
        ("2026-01-01T00:00:00Z ", Some("2026-01-01T00:00:00Z")),
        ("0999-12-31T23:59:59.010Z", Some("0999-12-31T23:59:59.010Z")),
        ("2026-01-01T00:00:00.000Z", Some("2026-01-01T00:00:00Z")),
    ];
    for (text, want) in golden {
        let got = serde_json::from_value::<BindingValidity>(json!({ "notBefore": text }))
            .ok()
            .map(|v| serde_json::to_value(v).expect("serializes")["notBefore"].clone());
        assert_eq!(got, want.map(|w| json!(w)), "{text}");
    }
}

#[test]
fn a_catalogue_day_round_trips_as_chrono_wrote_it() {
    let golden = [
        ("2026-03-29", Some("2026-03-29")),
        ("0999-01-02", Some("0999-01-02")),
        (" 2026-1-2", Some("2026-01-02")),
        ("2026-02-30", None),
        ("2026-03-29 ", Some("2026-03-29")),
        ("2026-03-29T00:00:00", None),
        ("+2026-03-29", Some("2026-03-29")),
        ("-0001-01-01", Some("-0001-01-01")),
        ("20260329", None),
    ];
    for (text, want) in golden {
        let got = serde_json::from_value::<Temporal>(json!({ "start": text }))
            .ok()
            .map(|t| serde_json::to_value(t).expect("serializes")["start"].clone());
        assert_eq!(got, want.map(|w| json!(w)), "{text}");
    }
}

/// `quarantinedAt` is written to the second with `Z`.
#[test]
fn quarantined_at_keeps_the_chrono_wire_format() {
    let mut entity = Map::new();
    crate::quality::quarantine(&mut entity, &["r"], now());
    assert_eq!(
        Value::Object(entity).to_string(),
        r#"{"quarantineRules":{"type":"Property","value":["r"]},"quarantinedAt":{"type":"Property","value":{"@type":"DateTime","@value":"2026-10-25T01:30:00Z"}}}"#
    );
}

/// `observedAt` read by chrono's RFC 3339 grammar (no space-free offsets, no outer spaces),
/// judged to the nanosecond against `maxAhead` PT5M and `maxAge` PT1H.
#[test]
fn freshness_reads_observed_at_as_the_chrono_build_did() {
    let rule: QualityRule = serde_json::from_value(json!({
        "name": "f", "entityType": "T", "kind": "freshness", "attribute": "a",
        "maxAhead": "PT5M", "maxAge": "PT1H"
    }))
    .expect("a rule");
    use crate::kinds::QualityOutcome::{Quarantine as Q, Reject as R};
    let golden = [
        ("2026-03-29T01:30:00Z", None, Some(Q)),
        ("2026-03-29T03:30:00.5+02:00", None, Some(Q)),
        ("2026-10-25T02:30:00.123456+01:00", None, None),
        ("2026-10-25T02:30:00.123456+03:00", None, Some(Q)),
        ("2026-01-01 00:00:00Z", None, Some(Q)),
        ("2026-01-01t00:00:00z", None, Some(Q)),
        ("2026-06-30T23:59:60Z", None, Some(Q)),
        ("2026-02-30T00:00:00Z", None, None),
        ("2026-01-01T00:00:00", None, None),
        ("2026-01-01T00:00:00+0100", None, None),
        (" 2026-01-01T00:00:00Z", None, None),
        ("2026-01-01T00:00:00Z ", None, None),
        ("0999-12-31T23:59:59.010Z", None, Some(Q)),
        ("2026-10-25T01:35:00.987654321Z", None, None),
        ("2026-10-25T01:35:00.987654322Z", Some(R), None),
        ("2026-10-25T00:30:00.987654321Z", None, None),
        ("2026-10-25T00:30:00.987654320Z", None, Some(Q)),
        ("2026-10-25T03:35:01+02:00", Some(R), None),
    ];
    for (text, write, sweep) in golden {
        let entity = json!({"id": "urn:ngsi-ld:T:1", "type": "T",
            "a": {"type": "Property", "value": 1, "observedAt": text}});
        let rules = std::slice::from_ref(&rule);
        let w = crate::quality::on_write(&entity, rules, now());
        let s = crate::quality::on_sweep(&entity, rules, now());
        assert_eq!((w[0].broken, s[0].broken), (write, sweep), "{text}");
    }
}

/// The refusal of an expired subscription names `expiresAt` as chrono's `to_rfc3339` did.
#[test]
fn an_expired_subscription_is_named_as_chrono_wrote_it() {
    let sub = ResourceEnvelope::<SubscriptionSpec>::from_yaml(include_str!(
        "../tests/golden/027-Subscription-06-configuration-as-code.yaml"
    ))
    .expect("the golden manifest");
    let golden = [
        ("2026-10-25T01:30:00.987654321Z", Some("invalid spec.expiresAt `2026-10-25T01:30:00.987654321+00:00`: a subscription that has already expired would never fire")),
        ("2026-10-25T01:30:00.987654322Z", None),
        ("2026-10-25T03:30:00.5+02:00", Some("invalid spec.expiresAt `2026-10-25T01:30:00.500+00:00`: a subscription that has already expired would never fire")),
        ("2026-10-25T01:30:00Z", Some("invalid spec.expiresAt `2026-10-25T01:30:00+00:00`: a subscription that has already expired would never fire")),
    ];
    for (text, want) in golden {
        let mut spec = sub.spec.clone();
        spec.expires_at = Some(text.parse().expect("an instant"));
        let got = spec.validate_at(now()).err().map(|e| e.to_string());
        assert_eq!(got.as_deref(), want, "{text}");
    }
}

/// `lifetime.at` takes chrono's RFC 3339 grammar; the end is the instant (chrono kept the
/// written offset, which only its Debug showed: every reader took the instant).
#[test]
fn a_lifetime_at_reads_as_the_chrono_build_did() {
    let refused = |t: &str| {
        format!("invalid p.at: `{t}` is not an RFC 3339 instant with an offset, such as `2026-12-31T23:00:00Z` or `2026-12-31T23:00:00+01:00` (PL-80)")
    };
    let golden = [
        ("2026-03-29T01:30:00Z", Ok("2026-03-29T01:30:00Z")),
        ("2026-03-29T03:30:00.5+02:00", Ok("2026-03-29T01:30:00.5Z")),
        (
            "2026-10-25T02:30:00.123456+01:00",
            Ok("2026-10-25T01:30:00.123456Z"),
        ),
        (
            "2026-10-25T02:30:00.123456+03:00",
            Ok("2026-10-24T23:30:00.123456Z"),
        ),
        (
            "2026-01-01T00:00:00.1234567891Z",
            Ok("2026-01-01T00:00:00.123456789Z"),
        ),
        ("2026-01-01 00:00:00Z", Ok("2026-01-01T00:00:00Z")),
        ("2026-01-01t00:00:00z", Ok("2026-01-01T00:00:00Z")),
        ("2026-06-30T23:59:60Z", Ok("2026-07-01T00:00:00Z")),
        ("2026-02-30T00:00:00Z", Err(())),
        ("2026-01-01T00:00:00", Err(())),
        ("2026-01-01T00:00:00+0100", Err(())),
        (" 2026-01-01T00:00:00Z", Err(())),
        ("2026-01-01T00:00:00Z ", Err(())),
        ("0999-12-31T23:59:59.010Z", Ok("0999-12-31T23:59:59.01Z")),
    ];
    for (text, want) in golden {
        let mode: LifetimeMode = serde_json::from_value(json!({ "at": text })).expect("a mode");
        let got = mode.end("p").map_err(|e| e.to_string());
        match want {
            Ok(at) => assert_eq!(got, Ok(End::At(at.parse().expect("an instant"))), "{text}"),
            Err(()) => assert_eq!(got, Err(refused(text)), "{text}"),
        }
    }
}

/// A validity window that ends before it starts is named as chrono's `Display` wrote it.
#[test]
fn a_reversed_window_is_named_as_chrono_displayed_it() {
    let golden = [
        ("2026-01-01T00:00:00Z", "2026-01-01 00:00:00 UTC"),
        ("2026-01-01T00:00:00.5Z", "2026-01-01 00:00:00.500 UTC"),
        (
            "2026-01-01T00:00:00.123456Z",
            "2026-01-01 00:00:00.123456 UTC",
        ),
        (
            "2026-01-01T00:00:00.000000001Z",
            "2026-01-01 00:00:00.000000001 UTC",
        ),
        ("0999-12-31T23:59:59Z", "0999-12-31 23:59:59 UTC"),
    ];
    for (text, want) in golden {
        let t = text.parse().expect("an instant");
        assert_eq!(crate::kinds::policy::chrono_display(t), want, "{text}");
    }
}
