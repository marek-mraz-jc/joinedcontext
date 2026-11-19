#![forbid(unsafe_code)]
//! Public dashboards implementation (R7, `GF pkg/services/publicdashboards`):
//! - Per-dashboard opt-in (`is_enabled: false` by default).
//! - 256-bit random access token (32 bytes = 64 hex chars), stored only as SHA-256 hash in the store.
//! - Anonymous viewers can only execute the saved queries of a panel on the saved dashboard:
//!   any caller-supplied `targets`, `queries`, `datasource`, `expr`, `rawSql`, `expressions`, or
//!   variable outside its saved allowed `options` is strictly refused!
//! - Time range and refresh interval are capped to the organization's `max_public_range_seconds`
//!   and `min_public_interval_ms`, and caller time range is ignored when `time_selection_enabled` is false.
//! - Results are cached per `(org_id, instance_id, dashboard_uid, dashboard_version, panel_id, range_bucket)`.
//! - Per-source-address rate limiter keyed on `jc_router::ClientAddr`.
//! - Answers carry frames only (no datasource uid, URL, credential, query text unless `show_query_text`
//!   is enabled by the owner, nor any other organization's data).
//! - Revoke = token rotation (`rotate_public_dashboard_token`): old token immediately answers 404.

use crate::render::{execute_targets_with_authorizer, find_panel_in_slice, parse_time_str_ms};
use crate::state::{DaiyaState, error_response, resolve_request_ctx};
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use daiya_expr::apply_transformations;
use daiya_model::{
    Action, CanonicalJson, CrError, CrResult, DashboardDocument, FrameSet, ObjectTarget,
    PublicDashboardAuthorizer, PublicDashboardRecord, RequestCtx, ShareType, Subject, sha256_hex,
};
use daiya_query::PublicCacheKey;
use jc_router::ClientAddr;
use jc_time::TimestampTz;
use parking_lot::Mutex;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Generates a 256-bit random access token (64 lowercase hex characters) and its SHA-256 hash (R7).
#[must_use]
pub fn generate_256bit_token() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let raw_token = hex::encode(bytes);
    let token_hash = sha256_hex(raw_token.as_bytes());
    (raw_token, token_hash)
}

/// Validates that `access_token` is a well-formedhex token (32..=64 hex chars).
#[must_use]
pub fn is_valid_access_token_format(token: &str) -> bool {
    (32..=64).contains(&token.len()) && token.chars().all(|c| c.is_ascii_hexdigit())
}

/// Who a public dashboard request is counted against (R7): the source address the router
/// resolved, or the authenticated subject when the call arrives through a door that knows one
/// (MCP). A door that knows neither refuses the request instead of counting it under a shared key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicCaller {
    Addr(IpAddr),
    Subject(String),
}

impl From<IpAddr> for PublicCaller {
    fn from(addr: IpAddr) -> Self {
        Self::Addr(addr)
    }
}

impl PublicCaller {
    /// The bucket key: an IPv6 caller counts by its /64 (one host's usual allocation), so
    /// rotating addresses inside it buys no extra quota; IPv4 and subjects count as they are.
    fn bucket(&self) -> String {
        match self {
            Self::Addr(IpAddr::V6(v6)) if v6.to_ipv4_mapped().is_none() => {
                let s = v6.segments();
                format!("addr:{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
            Self::Addr(IpAddr::V6(v6)) => format!("addr:{}", v6.to_canonical()),
            Self::Addr(IpAddr::V4(v4)) => format!("addr:{v4}"),
            Self::Subject(id) => format!("subject:{id}"),
        }
    }
}

/// Per-caller sliding window rate limiter for public dashboard endpoints (R7).
#[derive(Debug)]
pub struct PublicRateLimiter {
    max_requests: u32,
    window: Duration,
    buckets: Mutex<BTreeMap<String, (u32, Instant)>>,
}

impl PublicRateLimiter {
    #[must_use]
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            max_requests: max_requests.max(1),
            window: Duration::from_secs(window_secs.max(1)),
            buckets: Mutex::new(BTreeMap::new()),
        }
    }

