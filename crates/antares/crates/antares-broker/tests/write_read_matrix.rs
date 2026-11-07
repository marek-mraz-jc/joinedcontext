// SPDX-License-Identifier: EUPL-1.2
//! Write here, read everywhere (T-5913): one Entity of every attribute kind and value type,
//! written through each write surface of one view, read back through every read surface of the
//! same view, current, temporal and stream, each cell asserting the exact value, the kind where
//! the reader carries it, `unitCode`, `observedAt` and `datasetId`, and that the hidden
//! attribute is absent.
//!
//! - Writers: NGSI-LD create, batch create, merge, replace, temporal append; OData `POST {Set}`;
//!   OGC API - Features Part 4 `POST items`. STA writes are a `not served` row until T-5910.
//! - Readers current: NGSI-LD, OData (with `$expand` of the Relationship), OGC Features, STA
//!   (Things, Datastreams, Observations, Locations), MCP `get_entity`; with the `geojson` and
//!   `tabular` features also the `GeoJSON` surface and OGC `f=csv`.
//! - Readers temporal: NGSI-LD temporal, OGC `{type}-history`, STA Datastream Observations;
//!   OData history is `not served` until T-5908.
//! - Expiry rows (T-5933): an instance written with `expiresAt` 2 s ahead shows on every reader
//!   (the expiresAt windows of the temporal API and MCP included) and on none after it expired.
//! - Readers stream: the NGSI-LD Subscription's HTTP notification and the view's WebSocket; STA
//!   MQTT is `not served` until T-5911.
//!
//! A value a writer cannot express is a `not written` line of that writer, a value a reader does
//! not carry a `not served` line of that reader with its reason: both are printed in the table,
//! never skipped silently. Every Entity also gets the hidden `secret` through the Tenant root
//! after its write, so every reader cell proves the view keeps it out.
//!
//! The matrix runs over the built-in store, and again over Postgres when
//! `ANTARES_TEST_DATABASE_URL` is set (it says SKIP otherwise). It lives in the broker's tests
//! because a surface crate may dev-depend on no other surface (module rule surface-dev).
#![cfg(feature = "full")]

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::Service;

#[path = "common/ws_client.rs"]
mod ws_client;
use ws_client::Socket;

const TENANT: &str = "mx";
const VIEW: &str = "/x/views/m";

const SCHEMA: &str = r#"{
  "id": "https://example.org/mx", "name": "mx",
  "prefixes": {"d": {"prefix_prefix": "d", "prefix_reference": "https://uri.etsi.org/ngsi-ld/default-context/"}},
  "default_prefix": "d",
  "classes": {
    "Probe": {"slots": ["name", "count", "level", "ratio", "active", "seenAt", "day", "mode", "home",
                        "label", "location", "area", "site", "tags", "power", "secret"]},
    "Site": {"slots": ["name"]}
  },
  "enums": {"Mode": {"permissible_values": {"auto": {}, "manual": {}}}},
  "slots": {
    "name": {"range": "string"}, "count": {"range": "integer"},
    "level": {"range": "float", "unit": {"ucum_code": "Cel"}},
    "ratio": {"range": "decimal"}, "active": {"range": "boolean"},
    "seenAt": {"range": "datetime", "annotations": {"ogc_role": {"tag": "ogc_role", "value": "primary-instant"}}},
    "day": {"range": "date"}, "mode": {"range": "Mode"}, "home": {"range": "uri"},
    "label": {"range": "string", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "LanguageProperty"}}},
    "location": {"slot_uri": "https://uri.etsi.org/ngsi-ld/location",
                 "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
    "area": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
    "site": {"range": "Site"},
    "tags": {"range": "string", "multivalued": true,
             "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListProperty"}}},
    "power": {"range": "float", "unit": {"ucum_code": "kW"}},
    "secret": {"range": "string"}
  }
}"#;

/// What the view shows of a Probe: every slot but `secret`.
const SHOWN: [&str; 15] = [
    "name", "count", "level", "ratio", "active", "seenAt", "day", "mode", "home", "label",
    "location", "area", "site", "tags", "power",
];

const T0: &str = "2026-10-01T10:00:00Z";
const T1: &str = "2026-10-01T10:05:00Z";
const DATASET: &str = "urn:ngsi-ld:Dataset:a";
const SITE: &str = "urn:ngsi-ld:Site:1";

/// The Probe in the normalized form: one attribute per kind and value type.
fn probe(id: &str) -> Value {
    json!({
        "id": id, "type": "Probe",
        "name": {"type": "Property", "value": "Sonda Žilina"},
        "count": {"type": "Property", "value": 7},
        "level": {"type": "Property", "value": 21.375, "unitCode": "CEL", "observedAt": T0},
        "ratio": {"type": "Property", "value": 0.125},
        "active": {"type": "Property", "value": true},
        "seenAt": {"type": "Property", "value": {"@type": "DateTime", "@value": T0}},
        "day": {"type": "Property", "value": {"@type": "Date", "@value": "2026-10-01"}},
        "mode": {"type": "Property", "value": "auto"},
        "home": {"type": "Property", "value": "https://example.org/home"},
        "label": {"type": "LanguageProperty", "languageMap": {"sk": "sonda", "en": "probe"}},
        "location": {"type": "GeoProperty", "value": point()},
        "area": {"type": "GeoProperty", "value": polygon()},
        "site": {"type": "Relationship", "object": SITE},
        "tags": {"type": "ListProperty", "valueList": ["a", "b"]},
        "power": {"type": "Property", "value": 3.5, "unitCode": "KWT", "datasetId": DATASET}
    })
}

fn point() -> Value {
    json!({"type": "Point", "coordinates": [18.75, 49.25]})
}

fn polygon() -> Value {
    json!({"type": "Polygon", "coordinates": [[[18.0, 49.0], [19.0, 49.0], [19.0, 50.0], [18.0, 50.0], [18.0, 49.0]]]})
}

/// An Entity as every cell compares it: per attribute its value, and `{attr}.{member}` for the
/// kind (`type`), `unitCode`, `observedAt` and `datasetId` where the reader carries them.
type Canon = BTreeMap<String, Value>;

