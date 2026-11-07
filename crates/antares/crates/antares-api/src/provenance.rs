// SPDX-License-Identifier: EUPL-1.2
//! Who and what wrote a request's history (T-4665, platform ADR-N-054 2.3).
//!
//! The platform's gateway verifies the caller and names it in
//! `JC-Provenance-Channel`, `JC-Provenance-Actor`, `JC-Reason` and
//! `X-Request-Id`, the writer's Organization and kind in
//! `JC-Provenance-Organization` and `JC-Provenance-Actor-Kind`, the Keycloak
//! client the token came through in `JC-Provenance-Client` (T-5570), and an audited
//! Endpoint's write in `JC-Provenance-Audit: changes` (ADR-N-054 2.8). Those headers are believed only from the gateway itself:
//! the request must carry the mesh identity `ANTARES_PROVENANCE_FROM` names in
//! `l5d-client-id`, the header the Linkerd proxy in front of the broker sets
//! from the caller's mTLS certificate. Unset, or from anyone else, a write is
//! `channel: api` with no actor, whatever its headers say.
//!
//! All of that is the platform's (feature `jc`, T-4854): vanilla Antares
//! reads none of these headers and records every write as `channel: api`.

use axum::http::HeaderMap;
use serde_json::{json, Value};

// The provenance of the request in flight rides its task, as the history
// buffer does; wasm has no tokio task-locals.
#[cfg(not(target_arch = "wasm32"))]
tokio::task_local! {
    pub(crate) static PROVENANCE: Value;
}

#[cfg(feature = "jc")]
/// The channels a write may name (ADR-N-054 2.3); anything else is `api`.
const CHANNELS: &[&str] = &["pipeline", "api", "ui", "import", "backfill", "mqtt"];
#[cfg(feature = "jc")]
/// The kinds of writer (ADR-N-054 2.3); anything else is left out.
const ACTOR_KINDS: &[&str] = &["person", "serviceAccount", "pipeline", "agent"];
#[cfg(feature = "jc")]
const MAX_ACTOR: usize = 200;
#[cfg(feature = "jc")]
const MAX_REASON: usize = 500;
#[cfg(feature = "jc")]
const MAX_REQUEST_ID: usize = 128;

/// The provenance of a request with `headers`, the gateway's mesh identity
/// being `gateway` (None: no caller is believed).
#[cfg(feature = "jc")]
pub fn of_request(headers: &HeaderMap, gateway: Option<&str>) -> Value {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let believed = from_gateway(headers, gateway);
    let mut out = serde_json::Map::new();
    let channel = header("jc-provenance-channel")
        .filter(|c| believed && CHANNELS.contains(c))
        .unwrap_or("api");
    out.insert("channel".into(), json!(channel));
    if !believed {
        return Value::Object(out);
    }
    // The gateway percent-encodes the actor (`serviceAccount%3Apl-parking`).
    if let Some(actor) = header("jc-provenance-actor")
        .and_then(percent_decoded)
        .filter(|a| plain(a, MAX_ACTOR))
    {
        out.insert("actor".into(), json!(actor));
    }
    if let Some(organization) = header("jc-provenance-organization")
        .and_then(percent_decoded)
        .filter(|o| plain(o, MAX_ACTOR))
    {
        out.insert("organization".into(), json!(organization));
    }
    // The MCP client, CLI or App a person's write came through (T-5570).
    if let Some(client) = header("jc-provenance-client")
        .and_then(percent_decoded)
        .filter(|c| plain(c, MAX_ACTOR))
    {
        out.insert("client".into(), json!(client));
    }
    if let Some(kind) = header("jc-provenance-actor-kind").filter(|k| ACTOR_KINDS.contains(k)) {
        out.insert("actorKind".into(), json!(kind));
    }
    // An audited Endpoint's write (DM-136): kept as a version whatever the
    // type's history rule; the row says why it exists.
    if header("jc-provenance-audit") == Some("changes") {
        out.insert(AUDIT.into(), json!(true));
    }
    if let Some(reason) = header("jc-reason")
        .and_then(percent_decoded)
        .filter(|r| plain(r, MAX_REASON))
    {
        out.insert("reason".into(), json!(reason));
    }
    if let Some(id) = header("x-request-id").filter(|id| {
        (1..=MAX_REQUEST_ID).contains(&id.len())
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    }) {
        out.insert("requestId".into(), json!(id));
    }
    Value::Object(out)
}