    /// Most callers tracked at once. A full table drops expired windows first, then the oldest
    /// live window: callers rotating addresses can never lock new callers out.
    pub const MAX_TRACKED_ADDRESSES: usize = 65_536;

    pub fn check_rate_limit(&self, caller: impl Into<PublicCaller>) -> CrResult<()> {
        let key = caller.into().bucket();
        let mut guard = self.buckets.lock();
        let now = Instant::now();
        if guard.len() >= Self::MAX_TRACKED_ADDRESSES && !guard.contains_key(&key) {
            let window = self.window;
            guard.retain(|_, (_, started)| now.duration_since(*started) < window);
            if guard.len() >= Self::MAX_TRACKED_ADDRESSES
                && let Some(oldest) = guard
                    .iter()
                    .min_by_key(|(_, (_, started))| *started)
                    .map(|(k, _)| k.clone())
            {
                guard.remove(&oldest);
            }
        }
        let entry = guard.entry(key).or_insert((0, now));
        if now.duration_since(entry.1) >= self.window {
            *entry = (1, now);
            return Ok(());
        }
        if entry.0 >= self.max_requests {
            return Err(CrError::RateLimited(
                "public dashboard rate limit exceeded; retry after the window ends".into(),
            ));
        }
        entry.0 += 1;
        Ok(())
    }
}

/// Answer of a public door that cannot see the caller's source address.
fn unknown_caller() -> CrError {
    CrError::Forbidden(
        "public dashboards answer only requests whose source address the server can see".into(),
    )
}

/// The public caller of a door whose router resolves `ClientAddr`; none when it is absent.
#[must_use]
pub fn public_caller_of(client: Option<&ClientAddr>) -> Option<PublicCaller> {
    client.map(|c| PublicCaller::Addr(c.0))
}

/// An anonymous viewer gets a fixed text for a failed saved query: the datasource's own error
/// (upstream body, database message, refused address) stays with the dashboard's members.
fn public_query_error(err: CrError) -> CrError {
    match err {
        CrError::RateLimited(_) => err,
        _ => CrError::OutboundFailed("the panel's saved query failed".into()),
    }
}

/// Request payload to create or update a public dashboard configuration (R7).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicDashboardCreateReq {
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub is_enabled: Option<bool>,
    #[serde(default)]
    pub time_selection_enabled: Option<bool>,
    #[serde(default)]
    pub annotations_enabled: Option<bool>,
    #[serde(default)]
    pub show_query_text: Option<bool>,
    #[serde(default)]
    pub share: Option<ShareType>,
}

/// Request payload for an anonymous public dashboard panel query (R7).
/// Strictly inspects any extra fields so forged `targets`, `datasource`, `expr`, `rawSql`, or
/// unauthorized `variables` are immediately rejected!
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicDashboardQueryReq {
    #[serde(default)]
    pub interval_ms: Option<i64>,
    #[serde(default)]
    pub max_data_points: Option<u32>,
    #[serde(default)]
    pub time_range: Option<PublicTimeRangeDto>,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicTimeRangeDto {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub timezone: Option<String>,
}

/// Sanitized public dashboard view DTO (`GF pkg/services/publicdashboards/service/service.go:GetPublicDashboardForView`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicDashboardViewDto {
    pub meta: Value,
    pub dashboard: Value,
}