/// The Probe as [`Canon`]: what a reader that carries everything answers.
fn expected() -> Canon {
    let mut c = Canon::new();
    let p = probe("x");
    for name in SHOWN {
        normalized_attr(name, &p[name], &mut c);
    }
    c
}

/// A value in the form cells compare: a typed JSON-LD literal is its `@value`, an instant has
/// no zero milliseconds (STA writes `.000Z`).
fn canon(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.contains_key("@value") => canon(&m["@value"]),
        Value::Object(m) if m.len() == 1 && m.contains_key("languageMap") => {
            m["languageMap"].clone()
        }
        Value::String(s) => Value::String(s.replace(".000Z", "Z")),
        other => other.clone(),
    }
}

/// One normalized attribute (or its instances) into `out`.
fn normalized_attr(name: &str, attr: &Value, out: &mut Canon) {
    let instances = match attr {
        Value::Array(a) => a.clone(),
        one => vec![one.clone()],
    };
    for (n, i) in instances.iter().enumerate() {
        // a second instance is its own key, so an instance a write should have dropped is red
        let key = if n == 0 {
            name.to_owned()
        } else {
            format!("{name}#{n}")
        };
        let kind = i.get("type").and_then(Value::as_str).unwrap_or_default();
        let value = match kind {
            "Relationship" => i.get("object"),
            "LanguageProperty" => i.get("languageMap"),
            "ListProperty" => i.get("valueList"),
            _ => i.get("value"),
        };
        if let Some(v) = value {
            out.insert(key.clone(), canon(v));
        }
        out.insert(format!("{key}.type"), json!(kind));
        for meta in ["unitCode", "observedAt", "datasetId"] {
            if let Some(m) = i.get(meta) {
                out.insert(format!("{key}.{meta}"), canon(m));
            }
        }
    }
}

/// A normalized Entity as [`Canon`]; `None` for no Entity.
fn from_normalized(e: &Value) -> Option<Canon> {
    let m = e.as_object()?;
    let mut c = Canon::new();
    for (name, attr) in m {
        if !matches!(
            name.as_str(),
            "id" | "type" | "@context" | "createdAt" | "modifiedAt"
        ) {
            normalized_attr(name, attr, &mut c);
        }
    }
    Some(c)
}

/// Plain members (OData row, OGC properties, STA Thing properties) as [`Canon`]: `null` and
/// annotations dropped, a Relationship expanded to its target's id.
fn from_plain<'a>(members: impl Iterator<Item = (&'a String, &'a Value)>, c: &mut Canon) {
    for (k, v) in members {
        if k.contains('@') || v.is_null() || matches!(k.as_str(), "id" | "_types" | "type") {
            continue;
        }
        let v = v.get("id").filter(|_| k == "site").unwrap_or(v);
        c.insert(k.clone(), canon(v));
    }
}

/// A writer: how it puts the Probe `id` into the view, and what of it it cannot express.
#[derive(Clone, Copy, PartialEq)]
enum Writer {
    Create,
    Batch,
    Merge,
    Replace,
    TemporalAppend,
    OData,
    Ogc,
}

const WRITERS: [Writer; 7] = [
    Writer::Create,
    Writer::Batch,
    Writer::Merge,
    Writer::Replace,
    Writer::TemporalAppend,
    Writer::OData,
    Writer::Ogc,
];

/// Writers the view has no door for yet: a row of the table, never a silent gap.
const WRITERS_NOT_SERVED: [(&str, &str); 1] = [("STA create", "T-5910")];

