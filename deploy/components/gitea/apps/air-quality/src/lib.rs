//! The `air-quality` reference app (AP-34, AP-37, AP-39, AP-40).
//!
//! It shows the stations of one Context Space with their latest values and a day of history,
//! and gives a steward a record form: add a station, correct its name, position and note, and
//! remove a station a steward added (Architecture/16 §13). Everything it serves comes from a single
//! Endpoint whose URL it is handed at run time; it opens no other connection, holds no
//! credential of its own and contains no login, session or authorization logic. The platform
//! edge (APISIX `openid-connect`) in front of it does the Keycloak login and hands over the
//! user as `X-Userinfo` and the user's access token as `X-Access-Token`, and the gateway behind
//! it decides what that token may see (AP-28, GW10, ADR-N-019).
//!
//! The one rule worth stating twice: a write without `X-Access-Token` is refused here and
//! never retried anonymously (AP-40). An app that fell back would hand its own reachability to
//! whoever asked.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub mod assets;

/// The entity type this app was built for; it is also what the grant names.
const TYPE: &str = "AirQualityObserved";
/// The steward's free text on a station, one of the three attributes the form writes.
const NOTE: &str = "stewardNote";
/// A note is a sentence a person types, not a payload.
const NOTE_MAX: usize = 500;

/// What the reconciler hands the container (Architecture/16 §5).
#[derive(Clone, Debug)]
pub struct Config {
    /// `/apps/{name}/`, the path the app is served under.
    pub base_path: String,
    /// `https://{host}/api/endpoint/{slug}/`, the only data surface it may call (AP-04).
    pub endpoint_url: String,
    /// `true` on a `public` app, where a missing `X-Access-Token` is normal (AP-28).
    pub anonymous: bool,
    /// The Portal route that answers the caller's roles in this App (AP-109). Without it nobody
    /// holds a role, so the form is offered to nobody.
    pub me_url: Option<String>,
    /// `JC_APP_CONFIG`, the `#jc-config` object the reconciler hands a pod App (endpoints, and the
    /// project's `basemap` style URL when one is configured, AP-67), already escaped for the
    /// `<script>` element the page carries it in. `None`: the page is served as built.
    pub page_config: Option<String>,
}

impl Config {
    /// Reads the variables of Architecture/16 §5 and §13. A missing endpoint URL is fatal: an
    /// app with nowhere to read from has nothing to serve, and guessing a default would be
    /// guessing which data a user gets.
    pub fn from_env() -> Result<Self, String> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// [`Config::from_env`] over any lookup, so the parsing is tested without the process's
    /// environment, which tests running in parallel share.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let endpoint_url =
            var("JC_ENDPOINT_URL").ok_or_else(|| "JC_ENDPOINT_URL is not set".to_owned())?;
        Ok(Self {
            base_path: var("JC_BASE_PATH").unwrap_or_else(|| "/".to_owned()),
            endpoint_url: with_trailing_slash(&endpoint_url),
            anonymous: var("JC_ANONYMOUS").is_some_and(|value| value == "true"),
            me_url: var("JC_ME_URL").filter(|url| !url.is_empty()),
            page_config: match var("JC_APP_CONFIG") {
                Some(raw) if !raw.trim().is_empty() => Some(page_config(&raw)?),
                _ => None,
            },
        })
    }
}

/// `JC_APP_CONFIG` as the body of a `<script type="application/json">`: a JSON object, or the
/// app does not start. `<`, `>` and `&` only occur inside JSON strings, where their `\u` escapes
/// read the same, so no value can close the element early.
pub fn page_config(raw: &str) -> Result<String, String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|err| format!("JC_APP_CONFIG is not JSON: {err}"))?;
    if !value.is_object() {
        return Err("JC_APP_CONFIG is not a JSON object".to_owned());
    }
    Ok(value
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026"))
}

fn with_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_owned()
    } else {
        format!("{url}/")
    }
}

