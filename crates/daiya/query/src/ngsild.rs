#![forbid(unsafe_code)]
//! Built-in NGSI-LD datasource (R3, §3.2, ported from `NGP src/request.ts`, `frames.ts`,
//! `ngsildTypes.ts`, `units.ts`):
//! - Supports `queryType`: `entity`, `temporal` (including misspelled `timseries` and `timeseries`),
//!   `geo`, `nodegraph`.
//! - Keeps all target fields of `NGP src/types.ts:67-124` on import and exports them losslessly.
//! - Exchanges the caller's token for the Endpoint's audience slug via [`TokenExchanger`] (never
//!   uses a service token, R3, §4.2).

use crate::guard::{OutboundGuard, OutboundRequest};
use crate::{
    Datasource, DsError, DsFuture, EditorFieldHint, EditorSchema, Health, ImportOutcome, Query,
    QueryCtx, ResourceReq, ResourceResp, TokenExchanger,
};
use daiya_model::{FieldColumn, FieldType, Frame, FrameMeta, FrameSet};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const CORE_MEMBERS: &[&str] = &[
    "id",
    "type",
    "@context",
    "scope",
    "createdAt",
    "modifiedAt",
    "deletedAt",
    "expiresAt",
];

const AGGREGATION_METHODS: &[&str] = &[
    "totalCount",
    "distinctCount",
    "sum",
    "avg",
    "min",
    "max",
    "stddev",
    "sumsq",
];

/// Built-in NGSI-LD datasource reading through an Endpoint (R3, §3.2).
pub struct NgsiLdDatasource {
    pub uid: String,
    pub endpoint_url: String,
    pub endpoint_slug: Option<String>,
    /// Grafana's `oauthPassThru`, kept as stored. It changes no request: a caller's token is
    /// exchanged either way, and with none the read goes out without `Authorization`.
    pub oauth_pass_thru: bool,
    pub page_size: u32,
    pub context_url: Option<String>,
    pub tenant: Option<String>,
    pub guard: OutboundGuard,
    pub token_exchanger: Arc<dyn TokenExchanger>,
}

impl NgsiLdDatasource {
    #[must_use]
    pub fn new(
        uid: impl Into<String>,
        endpoint_url: impl Into<String>,
        oauth_pass_thru: bool,
        guard: OutboundGuard,
        token_exchanger: Arc<dyn TokenExchanger>,
    ) -> Self {
        let url_str = endpoint_url.into().trim_end_matches('/').to_owned();
        let slug = url_str
            .rsplit_once("/api/endpoint/")
            .map(|(_, s)| s.split('/').next().unwrap_or(s).to_owned())
            .filter(|s| !s.is_empty());
        Self {
            uid: uid.into(),
            endpoint_url: url_str,
            endpoint_slug: slug,
            oauth_pass_thru,
            page_size: 1000,
            context_url: None,
            tenant: None,
            guard,
            token_exchanger,
        }
    }

    fn base_ngsi_url(&self) -> String {
        if self.endpoint_url.ends_with("/ngsi-ld/v1") {
            self.endpoint_url.clone()
        } else {
            format!("{}/ngsi-ld/v1", self.endpoint_url)
        }
    }

    fn build_headers(&self, ctx: &QueryCtx) -> Result<Vec<(String, String)>, DsError> {
        let mut headers = vec![("Accept".to_owned(), "application/json".to_owned())];
        if let Some(ctx_url) = &self.context_url
            && !ctx_url.trim().is_empty()
        {
            headers.push((
                    "Link".to_owned(),
                    format!(
                        "<{}>; rel=\"http://www.w3.org/ns/json-ld#context\"; type=\"application/ld+json\"",
                        ctx_url.trim()
                    ),
                ));
        }
        if let Some(t) = &self.tenant
            && !t.trim().is_empty()
        {
            headers.push(("NGSILD-Tenant".to_owned(), t.trim().to_owned()));
        }

        // A caller's token leaves only exchanged for the Endpoint's audience. With no token the
        // read goes out anonymously, whether or not oauthPassThru is on (Grafana parity): the
        // gateway answers it by the Endpoint's audience.
        if let Some(caller_token) = &ctx.subject.bearer_token
            && !caller_token.is_empty()
        {
            let audience = self.endpoint_slug.as_deref().unwrap_or(self.uid.as_str());
            let exchanged = self
                .token_exchanger
                .exchange_for_audience(caller_token, audience)?;
            headers.push((
                "Authorization".to_owned(),
                format!("Bearer {}", exchanged.as_str()),
            ));
        }

        Ok(headers)
    }
}