impl Writer {
    fn name(self) -> &'static str {
        match self {
            Writer::Create => "NGSI-LD create",
            Writer::Batch => "NGSI-LD batch",
            Writer::Merge => "NGSI-LD merge",
            Writer::Replace => "NGSI-LD replace",
            Writer::TemporalAppend => "NGSI-LD temporal",
            Writer::OData => "OData POST",
            Writer::Ogc => "OGC Part 4",
        }
    }

    fn id(self) -> String {
        let slug = self.name().replace(' ', "-").replace("NGSI-LD", "ngsi");
        format!("urn:ngsi-ld:Probe:{slug}")
    }

    /// The members of [`Canon`] this writer cannot write, and why.
    fn unwritten(self) -> &'static [(&'static str, &'static str)] {
        const PLAIN: &[(&str, &str)] = &[
            ("unitCode", "a plain member carries no unitCode"),
            ("observedAt", "a plain member carries no observedAt"),
            (
                "power",
                "a plain member writes the default instance, no datasetId",
            ),
        ];
        const ODATA: &[(&str, &str)] = &[
            ("unitCode", "a plain member carries no unitCode"),
            ("observedAt", "a plain member carries no observedAt"),
            (
                "power",
                "a plain member writes the default instance, no datasetId",
            ),
            (
                "label",
                "a LanguageProperty is ReadOnly in OData (one language's text per read)",
            ),
        ];
        match self {
            Writer::OData => ODATA,
            Writer::Ogc => PLAIN,
            _ => &[],
        }
    }

    /// The Probe written through this writer; the status of each step.
    async fn write(self, app: &Router) -> Result<(), String> {
        let id = self.id();
        let full = probe(&id);
        let ngsi = format!("{VIEW}/ngsi-ld/v1");
        let steps: Vec<(&str, String, Value)> = match self {
            Writer::Create => vec![("POST", format!("{ngsi}/entities"), full)],
            Writer::Batch => vec![(
                "POST",
                format!("{ngsi}/entityOperations/create"),
                json!([full]),
            )],
            Writer::Merge => {
                let mut fragment = full.clone();
                if let Some(m) = fragment.as_object_mut() {
                    m.remove("id");
                    m.remove("type");
                }
                vec![
                    (
                        "POST",
                        format!("{ngsi}/entities"),
                        json!({"id": id, "type": "Probe",
                               "name": {"type": "Property", "value": "before"},
                               "count": {"type": "Property", "value": 1}}),
                    ),
                    ("PATCH", format!("{ngsi}/entities/{}", enc(&id)), fragment),
                ]
            }
            Writer::Replace => vec![
                (
                    "POST",
                    format!("{ngsi}/entities"),
                    json!({"id": id, "type": "Probe",
                           "mode": {"type": "Property", "value": "manual"},
                           "count": {"type": "Property", "value": 1, "datasetId": "urn:ngsi-ld:Dataset:old"}}),
                ),
                ("PUT", format!("{ngsi}/entities/{}", enc(&id)), full),
            ],
            Writer::TemporalAppend => vec![
                ("POST", format!("{ngsi}/entities"), full),
                (
                    "POST",
                    // a view serves no temporal write (ADR-0023, antares-views SERVED): the root
                    format!("/ngsi-ld/v1/temporal/entities/{}/attrs", enc(&id)),
                    json!({"level": [{"type": "Property", "value": 22.5, "unitCode": "CEL", "observedAt": T1}]}),
                ),
            ],
            Writer::OData => vec![(
                "POST",
                format!("{VIEW}/odata/v4/Probe"),
                json!({"id": id, "name": "Sonda Žilina", "count": 7, "level": 21.375,
                       "ratio": 0.125, "active": true, "seenAt": T0, "day": "2026-10-01",
                       "mode": "auto", "home": "https://example.org/home",
                       "location": point(), "area": polygon(), "tags": ["a", "b"],
                       "site@odata.bind": format!("Site('{SITE}')")}),
            )],
            Writer::Ogc => vec![(
                "POST",
                format!("{VIEW}/ogc/features/collections/Probe/items"),
                json!({"type": "Feature", "id": id, "geometry": point(),
                       "properties": {"name": "Sonda Žilina", "count": 7, "level": 21.375,
                       "ratio": 0.125, "active": true, "seenAt": T0, "day": "2026-10-01",
                       "mode": "auto", "home": "https://example.org/home",
                       "label": {"sk": "sonda", "en": "probe"},
                       "area": polygon(), "tags": ["a", "b"], "site": SITE}}),
            )],
        };
        for (method, uri, body) in steps {
            let (status, answer) = send(app, method, &uri, Some(&body)).await;
            if !status.is_success() {
                return Err(format!("{method} {uri}: {status} {answer}"));
            }
        }
        // the hidden attribute, through the Tenant root: every reader must keep it out
        let secret = json!({"secret": {"type": "Property", "value": "hidden"}});
        let uri = format!("/ngsi-ld/v1/entities/{}/attrs", enc(&id));
        let (status, answer) = send(app, "POST", &uri, Some(&secret)).await;
        if status.is_success() {
            Ok(())
        } else {
            Err(format!("secret: {status} {answer}"))
        }
    }
}

/// A reader: a surface of the view answering the Probe, and what it does not carry.
#[derive(Clone, Copy)]
enum Reader {
    Ngsi,
    OData,
    Ogc,
    Sta,
    Mcp,
    #[cfg(feature = "geojson")]
    GeoJson,
    #[cfg(feature = "tabular")]
    Csv,
    NgsiTemporal,
    OgcHistory,
    StaObservations,
    Notification,
    WebSocket,
}

const READERS: &[Reader] = &[
    Reader::Ngsi,
    Reader::OData,
    Reader::Ogc,
    Reader::Sta,
    Reader::Mcp,
    #[cfg(feature = "geojson")]
    Reader::GeoJson,
    #[cfg(feature = "tabular")]
    Reader::Csv,
    Reader::NgsiTemporal,
    Reader::OgcHistory,
    Reader::StaObservations,
    Reader::Notification,
    Reader::WebSocket,
];

/// Readers the view has no door for yet.
const READERS_NOT_SERVED: [(&str, &str); 2] = [("OData history", "T-5908"), ("STA MQTT", "T-5911")];

/// What a plain reader cannot carry.
const PLAIN_UNSERVED: &[(&str, &str)] = &[
    ("type", "a plain member names no NGSI-LD kind"),
    ("unitCode", "a plain member carries no unitCode"),
    ("observedAt", "a plain member carries no observedAt"),
    ("datasetId", "a plain member carries no datasetId"),
];

impl Reader {
    fn name(self) -> &'static str {
        match self {
            Reader::Ngsi => "NGSI-LD",
            Reader::OData => "OData",
            Reader::Ogc => "OGC",
            Reader::Sta => "STA",
            Reader::Mcp => "MCP",
            #[cfg(feature = "geojson")]
            Reader::GeoJson => "GeoJSON",
            #[cfg(feature = "tabular")]
            Reader::Csv => "CSV",
            Reader::NgsiTemporal => "NGSI-LD temporal",
            Reader::OgcHistory => "OGC history",
            Reader::StaObservations => "STA Observations",
            Reader::Notification => "notification",
            Reader::WebSocket => "WebSocket",
        }
    }

    /// The members this reader does not carry, by attribute or by `.{member}` suffix, and why.
    fn unserved(self) -> Vec<(&'static str, &'static str)> {
        let plain = match self {
            Reader::OData | Reader::Ogc | Reader::Sta => true,
            #[cfg(feature = "geojson")]
            Reader::GeoJson => true,
            #[cfg(feature = "tabular")]
            Reader::Csv => true,
            _ => false,
        };
        let mut out = if plain {
            PLAIN_UNSERVED.to_vec()
        } else {
            Vec::new()
        };
        match self {
            Reader::Ogc => out.push(("label", "a LanguageProperty is one language's text (lang)")),
            Reader::OData => out.extend([
                (
                    "label",
                    "a LanguageProperty is one language's text (ngsild.lang)",
                ),
                (
                    "power",
                    "a property reads its default instance; this one has a datasetId",
                ),
            ]),
            Reader::Sta => {
                // the measurements carry their metadata on the Datastream and Observation
                out.retain(|(m, _)| !matches!(*m, "unitCode" | "observedAt" | "datasetId"));
            }
            _ => {}
        }
        out
    }

    fn temporal(self) -> bool {
        matches!(
            self,
            Reader::NgsiTemporal | Reader::OgcHistory | Reader::StaObservations
        )
    }
}