/// The provenance member that marks a row kept because its Endpoint audits
/// changes (ADR-N-054 2.8, DM-136).
pub const AUDIT: &str = "audit";

/// Whether `provenance` is an audited write's (DM-136).
pub fn audited(provenance: &Value) -> bool {
    provenance.get(AUDIT) == Some(&Value::Bool(true))
}

/// Vanilla: no header is read, every write is `channel: api`.
#[cfg(not(feature = "jc"))]
pub fn of_request(_headers: &HeaderMap, _gateway: Option<&str>) -> Value {
    json!({"channel": "api"})
}

/// Whether the request may read `versions=all` (ADR-N-054 2.4) of any
/// Attribute: see [`versions_granted`].
#[cfg(feature = "jc")]
pub fn may_read_versions(headers: &HeaderMap, gateway: Option<&str>) -> bool {
    versions_granted(headers, gateway).is_some()
}

/// The Attributes whose versions the request may read (platform ADR-N-066
/// 2.3, DM-149): the gateway sends `JC-Read-Versions` once the space's
/// Policy granted the caller `readVersions`, as `*` (or `true`, the form
/// before ADR-N-066) for every Attribute or as a comma list of
/// percent-encoded short names; nobody else can. `Some(None)` is every
/// Attribute; a value out of shape grants nothing.
#[cfg(feature = "jc")]
pub fn versions_granted(headers: &HeaderMap, gateway: Option<&str>) -> Option<Option<Vec<String>>> {
    if !from_gateway(headers, gateway) {
        return None;
    }
    let raw = headers.get("jc-read-versions")?.to_str().ok()?;
    if raw == "true" || raw == "*" {
        return Some(None);
    }
    let names: Option<Vec<String>> = raw
        .split(',')
        .map(|n| percent_decoded(n.trim()).filter(|n| !n.is_empty() && n != "*"))
        .collect();
    names.map(Some)
}

/// Does the request carry the gateway's mesh identity `gateway`?
#[cfg(feature = "jc")]
pub(crate) fn from_gateway(headers: &HeaderMap, gateway: Option<&str>) -> bool {
    gateway.is_some_and(|g| {
        headers
            .get("l5d-client-id")
            .is_some_and(|v| v.as_bytes() == g.as_bytes())
    })
}

/// Whether a read may answer each attribute's `modifiedBy` (ADR-N-054 2.8,
/// DM-138): only to a caller the gateway grants `readVersions`, since
/// `actor` is a person's `sub` or a client id. Vanilla answers it to nobody.
#[cfg(feature = "jc")]
pub fn shows_modified_by(headers: &HeaderMap, gateway: Option<&str>) -> bool {
    may_read_versions(headers, gateway)
}

/// Vanilla: no read answers `modifiedBy`.
#[cfg(not(feature = "jc"))]
pub fn shows_modified_by(_headers: &HeaderMap, _gateway: Option<&str>) -> bool {
    false
}

/// The `modifiedBy` a write with `provenance` leaves on each attribute it
/// changes (DM-138): its `organization`, `actorKind` and `actor`; `None` when
/// it names none of them, as every write the gateway did not send.
pub fn modified_by(provenance: &Value) -> Option<Value> {
    let by: serde_json::Map<String, Value> = ["organization", "actorKind", "actor"]
        .into_iter()
        .filter_map(|k| Some((k.to_owned(), provenance.get(k)?.clone())))
        .collect();
    (!by.is_empty()).then_some(Value::Object(by))
}