/// The application state: its configuration and the one client it makes requests with.
pub struct App {
    pub config: Config,
    http: reqwest::Client,
}

impl App {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
        }
    }

    fn url(&self, tail: &str) -> String {
        format!("{}{tail}", self.config.endpoint_url)
    }
}

/// The app's routes under its own base path, with the embedded UI behind them.
///
/// The prefix is spelled into every route rather than nested. `Router::nest` does not match
/// the prefix with a trailing slash, and `https://{host}/apps/air-quality/` with that slash
/// is exactly what a browser follows from the Portal, so nesting would 404 the front page.
pub fn router(app: Arc<App>) -> Router {
    let base = app.config.base_path.trim_end_matches('/').to_owned();
    let at = |tail: &str| format!("{base}{tail}");
    let router = Router::new()
        // The readiness probe of the pod, at the root and outside the base path: the kubelet
        // asks it, not a browser (Architecture/16 §5).
        .route("/healthz", get(healthz))
        .route(&at("/api/me"), get(me))
        .route(&at("/api/stations"), get(stations).post(create_station))
        .route(
            &at("/api/stations/{id}"),
            patch(update_station).delete(delete_station),
        )
        .route(&at("/api/stations/{id}/history"), get(history))
        // Both spellings of the front page: the edge routes the prefix, and a person who
        // types it without the slash is not a different visitor.
        .route(&at("/"), get(assets::static_handler))
        .route(&at("/{*path}"), get(assets::static_handler));
    // On its own host the base is `/` (AP-133), which `at("/")` already serves; axum refuses
    // an empty path, and the pod crashed at start on dev with it (T-2909).
    if base.is_empty() {
        router.with_state(app)
    } else {
        router
            .route(&base, get(assets::static_handler))
            .with_state(app)
    }
}

/// What the edge says about the caller (AP-28). None of it is trusted for a decision: the
/// token is forwarded to the gateway, which is what actually decides; the userinfo is shown
/// on the page and nowhere else.
#[derive(Default)]
struct Caller {
    token: Option<String>,
    email: Option<String>,
    user: Option<String>,
}

/// The forwarded token stays out of every log line.
impl std::fmt::Debug for Caller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Caller")
            .field("token", &self.token.as_ref().map(|_| "[redacted]"))
            .field("email", &self.email)
            .field("user", &self.user)
            .finish()
    }
}

impl Caller {
    fn of(headers: &HeaderMap) -> Self {
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let userinfo = header("x-userinfo")
            .and_then(|encoded| decode_userinfo(&encoded))
            .unwrap_or_default();
        let field = |name: &str| userinfo.get(name)?.as_str().map(str::to_owned);
        Self {
            token: header("x-access-token"),
            email: field("email"),
            user: field("preferred_username")
                .or_else(|| field("name"))
                .or_else(|| field("sub")),
        }
    }
}

/// `X-Userinfo` is the userinfo document, base64 encoded, the way lua-resty-openidc sends it.
/// Padded standard base64 is what it produces; the other three spellings cost one line each
/// and a header that is none of them is simply no userinfo.
fn decode_userinfo(encoded: &str) -> Option<Value> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
        .iter()
        .find_map(|engine| engine.decode(encoded).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .filter(Value::is_object)
}

/// The readiness answer the pod's probe reads: the process is up and can serve.
async fn healthz() -> &'static str {
    "ok"
}