/// The streams the Probes were notified on: the HTTP notifications and the WebSocket frames,
/// each a normalized Entity, in arrival order.
#[derive(Default)]
struct Streams {
    notified: Vec<Value>,
    socket: Vec<Value>,
}

/// The newest Entity `id` a stream delivered.
fn last_of(stream: &[Value], id: &str) -> Option<Value> {
    stream
        .iter()
        .rev()
        .find(|e| e.get("id").and_then(Value::as_str) == Some(id))
        .cloned()
}

/// The Entities in a notification body.
fn entities_of(body: &Value) -> Vec<Value> {
    body.get("data")
        .or_else(|| body.pointer("/body/data"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The history of `level` as `(instant, value)` pairs, oldest first, plus its `unitCode` where
/// the reader carries one.
type History = (Vec<(String, Value)>, Option<Value>);

async fn read(app: &Router, reader: Reader, id: &str, streams: &Streams) -> Result<Canon, String> {
    let ok = |uri: &str, (s, b): (StatusCode, Value)| {
        if s == StatusCode::OK {
            Ok(b)
        } else {
            Err(format!("{uri}: {s} {b}"))
        }
    };
    let e = enc(id);
    match reader {
        Reader::Ngsi => {
            let uri = format!("{VIEW}/ngsi-ld/v1/entities/{e}");
            let body = ok(&uri, send(app, "GET", &uri, None).await)?;
            from_normalized(&body).ok_or_else(|| format!("{uri}: {body}"))
        }
        Reader::Mcp => {
            let answer = mcp(app, "get_entity", json!({"id": id})).await;
            answer
                .pointer("/result/structuredContent/entity")
                .and_then(from_normalized)
                .ok_or_else(|| format!("get_entity: {answer}"))
        }
        Reader::OData => {
            let uri = format!("{VIEW}/odata/v4/Probe('{e}')?$expand=site($select=id)");
            let row = ok(&uri, send(app, "GET", &uri, None).await)?;
            let mut c = Canon::new();
            from_plain(row.as_object().into_iter().flatten(), &mut c);
            Ok(c)
        }
        Reader::Ogc => {
            let uri = format!("{VIEW}/ogc/features/collections/Probe/items/{e}");
            let f = ok(&uri, send(app, "GET", &uri, None).await)?;
            Ok(feature(&f))
        }
        #[cfg(feature = "geojson")]
        Reader::GeoJson => {
            let uri = format!("{VIEW}/geojson/v1/Probe?limit=100");
            let fc = ok(&uri, send(app, "GET", &uri, None).await)?;
            fc.get("features")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|f| f.get("id").and_then(Value::as_str) == Some(id))
                .map(feature)
                .ok_or_else(|| format!("{uri}: no Feature {id}"))
        }
        #[cfg(feature = "tabular")]
        Reader::Csv => csv(app, id).await,
        Reader::Sta => sta(app, id).await,
        Reader::NgsiTemporal | Reader::OgcHistory | Reader::StaObservations => {
            let (series, unit) = history(app, reader, id).await?;
            let mut c = Canon::new();
            for (n, (at, v)) in series.into_iter().enumerate() {
                c.insert(format!("level@{n}"), v);
                c.insert(format!("level@{n}.at"), json!(at));
            }
            if let Some(u) = unit {
                c.insert("level.unitCode".into(), u);
            }
            Ok(c)
        }
        Reader::Notification => last_of(&streams.notified, id)
            .as_ref()
            .and_then(from_normalized)
            .ok_or_else(|| "no notification".to_owned()),
        Reader::WebSocket => last_of(&streams.socket, id)
            .as_ref()
            .and_then(from_normalized)
            .ok_or_else(|| "no WebSocket frame".to_owned()),
    }
}

/// A `GeoJSON` Feature as [`Canon`]: its geometry is `location`.
fn feature(f: &Value) -> Canon {
    let mut c = Canon::new();
    if let Some(g) = f.get("geometry").filter(|g| !g.is_null()) {
        c.insert("location".into(), g.clone());
    }
    from_plain(
        f.get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten(),
        &mut c,
    );
    c
}

/// OGC `f=csv` of the Probe as [`Canon`]: each cell parsed as JSON where it is JSON.
#[cfg(feature = "tabular")]
async fn csv(app: &Router, id: &str) -> Result<Canon, String> {
    let uri = format!(
        "{VIEW}/ogc/features/collections/Probe/items?f=csv&filter=id%3D%27{}%27",
        enc(id)
    );
    let (status, body) = send(app, "GET", &uri, None).await;
    let text = body.as_str().unwrap_or_default();
    if status != StatusCode::OK {
        return Err(format!("{uri}: {status} {text}"));
    }
    let mut lines = text.split("\r\n");
    let head = lines.next().unwrap_or_default();
    let row = lines.next().unwrap_or_default();
    let mut c = Canon::new();
    for (k, v) in split_csv(head).into_iter().zip(split_csv(row)) {
        if v.is_empty() || matches!(k.as_str(), "id" | "type") {
            continue;
        }
        let k = if k == "geometry" {
            "location".to_owned()
        } else {
            k
        };
        let v = serde_json::from_str(&v).unwrap_or(Value::String(v));
        c.insert(k, canon(&v));
    }
    Ok(c)
}

/// One CSV line's fields (RFC 4180 quotes).
#[cfg(feature = "tabular")]
fn split_csv(line: &str) -> Vec<String> {
    let (mut out, mut field, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match (ch, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => out.push(std::mem::take(&mut field)),
            (ch, _) => field.push(ch),
        }
    }
    out.push(field);
    out
}

/// STA as [`Canon`]: the Thing's name and properties, each Datastream's current Observation
/// (its unit `unitCode`, its `phenomenonTime` `observedAt` unless it is the modification time,
/// the `#` part of its id `datasetId`), and the Location.
async fn sta(app: &Router, id: &str) -> Result<Canon, String> {
    let sta = format!("{VIEW}/sta/v1.1");
    let e = enc(id);
    let uri = format!("{sta}/Things('{e}')?$expand=Locations,Datastreams/Observations");
    let (status, thing) = send(app, "GET", &uri, None).await;
    if status != StatusCode::OK {
        return Err(format!("{uri}: {status} {thing}"));
    }
    let mut c = Canon::new();
    from_plain(
        thing
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten(),
        &mut c,
    );
    for l in thing
        .get("Locations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(g) = l.get("location") {
            c.insert(text(l, "name"), g.clone());
        }
    }
    for d in thing
        .get("Datastreams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = text(d, "name");
        let obs = d.pointer("/Observations/0").cloned().unwrap_or(Value::Null);
        c.insert(
            name.clone(),
            obs.get("result").cloned().unwrap_or(Value::Null),
        );
        if let Some(unit) = d
            .pointer("/unitOfMeasurement/name")
            .filter(|u| !u.is_null())
        {
            c.insert(format!("{name}.unitCode"), unit.clone());
        }
        if obs.pointer("/parameters/timeSource").is_none() {
            if let Some(t) = obs.get("phenomenonTime") {
                c.insert(format!("{name}.observedAt"), canon(t));
            }
        }
        if let Some((_, dataset)) = text(d, "@iot.id").split_once('#') {
            c.insert(format!("{name}.datasetId"), json!(dataset));
        }
    }
    Ok(c)
}

/// The history of the Probe's `level` through a temporal reader.
async fn history(app: &Router, reader: Reader, id: &str) -> Result<History, String> {
    let e = enc(id);
    let get = |uri: String| async move {
        let (s, b) = send(app, "GET", &uri, None).await;
        if s == StatusCode::OK {
            Ok(b)
        } else {
            Err(format!("{uri}: {s} {b}"))
        }
    };
    let mut series = Vec::new();
    let mut unit = None;
    match reader {
        Reader::NgsiTemporal => {
            let uri = format!(
                "{VIEW}/ngsi-ld/v1/temporal/entities/{e}?attrs=level&timerel=after&timeAt=2026-01-01T00:00:00Z"
            );
            let (s, t) = send(app, "GET", &uri, None).await;
            // no instance in the window: Postgres drops the Entity (11.3.2.4, 404), the memory
            // store answers it without instances (200); both read as an empty history here
            let none = s == StatusCode::NOT_FOUND
                && text(&t, "detail").ends_with("has none of the requested attributes");
            if s != StatusCode::OK && !none {
                return Err(format!("{uri}: {s} {t}"));
            }
            for i in t
                .get("level")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                series.push((text(i, "observedAt"), i["value"].clone()));
                unit = i.get("unitCode").cloned().or(unit);
            }
        }
        Reader::OgcHistory => {
            let fc = get(format!(
                "{VIEW}/ogc/features/collections/Probe-history/items?limit=100"
            ))
            .await?;
            for f in fc
                .get("features")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if f.pointer("/properties/entityId").and_then(Value::as_str) == Some(id)
                    || text(f, "id").starts_with(&format!("{id}/"))
                {
                    if let Some(v) = f.pointer("/properties/level") {
                        let at = f
                            .pointer("/properties/validFrom")
                            .map(canon)
                            .unwrap_or(Value::Null);
                        series.push((at.as_str().unwrap_or_default().to_owned(), v.clone()));
                    }
                }
            }
        }
        Reader::StaObservations => {
            let sta = format!("{VIEW}/sta/v1.1");
            let ds = format!("{sta}/Datastreams('{e}%2Flevel')");
            let d = get(ds.clone()).await?;
            unit = d
                .pointer("/unitOfMeasurement/name")
                .filter(|u| !u.is_null())
                .cloned();
            let o = get(format!("{ds}/Observations?$orderby=phenomenonTime%20desc")).await?;
            for i in o
                .get("value")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                // an instance without observedAt is observed at its modification: no fixed instant
                let at = match i.pointer("/parameters/timeSource") {
                    Some(source) => source.clone(),
                    None => canon(&i["phenomenonTime"]),
                };
                series.push((
                    at.as_str().unwrap_or_default().to_owned(),
                    i["result"].clone(),
                ));
            }
        }
        _ => {}
    }
    series.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((series, unit))
}

