// SPDX-License-Identifier: EUPL-1.2
//! The metadata members an engine's grants pass per Attribute (platform
//! ADR-N-066 2.4, DM-139, DM-151): [`Metadata`] on a read's
//! [`Filter`].
//!
//! One layer around the NGSI-LD API applies it after the handler, so every
//! answer the broker renders from an NGSI-LD read is stripped once: the
//! API itself and every surface reading through `AppState::call` (MCP,
//! OData, OGC Features, SensorThings), whose mappings never see a member
//! the caller may not read. A `q`, `orderBy` or `timeproperty` naming a
//! stripped member is refused 400 naming it, so the hidden value cannot be
//! probed by filtering or sorting on it. A notification is stripped under
//! its subscriber's narrowing (`notify::delivery`). The platform gateway
//! strips the NGSI-LD view answers too, as defense in depth.
//!
//! An answer key and a `q` path are expanded through the request's
//! `@context` (its Link header, or the one a surface's in-process call
//! carries), the one the answer was compacted with, and compared with the
//! engine's IRIs. A POST query's body `q`, `orderBy` and `temporalQ` are
//! refused inside its handler (`Metadata::refuse_body`), with the body's
//! `@context`.

use crate::policy::Denied;
use antares_jsonld::Context;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Every metadata member a grant may pass (platform `MetadataMember`).
pub const MEMBERS: [&str; 9] = [
    "observedAt",
    "unitCode",
    "createdAt",
    "modifiedAt",
    "modifiedBy.organization",
    "modifiedBy.actorKind",
    "modifiedBy.actor",
    "provenance",
    "reason",
];

/// The words every refusal of a query on a stripped member carries: a
/// surface passes such a 400 on as it is, since it names only what the
/// caller wrote.
pub const STRIPPED: &str = "a metadata member your access does not pass";

/// The provenance members a version keeps without `provenance` (DM-151).
const VERSION_PROVENANCE: [&str; 4] = ["writtenAt", "operation", "organization", "actorKind"];

/// The members of an Entity that are not Attributes.
const NOT_ATTRIBUTES: [&str; 7] = [
    "id",
    "type",
    "@context",
    "scope",
    "createdAt",
    "modifiedAt",
    "deletedAt",
];

/// What a read may show of each Attribute's metadata members.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    /// Per Attribute, its IRI expanded against the core `@context`, the
    /// members passed.
    pub named: BTreeMap<String, BTreeSet<String>>,
    /// The members passed for every Attribute `named` lacks.
    pub rest: BTreeSet<String>,
}

impl Metadata {
    /// The members passed for the Attribute `name`, as an answer or a `q`
    /// compacted with `ctx` writes it.
    pub fn members(&self, name: &str, ctx: &Context) -> &BTreeSet<String> {
        self.named.get(&ctx.expand_key(name)).unwrap_or(&self.rest)
    }

    /// What two narrowings both pass: a view's and an engine's.
    pub fn both(a: Option<&Self>, b: Option<Box<Self>>) -> Option<Box<Self>> {
        let Some(a) = a else {
            return b;
        };
        let Some(b) = b else {
            return Some(Box::new(a.clone()));
        };
        let of = |m: &Self, k: &str| m.named.get(k).unwrap_or(&m.rest).clone();
        let named = a
            .named
            .keys()
            .chain(b.named.keys())
            .map(|k| (k.clone(), &of(a, k) & &of(&b, k)))
            .collect();
        Some(Box::new(Self {
            named,
            rest: &a.rest & &b.rest,
        }))
    }

    /// Whether `member` passes for every Attribute.
    fn everywhere(&self, member: &str) -> bool {
        self.rest.contains(member) && self.named.values().all(|m| m.contains(member))
    }