/// Creates or updates a public dashboard opt-in record for `dashboard_uid` (R7: off by default unless `is_enabled` is true).
pub async fn create_or_update_public_dashboard(
    state: &DaiyaState,
    req_ctx: &RequestCtx,
    dashboard_uid: &str,
    dto: PublicDashboardCreateReq,
) -> CrResult<PublicDashboardRecord> {
    let decision = state.authorizer.check(
        req_ctx,
        Action::Approve,
        &ObjectTarget::PublicDashboard {
            org_id: req_ctx.org_id,
            dashboard_uid: dashboard_uid.to_owned(),
        },
    );
    decision.into_result(format!("Dashboard '{dashboard_uid}' not found"))?;
    if state
        .store
        .get_dashboard(req_ctx.org_id, dashboard_uid)
        .await?
        .is_none()
    {
        return Err(CrError::NotFound(format!(
            "Dashboard '{dashboard_uid}' not found"
        )));
    }

    let existing = state
        .store
        .get_public_dashboard_by_dashboard_uid(req_ctx.org_id, dashboard_uid)
        .await?;

    let now = TimestampTz::now();
    let record = if let Some(mut ex) = existing {
        if let Some(en) = dto.is_enabled {
            ex.is_enabled = en;
        }
        if let Some(ts) = dto.time_selection_enabled {
            ex.time_selection_enabled = ts;
        }
        if let Some(an) = dto.annotations_enabled {
            ex.annotations_enabled = an;
        }
        if let Some(sq) = dto.show_query_text {
            ex.show_query_text = sq;
        }
        if let Some(sh) = dto.share {
            ex.share = sh;
        }
        ex.updated_by = req_ctx.actor_name().to_owned();
        ex.updated_at = now;
        state.store.upsert_public_dashboard(ex).await?
    } else {
        let (raw_token, token_hash) = generate_256bit_token();
        let uid = dto
            .uid
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("pub-{}", &raw_token[..12]));
        // A uid names one public dashboard: another dashboard's record is never taken over.
        if state
            .store
            .get_public_dashboard(req_ctx.org_id, &uid)
            .await?
            .is_some()
        {
            return Err(CrError::Conflict(format!(
                "Public dashboard uid '{uid}' is already in use; choose another uid or leave it empty"
            )));
        }
        let new_rec = PublicDashboardRecord {
            installation_id: state.installation_id,
            org_id: req_ctx.org_id,
            instance_id: state.default_instance_id,
            uid,
            dashboard_uid: dashboard_uid.to_owned(),
            access_token_hash: token_hash,
            access_token: Some(raw_token),
            // Per R7: "per dashboard opt-in, off by default"
            is_enabled: dto.is_enabled.unwrap_or(false),
            time_selection_enabled: dto.time_selection_enabled.unwrap_or(false),
            annotations_enabled: dto.annotations_enabled.unwrap_or(false),
            show_query_text: dto.show_query_text.unwrap_or(false),
            share: dto.share.unwrap_or(ShareType::Public),
            created_by: req_ctx.actor_name().to_owned(),
            updated_by: req_ctx.actor_name().to_owned(),
            created_at: now,
            updated_at: now,
        };
        state.store.upsert_public_dashboard(new_rec).await?
    };

    Ok(record)
}

/// Rotates the 256-bit access token of a public dashboard (`PublicDashboard/Rotate`, R7).
/// Immediately revokes the previous token so it answers 404 on subsequent calls.
pub async fn rotate_public_dashboard_token(
    state: &DaiyaState,
    req_ctx: &RequestCtx,
    uid_or_dashboard_uid: &str,
) -> CrResult<PublicDashboardRecord> {
    let existing = match state
        .store
        .get_public_dashboard(req_ctx.org_id, uid_or_dashboard_uid)
        .await?
    {
        Some(pd) => pd,
        None => state
            .store
            .get_public_dashboard_by_dashboard_uid(req_ctx.org_id, uid_or_dashboard_uid)
            .await?
            .ok_or_else(|| {
                CrError::NotFound(format!(
                    "Public dashboard '{uid_or_dashboard_uid}' not found"
                ))
            })?,
    };

    let decision = state.authorizer.check(
        req_ctx,
        Action::Approve,
        &ObjectTarget::PublicDashboard {
            org_id: req_ctx.org_id,
            dashboard_uid: existing.dashboard_uid.clone(),
        },
    );
    decision.into_result(format!(
        "Public dashboard '{uid_or_dashboard_uid}' not found"
    ))?;

    let (new_raw_token, new_hash) = generate_256bit_token();
    let mut updated = existing;
    updated.access_token_hash = new_hash;
    updated.access_token = Some(new_raw_token);
    updated.updated_by = req_ctx.actor_name().to_owned();
    updated.updated_at = TimestampTz::now();

    state.store.upsert_public_dashboard(updated).await
}