/// What `reader` must answer for the Probe `writer` wrote; the notes of what is left out.
fn want(writer: Writer, reader: Reader) -> (Canon, Vec<String>) {
    let mut notes = Vec::new();
    let mut c = if reader.temporal() {
        let mut c = Canon::new();
        let mut series = vec![(T0, json!(21.375))];
        if writer == Writer::TemporalAppend {
            series.push((T1, json!(22.5)));
        }
        for (n, (at, v)) in series.into_iter().enumerate() {
            c.insert(format!("level@{n}"), v);
            c.insert(format!("level@{n}.at"), json!(at));
        }
        if !matches!(reader, Reader::OgcHistory) {
            c.insert("level.unitCode".into(), json!("CEL"));
        } else {
            notes.push("unitCode: a history Feature carries no unitCode".to_owned());
        }
        if writer.unwritten().iter().any(|(m, _)| *m == "observedAt") {
            c.clear();
            if matches!(reader, Reader::StaObservations) {
                // STA observes an instance without observedAt at its modifiedAt
                c.insert("level@0".into(), json!(21.375));
                c.insert("level@0.at".into(), json!("modifiedAt"));
            } else {
                notes.push("no observedAt written, so no history instance".to_owned());
            }
        }
        c
    } else {
        expected()
    };
    let drop = |c: &mut Canon, key: &str, why: &str, notes: &mut Vec<String>, who: &str| {
        let before = c.len();
        c.retain(|k, _| !covers(k, key));
        if c.len() != before {
            notes.push(format!("{key}: {who} {why}"));
        }
    };
    for (key, why) in writer.unwritten() {
        drop(&mut c, key, why, &mut notes, "not written:");
    }
    for (key, why) in reader.unserved() {
        drop(&mut c, key, why, &mut notes, "not served:");
    }
    (c, notes)
}

/// Whether the [`Canon`] key `k` belongs to `key`: the attribute itself, one of its instances, or
/// the member `key` of any attribute.
fn covers(k: &str, key: &str) -> bool {
    let attr = k.split(['.', '#', '@']).next().unwrap_or_default();
    attr == key || k.ends_with(&format!(".{key}"))
}