/// A failure the caller is allowed to see. The gateway's own problem document is passed
/// through byte for byte where there is one, because its refusal is more precise than
/// anything this app could invent (GW6).
#[derive(Debug)]
pub enum AppError {
    /// The app's own refusal.
    Refused(StatusCode, &'static str),
    /// The endpoint answered, and its answer is the answer.
    Upstream(StatusCode, String),
    /// The endpoint could not be reached at all.
    Unreachable,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::Refused(status, detail) => (
                status,
                json!({
                    "type": "https://joinedcontext.com/errors/app",
                    "title": status.canonical_reason().unwrap_or("Error"),
                    "status": status.as_u16(),
                    "detail": detail,
                })
                .to_string(),
            ),
            Self::Upstream(status, body) => (status, body),
            Self::Unreachable => (
                StatusCode::BAD_GATEWAY,
                json!({
                    "type": "https://joinedcontext.com/errors/app",
                    "title": "Bad Gateway",
                    "status": 502,
                    "detail": "the endpoint did not answer",
                })
                .to_string(),
            ),
        };
        (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
            body,
        )
            .into_response()
    }
}

/// An entity id reaches the upstream URL as a path segment, so it is checked before it is
/// used rather than escaped afterwards: the platform's ids are URNs of a known shape (R2),
/// and anything else is a caller trying to steer the request somewhere else.
fn valid_urn(id: &str) -> bool {
    id.starts_with("urn:ngsi-ld:")
        && id.len() <= 256
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-'))
}