/// Dashboard members an anonymous viewer needs to draw the dashboard (R7, "frames only").
const PUBLIC_DASHBOARD_KEYS: &[&str] = &[
    "uid",
    "title",
    "description",
    "tags",
    "timezone",
    "weekStart",
    "fiscalYearStartMonth",
    "graphTooltip",
    "liveNow",
    "refresh",
    "schemaVersion",
    "version",
    "time",
    "timepicker",
    "panels",
    "templating",
    "annotations",
];

/// Panel members needed to lay out and draw a panel from its frames.
const PUBLIC_PANEL_KEYS: &[&str] = &[
    "id",
    "type",
    "title",
    "description",
    "gridPos",
    "collapsed",
    "transparent",
    "repeat",
    "repeatDirection",
    "maxPerRow",
    "interval",
    "maxDataPoints",
    "timeFrom",
    "timeShift",
    "hideTimeOverride",
    "pluginVersion",
    "options",
    "fieldConfig",
    "panels",
    "targets",
];

/// Variable members: the name, the choice and its options.
const PUBLIC_VARIABLE_KEYS: &[&str] = &[
    "name",
    "label",
    "type",
    "description",
    "hide",
    "multi",
    "includeAll",
    "skipUrlSync",
    "sort",
    "current",
    "options",
];

/// Annotation members: the name and how it shows.
const PUBLIC_ANNOTATION_KEYS: &[&str] = &["name", "enable", "hide", "iconColor", "builtIn"];

/// Query members an owner can choose to show (`show_query_text`).
const QUERY_TEXT_KEYS: &[&str] = &[
    "expr",
    "query",
    "rawSql",
    "customQuery",
    "expression",
    "definition",
    "target",
    "format",
    "legendFormat",
];

fn keep_only(obj: &mut serde_json::Map<String, Value>, allowed: &[&str]) {
    obj.retain(|k, _| allowed.contains(&k.as_str()));
}

/// Builds the document an anonymous public viewer receives from an allow-list: layout, panel
/// types and drawing options, variable names and choices. Datasources, links, library panel
/// references, filters, entity ids and any member not listed never reach the viewer; query text
/// only when the owner enabled `show_query_text` (`sanitizeData` in
/// `GF pkg/services/publicdashboards/service/query.go:380`, R7).
#[must_use]
pub fn sanitize_dashboard_for_public_view(
    raw_dash: &Value,
    time_selection_enabled: bool,
    show_query_text: bool,
) -> Value {
    let mut dash = CanonicalJson::canonicalize(raw_dash);
    let Some(obj) = dash.as_object_mut() else {
        return dash;
    };
    keep_only(obj, PUBLIC_DASHBOARD_KEYS);

    let mut tp = obj
        .get("timepicker")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    keep_only(&mut tp, &["refresh_intervals", "time_options", "nowDelay"]);
    tp.insert("hidden".to_owned(), Value::Bool(!time_selection_enabled));
    obj.insert("timepicker".to_owned(), Value::Object(tp));

    if let Some(panels) = obj.get_mut("panels").and_then(Value::as_array_mut) {
        sanitize_panels_for_public(panels, show_query_text);
    }
    for (section, allowed) in [
        ("templating", PUBLIC_VARIABLE_KEYS),
        ("annotations", PUBLIC_ANNOTATION_KEYS),
    ] {
        let Some(holder) = obj.get_mut(section).and_then(Value::as_object_mut) else {
            continue;
        };
        keep_only(holder, &["list"]);
        let Some(list) = holder.get_mut("list").and_then(Value::as_array_mut) else {
            continue;
        };
        list.retain(Value::is_object);
        for entry in list.iter_mut().filter_map(Value::as_object_mut) {
            entry.retain(|k, _| {
                allowed.contains(&k.as_str())
                    || (show_query_text && QUERY_TEXT_KEYS.contains(&k.as_str()))
            });
        }
    }
    dash
}

/// Field config without data links: a link can carry an internal URL or a filter.
fn sanitize_field_config(fc: &mut Value) {
    let Some(fc) = fc.as_object_mut() else {
        return;
    };
    keep_only(fc, &["defaults", "overrides"]);
    if let Some(defaults) = fc.get_mut("defaults").and_then(Value::as_object_mut) {
        defaults.remove("links");
    }
    if let Some(overrides) = fc.get_mut("overrides").and_then(Value::as_array_mut) {
        for o in overrides.iter_mut().filter_map(Value::as_object_mut) {
            keep_only(o, &["matcher", "properties"]);
            if let Some(props) = o.get_mut("properties").and_then(Value::as_array_mut) {
                props.retain(|p| p.get("id").and_then(Value::as_str) != Some("links"));
            }
        }
    }
}