/// The cells' verdicts: `ok`, or the differences, keyed by writer and reader name.
struct Matrix {
    store: &'static str,
    cells: BTreeMap<(usize, usize), Result<(), String>>,
    notes: Vec<String>,
}

impl Matrix {
    fn table(&self) -> String {
        let mut out = format!("store {}\n| writer \\ reader |", self.store);
        for r in READERS {
            let _ = write!(out, " {} |", r.name());
        }
        for (r, task) in READERS_NOT_SERVED {
            let _ = write!(out, " {r} |");
            let _ = task;
        }
        out.push_str("\n|---|");
        for _ in 0..READERS.len() + READERS_NOT_SERVED.len() {
            out.push_str("---|");
        }
        for (w, writer) in WRITERS.iter().enumerate() {
            let _ = write!(out, "\n| {} |", writer.name());
            for r in 0..READERS.len() {
                let cell = match self.cells.get(&(w, r)) {
                    Some(Ok(())) => "ok",
                    Some(Err(_)) => "RED",
                    None => "-",
                };
                let _ = write!(out, " {cell} |");
            }
            for (_, task) in READERS_NOT_SERVED {
                let _ = write!(out, " not served ({task}) |");
            }
        }
        for (writer, task) in WRITERS_NOT_SERVED {
            let _ = write!(out, "\n| {writer} |");
            for _ in 0..READERS.len() + READERS_NOT_SERVED.len() {
                let _ = write!(out, " not served ({task}) |");
            }
        }
        for ((w, r), cell) in &self.cells {
            if let Err(e) = cell {
                let (w, r) = (WRITERS[*w].name(), READERS[*r].name());
                let _ = write!(out, "\nRED {w} -> {r}: {e}");
            }
        }
        for n in &self.notes {
            let _ = write!(out, "\n{n}");
        }
        out
    }

    fn red(&self) -> bool {
        self.cells.values().any(Result::is_err)
    }
}

/// Every writer, then every reader of every Probe; the matrix.
async fn run(app: &Router, port: u16, store: &'static str) -> Matrix {
    // the Site the Relationship and the OData bind name
    let site = json!({"id": SITE, "type": "Site", "name": {"type": "Property", "value": "Žilina"}});
    let (s, b) = send(
        app,
        "POST",
        "/ngsi-ld/v1/entityOperations/upsert",
        Some(&json!([site])),
    )
    .await;
    assert!(s.is_success(), "site: {s} {b}");

    // the streams, open before anything is written
    let notified = Arc::new(Mutex::new(Vec::new()));
    let sink = notified.clone();
    let receiver = Router::new().route(
        "/n",
        axum::routing::post(move |body: axum::Json<Value>| {
            let sink = sink.clone();
            async move {
                if let Ok(mut n) = sink.lock() {
                    n.extend(entities_of(&body.0));
                }
                StatusCode::NO_CONTENT
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let at = listener.local_addr().expect("addr").port();
    tokio::spawn(async move { axum::serve(listener, receiver).await });
    let subscription = json!({"type": "Subscription", "entities": [{"type": "Probe"}],
        "notification": {"endpoint": {"uri": format!("http://localhost:{at}/n"), "accept": "application/json"}}});
    let (s, b) = send(
        app,
        "POST",
        &format!("{VIEW}/ngsi-ld/v1/subscriptions"),
        Some(&subscription),
    )
    .await;
    assert!(s.is_success(), "subscription: {s} {b}");
    let mut socket = tokio::task::spawn_blocking(move || {
        let mut s = Socket::open(
            port,
            &format!("{VIEW}/ws"),
            &[("NGSILD-Tenant".to_owned(), TENANT.to_owned())],
            Duration::from_secs(2),
        )
        .expect("the socket opens");
        let welcome = s.next().unwrap_or_default();
        assert_eq!(welcome["metadata"]["x-op"], "x-welcome", "{welcome}");
        s.send(
            &json!({"metadata": {"x-op": "x-subscribe", "x-id": "matrix"},
                       "body": {"type": "Subscription", "entities": [{"type": "Probe"}]}}),
        )
        .expect("x-subscribe");
        let made = s.next().unwrap_or_default();
        assert_eq!(made["metadata"]["x-status"], 201, "{made}");
        s
    })
    .await
    .expect("the socket");

    let mut matrix = Matrix {
        store,
        cells: BTreeMap::new(),
        notes: Vec::new(),
    };
    let mut written = Vec::new();
    for (w, writer) in WRITERS.iter().enumerate() {
        match writer.write(app).await {
            Ok(()) => written.push(w),
            Err(e) => {
                for r in 0..READERS.len() {
                    matrix
                        .cells
                        .insert((w, r), Err(format!("the write failed: {e}")));
                }
            }
        }
    }

    // the streams: every frame until the socket is quiet, the notifications until they stop
    let frames = tokio::task::spawn_blocking(move || {
        let mut frames = Vec::new();
        while let Some(f) = socket.next() {
            frames.extend(entities_of(&f));
        }
        frames
    });
    let mut seen = usize::MAX;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = notified.lock().map(|n| n.len()).unwrap_or_default();
        if now == seen && now > 0 {
            break;
        }
        seen = now;
    }
    let streams = Streams {
        notified: notified.lock().map(|n| n.clone()).unwrap_or_default(),
        socket: frames.await.unwrap_or_default(),
    };

    for w in written {
        let writer = WRITERS[w];
        for (r, reader) in READERS.iter().enumerate() {
            let (want, notes) = want(writer, *reader);
            for n in notes {
                let line = format!("{} -> {}: {n}", writer.name(), reader.name());
                matrix.notes.push(line);
            }
            let got = read(app, *reader, &writer.id(), &streams)
                .await
                .map(|mut got| {
                    for (key, _) in reader.unserved() {
                        got.retain(|k, _| !covers(k, key));
                    }
                    got
                });
            let cell = match got {
                Ok(got) if got.contains_key("secret") => {
                    Err(format!("the hidden attribute: {got:?}"))
                }
                Ok(got) if got == want => Ok(()),
                Ok(got) => Err(diff(&want, &got)),
                Err(e) => Err(e),
            };
            matrix.cells.insert((w, r), cell);
        }
    }
    matrix.notes.sort();
    matrix.notes.dedup();
    matrix
}

/// The members `want` and `got` disagree on.
fn diff(want: &Canon, got: &Canon) -> String {
    let mut out = String::new();
    for (k, v) in want {
        match got.get(k) {
            Some(g) if g == v => {}
            Some(g) => {
                let _ = write!(out, " {k}: want {v} got {g};");
            }
            None => {
                let _ = write!(out, " {k}: want {v} got nothing;");
            }
        }
    }
    for (k, g) in got {
        if !want.contains_key(k) {
            let _ = write!(out, " {k}: unexpected {g};");
        }
    }
    out
}

/// The `GeoJSON` surface where the build has it.
fn geojson() -> Option<Box<dyn antares_api::ApiSurface>> {
    #[cfg(feature = "geojson")]
    return Some(antares_ops::mount(antares_geojson::GeoJson));
    #[cfg(not(feature = "geojson"))]
    None
}

/// The broker over `state`'s store: every surface of a `full` build, the view `m` and the
/// schema; served on a port for the WebSocket.
async fn broker(mut state: AppState) -> (Router, u16, antares_ops::views::Reload) {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    state = state.with_sink(Box::new(antares_ws::Sink::new(antares_ws::Hub::process())));
    antares_api::wire(&mut state).await;
    let surfaces: Vec<Box<dyn antares_api::ApiSurface>> = vec![
        Box::new(antares_api::Admin),
        antares_ops::mount(antares_views::Views),
        antares_ops::mount(antares_odata::OData),
        antares_ops::mount(antares_ogc::Ogc),
        antares_ops::mount(antares_sta::Sta),
        antares_ops::mount(antares_mcp::Mcp),
        antares_ops::mount(antares_ws::Ws::process()),
        antares_ops::mount(antares_ws::ViewWs::process()),
    ]
    .into_iter()
    .chain(geojson())
    .collect();
    let state = state.with_surfaces(surfaces).expect("the surfaces mount");
    let reload = antares_ops::views::install(&state)
        .await
        .expect("the view table");
    let app = antares_api::router(state);
    let ops: Vec<&str> = antares_ops::OperationId::ALL
        .iter()
        .map(|op| op.id())
        .collect();
    let view = json!({"id": "m", "tenant": TENANT, "selector": ["Probe", "Site"],
        "rules": [{"types": ["Probe"], "attributes": SHOWN}, {"types": ["Site"], "attributes": ["name"]}],
        "operations": ops});
    let (s, b) = send(&app, "PUT", "/q/views/m", Some(&view)).await;
    assert!(s.is_success(), "view: {s} {b}");
    let schema: Value = serde_json::from_str(SCHEMA).expect("the schema");
    let (s, b) = send(&app, "PUT", &format!("/q/schemas/{TENANT}"), Some(&schema)).await;
    assert!(s.is_success(), "schema: {s} {b}");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await });
    (app, port, reload)
}

async fn send(app: &Router, method: &str, uri: &str, body: Option<&Value>) -> (StatusCode, Value) {
    let text = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Host", "localhost")
        .header("NGSILD-Tenant", TENANT);
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", text.len());
    }
    let Ok(res) = app
        .clone()
        .call(req.body(Body::from(text)).expect("request"))
        .await;
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, body)
}