/// Normalizes `queryType` (`NGP src/types.ts:queryTypeForValue`), accepting misspelled `timseries`.
#[must_use]
pub fn normalize_query_type(raw: Option<&str>) -> &'static str {
    match raw.unwrap_or("temporal").to_ascii_lowercase().as_str() {
        "entity" | "entities" => "entity",
        "temporal" | "timseries" | "timeseries" => "temporal",
        "geo" => "geo",
        "graph" | "nodegraph" | "node_graph" => "nodegraph",
        _ => "temporal",
    }
}

/// Maps UN/CEFACT unitCode to Grafana unit ID (`NGP src/units.ts`).
#[must_use]
pub fn grafana_unit(code: Option<&str>) -> Option<String> {
    let code = code?.trim();
    if code.is_empty() {
        return None;
    }
    let mapped = match code {
        "CEL" => "celsius",
        "FAH" => "fahrenheit",
        "KEL" => "kelvin",
        "P1" => "percent",
        "59" => "ppm",
        "61" => "conppb",
        "GQ" => "conμgm3",
        "GP" => "conmgm3",
        "MMT" => "lengthmm",
        "MTR" => "lengthm",
        "KMT" | "KTM" => "lengthkm",
        "DD" => "degree",
        "C26" => "ms",
        "SEC" => "s",
        "MIN" => "m",
        "HUR" => "h",
        "DAY" => "d",
        "MTS" => "velocityms",
        "KMH" => "velocitykmh",
        "KGM" => "masskg",
        "GRM" => "massg",
        "TNE" => "masst",
        "PAL" => "pressurepa",
        "A97" => "pressurehpa",
        "KPA" => "pressurekpa",
        "MBR" => "pressurembar",
        "BAR" => "pressurebar",
        "JOU" => "joule",
        "WHR" => "watth",
        "KWH" => "kwatth",
        "WTT" => "watt",
        "KWT" => "kwatt",
        "AMP" => "amp",
        "VLT" => "volt",
        "HTZ" => "hertz",
        "MTK" => "areaM2",
        "MTQ" => "m3",
        "LTR" => "litre",
        other => return Some(format!("suffix: {other}")),
    };
    Some(mapped.to_owned())
}

/// The target with the dashboard's variables put into its `customQuery`, as Grafana's
/// `templateSrv.replace` does for the plugin: `${name:regex}`, `${name}` and `$name` each become
/// the text `variables` holds for `name` (already formatted by the caller, a multi-value pick as an
/// escaped `(a|b)`). A name the dashboard does not hold is left as written.
#[must_use]
pub fn with_variables(model: &Value, variables: &BTreeMap<String, String>) -> Value {
    let Some(text) = model.get("customQuery").and_then(Value::as_str) else {
        return model.clone();
    };
    if variables.is_empty() || !text.contains('$') {
        return model.clone();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, used) = if let Some(braced) = after.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => {
                    let inner = &braced[..end];
                    let name = inner.split_once(':').map_or(inner, |(name, _)| name);
                    (name, end + 2)
                }
                None => ("", 0),
            }
        } else {
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            (&after[..end], end)
        };
        match variables.get(name).filter(|_| used > 0) {
            Some(value) => {
                out.push_str(value);
                rest = &after[used..];
            }
            None => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    let mut model = model.clone();
    model["customQuery"] = Value::String(out);
    model
}