/// The `modifiedBy` of the request in flight (DM-138); `None` outside a
/// request and for a write the gateway did not send.
pub(crate) fn modified_by_now() -> Option<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    return PROVENANCE.try_with(modified_by).ok().flatten();
    #[cfg(target_arch = "wasm32")]
    None
}

/// `provenance` with its `operation`.
pub fn with_operation(provenance: &Value, operation: &str) -> Value {
    let mut out = provenance.clone();
    if let Some(o) = out.as_object_mut() {
        o.insert("operation".into(), json!(operation));
    }
    out
}

/// Non-empty, at most `max` characters, no control character.
#[cfg(feature = "jc")]
fn plain(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= max && !text.chars().any(char::is_control)
}

/// `%XX`-decoded UTF-8; `None` for a broken escape or bytes that are not UTF-8.
#[cfg(feature = "jc")]
fn percent_decoded(raw: &str) -> Option<String> {
    let mut out = Vec::with_capacity(raw.len());
    let mut bytes = raw.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATEWAY: &str = "context-gateway.dev.serviceaccount.identity.linkerd.cluster.local";

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().expect("header"));
        }
        h
    }

    #[cfg(not(feature = "jc"))]
    #[test]
    fn vanilla_reads_no_platform_header() {
        let h = headers(&[
            ("l5d-client-id", GATEWAY),
            ("jc-provenance-channel", "pipeline"),
            ("jc-provenance-actor", "pipeline-parking"),
            ("jc-reason", "stuck"),
            ("x-request-id", "4f1c-9a07"),
        ]);
        assert_eq!(of_request(&h, Some(GATEWAY)), json!({"channel": "api"}));
    }

    #[test]
    fn the_operation_is_added() {
        assert_eq!(
            with_operation(&json!({"channel": "ui"}), "delete"),
            json!({"channel": "ui", "operation": "delete"})
        );
    }

    #[cfg(feature = "jc")]
    #[test]
    fn the_gateways_headers_are_believed_from_the_gateway_alone() {
        let named = [
            ("jc-provenance-channel", "pipeline"),
            ("jc-provenance-actor", "pipeline-parking"),
            ("jc-reason", "Sensor%20stuck%2C%20checked%20on%20site"),
            ("x-request-id", "4f1c-9a07"),
        ];
        let mut from_gateway = named.to_vec();
        from_gateway.push(("l5d-client-id", GATEWAY));
        assert_eq!(
            of_request(&headers(&from_gateway), Some(GATEWAY)),
            json!({"channel": "pipeline", "actor": "pipeline-parking",
                   "reason": "Sensor stuck, checked on site", "requestId": "4f1c-9a07"})
        );
        // A forged identity, no identity, or a broker that names no gateway: api, nobody.
        let mut forged = named.to_vec();
        forged.push((
            "l5d-client-id",
            "someone.else.serviceaccount.identity.linkerd.cluster.local",
        ));
        for (h, gateway) in [
            (headers(&forged), Some(GATEWAY)),
            (headers(&named), Some(GATEWAY)),
            (headers(&from_gateway), None),
        ] {
            assert_eq!(of_request(&h, gateway), json!({"channel": "api"}));
        }
    }

    #[cfg(feature = "jc")]
    #[test]
    fn the_actor_is_stored_as_the_gateway_named_it() {
        let h = headers(&[
            ("l5d-client-id", GATEWAY),
            ("jc-provenance-channel", "pipeline"),
            ("jc-provenance-actor", "serviceAccount%3Apl-parking"),
        ]);
        assert_eq!(
            of_request(&h, Some(GATEWAY)),
            json!({"channel": "pipeline", "actor": "serviceAccount:pl-parking"})
        );
        // A broken escape or a decoded control character names nobody.
        for bad in ["user%3", "user%0Ajana"] {
            let h = headers(&[("l5d-client-id", GATEWAY), ("jc-provenance-actor", bad)]);
            assert_eq!(of_request(&h, Some(GATEWAY)), json!({"channel": "api"}));
        }
    }

    #[cfg(feature = "jc")]
    #[test]
    fn organization_kind_and_audit_are_believed_from_the_gateway_alone() {
        let named = [
            ("jc-provenance-actor", "user%3Ajana"),
            ("jc-provenance-organization", "banska-bystrica"),
            ("jc-provenance-actor-kind", "agent"),
            ("jc-provenance-client", "claude%20desktop"),
            ("jc-provenance-audit", "changes"),
        ];
        let mut from_gateway = named.to_vec();
        from_gateway.push(("l5d-client-id", GATEWAY));
        let stored = of_request(&headers(&from_gateway), Some(GATEWAY));
        assert_eq!(
            stored,
            json!({"channel": "api", "actor": "user:jana", "organization": "banska-bystrica",
                   "client": "claude desktop", "actorKind": "agent", "audit": true})
        );
        assert!(audited(&stored));
        // Forged by anyone else: nothing of it is kept, no audit row forced.
        let forged = of_request(&headers(&named), Some(GATEWAY));
        assert_eq!(forged, json!({"channel": "api"}));
        assert!(!audited(&forged));
        // An unknown kind, an organization with a control character, another audit word.
        let odd = headers(&[
            ("l5d-client-id", GATEWAY),
            ("jc-provenance-organization", "hel%0Afi"),
            ("jc-provenance-client", "claude%0Adesktop"),
            ("jc-provenance-actor-kind", "robot"),
            ("jc-provenance-audit", "all"),
        ]);
        assert_eq!(of_request(&odd, Some(GATEWAY)), json!({"channel": "api"}));
    }

    #[cfg(feature = "jc")]
    #[test]
    fn versions_are_read_only_where_the_gateway_grants_them() {
        let grant = [("l5d-client-id", GATEWAY), ("jc-read-versions", "true")];
        assert!(may_read_versions(&headers(&grant), Some(GATEWAY)));
        assert!(!may_read_versions(&headers(&grant), None));
        assert!(!may_read_versions(
            &headers(&[("l5d-client-id", "x.y"), ("jc-read-versions", "true")]),
            Some(GATEWAY)
        ));
        assert!(!may_read_versions(
            &headers(&[("l5d-client-id", GATEWAY), ("jc-read-versions", "")]),
            Some(GATEWAY)
        ));
    }

    #[cfg(feature = "jc")]
    #[test]
    fn versions_are_granted_per_attribute_as_the_gateway_lists_them() {
        let of = |v: &str| {
            versions_granted(
                &headers(&[("l5d-client-id", GATEWAY), ("jc-read-versions", v)]),
                Some(GATEWAY),
            )
        };
        assert_eq!(of("*"), Some(None));
        assert_eq!(of("true"), Some(None));
        assert_eq!(of("name"), Some(Some(vec!["name".into()])));
        assert_eq!(
            of("name, air%3Apm10"),
            Some(Some(vec!["name".into(), "air:pm10".into()]))
        );
        for odd in ["", "name,,pm10", "name,*", "%zz", "yes,"] {
            assert_eq!(of(odd), None, "{odd:?}");
        }
    }

    #[cfg(feature = "jc")]
    #[test]
    fn a_value_out_of_shape_is_left_out() {
        let long = "x".repeat(MAX_REASON + 1);
        let h = headers(&[
            ("l5d-client-id", GATEWAY),
            ("jc-provenance-channel", "carrier-pigeon"),
            ("jc-reason", &long),
            ("x-request-id", "a b"),
            ("jc-provenance-actor", " "),
        ]);
        assert_eq!(of_request(&h, Some(GATEWAY)), json!({"channel": "api"}));
        assert_eq!(percent_decoded("%zz"), None);
        assert_eq!(percent_decoded("%+1"), None, "a sign is no hex digit");
        assert_eq!(percent_decoded("50%"), None, "an escape cut short");
        assert_eq!(percent_decoded("%C3"), None, "half a character");
        assert_eq!(percent_decoded("N%C3%A1m"), Some("Nám".into()));
    }
}