/// One MCP `tools/call` on the view; the JSON-RPC answer.
async fn mcp(app: &Router, tool: &str, args: Value) -> Value {
    const MODERN: &str = "2026-07-28";
    let params = json!({"name": tool, "arguments": args, "_meta": {
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": {"name": "matrix", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {}
    }});
    let body =
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params}).to_string();
    let req = Request::post(format!("{VIEW}/mcp"))
        .header("Host", "localhost")
        .header("NGSILD-Tenant", TENANT)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", MODERN)
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", tool)
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    let Ok(res) = app.clone().call(req).await;
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn enc(s: &str) -> String {
    s.replace(':', "%3A").replace('/', "%2F")
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// `t` as a 4.6.3 DateTime, UTC, whole seconds (days to civil date: H. Hinnant's algorithm).
fn stamp(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let z = days + 719_468;
    let (era, doe) = (z / 146_097, z % 146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    let (h, mi, sec) = (rem / 3600, rem % 3600 / 60, rem % 60);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{sec:02}Z")
}

/// The Probe whose `level` expires: the T-5933 rows.
const EXPIRING: &str = "urn:ngsi-ld:Probe:expiry";

/// Each expiry row's reader and whether it shows the expiring `level` (`21.375`). `earlier` opens
/// the expiresAt windows: `after` it holds the instance until it expires, `before` it never does.
async fn expiry_reads(app: &Router, earlier: &str) -> Vec<(&'static str, bool)> {
    let e = enc(EXPIRING);
    let shows = |b: &Value| b.to_string().contains("21.375");
    let get = |uri: String| async move { send(app, "GET", &uri, None).await };
    let temporal = |q: &str| format!("{VIEW}/ngsi-ld/v1/temporal/entities/{e}?attrs=level&{q}");
    let window = |rel: &str| format!("timeproperty=expiresAt&timerel={rel}&timeAt={earlier}");
    let mut rows = Vec::new();
    let (_, b) = get(format!("{VIEW}/ngsi-ld/v1/entities/{e}")).await;
    rows.push(("NGSI-LD current", shows(&b)));
    let (_, b) = get(temporal("timerel=after&timeAt=2026-01-01T00:00:00Z")).await;
    rows.push(("NGSI-LD temporal observedAt", shows(&b)));
    let (_, b) = get(temporal(&window("after"))).await;
    rows.push(("NGSI-LD temporal expiresAt after", shows(&b)));
    let (_, b) = get(temporal(&window("before"))).await;
    rows.push(("NGSI-LD temporal expiresAt before", shows(&b)));
    let (_, b) = get(format!(
        "{VIEW}/ngsi-ld/v1/temporal/entities?type=Probe&attrs=level&{}",
        window("after")
    ))
    .await;
    rows.push(("NGSI-LD temporal query expiresAt after", shows(&b)));
    let args = json!({"id": EXPIRING, "attrs": ["level"], "timerel": "after",
        "timeAt": earlier, "timeproperty": "expiresAt"});
    let b = mcp(app, "retrieve_temporal", args).await;
    rows.push(("MCP retrieve_temporal expiresAt after", shows(&b["result"])));
    let (_, b) = get(format!(
        "{VIEW}/ogc/features/collections/Probe-history/items?limit=100"
    ))
    .await;
    let mine = b
        .get("features")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|f| f.to_string().contains(EXPIRING))
        .any(shows);
    rows.push(("OGC history", mine));
    let (_, b) = get(format!(
        "{VIEW}/sta/v1.1/Datastreams('{e}%2Flevel')/Observations"
    ))
    .await;
    rows.push(("STA Observations", shows(&b)));
    rows
}

/// TS 104 175 5.2.4: an instance "shall become invalid" at its expiresAt, whenever the deletion
/// runs. `level` expires 2 s after its write; each reader shows it before (the expiresAt `before`
/// window never does) and none after, with no reap run in between. OData history is not served
/// (T-5908). The failures, empty when every row holds, and the printed table.
async fn expiry_rows(app: &Router) -> (Vec<String>, String) {
    let now = std::time::SystemTime::now();
    let soon = stamp(now + Duration::from_secs(2));
    let earlier = stamp(now - Duration::from_secs(60));
    let probe = json!({"id": EXPIRING, "type": "Probe",
        "count": {"type": "Property", "value": 7, "observedAt": T0},
        "level": {"type": "Property", "value": 21.375, "observedAt": T0, "expiresAt": soon}});
    let (s, b) = send(
        app,
        "POST",
        &format!("{VIEW}/ngsi-ld/v1/entities"),
        Some(&probe),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let before = expiry_reads(app, &earlier).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let after = expiry_reads(app, &earlier).await;
    let uri = format!("/ngsi-ld/v1/entities/{}", enc(EXPIRING));
    let _ = send(app, "DELETE", &uri, None).await;
    let mut red = Vec::new();
    let mut table = String::from("| reader | before expiry | after expiry |\n|---|---|---|\n");
    for ((reader, was), (_, is)) in before.into_iter().zip(after) {
        let _ = writeln!(table, "| {reader} | {was} | {is} |");
        if was != !reader.ends_with(" before") {
            red.push(format!("{reader}: before expiry shows level = {was}"));
        }
        if is {
            red.push(format!("{reader}: still shows level after expiry"));
        }
    }
    (red, table)
}

/// The expiry rows over the built-in store.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_instance_leaves_every_reader_before_the_reap() {
    let (app, _port, _reload) = broker(AppState::new("matrix".into())).await;
    let (red, table) = expiry_rows(&app).await;
    println!("{table}");
    assert!(red.is_empty(), "{red:?}\n{table}");
}

/// The matrix over the built-in store: every cell green, every gap named.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_probe_written_on_any_surface_reads_the_same_on_every_surface() {
    let (app, port, _reload) = broker(AppState::new("matrix".into())).await;
    let matrix = run(&app, port, "memory").await;
    let table = matrix.table();
    println!("{table}");
    assert!(!matrix.red(), "{table}");
}