fn sanitize_panels_for_public(panels: &mut Vec<Value>, show_query_text: bool) {
    panels.retain(Value::is_object);
    for p_obj in panels.iter_mut().filter_map(Value::as_object_mut) {
        keep_only(p_obj, PUBLIC_PANEL_KEYS);
        if let Some(nested) = p_obj.get_mut("panels").and_then(Value::as_array_mut) {
            sanitize_panels_for_public(nested, show_query_text);
        }
        if let Some(fc) = p_obj.get_mut("fieldConfig") {
            sanitize_field_config(fc);
        }
        if let Some(targets) = p_obj.get_mut("targets").and_then(Value::as_array_mut) {
            targets.retain(Value::is_object);
            for t_obj in targets.iter_mut().filter_map(Value::as_object_mut) {
                t_obj.retain(|k, _| {
                    matches!(k.as_str(), "refId" | "hide")
                        || (show_query_text && QUERY_TEXT_KEYS.contains(&k.as_str()))
                });
            }
        }
    }
}

/// Anonymous view of a public dashboard by `access_token` (R7).
/// Unknown, disabled, or rotated tokens return the exact same 404 error.
pub async fn view_public_dashboard(
    state: &DaiyaState,
    caller: Option<PublicCaller>,
    access_token: &str,
) -> CrResult<PublicDashboardViewDto> {
    state
        .public_rate_limiter
        .check_rate_limit(caller.ok_or_else(unknown_caller)?)?;
    if !is_valid_access_token_format(access_token) {
        return Err(CrError::NotFound("Public dashboard not found".into()));
    }

    let token_hash = sha256_hex(access_token.as_bytes());
    let Some((pubdash, dash)) = state
        .store
        .get_public_dashboard_by_token_hash(&token_hash)
        .await?
    else {
        return Err(CrError::NotFound("Public dashboard not found".into()));
    };

    if !pubdash.is_enabled {
        return Err(CrError::NotFound("Public dashboard not found".into()));
    }

    let sanitized = sanitize_dashboard_for_public_view(
        &dash.data,
        pubdash.time_selection_enabled,
        pubdash.show_query_text,
    );

    Ok(PublicDashboardViewDto {
        meta: json!({
            "slug": dash.slug,
            "type": "db",
            "canStar": false,
            "canSave": false,
            "canEdit": false,
            "canAdmin": false,
            "canDelete": false,
            "version": dash.version,
            "isFolder": false,
            "publicDashboardEnabled": true,
            "timeSelectionEnabled": pubdash.time_selection_enabled,
            "annotationsEnabled": pubdash.annotations_enabled,
        }),
        dashboard: sanitized,
    })
}