    /// Why a request whose query string is `query` is refused (DM-139): it
    /// filters or orders by a member this read strips.
    pub fn refusal(&self, query: &str, ctx: &Context) -> Option<String> {
        let pairs = form_urlencoded(query);
        self.refusal_of(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())), ctx)
    }

    /// [`Self::refusal`] of parsed parameters: a URL's, or the members a POST
    /// query's body carries (`q`, `orderBy`, its `temporalQ` flattened), read
    /// with the body's `@context` (F-10). A history window (`timerel`) with
    /// no `timeproperty` is judged on `observedAt`, its default (5.2.21).
    pub fn refusal_of<'a>(
        &self,
        pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
        ctx: &Context,
    ) -> Option<String> {
        let mut windowed = false;
        let mut timed = false;
        for (name, value) in pairs {
            windowed |= name == "timerel";
            timed |= name == "timeproperty";
            let named = match name {
                "q" | "orderBy" => named_members(value),
                "timeproperty" if ["observedAt", "createdAt", "modifiedAt"].contains(&value) => {
                    if self.everywhere(value) {
                        Vec::new()
                    } else {
                        return Some(format!(
                            "{name} names `{value}`, {STRIPPED}; ask without it"
                        ));
                    }
                }
                _ => Vec::new(),
            };
            if let Some((attr, member)) = named
                .into_iter()
                .find(|(attr, member)| !self.members(attr, ctx).contains(*member))
            {
                return Some(format!(
                    "{name} names `{attr}.{member}`, {STRIPPED}; ask without it"
                ));
            }
        }
        (windowed && !timed && !self.everywhere("observedAt")).then(|| {
            format!(
                "timerel without timeproperty filters by `observedAt`, {STRIPPED}; ask without it"
            )
        })
    }

    /// Refuse a POST query (5.7.2, 5.7.4) whose body filters or orders by a
    /// member `filter` strips: the layer reads only the URL (F-10).
    pub(crate) fn refuse_body(
        filter: &crate::policy::Filter,
        params: &std::collections::HashMap<String, String>,
        ctx: &Context,
    ) -> Result<(), antares_model::NgsiError> {
        let Some(metadata) = filter.metadata.as_deref() else {
            return Ok(());
        };
        match metadata.refusal_of(params.iter().map(|(k, v)| (k.as_str(), v.as_str())), ctx) {
            Some(why) => Err(antares_model::NgsiError::BadRequestData(why)),
            None => Ok(()),
        }
    }

    /// Strip every member not passed from an NGSI-LD answer: one Entity, a
    /// list of them, their temporal evolutions with versions, or a GeoJSON
    /// Feature or FeatureCollection.
    pub fn strip(&self, body: &mut Value, ctx: &Context) {
        match body {
            Value::Array(items) => items.iter_mut().for_each(|item| self.strip(item, ctx)),
            Value::Object(object) => {
                if let Some(Value::Array(features)) = object.get_mut("features") {
                    for f in features {
                        self.strip(f, ctx);
                    }
                } else if object.get("type").and_then(Value::as_str) == Some("Feature") {
                    if let Some(Value::Object(properties)) = object.get_mut("properties") {
                        self.strip_entity(properties, ctx);
                    }
                } else {
                    self.strip_entity(object, ctx);
                }
            }
            _ => {}
        }
    }

    /// Strip one Entity's Attributes.
    pub fn strip_entity(&self, entity: &mut Map<String, Value>, ctx: &Context) {
        entity.retain(|name, value| {
            if NOT_ATTRIBUTES.contains(&name.as_str()) {
                return true;
            }
            let members = self.members(name, ctx);
            match value {
                Value::Object(instance) => strip_instance(members, instance),
                Value::Array(instances) => {
                    instances.retain_mut(|i| match i {
                        Value::Object(instance) => strip_instance(members, instance),
                        _ => true,
                    });
                    true
                }
                _ => true,
            }
        });
    }
}

/// Strip one Attribute instance; `false` drops it whole: a temporal
/// `values` list whose times are `observedAt`, which the caller may not read.
fn strip_instance(members: &BTreeSet<String>, instance: &mut Map<String, Value>) -> bool {
    let passes = |m: &str| members.contains(m);
    if instance.contains_key("values") && !passes("observedAt") {
        return false;
    }
    for key in ["observedAt", "unitCode", "createdAt", "modifiedAt"] {
        if !passes(key) {
            instance.remove(key);
        }
    }
    if let Some(Value::Object(by)) = instance.get_mut("modifiedBy") {
        for key in ["organization", "actorKind", "actor"] {
            if !passes(&format!("modifiedBy.{key}")) {
                by.remove(key);
            }
        }
        if by.is_empty() {
            instance.remove("modifiedBy");
        }
    }
    if !passes("provenance") {
        if let Some(Value::Object(provenance)) = instance.get_mut("provenance") {
            provenance.retain(|key, _| {
                VERSION_PROVENANCE.contains(&key.as_str()) || (key == "reason" && passes("reason"))
            });
        }
    }
    // a sub-Attribute carries the same members, under the parent's grant (F-10)
    for (key, value) in instance.iter_mut() {
        if matches!(
            key.as_str(),
            "value" | "values" | "object" | "languageMap" | "vocab" | "json"
        ) {
            continue;
        }
        match value {
            Value::Object(sub) if is_attribute(sub) => {
                strip_instance(members, sub);
            }
            Value::Array(subs) => {
                subs.retain_mut(|s| match s {
                    Value::Object(sub) if is_attribute(sub) => strip_instance(members, sub),
                    _ => true,
                });
            }
            _ => {}
        }
    }
    true
}