/// The same matrix over Postgres (`ANTARES_TEST_DATABASE_URL`); the Probes deleted after.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_matrix_holds_over_postgres() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let tenant = antares_model::TenantId::new(TENANT).expect("tenant");
    antares_sql::store::pg::ensure_tenant(&pool, &tenant)
        .await
        .expect("tenant row");
    let store = Arc::new(antares_sql::store::any::AnyStore::Pg(
        antares_sql::store::any::PgBackend::new(pool),
    ));
    let (app, port, _reload) =
        broker(AppState::with_store("matrix".into(), store, "postgres")).await;
    let clean = |app: Router| async move {
        for w in WRITERS {
            let uri = format!("/ngsi-ld/v1/entities/{}", enc(&w.id()));
            let _ = send(&app, "DELETE", &uri, None).await;
        }
        let _ = send(
            &app,
            "DELETE",
            &format!("/ngsi-ld/v1/entities/{}", enc(SITE)),
            None,
        )
        .await;
    };
    clean(app.clone()).await;
    let matrix = run(&app, port, "postgres").await;
    clean(app.clone()).await;
    let table = matrix.table();
    println!("{table}");
    assert!(!matrix.red(), "{table}");
    let (red, table) = expiry_rows(&app).await;
    println!("{table}");
    assert!(red.is_empty(), "{red:?}\n{table}");
}

/// The comparison itself: a rounded float, a dropped unit and a leaked hidden attribute are
/// each a red cell, so a green matrix proves the values.
#[test]
fn the_comparison_sees_a_rounded_float_a_dropped_unit_and_the_hidden_attribute() {
    let (want, _) = want(Writer::Create, Reader::Ngsi);
    let mut rounded = want.clone();
    rounded.insert("level".into(), json!(21.38));
    assert!(diff(&want, &rounded).contains("level: want 21.375 got 21.38"));
    let mut unitless = want.clone();
    unitless.remove("level.unitCode");
    assert!(diff(&want, &unitless).contains("level.unitCode: want \"CEL\" got nothing"));
    let mut leaked = want.clone();
    leaked.insert("secret".into(), json!("hidden"));
    assert!(diff(&want, &leaked).contains("secret: unexpected"));
    assert_eq!(diff(&want, &want), "");
}

/// A plain member that holds a whole NGSI-LD attribute (`createdAt` and all) is no value.
#[test]
fn a_plain_reader_answering_a_normalized_member_is_red() {
    let mut c = Canon::new();
    let raw = json!({"tags": {"type": "ListProperty", "valueList": ["a", "b"], "createdAt": T0}});
    from_plain(raw.as_object().into_iter().flatten(), &mut c);
    assert_ne!(c.get("tags"), Some(&json!(["a", "b"])));
}