/// Builds the `/entities` or `/temporal/entities` path and query parameters (`NGP src/request.ts:buildRequest`).
#[must_use]
pub fn build_ngsild_request(
    model: &Value,
    from_ms: i64,
    to_ms: i64,
    interval_ms: u64,
    page_size: u32,
) -> (String, Vec<(String, String)>) {
    let q_type = normalize_query_type(model.get("queryType").and_then(Value::as_str));
    let temporal = q_type == "temporal";
    let mut params: Vec<(String, String)> = Vec::new();

    fn add(params: &mut Vec<(String, String)>, k: &str, v: Option<String>) {
        if let Some(s) = v
            && !s.is_empty()
        {
            params.push((k.to_owned(), s));
        }
    }

    add(
        &mut params,
        "id",
        model
            .get("entityId")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_owned()),
    );
    add(
        &mut params,
        "type",
        model
            .get("entityType")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_owned()),
    );
    add(
        &mut params,
        "idPattern",
        model
            .get("idPattern")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_owned()),
    );

    let attrs = projected_attrs(model);
    if !attrs.is_empty() {
        add(&mut params, "attrs", Some(attrs.join(",")));
    }

    add(
        &mut params,
        "q",
        model
            .get("customQuery")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_owned()),
    );
    add(
        &mut params,
        "scopeQ",
        model
            .get("scopeQuery")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_owned()),
    );

    if let (Some(georel), Some(coords)) = (
        model
            .get("georel")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        model
            .get("coordinates")
            .and_then(Value::as_str)
            .map(|s| s.replace(char::is_whitespace, ""))
            .filter(|s| !s.is_empty()),
    ) {
        add(&mut params, "georel", Some(georel.to_owned()));
        let geom = model
            .get("geometry")
            .and_then(Value::as_str)
            .unwrap_or("Point");
        add(&mut params, "geometry", Some(geom.to_owned()));
        add(&mut params, "coordinates", Some(coords));
        add(
            &mut params,
            "geoproperty",
            model
                .get("geoproperty")
                .and_then(Value::as_str)
                .map(str::to_owned),
        );
    }

    add(
        &mut params,
        "lang",
        model
            .get("lang")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_owned()),
    );

    let selectors: &[&str] = if temporal {
        &["type", "attrs"]
    } else {
        &["type", "attrs", "q", "georel"]
    };
    let has_selector = params.iter().any(|(k, _)| selectors.contains(&k.as_str()));
    if !has_selector {
        add(&mut params, "local", Some("true".to_owned()));
    }

    let aggr_method = model
        .get("aggrMethod")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if temporal {
        let fmt = if aggr_method.is_some() {
            "aggregatedValues"
        } else {
            "temporalValues"
        };
        add(&mut params, "format", Some(fmt.to_owned()));
        add(&mut params, "timerel", Some("between".to_owned()));
        let from_iso = chrono::DateTime::from_timestamp_millis(from_ms)
            .unwrap_or_default()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let to_iso = chrono::DateTime::from_timestamp_millis(to_ms)
            .unwrap_or_default()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        add(&mut params, "timeAt", Some(from_iso));
        add(&mut params, "endTimeAt", Some(to_iso));

        if let Some(tp) = model.get("timeProperty").and_then(Value::as_str)
            && tp != "observedAt"
            && !tp.is_empty()
        {
            add(&mut params, "timeproperty", Some(tp.to_owned()));
        }
        if let Some(methods) = aggr_method {
            add(&mut params, "aggrMethods", Some(methods.to_owned()));
            if let Some(period) = model
                .get("aggrPeriodDuration")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let p = if period == "auto" {
                    format!("PT{}S", (interval_ms / 1000).max(1))
                } else {
                    period.to_owned()
                };
                add(&mut params, "aggrPeriodDuration", Some(p));
            }
        }
        if let Some(last_n) = model.get("lastN").and_then(Value::as_u64)
            && last_n > 0
        {
            add(&mut params, "lastN", Some(last_n.to_string()));
        }
    }

    let limit = model
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(page_size, |l| (l as u32).min(page_size));
    add(&mut params, "limit", Some(limit.to_string()));

    let path = if temporal {
        "/temporal/entities".to_owned()
    } else {
        "/entities".to_owned()
    };
    (path, params)
}