/// Whether an object inside an Attribute instance is a sub-Attribute (4.5.6).
fn is_attribute(o: &Map<String, Value>) -> bool {
    o.get("type").and_then(Value::as_str).is_some_and(|t| {
        matches!(
            t,
            "Property"
                | "Relationship"
                | "GeoProperty"
                | "LanguageProperty"
                | "VocabProperty"
                | "JsonProperty"
                | "ListProperty"
                | "ListRelationship"
        )
    })
}

/// The query string's pairs, decoded as `negotiate::CleanParams` decodes them.
fn form_urlencoded(query: &str) -> Vec<(String, String)> {
    let decode = |s: &str| crate::negotiate::percent_decode(s.replace('+', " ").as_bytes());
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(p), String::new()),
        })
        .collect()
}

/// The `(Attribute, member)` pairs a `q` or `orderBy` names as `attr.member`.
fn named_members(text: &str) -> Vec<(String, &'static str)> {
    let mut found = Vec::new();
    let words = text.split(|c: char| {
        matches!(
            c,
            ';' | '|' | '(' | ')' | '!' | '=' | '<' | '>' | '~' | ',' | '"'
        ) || c.is_whitespace()
    });
    for word in words {
        let path: Vec<&str> = word.split('.').collect();
        let [attribute, member, rest @ ..] = path.as_slice() else {
            continue;
        };
        let attribute = attribute.split('[').next().unwrap_or_default();
        let member = match (*member, rest.first().copied()) {
            ("observedAt", _) => "observedAt",
            ("unitCode", _) => "unitCode",
            ("createdAt", _) => "createdAt",
            ("modifiedAt", _) => "modifiedAt",
            ("modifiedBy", Some("organization")) => "modifiedBy.organization",
            ("modifiedBy", Some("actorKind")) => "modifiedBy.actorKind",
            ("modifiedBy", _) => "modifiedBy.actor",
            _ => continue,
        };
        if !attribute.is_empty() {
            found.push((attribute.to_owned(), member));
        }
    }
    found
}

#[cfg(not(target_arch = "wasm32"))]
tokio::task_local! {
    static HELD: std::cell::RefCell<Option<Box<Metadata>>>;
}

/// Hand the request's [`Metadata`] to the layer that strips the answer:
/// the gate's call for every narrowing that carries some. Refused where no
/// layer runs (a handler called outside the router, the wasm build): an
/// answer nothing would strip is not served.
pub(crate) fn hold(metadata: Option<&Metadata>) -> Result<(), Denied> {
    let Some(metadata) = metadata else {
        return Ok(());
    };
    #[cfg(not(target_arch = "wasm32"))]
    if HELD
        .try_with(|h| *h.borrow_mut() = Some(Box::new(metadata.clone())))
        .is_ok()
    {
        return Ok(());
    }
    let _ = metadata;
    Err(Denied(
        "this read passes only some metadata members, and nothing here strips the others".into(),
    ))
}

/// Strip the answer of a read whose gate held [`Metadata`]; refuse it 400
/// when its query filters or orders by a stripped member.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn layer(
    axum::extract::State(st): axum::extract::State<crate::AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let query = req.uri().query().unwrap_or_default().to_owned();
    let headers = context_headers(req.headers());
    let (resp, held) = HELD
        .scope(std::cell::RefCell::new(None), async {
            let resp = next.run(req).await;
            (resp, HELD.with(std::cell::RefCell::take))
        })
        .await;
    let Some(metadata) = held else {
        return resp;
    };
    let fail = |e: antares_model::NgsiError| crate::negotiate::ApiError::from(e).into_response();
    let ctx = match crate::negotiate::request_context_quiet(&st.loader, &headers).await {
        Ok(ctx) => ctx,
        Err(e) => return e.into_response(),
    };
    if let Some(why) = metadata.refusal(&query, &ctx) {
        return fail(antares_model::NgsiError::BadRequestData(why));
    }
    let json = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.contains("json"));
    if !resp.status().is_success() || !json {
        return resp;
    }
    let (mut parts, body) = resp.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, st.max_answer_bytes).await else {
        return fail(antares_model::NgsiError::InternalError(
            crate::bounds::ANSWER_TOO_LARGE.into(),
        ));
    };
    let Ok(mut doc) = serde_json::from_slice::<Value>(&bytes) else {
        return fail(antares_model::NgsiError::InternalError(
            "a narrowed answer is not JSON".into(),
        ));
    };
    metadata.strip(&mut doc, &ctx);
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    parts.headers.remove(axum::http::header::ETAG);
    axum::response::Response::from_parts(parts, axum::body::Body::from(doc.to_string()))
}