impl App {
    /// One GET against the endpoint, with the caller's own token when the edge handed one
    /// over. Without a token the call is anonymous, which is what a `public` app does (AP-28).
    async fn get(
        &self,
        tail: &str,
        query: &[(&str, String)],
        caller: &Caller,
    ) -> Result<Value, AppError> {
        let mut request = self.http.get(self.url(tail)).query(query);
        if let Some(token) = &caller.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|_| AppError::Unreachable)?;
        let status =
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let body = response.text().await.map_err(|_| AppError::Unreachable)?;
        if !status.is_success() {
            return Err(AppError::Upstream(status, body));
        }
        serde_json::from_str(&body).map_err(|_| AppError::Unreachable)
    }

    /// The caller's roles in this App, from the Portal's `/me` (AP-109, Architecture/16 §13).
    /// A failure to ask is no role: the form is hidden when the answer is not a clear list
    /// (fail closed). The roles only decide what the page offers; the gateway decides the write.
    async fn roles(&self, caller: &Caller) -> Vec<String> {
        let (Some(url), Some(token)) = (&self.config.me_url, &caller.token) else {
            return Vec::new();
        };
        let Ok(response) = self.http.get(url).bearer_auth(token).send().await else {
            return Vec::new();
        };
        if !response.status().is_success() {
            return Vec::new();
        }
        response
            .json::<Value>()
            .await
            .ok()
            .and_then(|me| {
                me.get("roles")?.as_array().map(|roles| {
                    roles
                        .iter()
                        .filter_map(|role| role.as_str().map(str::to_owned))
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    /// One write against the endpoint with the caller's token; the gateway's refusal is the
    /// answer, word for word (AP-40, GW6).
    async fn write(&self, request: reqwest::RequestBuilder) -> Result<StatusCode, AppError> {
        let response = request.send().await.map_err(|_| AppError::Unreachable)?;
        let status =
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        if status.is_success() {
            return Ok(status);
        }
        Err(AppError::Upstream(
            status,
            response.text().await.unwrap_or_default(),
        ))
    }
}

/// Who the browser is talking as, and the roles that decide whether the form is offered.
async fn me(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let caller = Caller::of(&headers);
    let roles = app.roles(&caller).await;
    Json(json!({
        "signedIn": caller.token.is_some(),
        "email": caller.email,
        "user": caller.user,
        "anonymous": app.config.anonymous,
        "roles": roles,
    }))
    .into_response()
}

/// The station list, flattened out of the normalized NGSI-LD the endpoint serves.
async fn stations(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    let caller = Caller::of(&headers);
    let entities = app
        .get(
            "ngsi-ld/v1/entities",
            &[("type", TYPE.to_owned()), ("limit", "100".to_owned())],
            &caller,
        )
        .await?;
    let list: Vec<Value> = entities
        .as_array()
        .map(|entities| entities.iter().map(station).collect())
        .unwrap_or_default();
    Ok(Json(json!(list)))
}

/// One day of one station, from the temporal surface the grant already covers.
async fn history(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    if !valid_urn(&id) {
        return Err(AppError::Refused(
            StatusCode::BAD_REQUEST,
            "a station id is an NGSI-LD URN",
        ));
    }
    let caller = Caller::of(&headers);
    let entity = app
        .get(
            &format!("ngsi-ld/v1/temporal/entities/{id}"),
            &[
                ("attrs", "pm10,pm25".to_owned()),
                ("timerel", "after".to_owned()),
                // The grant itself is windowed to P1D (AP-39); asking for more would be
                // asking the PDP to narrow it again for nothing.
                ("timeAt", day_ago()),
            ],
            &caller,
        )
        .await?;
    Ok(Json(entity))
}

/// The caller's token, or the refusal every write gives without one (AP-40). A public app reads
/// anonymously, but nothing writes anonymously, so this does not look at `config.anonymous`:
/// there is no configuration that turns it off.
fn token_of(caller: &Caller) -> Result<&str, AppError> {
    caller.token.as_deref().ok_or(AppError::Refused(
        StatusCode::UNAUTHORIZED,
        "a write needs a signed-in user",
    ))
}

fn refused(detail: &'static str) -> AppError {
    AppError::Refused(StatusCode::BAD_REQUEST, detail)
}

/// What the record form sends. Only the station attributes a person may correct exist here and
/// an unknown field is refused, so a measured value (`pm10`, `pm25`, `airQualityIndex`,
/// `dateObserved`) or `source` can never be written through this app; the gateway's grant says
/// the same a second time.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StationFields {
    /// The last segment of a new station's URN; on create only.
    pub local_id: Option<String>,
    /// The station's name per language, written whole (a `LanguageProperty`).
    pub name: Option<BTreeMap<String, String>>,
    /// `[longitude, latitude]`.
    pub coordinates: Option<[f64; 2]>,
    pub steward_note: Option<String>,
}

/// `fi`, `en`, `sv`, `en-GB`: a BCP 47 language with an optional region, nothing longer.
fn valid_language(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let language = parts.next().unwrap_or_default();
    let region = parts.next();
    (2..=3).contains(&language.len())
        && language.chars().all(|c| c.is_ascii_lowercase())
        && region.is_none_or(|r| r.len() == 2 && r.chars().all(|c| c.is_ascii_uppercase()))
        && parts.next().is_none()
}

fn valid_local_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// The NGSI-LD attributes of the fields that were sent, checked before anything leaves the pod.
fn attributes(fields: &StationFields) -> Result<Map<String, Value>, AppError> {
    let mut attrs = Map::new();
    if let Some(names) = &fields.name {
        let mut map = Map::new();
        for (language, text) in names {
            let text = text.trim();
            if !valid_language(language) {
                return Err(refused("a name's language is a code such as fi or en"));
            }
            if text.is_empty() {
                continue;
            }
            if text.chars().count() > 200 {
                return Err(refused("a name is at most 200 characters"));
            }
            map.insert(language.clone(), json!(text));
        }
        if map.is_empty() {
            return Err(refused("a station has a name in at least one language"));
        }
        attrs.insert(
            "name".to_owned(),
            json!({ "type": "LanguageProperty", "languageMap": map }),
        );
    }
    if let Some([longitude, latitude]) = fields.coordinates {
        if !(-180.0..=180.0).contains(&longitude) || !(-90.0..=90.0).contains(&latitude) {
            return Err(refused(
                "a position is a longitude between -180 and 180 and a latitude between -90 and 90",
            ));
        }
        attrs.insert(
            "location".to_owned(),
            json!({ "type": "GeoProperty", "value": { "type": "Point", "coordinates": [longitude, latitude] } }),
        );
    }
    if let Some(note) = &fields.steward_note {
        let note = note.trim();
        if note.is_empty() || note.chars().count() > NOTE_MAX {
            return Err(refused("a note is between one and 500 characters"));
        }
        attrs.insert(
            NOTE.to_owned(),
            json!({ "type": "Property", "value": note }),
        );
    }
    Ok(attrs)
}

/// A new station: one `POST /entities` with the caller's token (AP-62).
async fn create_station(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(fields): Json<StationFields>,
) -> Result<Response, AppError> {
    let caller = Caller::of(&headers);
    let token = token_of(&caller)?;
    let local_id = fields.local_id.as_deref().unwrap_or_default();
    if !valid_local_id(local_id) {
        return Err(refused(
            "a station id is 1 to 64 letters, digits, '-' or '_'",
        ));
    }
    if fields.name.is_none() || fields.coordinates.is_none() {
        return Err(refused("a new station has a name and a position"));
    }
    let attrs = attributes(&fields)?;
    // ponytail: the `{domain}:{space}` of the id is read off a station the endpoint already
    // serves, so the first record of an empty space cannot be added here; the reconciler handing
    // the app its organization domain and space would lift that.
    let known = app
        .get(
            "ngsi-ld/v1/entities",
            &[("type", TYPE.to_owned()), ("limit", "1".to_owned())],
            &caller,
        )
        .await?;
    let Some(prefix) = known
        .as_array()
        .and_then(|list| list.first())
        .and_then(|entity| entity.get("id")?.as_str())
        .and_then(|id| id.rsplit_once(':'))
        .map(|(prefix, _)| prefix.to_owned())
    else {
        return Err(AppError::Refused(
            StatusCode::CONFLICT,
            "the space has no station yet to take the id's organization and space from",
        ));
    };
    let id = format!("{prefix}:{local_id}");
    if !valid_urn(&id) || !id.starts_with(&format!("urn:ngsi-ld:{TYPE}:")) {
        return Err(AppError::Unreachable);
    }
    let mut entity = attrs;
    entity.insert("id".to_owned(), json!(id));
    entity.insert("type".to_owned(), json!(TYPE));
    app.write(
        app.http
            .post(app.url("ngsi-ld/v1/entities"))
            .bearer_auth(token)
            .json(&entity),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

/// A correction: one `PATCH /entities/{id}/attrs` with only the attributes that were sent.
async fn update_station(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(fields): Json<StationFields>,
) -> Result<Response, AppError> {
    let caller = Caller::of(&headers);
    let token = token_of(&caller)?;
    if !valid_urn(&id) {
        return Err(refused("a station id is an NGSI-LD URN"));
    }
    if fields.local_id.is_some() {
        return Err(refused("a station's id does not change"));
    }
    let attrs = attributes(&fields)?;
    if attrs.is_empty() {
        return Err(refused("nothing to change"));
    }
    app.write(
        app.http
            .patch(app.url(&format!("ngsi-ld/v1/entities/{id}/attrs")))
            .bearer_auth(token)
            .json(&attrs),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// A removal. The gateway admits it only for a station a steward added, one without `source`,
/// by evaluating the grant's `q` against the stored entity (R45); this app does not repeat that.
async fn delete_station(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let caller = Caller::of(&headers);
    let token = token_of(&caller)?;
    if !valid_urn(&id) {
        return Err(refused("a station id is an NGSI-LD URN"));
    }
    app.write(
        app.http
            .delete(app.url(&format!("ngsi-ld/v1/entities/{id}")))
            .bearer_auth(token),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The normalized form carries `{"pm10": {"type": "Property", "value": 34.2}}`; the browser
/// wants the number. An attribute the grant hides is simply absent, and stays absent here.
fn station(entity: &Value) -> Value {
    let mut out = Map::new();
    out.insert("id".to_owned(), entity["id"].clone());
    for attr in ["name", "pm10", "pm25", "airQualityIndex", NOTE] {
        if let Some(value) = entity.get(attr).and_then(|attr| attr.get("value")) {
            out.insert(attr.to_owned(), value.clone());
        }
    }
    // The `helsinki` model's `name` is a `LanguageProperty`: the form gets every language, the
    // card the English one, else the Finnish one, else whichever there is.
    if let Some(names) = entity
        .get("name")
        .and_then(|name| name.get("languageMap"))
        .and_then(Value::as_object)
    {
        if let Some(shown) = ["en", "fi"]
            .iter()
            .find_map(|language| names.get(*language))
            .or_else(|| names.values().next())
        {
            out.insert("name".to_owned(), shown.clone());
        }
        out.insert("names".to_owned(), Value::Object(names.clone()));
    }
    // A record a pipeline wrote says where it came from; a station a steward added does not,
    // and only that one is the steward's to remove.
    out.insert("own".to_owned(), json!(entity.get("source").is_none()));
    if let Some(coordinates) = entity
        .get("location")
        .and_then(|location| location.get("value"))
        .and_then(|geometry| geometry.get("coordinates"))
    {
        out.insert("coordinates".to_owned(), coordinates.clone());
    }
    if let Some(observed) = entity.get("pm10").and_then(|pm10| pm10.get("observedAt")) {
        out.insert("observedAt".to_owned(), observed.clone());
    }
    Value::Object(out)
}

/// An ISO 8601 instant 24 hours ago, formatted the way the temporal API wants it.
fn day_ago() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
        .saturating_sub(24 * 60 * 60);
    format_utc(seconds)
}

/// `seconds` since the epoch as `YYYY-MM-DDThh:mm:ssZ`. The civil-date conversion is Howard
/// Hinnant's, the same arithmetic every date library runs; a date crate for one call is a
/// dependency for one call.
fn format_utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caller_debug_never_shows_the_token() {
        let caller = Caller {
            token: Some("s3cr3t-test".into()),
            ..Caller::default()
        };
        let text = format!("{caller:?}");
        assert!(
            !text.contains("s3cr3t-test") && text.contains("[redacted]"),
            "{text}"
        );
    }

    #[test]
    fn only_ngsi_ld_urns_reach_the_upstream_path() {
        assert!(valid_urn(
            "urn:ngsi-ld:AirQualityObserved:hel.fi:air-quality:station-01"
        ));
        assert!(!valid_urn("urn:ngsi-ld:X/../../admin"));
        assert!(!valid_urn("urn:ngsi-ld:X?type=Other"));
        assert!(!valid_urn("../secrets"));
        assert!(!valid_urn(""));
    }

    #[test]
    fn a_station_keeps_only_what_the_grant_returned() {
        let entity = json!({
            "id": "urn:ngsi-ld:AirQualityObserved:hel.fi:air-quality:s1",
            "type": TYPE,
            "name": { "type": "Property", "value": "Kallio" },
            "pm10": { "type": "Property", "value": 34.2, "observedAt": "2026-09-06T10:00:00Z" },
            "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [19.1, 48.7] } }
        });
        let flat = station(&entity);
        assert_eq!(flat["pm10"], json!(34.2));
        assert_eq!(flat["coordinates"], json!([19.1, 48.7]));
        assert_eq!(flat["observedAt"], json!("2026-09-06T10:00:00Z"));
        // pm25 was not in the answer, so it is not in the card either.
        assert!(flat.get("pm25").is_none());
    }

    #[test]
    fn the_epoch_and_a_known_instant_format_the_way_the_temporal_api_reads_them() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_788_696_000), "2026-09-06T12:00:00Z");
    }

    #[test]
    fn the_endpoint_url_always_ends_in_a_slash_so_paths_append_cleanly() {
        assert_eq!(
            with_trailing_slash("https://h/api/endpoint/s"),
            "https://h/api/endpoint/s/"
        );
        assert_eq!(
            with_trailing_slash("https://h/api/endpoint/s/"),
            "https://h/api/endpoint/s/"
        );
    }
}