/// The `attrs` projection of a target: its attribute list plus the entity name attribute and,
/// for a geo query, the geoproperty; empty when the target names no attributes.
fn projected_attrs(model: &Value) -> Vec<String> {
    let mut attrs: Vec<String> = model
        .get("attributes")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if attrs.is_empty() {
        return attrs;
    }
    let entity_name = model
        .get("entityName")
        .and_then(Value::as_str)
        .unwrap_or("");
    let name_pattern = model
        .get("namePattern")
        .and_then(Value::as_str)
        .unwrap_or("entity_plus_attribute");
    if !entity_name.is_empty()
        && entity_name != "id"
        && entity_name != "id_short"
        && name_pattern != "attribute"
        && !attrs.iter().any(|a| a == entity_name)
    {
        attrs.push(entity_name.to_owned());
    }
    if normalize_query_type(model.get("queryType").and_then(Value::as_str)) == "geo" {
        let geoprop = model
            .get("geoproperty")
            .and_then(Value::as_str)
            .unwrap_or("location");
        if !attrs.iter().any(|a| a == geoprop) {
            attrs.push(geoprop.to_owned());
        }
    }
    attrs
}

fn short_id(id: &str) -> &str {
    id.rsplit_once([':', '/', '#'])
        .map_or(id, |(_, tail)| if tail.is_empty() { id } else { tail })
}

/// Extracts the plain value of an NGSI-LD attribute instance (`NGP src/ngsildTypes.ts:getValue`).
#[must_use]
pub fn extract_ngsi_value(attr: &Value, lang: Option<&str>) -> Value {
    let Some(obj) = attr.as_object() else {
        return attr.clone();
    };
    // A JSON-LD typed literal (`{"@value": "2024-01-01", "@type": "Date"}`), as in temporalValues.
    if let Some(v) = obj.get("@value") {
        return v.clone();
    }
    if let Some(v) = obj.get("value") {
        if let Some(inner) = v.get("@value") {
            return inner.clone();
        }
        return v.clone();
    }
    if let Some(o) = obj.get("object") {
        return o.clone();
    }
    if let Some(lm) = obj.get("languageMap").and_then(Value::as_object) {
        if let Some(l) = lang
            && let Some(v) = lm.get(l)
        {
            return v.clone();
        }
        if let Some(v) = lm.get("@none").or_else(|| lm.get("en")) {
            return v.clone();
        }
        if let Some(v) = lm.values().next() {
            return v.clone();
        }
    }
    if let Some(vocab) = obj.get("vocab") {
        return vocab.clone();
    }
    if let Some(j) = obj.get("json") {
        return j.clone();
    }
    if let Some(vl) = obj.get("valueList") {
        return vl.clone();
    }
    attr.clone()
}

fn entity_label_of(entity: &serde_json::Map<String, Value>, model: &Value) -> String {
    let lang = model.get("lang").and_then(Value::as_str);
    let entity_name = model
        .get("entityName")
        .and_then(Value::as_str)
        .unwrap_or("id_short");
    if entity_name != "id"
        && entity_name != "id_short"
        && let Some(attr) = entity.get(entity_name)
    {
        let v = extract_ngsi_value(attr, lang);
        if let Some(s) = v.as_str()
            && !s.is_empty()
        {
            return s.to_owned();
        }
    }
    let id = entity.get("id").and_then(Value::as_str).unwrap_or("");
    if entity_name == "id"
        || model
            .get("useLongEntityName")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        id.to_owned()
    } else {
        short_id(id).to_owned()
    }
}