/// The two headers [`crate::negotiate::request_context`] reads (Link and
/// NGSILD-Tenant), every value kept so a repeated header is refused as
/// before: copied before the handler takes the request, without cloning
/// every other header of it (R1-020).
#[cfg(not(target_arch = "wasm32"))]
fn context_headers(all: &axum::http::HeaderMap) -> axum::http::HeaderMap {
    let mut kept = axum::http::HeaderMap::new();
    for name in [
        axum::http::header::LINK,
        axum::http::HeaderName::from_static("ngsild-tenant"),
    ] {
        for v in all.get_all(&name) {
            kept.append(name.clone(), v.clone());
        }
    }
    kept
}

/// No task-locals on wasm: [`hold`] refuses instead, so nothing to strip.
#[cfg(target_arch = "wasm32")]
pub(crate) async fn layer(
    axum::extract::State(_st): axum::extract::State<crate::AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn iri(name: &str) -> String {
        core().expand_key(name)
    }

    fn core() -> &'static Context {
        crate::repr::policy_context()
    }

    /// R1-020: the layer copies only what `request_context` reads, every
    /// value of it, so a repeated NGSILD-Tenant is still refused.
    #[test]
    fn the_layer_keeps_only_the_link_and_tenant_headers() {
        use axum::http::{header, HeaderMap, HeaderValue};
        let mut all = HeaderMap::new();
        for i in 0..10 {
            all.insert(
                axum::http::HeaderName::try_from(format!("x-other-{i}")).unwrap_or(header::FROM),
                HeaderValue::from_static("v"),
            );
        }
        all.append(
            header::LINK,
            HeaderValue::from_static("<https://a/c.jsonld>"),
        );
        all.append("ngsild-tenant", HeaderValue::from_static("a"));
        all.append("ngsild-tenant", HeaderValue::from_static("b"));
        let kept = context_headers(&all);
        assert_eq!(kept.len(), 3, "{kept:?}");
        assert_eq!(kept.get_all("ngsild-tenant").iter().count(), 2);
        assert!(crate::negotiate::tenant_from(&kept).is_err());
        assert_eq!(
            crate::negotiate::link_context(&kept).ok().flatten(),
            crate::negotiate::link_context(&all).ok().flatten()
        );
    }

    fn set(members: &[&str]) -> BTreeSet<String> {
        members.iter().map(|m| (*m).to_owned()).collect()
    }

    /// `pm10` passes only `observedAt`; every other Attribute the public
    /// defaults.
    fn public() -> Metadata {
        Metadata {
            named: BTreeMap::from([(iri("pm10"), set(&["observedAt"]))]),
            rest: set(&["observedAt", "unitCode", "modifiedAt"]),
        }
    }

    fn instance() -> Value {
        json!({"type": "Property", "value": 41, "observedAt": "2026-11-01T10:00:00Z",
               "unitCode": "GQ", "createdAt": "2026-10-01T00:00:00Z",
               "modifiedAt": "2026-11-01T10:00:01Z",
               "modifiedBy": {"organization": "city", "actorKind": "person", "actor": "0f5a"},
               "provenance": {"writtenAt": "2026-11-01T10:00:01Z", "operation": "update",
                              "actor": "0f5a", "reason": "stuck", "channel": "ui"}})
    }

    #[test]
    fn each_attribute_keeps_only_the_members_it_passes() {
        let mut doc = json!({"id": "urn:ngsi-ld:A:1", "type": "A", "createdAt": "x",
                             "pm10": instance(), "name": [instance()]});
        public().strip(&mut doc, core());
        assert_eq!(
            doc["pm10"],
            json!({"type": "Property", "value": 41, "observedAt": "2026-11-01T10:00:00Z",
                   "provenance": {"writtenAt": "2026-11-01T10:00:01Z", "operation": "update"}})
        );
        assert_eq!(doc["name"][0]["unitCode"], "GQ");
        assert_eq!(doc["name"][0]["modifiedAt"], "2026-11-01T10:00:01Z");
        assert!(doc["name"][0].get("createdAt").is_none(), "{doc}");
        assert!(doc["name"][0].get("modifiedBy").is_none(), "{doc}");
        assert_eq!(
            doc["createdAt"], "x",
            "an Entity's own createdAt is no member"
        );
    }

    /// F-10: a sub-Attribute carries the members of its parent's grant, stripped the same way.
    #[test]
    fn a_sub_attribute_is_stripped_as_its_parent() {
        let mut parent = instance();
        parent["accuracy"] = instance();
        parent["readings"] = json!([instance()]);
        let mut doc = json!({"id": "urn:ngsi-ld:A:1", "type": "A", "pm10": parent});
        public().strip(&mut doc, core());
        for sub in [&doc["pm10"]["accuracy"], &doc["pm10"]["readings"][0]] {
            assert!(sub.get("unitCode").is_none(), "{doc}");
            assert!(sub.get("modifiedBy").is_none(), "{doc}");
            assert_eq!(sub["observedAt"], "2026-11-01T10:00:00Z", "{doc}");
        }
        assert_eq!(doc["pm10"]["value"], 41);
    }

    /// F-10: a history window without `timeproperty` filters by `observedAt`, its default, and
    /// parsed parameters (a POST body's) are judged as a query string is.
    #[test]
    fn a_window_on_the_default_timeproperty_is_refused_when_observed_at_is_stripped() {
        let m = Metadata {
            named: BTreeMap::new(),
            rest: set(&["unitCode"]),
        };
        assert!(m
            .refusal("timerel=after&timeAt=2026-01-01T00:00:00Z", core())
            .is_some_and(|w| w.contains("observedAt")));
        assert!(m
            .refusal(
                "timerel=after&timeAt=2026-01-01T00:00:00Z&timeproperty=observedAt",
                core()
            )
            .is_some());
        assert!(public()
            .refusal("timerel=after&timeAt=2026-01-01T00:00:00Z", core())
            .is_none());
        let body = [("q", "pm10.unitCode==\"GQ\"")];
        assert!(public().refusal_of(body, core()).is_some());
        assert!(public().refusal_of([("q", "pm10>40")], core()).is_none());
    }

    #[test]
    fn a_reason_passes_only_where_listed() {
        let m = Metadata {
            named: BTreeMap::new(),
            rest: set(&["reason", "modifiedBy.organization"]),
        };
        let mut e = json!({"id": "urn:ngsi-ld:A:1", "type": "A", "name": instance()});
        m.strip(&mut e, core());
        let i = &e["name"];
        assert_eq!(i["provenance"]["reason"], "stuck");
        assert!(i["provenance"].get("actor").is_none(), "{i}");
        assert_eq!(i["modifiedBy"], json!({"organization": "city"}));
    }

    #[test]
    fn geojson_features_and_temporal_values_are_stripped() {
        let mut fc = json!({"type": "FeatureCollection", "features": [
            {"type": "Feature", "id": "urn:ngsi-ld:A:1", "properties": {"pm10": instance()}}]});
        public().strip(&mut fc, core());
        assert!(
            fc["features"][0]["properties"]["pm10"]
                .get("unitCode")
                .is_none(),
            "{fc}"
        );
        let no_times = Metadata {
            named: BTreeMap::new(),
            rest: set(&["unitCode"]),
        };
        let mut t = json!({"id": "urn:ngsi-ld:A:1", "type": "A",
                           "pm10": {"type": "Property", "values": [[41, "2026-11-01T10:00:00Z"]]}});
        no_times.strip(&mut t, core());
        assert!(t.get("pm10").is_none(), "the values list goes whole: {t}");
    }

    #[test]
    fn a_query_on_a_stripped_member_is_refused_naming_it() {
        let m = public();
        assert_eq!(
            m.refusal("type=A&q=pm10.unitCode%3D%3D%22GQ%22", core())
                .as_deref(),
            Some(
                "q names `pm10.unitCode`, a metadata member your access does not pass; \
                 ask without it"
            )
        );
        assert!(m
            .refusal("q=pm10.observedAt>2026-01-01T00:00:00Z", core())
            .is_none());
        assert!(m.refusal("q=name.unitCode==%22GQ%22", core()).is_none());
        assert!(m
            .refusal("orderBy=name.createdAt", core())
            .is_some_and(|w| w.contains("`name.createdAt`")));
        assert!(m.refusal("timeproperty=modifiedAt", core()).is_some());
        assert!(m.refusal("timeproperty=observedAt", core()).is_none());
        assert!(m.refusal("q=pm10>40&attrs=pm10", core()).is_none());
    }
}