/// Executes a panel query on a public dashboard for an anonymous viewer, enforcing every rule of R7:
/// 1. Per-IP rate limit.
/// 2. Constant-time lookup by SHA-256 token hash; unknown/rotated/disabled tokens answer 404.
/// 3. Strictly refuses any request carrying forged `targets`, `queries`, `datasource`, `expr`,
///    `rawSql`, `expression`, or variables outside saved allowed `options`.
/// 4. Caps time range (`max_public_range_seconds`) and interval (`min_public_interval_ms`), and
///    ignores caller time range when `time_selection_enabled == false`.
/// 5. Caches results per `(org_id, instance_id, dashboard_uid, dashboard_version, panel_id, range_bucket)`.
/// 6. Sanitizes returned frames (`sanitize_for_public(pubdash.show_query_text)`): frames only,
///    never datasource UID, URL, credentials, or query text (unless `show_query_text == true`).
pub async fn execute_public_dashboard_query(
    state: &DaiyaState,
    caller: Option<PublicCaller>,
    access_token: &str,
    panel_id: u32,
    req: PublicDashboardQueryReq,
) -> CrResult<Vec<FrameSet>> {
    state
        .public_rate_limiter
        .check_rate_limit(caller.ok_or_else(unknown_caller)?)?;
    if !is_valid_access_token_format(access_token) {
        return Err(CrError::NotFound("Public dashboard not found".into()));
    }

    // Rule R7: Refuse any forged query/target/datasource/expression fields in the public request body!
    for forbidden_key in [
        "targets",
        "queries",
        "datasource",
        "datasourceUid",
        "expr",
        "query",
        "rawSql",
        "expression",
        "conditions",
    ] {
        if req.extra.contains_key(forbidden_key) {
            return Err(CrError::BadRequest(format!(
                "public dashboard query cannot specify '{forbidden_key}': only saved panel queries may run"
            )));
        }
    }

    let token_hash = sha256_hex(access_token.as_bytes());
    let Some((pubdash, dash)) = state
        .store
        .get_public_dashboard_by_token_hash(&token_hash)
        .await?
    else {
        return Err(CrError::NotFound("Public dashboard not found".into()));
    };

    if !pubdash.is_enabled {
        return Err(CrError::NotFound("Public dashboard not found".into()));
    }

    let doc: DashboardDocument = serde_json::from_value(dash.data.clone())
        .map_err(|e| CrError::Internal(format!("invalid stored dashboard JSON: {e}")))?;

    // Validate any caller-supplied template variables against the dashboard's saved allowed options (R7)
    let mut resolved_vars = BTreeMap::new();
    let saved_vars = doc
        .templating
        .as_ref()
        .map_or(&[][..], |t| t.list.as_slice());

    for (var_name, caller_val) in &req.variables {
        let Some(saved_var) = saved_vars.iter().find(|v| &v.name == var_name) else {
            return Err(CrError::BadRequest(format!(
                "variable '{var_name}' is not declared on this public dashboard"
            )));
        };
        let allowed_options = saved_var.options.as_deref().unwrap_or(&[]);
        let is_allowed = allowed_options
            .iter()
            .any(|opt| opt.value.as_ref().and_then(Value::as_str) == Some(caller_val.as_str()))
            || saved_var
                .current
                .as_ref()
                .and_then(|c| c.value.as_ref())
                .and_then(Value::as_str)
                == Some(caller_val.as_str());

        if !is_allowed {
            return Err(CrError::BadRequest(format!(
                "value '{caller_val}' for variable '{var_name}' is outside its saved allowed values"
            )));
        }
        resolved_vars.insert(var_name.clone(), caller_val.clone());
    }

    // Populate defaults for remaining saved variables
    for sv in saved_vars {
        if !resolved_vars.contains_key(&sv.name)
            && let Some(curr) = &sv.current
            && let Some(v_str) = curr.value.as_ref().and_then(Value::as_str)
        {
            resolved_vars.insert(sv.name.clone(), v_str.to_owned());
        }
    }

    let panels = doc.panels.as_deref().unwrap_or(&[]);
    let panel = find_panel_in_slice(panels, panel_id)
        .ok_or_else(|| CrError::NotFound("Dashboard panel not found".into()))?;

    let quotas = state.store.get_quotas(pubdash.org_id).await?;
    let dash_from = doc.time.as_ref().map(|t| t.from.as_str());
    let dash_to = doc.time.as_ref().map(|t| t.to.as_str());

    let (mut from_ms, to_ms) = if pubdash.time_selection_enabled {
        let f_str = req
            .time_range
            .as_ref()
            .map(|tr| tr.from.as_str())
            .filter(|s| !s.is_empty())
            .or(dash_from);
        let t_str = req
            .time_range
            .as_ref()
            .map(|tr| tr.to.as_str())
            .filter(|s| !s.is_empty())
            .or(dash_to);
        (
            parse_time_str_ms(f_str, -6 * 3_600_000),
            parse_time_str_ms(t_str, 0),
        )
    } else {
        (
            parse_time_str_ms(dash_from, -6 * 3_600_000),
            parse_time_str_ms(dash_to, 0),
        )
    };

    if to_ms <= from_ms {
        return Err(CrError::BadRequest("Invalid time range".into()));
    }

    // Cap range to organization's max_public_range_seconds (R7)
    // Saturating: a `from` near i64::MIN must not wrap the span negative and skip the cap.
    let max_range_ms = quotas
        .max_public_range_seconds
        .max(60)
        .saturating_mul(1_000);
    if to_ms.saturating_sub(from_ms) > max_range_ms {
        from_ms = to_ms.saturating_sub(max_range_ms);
    }

    // Cap interval to organization's min_public_interval_ms and safe resolution 11000 (R7, `service.go:349`)
    let safe_interval_ms = (to_ms.saturating_sub(from_ms) / 11_000).max(1_000);
    let requested_interval = req.interval_ms.unwrap_or(quotas.min_public_interval_ms);
    if requested_interval <= 0 {
        return Err(CrError::BadRequest(
            "intervalMS should be greater than 0".into(),
        ));
    }
    let effective_interval_ms = requested_interval
        .max(quotas.min_public_interval_ms)
        .max(safe_interval_ms) as u64;
    let max_dp = req.max_data_points.unwrap_or(1_500).clamp(1, 11_000);

    // Check public cache keyed by (org_id, instance_id, dashboard_uid, dashboard_version, panel_id, range_bucket, interval, maxDataPoints) (R7)
    let pub_cache_key = PublicCacheKey::new(
        pubdash.org_id,
        pubdash.instance_id,
        &dash.uid,
        dash.version,
        panel_id,
        from_ms,
        to_ms,
        effective_interval_ms as i64,
        max_dp,
    );

    if resolved_vars.is_empty()
        && let Some(cached) = state.query_cache.get_public(&pub_cache_key)
    {
        return Ok(cached);
    }

    let raw_targets: Vec<Value> = panel
        .targets
        .as_ref()
        .map(|ts| {
            ts.iter()
                .filter_map(|m| serde_json::to_value(m).ok())
                .collect()
        })
        .unwrap_or_default();

    if raw_targets.is_empty() {
        return Err(CrError::BadRequest(
            "Failed to extract queries from panel".into(),
        ));
    }

    // The saved queries run as the dashboard's own public subject (read-only, no token, no
    // role anywhere) under an authorizer that allows reading exactly this dashboard's
    // datasources: never the host's authorizer, never a caller credential.
    let default_ds_uid = panel.datasource.as_ref().and_then(|d| d.uid.as_deref());
    let panel_datasources = raw_targets
        .iter()
        .filter_map(|t| {
            t.get("datasource")
                .and_then(|d| d.get("uid").or(Some(d)))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .chain(default_ds_uid.map(str::to_owned));
    let public_authz = PublicDashboardAuthorizer::new(pubdash.org_id, &dash.uid, panel_datasources);
    let svc_ctx = RequestCtx::new(
        pubdash.installation_id,
        pubdash.org_id,
        pubdash.instance_id,
        Subject::public_dashboard(pubdash.org_id, &dash.uid),
    );

    let mut frame_sets = execute_targets_with_authorizer(
        state,
        &public_authz,
        &svc_ctx,
        default_ds_uid,
        &raw_targets,
        from_ms,
        to_ms,
        effective_interval_ms,
        max_dp,
        &resolved_vars,
        false,
    )
    .await
    .map_err(public_query_error)?;

    if let Some(txs) = &panel.transformations
        && !txs.is_empty()
    {
        for fs in &mut frame_sets {
            let outcome = apply_transformations(std::mem::take(&mut fs.frames), txs);
            fs.frames = outcome.frames;
        }
    }

    // Sanitize frames for public viewer (R7: strip query text unless show_query_text is true)
    for fs in &mut frame_sets {
        fs.sanitize_for_public(pubdash.show_query_text);
    }

    if resolved_vars.is_empty() {
        state
            .query_cache
            .insert_public(pub_cache_key, frame_sets.clone(), Duration::from_secs(30));
    }

    Ok(frame_sets)
}

async fn http_view_public_dashboard(
    State(state): State<DaiyaState>,
    client: Option<Extension<ClientAddr>>,
    Path(access_token): Path<String>,
) -> Response {
    match view_public_dashboard(&state, public_caller_of(client.as_deref()), &access_token).await {
        Ok(dto) => (
            StatusCode::OK,
            [(axum::http::header::CACHE_CONTROL, "private, no-store")],
            Json(dto),
        )
            .into_response(),
        Err(e) => error_response(&e),
    }
}

async fn http_query_public_dashboard(
    State(state): State<DaiyaState>,
    client: Option<Extension<ClientAddr>>,
    Path((access_token, panel_id)): Path<(String, u32)>,
    body: Option<Json<PublicDashboardQueryReq>>,
) -> Response {
    let req = body.map_or_else(PublicDashboardQueryReq::default, |Json(b)| b);
    match execute_public_dashboard_query(
        &state,
        public_caller_of(client.as_deref()),
        &access_token,
        panel_id,
        req,
    )
        .await
    {
        Ok(frame_sets) => {
            let mut results = serde_json::Map::new();
            for fs in frame_sets {
                results.insert(fs.ref_id.clone(), json!({ "frames": fs.frames }));
            }
            (
                StatusCode::OK,
                [(axum::http::header::CACHE_CONTROL, "private, no-store")],
                Json(json!({ "results": results })),
            )
                .into_response()
        }
        Err(e) => error_response(&e),
    }
}

async fn http_create_public_dashboard(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(dashboard_uid): Path<String>,
    Json(dto): Json<PublicDashboardCreateReq>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    match create_or_update_public_dashboard(&state, &ctx, &dashboard_uid, dto).await {
        Ok(rec) => (StatusCode::OK, Json(rec)).into_response(),
        Err(e) => error_response(&e),
    }
}

async fn http_update_public_dashboard(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path((dashboard_uid, uid)): Path<(String, String)>,
    Json(dto): Json<PublicDashboardCreateReq>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    // As Grafana, the record is the one the path names: another uid is 404, never a new record.
    match state
        .store
        .get_public_dashboard_by_dashboard_uid(ctx.org_id, &dashboard_uid)
        .await
    {
        Ok(Some(existing)) if existing.uid == uid => {}
        Ok(_) => return error_response(&CrError::NotFound("Public dashboard not found".into())),
        Err(e) => return error_response(&e),
    }
    match create_or_update_public_dashboard(&state, &ctx, &dashboard_uid, dto).await {
        Ok(rec) => (StatusCode::OK, Json(rec)).into_response(),
        Err(e) => error_response(&e),
    }
}

async fn http_get_public_dashboard(
    State(state): State<DaiyaState>,
    headers: HeaderMap,
    Path(dashboard_uid): Path<String>,
) -> Response {
    let ctx = resolve_request_ctx(&state, &headers);
    let dec = state.authorizer.check(
        &ctx,
        Action::Read,
        &ObjectTarget::PublicDashboard {
            org_id: ctx.org_id,
            dashboard_uid: dashboard_uid.clone(),
        },
    );
    if !dec.allowed {
        return error_response(&CrError::NotFound("Public dashboard not found".into()));
    }
    match state
        .store
        .get_public_dashboard_by_dashboard_uid(ctx.org_id, &dashboard_uid)
        .await
    {
        Ok(Some(rec)) => (StatusCode::OK, Json(rec)).into_response(),
        Ok(None) => error_response(&CrError::NotFound("Public dashboard not found".into())),
        Err(e) => error_response(&e),
    }
}

/// Router for Grafana-compatible public dashboard endpoints (`/api/public/dashboards/...` and `/api/dashboards/uid/{dashboardUid}/public-dashboards`).
pub fn public_router() -> Router<DaiyaState> {
    Router::new()
        .route(
            "/api/public/dashboards/{accessToken}",
            get(http_view_public_dashboard),
        )
        .route(
            "/api/public/dashboards/{accessToken}/panels/{panelId}/query",
            post(http_query_public_dashboard),
        )
        .route(
            "/api/dashboards/uid/{dashboardUid}/public-dashboards",
            get(http_get_public_dashboard).post(http_create_public_dashboard),
        )
        .route(
            "/api/dashboards/uid/{dashboardUid}/public-dashboards/{uid}",
            patch(http_update_public_dashboard),
        )
}