fn entities_to_table_frame(ref_id: &str, entities: &[Value], model: &Value) -> Frame {
    let lang = model.get("lang").and_then(Value::as_str);
    // An explicit attribute list is a projection (NGSI-LD `attrs`, as requested): the broker
    // answers only those attributes, and a broker that answers more must not widen the table.
    let requested_attrs = projected_attrs(model);
    let mut attr_order: Vec<String> = requested_attrs.clone();
    for e in entities.iter().filter(|_| requested_attrs.is_empty()) {
        if let Some(obj) = e.as_object() {
            for k in obj.keys() {
                if !CORE_MEMBERS.contains(&k.as_str()) && !attr_order.contains(k) {
                    attr_order.push(k.clone());
                }
            }
        }
    }

    let mut ids = Vec::with_capacity(entities.len());
    let mut types = Vec::with_capacity(entities.len());
    let mut observed_times = Vec::with_capacity(entities.len());
    let mut has_observed = false;
    let mut cols: BTreeMap<String, (Vec<Value>, Option<String>)> = attr_order
        .iter()
        .map(|a| (a.clone(), (Vec::with_capacity(entities.len()), None)))
        .collect();

    for e in entities {
        let Some(obj) = e.as_object() else {
            continue;
        };
        ids.push(obj.get("id").and_then(Value::as_str).map(str::to_owned));
        let t_str = match obj.get("type") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Array(arr)) => Some(
                arr.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            _ => None,
        };
        types.push(t_str);

        let mut newest_obs: Option<i64> = None;
        for a_name in &attr_order {
            let raw_attr = obj.get(a_name);
            let inst = match raw_attr {
                Some(Value::Array(arr)) => arr.first(),
                other => other,
            };
            if let Some(inst_val) = inst {
                if let Some(obs_str) = inst_val.get("observedAt").and_then(Value::as_str)
                    && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(obs_str)
                {
                    let ms = dt.timestamp_millis();
                    newest_obs = Some(newest_obs.map_or(ms, |prev| prev.max(ms)));
                    has_observed = true;
                }
                let unit = inst_val
                    .get("unitCode")
                    .and_then(Value::as_str)
                    .and_then(|u| grafana_unit(Some(u)));
                let v = extract_ngsi_value(inst_val, lang);
                if let Some((vec, u_slot)) = cols.get_mut(a_name) {
                    vec.push(v);
                    if u_slot.is_none() && unit.is_some() {
                        *u_slot = unit;
                    }
                }
            } else if let Some((vec, _)) = cols.get_mut(a_name) {
                vec.push(Value::Null);
            }
        }
        observed_times.push(newest_obs);
    }

    let mut fields = vec![
        FieldColumn::strings("id", ids),
        FieldColumn::strings("type", types),
    ];
    if has_observed {
        fields.push(FieldColumn {
            name: "observedAt".to_owned(),
            field_type: FieldType::Time,
            unit: Some("dateTimeAsIso".to_owned()),
            display_name: None,
            labels: BTreeMap::new(),
            values: observed_times
                .into_iter()
                .map(|t| t.map_or(Value::Null, Value::from))
                .collect(),
        });
    }

    for a_name in attr_order {
        if let Some((values, unit)) = cols.remove(&a_name) {
            let field_type = infer_field_type(&values);
            fields.push(FieldColumn {
                name: a_name,
                field_type,
                unit,
                display_name: None,
                labels: BTreeMap::new(),
                values,
            });
        }
    }

    Frame {
        name: ref_id.to_owned(),
        ref_id: Some(ref_id.to_owned()),
        fields,
        meta: FrameMeta {
            preferred_visualisation_type: Some("table".to_owned()),
            ..FrameMeta::default()
        },
    }
}

fn temporal_entities_to_frames(ref_id: &str, entities: &[Value], model: &Value) -> Vec<Frame> {
    let lang = model.get("lang").and_then(Value::as_str);
    let entity_name_attr = model.get("entityName").and_then(Value::as_str);
    let name_pattern = model
        .get("namePattern")
        .and_then(Value::as_str)
        .unwrap_or("entity_plus_attribute");
    let mut frames = Vec::new();

    for e in entities {
        let Some(obj) = e.as_object() else {
            continue;
        };
        let entity_id = obj.get("id").and_then(Value::as_str).unwrap_or("");
        let entity_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
        let label = entity_label_of(obj, model);

        for (attr, raw_val) in obj {
            if CORE_MEMBERS.contains(&attr.as_str()) {
                continue;
            }
            if entity_name_attr == Some(attr.as_str()) && name_pattern != "attribute" {
                continue;
            }
            let instances = match raw_val {
                Value::Array(arr) => arr.as_slice(),
                other => std::slice::from_ref(other),
            };
            for inst in instances {
                let Some(inst_obj) = inst.as_object() else {
                    continue;
                };
                let unit = inst_obj
                    .get("unitCode")
                    .and_then(Value::as_str)
                    .and_then(|u| grafana_unit(Some(u)));

                // Check simplified temporal form (`values`: [[val, time], ...])
                if let Some(pairs) = inst_obj.get("values").and_then(Value::as_array) {
                    let mut pts: Vec<(i64, Option<f64>)> = Vec::with_capacity(pairs.len());
                    for p in pairs {
                        if let Some(arr) = p.as_array()
                            && arr.len() >= 2
                        {
                            let v = extract_ngsi_value(&arr[0], lang).as_f64();
                            let t = arr[1]
                                .as_str()
                                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                .map(|dt| dt.timestamp_millis())
                                .unwrap_or(0);
                            pts.push((t, v));
                        }
                    }
                    pts.sort_by_key(|(t, _)| *t);
                    frames.push(build_ngsi_series_frame(
                        ref_id,
                        entity_id,
                        entity_type,
                        &label,
                        attr,
                        name_pattern,
                        None,
                        unit,
                        pts,
                    ));
                    continue;
                }

                // Check aggregated form (`avg`, `sum`, `min`, `max`, etc.)
                for &method in AGGREGATION_METHODS {
                    if let Some(buckets) = inst_obj.get(method).and_then(Value::as_array) {
                        let mut pts: Vec<(i64, Option<f64>)> = Vec::with_capacity(buckets.len());
                        for b in buckets {
                            if let Some(arr) = b.as_array()
                                && arr.len() >= 2
                            {
                                let v = arr[0].as_f64();
                                let t = arr[1]
                                    .as_str()
                                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                    .map(|dt| dt.timestamp_millis())
                                    .unwrap_or(0);
                                pts.push((t, v));
                            }
                        }
                        pts.sort_by_key(|(t, _)| *t);
                        frames.push(build_ngsi_series_frame(
                            ref_id,
                            entity_id,
                            entity_type,
                            &label,
                            attr,
                            name_pattern,
                            Some(method),
                            unit.clone(),
                            pts,
                        ));
                    }
                }
            }
        }
    }

    frames
}

#[allow(clippy::too_many_arguments)]
fn build_ngsi_series_frame(
    ref_id: &str,
    entity_id: &str,
    entity_type: &str,
    label: &str,
    attr: &str,
    name_pattern: &str,
    method: Option<&str>,
    unit: Option<String>,
    pts: Vec<(i64, Option<f64>)>,
) -> Frame {
    let suffix = method.map_or_else(String::new, |m| format!(" ({m})"));
    let display = match name_pattern {
        "entity_name" => format!("{label}{suffix}"),
        "attribute" => format!("{attr}{suffix}"),
        _ => format!("{label}:{attr}{suffix}"),
    };
    let mut labels = BTreeMap::new();
    labels.insert("entity".to_owned(), entity_id.to_owned());
    labels.insert("type".to_owned(), entity_type.to_owned());
    if let Some(m) = method {
        labels.insert("aggregation".to_owned(), m.to_owned());
    }

    let times: Vec<i64> = pts.iter().map(|(t, _)| *t).collect();
    let vals: Vec<Option<f64>> = pts.iter().map(|(_, v)| *v).collect();
    let mut val_col = FieldColumn::numbers(attr, labels, vals);
    val_col.display_name = Some(display);
    val_col.unit = unit;

    Frame {
        name: label.to_owned(),
        ref_id: Some(ref_id.to_owned()),
        fields: vec![FieldColumn::time("Time", times), val_col],
        meta: FrameMeta::default(),
    }
}

fn infer_field_type(values: &[Value]) -> FieldType {
    for v in values {
        match v {
            Value::Null => continue,
            Value::Number(_) => return FieldType::Number,
            Value::Bool(_) => return FieldType::Bool,
            Value::String(_) => return FieldType::String,
            Value::Array(_) | Value::Object(_) => return FieldType::Json,
        }
    }
    FieldType::Number
}

impl Datasource for NgsiLdDatasource {
    fn kind(&self) -> &'static str {
        "ngsild"
    }

    fn query<'a>(
        &'a self,
        ctx: &'a QueryCtx,
        queries: &'a [Query],
    ) -> DsFuture<'a, Result<Vec<FrameSet>, DsError>> {
        Box::pin(async move {
            let headers = self.build_headers(ctx)?;
            let mut out = Vec::with_capacity(queries.len());

            for q in queries {
                if q.hide {
                    continue;
                }
                let model = with_variables(&q.model, &ctx.variables);
                let (path, params) = build_ngsild_request(
                    &model,
                    ctx.from_ms,
                    ctx.to_ms,
                    ctx.interval_ms,
                    self.page_size,
                );
                let qs: String = params
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{}={}",
                            utf8_percent_encode(k, NON_ALPHANUMERIC),
                            utf8_percent_encode(v, NON_ALPHANUMERIC)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("&");

                let url = if qs.is_empty() {
                    format!("{}{path}", self.base_ngsi_url())
                } else {
                    format!("{}{path}?{qs}", self.base_ngsi_url())
                };

                let resp = self
                    .guard
                    .send(OutboundRequest {
                        method: Method::GET,
                        url,
                        headers: headers.clone(),
                        body: None,
                        timeout: Some(ctx.timeout),
                    })
                    .await?;

                if resp.status >= 400 {
                    let detail = String::from_utf8_lossy(&resp.body);
                    return Err(DsError::Upstream {
                        status: resp.status,
                        message: detail.chars().take(200).collect(),
                    });
                }

                let parsed: Value =
                    serde_json::from_slice(&resp.body).map_err(|e| DsError::Upstream {
                        status: 502,
                        message: format!("NGSI-LD endpoint did not return valid JSON: {e}"),
                    })?;

                let entities: Vec<Value> = match parsed {
                    Value::Array(arr) => arr,
                    Value::Object(_) => vec![parsed],
                    _ => Vec::new(),
                };

                let q_type = normalize_query_type(q.model.get("queryType").and_then(Value::as_str));
                let frames = if q_type == "temporal" {
                    temporal_entities_to_frames(&q.ref_id, &entities, &q.model)
                } else if q.model.get("format").and_then(Value::as_str) == Some("table")
                    || q_type == "entity"
                    || q_type == "geo"
                {
                    vec![entities_to_table_frame(&q.ref_id, &entities, &q.model)]
                } else {
                    temporal_entities_to_frames(&q.ref_id, &entities, &q.model)
                };

                out.push(FrameSet::new(q.ref_id.clone(), frames));
            }
            Ok(out)
        })
    }

    fn health<'a>(&'a self, ctx: &'a QueryCtx) -> DsFuture<'a, Health> {
        Box::pin(async move {
            let headers = match self.build_headers(ctx) {
                Ok(h) => h,
                Err(e) => return Health::error(e.to_string()),
            };
            let url = format!("{}/types?details=true", self.base_ngsi_url());
            match self
                .guard
                .send(OutboundRequest {
                    method: Method::GET,
                    url,
                    headers,
                    body: None,
                    timeout: Some(ctx.timeout),
                })
                .await
            {
                Ok(resp) if resp.status == 200 => Health::ok("NGSI-LD broker reachable"),
                Ok(resp) => Health::error(format!("NGSI-LD API returned HTTP {}", resp.status)),
                Err(e) => Health::error(format!("Cannot reach the NGSI-LD API ({e})")),
            }
        })
    }

    fn resource<'a>(
        &'a self,
        ctx: &'a QueryCtx,
        r: ResourceReq,
    ) -> DsFuture<'a, Result<ResourceResp, DsError>> {
        Box::pin(async move {
            let headers = self.build_headers(ctx)?;
            let path = r.path.trim_matches('/');
            let url = match path {
                "types" => format!("{}/types?details=true", self.base_ngsi_url()),
                "attributes" => format!("{}/attributes", self.base_ngsi_url()),
                other => {
                    return Err(DsError::InvalidQuery {
                        ref_id: "resource".into(),
                        reason: format!("unsupported NGSI-LD resource path '{other}'"),
                    });
                }
            };
            let resp = self
                .guard
                .send(OutboundRequest {
                    method: Method::GET,
                    url,
                    headers,
                    body: None,
                    timeout: Some(ctx.timeout),
                })
                .await?;
            if resp.status >= 400 {
                return Err(DsError::Upstream {
                    status: resp.status,
                    message: String::from_utf8_lossy(&resp.body)
                        .chars()
                        .take(200)
                        .collect(),
                });
            }
            let data: Value =
                serde_json::from_slice(&resp.body).map_err(|e| DsError::Upstream {
                    status: 502,
                    message: format!("invalid JSON from NGSI-LD resource: {e}"),
                })?;
            Ok(ResourceResp { data })
        })
    }

    fn editor(&self) -> EditorSchema {
        EditorSchema {
            kind: "ngsild".into(),
            json_schema: json!({
                "$schema": "https://json-schema.org/draft-04/schema",
                "type": "object",
                "properties": {
                    "queryType": { "type": "string", "enum": ["entity", "temporal", "geo", "nodegraph"] },
                    "entityType": { "type": "string" },
                    "entityId": { "type": "string" },
                    "idPattern": { "type": "string" },
                    "attributes": { "type": "array", "items": { "type": "string" } },
                    "customQuery": { "type": "string" },
                    "scopeQuery": { "type": "string" },
                    "aggrMethod": { "type": "string" },
                    "aggrPeriodDuration": { "type": "string" },
                    "lang": { "type": "string" },
                    "format": { "type": "string", "enum": ["timeseries", "table"] },
                    "limit": { "type": "integer" }
                }
            }),
            field_hints: vec![
                EditorFieldHint {
                    name: "queryType".into(),
                    label: "Query Type".into(),
                    widget: "select".into(),
                    resource_path: None,
                    required: true,
                },
                EditorFieldHint {
                    name: "entityType".into(),
                    label: "Entity Type".into(),
                    widget: "select".into(),
                    resource_path: Some("types".into()),
                    required: false,
                },
                EditorFieldHint {
                    name: "attributes".into(),
                    label: "Attributes".into(),
                    widget: "multi-select".into(),
                    resource_path: Some("attributes".into()),
                    required: false,
                },
                EditorFieldHint {
                    name: "customQuery".into(),
                    label: "NGSI-LD Filter (q)".into(),
                    widget: "text".into(),
                    resource_path: None,
                    required: false,
                },
            ],
        }
    }

    fn import(&self, grafana_target: &Value) -> Result<ImportOutcome, DsError> {
        let obj = grafana_target
            .as_object()
            .ok_or_else(|| DsError::InvalidQuery {
                ref_id: "A".into(),
                reason: "NGSI-LD target must be a JSON object".into(),
            })?;
        let ref_id = obj
            .get("refId")
            .and_then(Value::as_str)
            .unwrap_or("A")
            .to_owned();
        let hide = obj.get("hide").and_then(Value::as_bool).unwrap_or(false);

        let mut warnings = Vec::new();
        if obj.get("queryType").and_then(Value::as_str) == Some("timseries") {
            warnings.push(format!(
                "target '{ref_id}': misspelled queryType 'timseries' accepted as 'temporal'"
            ));
        }

        // Keep all fields of NGP src/types.ts:67-124 verbatim
        let allowed_keys: BTreeSet<&str> = [
            "refId",
            "hide",
            "datasource",
            "queryType",
            "entityId",
            "idPattern",
            "attributes",
            "entityType",
            "namePattern",
            "entityName",
            "useLongEntityName",
            "customQuery",
            "scopeQuery",
            "lang",
            "georel",
            "geometry",
            "coordinates",
            "geoproperty",
            "geoLocation",
            "timeProperty",
            "aggrMethod",
            "aggrPeriodDuration",
            "lastN",
            "format",
            "primaryNodeAttributes",
            "secondaryNodeAttributes",
            "arcColorNodeAttributes",
            "arcColorComplementLabel",
            "primaryArcColor",
            "secondaryArcColor",
            "graphDepth",
            "ngsildOptions",
            "limit",
        ]
        .into_iter()
        .collect();

        for k in obj.keys() {
            if !allowed_keys.contains(k.as_str()) {
                warnings.push(format!(
                    "target '{ref_id}': unknown NGSI-LD property '{k}' kept for export"
                ));
            }
        }

        Ok(ImportOutcome {
            query: Query {
                ref_id,
                hide,
                model: Value::Object(obj.clone()),
            },
            warnings,
        })
    }

    fn export(&self, q: &Query) -> Value {
        let mut map = q.model.as_object().cloned().unwrap_or_default();
        map.insert("refId".to_owned(), Value::String(q.ref_id.clone()));
        if q.hide {
            map.insert("hide".to_owned(), Value::Bool(true));
        }
        Value::Object(map)
    }
}
