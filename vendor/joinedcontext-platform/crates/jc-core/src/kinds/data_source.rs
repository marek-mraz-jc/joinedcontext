//! Manifest kind for the connection of one external feed (T-0290, MF-35, PL-39).
//!
//! A `DataSource` says where a feed is and how to authenticate to it, and stops there: no
//! mapping, no schedule, no target. It moves nothing on its own; a [`PipelineSpec`] references
//! one and the reconciler renders it into that pipeline's Bento input (Architecture/08 §6).
//!
//! Every credential is a [`SecretRef`]. That is not a convention here, it is the shape of the
//! type: there is no `password`, `token` or `apiKey` member anywhere below, so a plaintext
//! credential is a parse error rather than a review finding (CC-06, PL-14).
//!
//! [`PipelineSpec`]: crate::kinds::PipelineSpec

use crate::envelope::{Kind, ObjectMeta, Scope, SecretRef, TypedRef};
use crate::error::{Error, Result};
use crate::kinds::PipelineSpec;
use crate::names;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::LazyLock;

use super::bento_inputs;

static ENV_VAR_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Z][A-Z0-9_]*$").expect("valid regex"));

/// The password of a `user:password@` part, or of a `password=` parameter, in a connection string.
static EMBEDDED_PASSWORD_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)(?:[^\s/@:?&=]+:([^\s/@]+)@|(?:^|[?&;\s])password=([^&;\s]+))")
        .expect("valid regex")
});

/// Which kind of feed a [`DataSourceSpec`] connects to: one of the four typed feeds or any
/// input the runner ships (MF-35, PL-50).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DataSourceType {
    /// An MQTT broker the runner subscribes to.
    Mqtt,
    /// An HTTP resource the runner polls.
    Http,
    /// A WebSocket the runner keeps open.
    WebSocket,
    /// A GTFS-realtime protobuf feed, polled over HTTP and decoded by the runner.
    GtfsRt,
    /// Events a sender pushes to the runner's shared ingest listener (PL-122, ADR-N-071).
    Webhook,
    /// Any input the pinned Bento runner ships (PL-50).
    Runner(String),
}

impl DataSourceType {
    /// The wire name, as written in `spec.type`.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Mqtt => "mqtt",
            Self::Http => "http",
            Self::WebSocket => "websocket",
            Self::GtfsRt => "gtfs-rt",
            Self::Webhook => "webhook",
            Self::Runner(name) => name.as_str(),
        }
    }

    /// The runner input name if this is a runner-provided input.
    pub fn runner_name(&self) -> Option<&str> {
        match self {
            Self::Runner(name) => Some(name.as_str()),
            _ => None,
        }
    }
}

impl fmt::Display for DataSourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Serialize for DataSourceType {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DataSourceType {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "mqtt" => Self::Mqtt,
            "http" => Self::Http,
            "websocket" => Self::WebSocket,
            "gtfs-rt" => Self::GtfsRt,
            "webhook" => Self::Webhook,
            _ => Self::Runner(s),
        })
    }
}

impl JsonSchema for DataSourceType {
    fn schema_name() -> String {
        "DataSourceType".to_string()
    }

    fn json_schema(_gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        schemars::schema::SchemaObject {
            instance_type: Some(schemars::schema::InstanceType::String.into()),
            metadata: Some(Box::new(schemars::schema::Metadata {
                description: Some(
                    "Which kind of feed a DataSource connects to: a typed connection or any runner input (MF-35, PL-50)."
                        .to_string(),
                ),
                examples: vec![
                    serde_json::json!("mqtt"),
                    serde_json::json!("http"),
                    serde_json::json!("websocket"),
                    serde_json::json!("gtfs-rt"),
                    serde_json::json!("webhook"),
                    serde_json::json!("nats"),
                    serde_json::json!("csv"),
                    serde_json::json!("http_client"),
                ],
                ..Default::default()
            })),
            ..Default::default()
        }
        .into()
    }
}

/// An MQTT connection (Architecture/08 §6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MqttConnection {
    /// Broker URLs, `tcp://host:1883` or `tls://host:8883`; at least one, unless `brokerRef`
    /// names a broker of the platform instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    /// An `MqttBroker` of the DataSource's own project, read on its internal listener with no
    /// credential (PL-76); takes the place of `urls`, `username` and `passwordRef`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_ref: Option<TypedRef>,
    /// Topic filters to subscribe to; at least one.
    pub topics: Vec<String>,
    /// Quality of service, 0, 1 or 2. Absent leaves Bento's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qos: Option<u8>,
    /// Whether the broker forgets the subscription between connections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean_session: Option<bool>,
    /// The user the runner authenticates as; the password is a reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Where the password comes from (CC-06).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_ref: Option<SecretRef>,
}

/// An HTTP resource the runner polls (Architecture/08 §6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HttpConnection {
    /// The URL to fetch, `http://` or `https://`.
    pub url: String,
    /// HTTP method, `GET` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    /// Static request headers. A header carrying a credential belongs in `authorization`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// The `Authorization` header, built from a secret (CC-06).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<Authorization>,
    /// Request timeout as a Bento duration, `10s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    /// The members of a zip archive to read, by HTTP `Range`, instead of the whole body (PL-72).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<HttpArchive>,
    /// How the body becomes messages: `csv` or `xlsx` hands each row to the steps as one message
    /// keyed by the header's column names; absent, the whole body is one message (PL-114).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<HttpBodyFormat>,
    /// Where the records sit in a JSON body: a dotted path to an array, `data.stations` for a
    /// GBFS feed. The runner hands each element to the steps as one message; absent, the whole
    /// body is one message. Names only, never an expression (PL-114).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<String>,
    /// The largest response body accepted, in bytes; 32 MiB when absent, at most 256 MiB. The
    /// runner refuses a larger body before any step of the pipeline sees it (PL-113).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// How the runner walks a paged source page by page (PL-31, T-7056): each page is one
    /// message, split by `records` as a single body is. Absent, the URL is read once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paging: Option<HttpPaging>,
    /// A known kind of open data API (T-7067): the runner sends its query parameters and walks
    /// its pages, so `paging` and `records` stay unset. CKAN, ArcGIS, OData, WFS, OGC API
    /// Features or SensorThings, with the URL of its search, query or items resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<HttpPreset>,
}

impl HttpConnection {
    /// The body bound when the manifest names none.
    pub const DEFAULT_MAX_BYTES: u64 = 32 * 1024 * 1024;
    /// The largest bound a manifest may set.
    pub const MAX_BYTES: u64 = 256 * 1024 * 1024;

    /// The bound in force.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.unwrap_or(Self::DEFAULT_MAX_BYTES)
    }

    /// The connection the runner reads: with a `preset`, its query parameters on the URL and its
    /// `paging` and `records` in place (T-7067); without one, the connection as written.
    pub fn effective(&self) -> std::borrow::Cow<'_, Self> {
        match &self.preset {
            Some(preset) => std::borrow::Cow::Owned(preset.apply(self)),
            None => std::borrow::Cow::Borrowed(self),
        }
    }
}

/// The most names a `records` path holds.
pub const RECORDS_PATH_NAMES: usize = 8;
/// The most bytes one name of a `records` path holds.
pub const RECORDS_NAME_BYTES: usize = 64;

/// The Bloblang mapping that takes the array at the dotted `records` path out of a JSON body
/// (PL-114), the one split the runner, the sample and the DataSource Check run: a body without
/// an array there fails with what it holds instead. Each name is letters, digits, `_` or `-` and
/// goes into the mapping quoted, so the path is read, never evaluated.
pub fn records_mapping(path: &str) -> std::result::Result<String, &'static str> {
    let names: Vec<&str> = path.split('.').collect();
    if names.len() > RECORDS_PATH_NAMES {
        return Err("a records path holds at most 8 names");
    }
    let plain = |name: &&str| {
        !name.is_empty()
            && name.len() <= RECORDS_NAME_BYTES
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    if !names.iter().all(plain) {
        return Err("a records path is dot-separated names of letters, digits, `_` and `-`, each 1 to 64, such as data.stations");
    }
    let at: String = names.iter().map(|name| format!(".\"{name}\"")).collect();
    Ok(format!(
        "let records = this{at}\nroot = if $records.type() == \"array\" {{ $records }} else {{ throw(\"the body holds no array at records path {path}, it holds %s\".format($records.type())) }}"
    ))
}

/// How an `http` connection splits the body it fetched into messages (PL-114).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HttpBodyFormat {
    /// A CSV file with a header row: one message per row, the shape Bento's `csv` input sends.
    Csv,
    /// A spreadsheet (xlsx, xlsm, xlsb, xls or ods): one message per row of its first sheet,
    /// keyed by the header row, each value as text the way the sheet's CSV export writes it,
    /// dates in RFC 3339 (the runner's `unarchive` format `xlsx`, T-7058).
    Xlsx,
}

/// The named members of the archive an `http` connection serves (PL-72, Architecture/08 §6).
///
/// The runner reads the directory from the archive's last 64 KiB and then each member by its
/// own range, so a member nobody named is never fetched or inflated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HttpArchive {
    /// The archive format; `zip` is the only one.
    pub format: ArchiveFormat,
    /// The member names to read, at most 20, each letters, digits, `.`, `_`, `-` and `/`.
    pub members: Vec<String>,
    /// The largest member accepted, packed and unpacked, in bytes; 32 MiB when absent, at most
    /// 256 MiB. A deflated member's inflate stops at the size its directory declares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

impl HttpArchive {
    /// The member size bound when the manifest names none.
    pub const DEFAULT_MAX_BYTES: u64 = 32 * 1024 * 1024;
    /// The largest bound a manifest may set.
    pub const MAX_BYTES: u64 = 256 * 1024 * 1024;
    /// The most members one connection reads.
    pub const MAX_MEMBERS: usize = 20;

    /// The bound in force.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.unwrap_or(Self::DEFAULT_MAX_BYTES)
    }

    fn validate(&self, http: &HttpConnection) -> Result<()> {
        let refuse = |field, value: String, reason| {
            Err(Error::Name {
                field,
                value,
                reason,
            })
        };
        if self.members.is_empty() || self.members.len() > Self::MAX_MEMBERS {
            return refuse(
                "spec.http.archive.members",
                self.members.len().to_string(),
                "an archive names 1 to 20 members",
            );
        }
        for member in &self.members {
            let plain = !member.is_empty()
                && member.len() <= 255
                && member
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c));
            if !plain {
                return refuse(
                    "spec.http.archive.members",
                    member.clone(),
                    "a member name is 1 to 255 letters, digits, '.', '_', '-' and '/'",
                );
            }
        }
        if !(1..=Self::MAX_BYTES).contains(&self.max_bytes()) {
            return refuse(
                "spec.http.archive.maxBytes",
                self.max_bytes().to_string(),
                "maxBytes is 1 to 268435456 (256 MiB)",
            );
        }
        if http.verb.as_deref().is_some_and(|verb| verb != "GET") {
            return refuse(
                "spec.http.verb",
                http.verb.clone().unwrap_or_default(),
                "an archive is read with GET",
            );
        }
        if let Some(range) = http
            .headers
            .keys()
            .find(|h| h.eq_ignore_ascii_case("range"))
        {
            return refuse(
                "spec.http.headers",
                range.clone(),
                "an archive read sets its own Range header",
            );
        }
        Ok(())
    }
}

/// How a paged `http` source is walked (PL-31, T-7056): rendered to the runner's `paginate`
/// option, so no mapping writes its own loop. `records` (`spec.http.records`) names each page's
/// array; `page`, `offset`, `total` and `maxRecords` count it. A run is complete only when a stop
/// rule held: an empty or short page, the total reached, `more` false, or no next cursor or URL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HttpPaging {
    /// `page`, `offset`, `cursor`, `link` (RFC 8288 `rel=next`) or `nextUrl`.
    pub mode: HttpPagingMode,
    /// `page`: the page number parameter, `page` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    /// `page` and `offset`: the first page number (1) or offset (0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    /// `page`: the page size parameter, sent with `size`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_param: Option<String>,
    /// `page`: the page size asked for; a shorter page ends the walk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// `offset`: the offset parameter, `offset` when absent (ArcGIS `resultOffset`, OData `$skip`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_param: Option<String>,
    /// `offset`: the page size parameter, `limit` when absent (ArcGIS `resultRecordCount`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_param: Option<String>,
    /// `offset`: the page size asked for. The offset moves by the records received.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// `cursor`: the body path to the next cursor, `meta.next`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// `cursor`: the query parameter the cursor is sent in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_param: Option<String>,
    /// `cursor`: the header the cursor is sent in, instead of a parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_header: Option<String>,
    /// `nextUrl`: the body path to the next URL, `@odata.nextLink` or `links[rel=next].href`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// The body path to the count the first page announces, `numberMatched`; a run that read
    /// another number of records fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<String>,
    /// The body path to a boolean saying another page exists, `exceededTransferLimit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more: Option<String>,
    /// The most pages one run reads, 1 to 10000, 1000 when absent; reaching it fails the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pages: Option<u64>,
    /// The most records one run reads; reaching it before a stop rule fails the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_records: Option<u64>,
    /// The pause between pages as a duration, `200ms`, at most a minute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<String>,
    /// A parameter that keeps the order stable across pages, `orderByFields=OBJECTID`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_by: Option<HttpPagingOrder>,
}

/// How a paged source names its next page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HttpPagingMode {
    /// A page number parameter.
    #[default]
    Page,
    /// An offset and a page size parameter.
    Offset,
    /// A cursor read from the body and sent back.
    Cursor,
    /// The `Link` header's `rel=next` target.
    Link,
    /// A next URL read from the body.
    NextUrl,
}

impl HttpPagingMode {
    /// The runner's name of the mode.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Offset => "offset",
            Self::Cursor => "cursor",
            Self::Link => "link",
            Self::NextUrl => "next_url",
        }
    }
}

/// The order parameter of a paged source.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HttpPagingOrder {
    /// The parameter name, `orderByFields` or `$orderby`.
    pub param: String,
    /// Its value, `OBJECTID`.
    pub value: String,
}

impl HttpPaging {
    /// The most pages a manifest may set, the runner's own bound.
    pub const MAX_PAGES: u64 = 10_000;

    fn validate(&self, http: &HttpConnection) -> Result<()> {
        let refuse = |field, value: String, reason| {
            Err(Error::Name {
                field,
                value,
                reason,
            })
        };
        use HttpPagingMode as M;
        let fields: [(&'static str, bool, &[M]); 11] = [
            ("spec.http.paging.param", self.param.is_some(), &[M::Page]),
            (
                "spec.http.paging.start",
                self.start.is_some(),
                &[M::Page, M::Offset],
            ),
            (
                "spec.http.paging.sizeParam",
                self.size_param.is_some(),
                &[M::Page],
            ),
            ("spec.http.paging.size", self.size.is_some(), &[M::Page]),
            (
                "spec.http.paging.offsetParam",
                self.offset_param.is_some(),
                &[M::Offset],
            ),
            (
                "spec.http.paging.limitParam",
                self.limit_param.is_some(),
                &[M::Offset],
            ),
            ("spec.http.paging.limit", self.limit.is_some(), &[M::Offset]),
            (
                "spec.http.paging.cursor",
                self.cursor.is_some(),
                &[M::Cursor],
            ),
            (
                "spec.http.paging.cursorParam",
                self.cursor_param.is_some(),
                &[M::Cursor],
            ),
            (
                "spec.http.paging.cursorHeader",
                self.cursor_header.is_some(),
                &[M::Cursor],
            ),
            ("spec.http.paging.next", self.next.is_some(), &[M::NextUrl]),
        ];
        if let Some((field, _, _)) = fields
            .iter()
            .find(|(_, set, modes)| *set && !modes.contains(&self.mode))
        {
            return refuse(
                field,
                self.mode.as_str().to_string(),
                "this field belongs to another paging mode",
            );
        }
        let names = [
            ("spec.http.paging.param", &self.param),
            ("spec.http.paging.sizeParam", &self.size_param),
            ("spec.http.paging.offsetParam", &self.offset_param),
            ("spec.http.paging.limitParam", &self.limit_param),
            ("spec.http.paging.cursorParam", &self.cursor_param),
        ];
        for (field, name) in names {
            if let Some(name) = name {
                if !plain(name, 64, "_-.$:") {
                    return refuse(field, name.clone(), "a parameter name is 1 to 64 letters, digits, '_', '-', '.', '$' and ':', such as resultOffset or $skip");
                }
            }
        }
        if let Some(name) = &self.cursor_header {
            if header_name(name).is_err() {
                return refuse(
                    "spec.http.paging.cursorHeader",
                    name.clone(),
                    "a header name is a token: letters, digits and !#$%&'*+-.^_`|~",
                );
            }
        }
        let paths = [
            ("spec.http.paging.cursor", &self.cursor),
            ("spec.http.paging.next", &self.next),
            ("spec.http.paging.total", &self.total),
            ("spec.http.paging.more", &self.more),
        ];
        for (field, path) in paths {
            if let Some(path) = path {
                if !plain(path, 256, "_-.@$:[]=") {
                    return refuse(field, path.clone(), "a body path is 1 to 256 letters, digits and _-.@$:[]=, such as @odata.nextLink or links[rel=next].href");
                }
            }
        }
        let mode = self.mode.as_str().to_string();
        let needs = |field, set: bool, reason| {
            if set {
                Ok(())
            } else {
                refuse(field, mode.clone(), reason)
            }
        };
        match self.mode {
            M::Page => needs(
                "spec.http.paging.size",
                self.size_param.is_none() || self.size.is_some(),
                "sizeParam is sent with size, the page size asked for",
            )?,
            M::Offset => needs(
                "spec.http.paging.limit",
                self.limit.is_some_and(|n| n > 0),
                "offset paging needs limit, the page size asked for, at least 1",
            )?,
            M::Cursor => {
                needs(
                    "spec.http.paging.cursor",
                    self.cursor.is_some(),
                    "cursor paging needs cursor, the body path to the next cursor",
                )?;
                needs(
                    "spec.http.paging.cursorParam",
                    self.cursor_param.is_some() != self.cursor_header.is_some(),
                    "cursor paging sends the cursor in one of cursorParam or cursorHeader",
                )?;
            }
            M::NextUrl => needs(
                "spec.http.paging.next",
                self.next.is_some(),
                "nextUrl paging needs next, the body path to the next URL",
            )?,
            M::Link => {}
        }
        let counted = matches!(self.mode, M::Page | M::Offset)
            || self.total.is_some()
            || self.max_records.is_some();
        if counted && http.records.is_none() {
            return refuse(
                "spec.http.records",
                self.mode.as_str().to_string(),
                "page and offset paging, total and maxRecords count the records: name their array in records",
            );
        }
        if http.archive.is_some() || http.format == Some(HttpBodyFormat::Csv) {
            return refuse(
                "spec.http.paging",
                if http.archive.is_some() {
                    "archive"
                } else {
                    "csv"
                }
                .to_string(),
                "paging reads JSON pages; an archive or a csv body is read whole",
            );
        }
        if let Some(max) = self.max_pages {
            if !(1..=Self::MAX_PAGES).contains(&max) {
                return refuse(
                    "spec.http.paging.maxPages",
                    max.to_string(),
                    "maxPages is 1 to 10000",
                );
            }
        }
        if self.max_records == Some(0) {
            return refuse(
                "spec.http.paging.maxRecords",
                "0".to_string(),
                "maxRecords is at least 1",
            );
        }
        if let Some(delay) = &self.delay {
            if !short_duration(delay) {
                return refuse(
                    "spec.http.paging.delay",
                    delay.clone(),
                    "delay is a duration of at most a minute, such as 200ms or 2s",
                );
            }
        }
        if let Some(order) = &self.order_by {
            if !plain(&order.param, 64, "_-.$:") {
                return refuse(
                    "spec.http.paging.orderBy.param",
                    order.param.clone(),
                    "a parameter name is 1 to 64 letters, digits, '_', '-', '.', '$' and ':'",
                );
            }
            let printable = !order.value.is_empty()
                && order.value.len() <= 128
                && !order.value.chars().any(char::is_control);
            if !printable {
                return refuse(
                    "spec.http.paging.orderBy.value",
                    order.value.clone(),
                    "an order value is 1 to 128 printable characters, such as OBJECTID",
                );
            }
        }
        Ok(())
    }
}

/// A known kind of open data API an `http` connection reads (T-7067): the preset sends the
/// API's own query parameters and walks its pages, so a person picks the kind and the URL and
/// types no parameter. Each field belongs to the kinds its comment names; another kind refuses it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HttpPreset {
    /// `ckan`, `arcgisFeature`, `odata`, `wfs`, `ogcFeatures` or `sensorThings`.
    pub kind: HttpPresetKind,
    /// Every kind: the records asked for per page, 1 to 10000, 1000 when absent. `odata` with
    /// a page size pages by `$top`/`$skip`; without one it follows `@odata.nextLink`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u64>,
    /// `ckan` (`package_search`): the one organization whose datasets are read, `bcn`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    /// `ckan` (`datastore_search`): the resource whose rows are read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    /// `arcgisFeature`: the field that keeps the order stable across pages, `OBJECTID` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id_field: Option<String>,
    /// `arcgisFeature`: the fields read, named one by one (never `*`), so a new column of the
    /// layer reaches no pipeline unasked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub out_fields: Vec<String>,
    /// `wfs`: the feature type read, `avoindata:Helsinki_osoiteluettelo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_names: Option<String>,
    /// `wfs`: the coordinate system asked for, `EPSG:4326` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub srs_name: Option<String>,
    /// `sensorThings`: the related entities read with each one, `Datastreams/Observations`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expand: Option<String>,
}

/// The open data APIs a preset reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HttpPresetKind {
    /// CKAN `package_search` (datasets) or `datastore_search` (rows of one resource).
    #[default]
    Ckan,
    /// An ArcGIS Feature Service layer's `query`.
    ArcgisFeature,
    /// An OData v4 entity set.
    Odata,
    /// A WFS 2.0 `GetFeature`.
    Wfs,
    /// An OGC API Features collection's `items`.
    OgcFeatures,
    /// An OGC SensorThings entity set, `Things` or `Datastreams`.
    SensorThings,
}

impl HttpPresetKind {
    /// The manifest's name of the kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ckan => "ckan",
            Self::ArcgisFeature => "arcgisFeature",
            Self::Odata => "odata",
            Self::Wfs => "wfs",
            Self::OgcFeatures => "ogcFeatures",
            Self::SensorThings => "sensorThings",
        }
    }
}

impl HttpPreset {
    /// The page size when the manifest names none.
    pub const DEFAULT_PAGE_SIZE: u64 = 1000;
    /// The largest page size a manifest may set.
    pub const MAX_PAGE_SIZE: u64 = 10_000;

    fn page_size(&self) -> u64 {
        self.page_size.unwrap_or(Self::DEFAULT_PAGE_SIZE)
    }

    fn validate(&self, http: &HttpConnection) -> Result<()> {
        let refuse = |field, value: String, reason| {
            Err(Error::Name {
                field,
                value,
                reason,
            })
        };
        if http.paging.is_some() || http.records.is_some() {
            let field = if http.paging.is_some() {
                "spec.http.paging"
            } else {
                "spec.http.records"
            };
            return refuse(
                field,
                self.kind.as_str().to_string(),
                "a preset sets paging and records itself: leave them out",
            );
        }
        use HttpPresetKind as K;
        let fields: [(&'static str, bool, &[K]); 7] = [
            (
                "spec.http.preset.organization",
                self.organization.is_some(),
                &[K::Ckan],
            ),
            (
                "spec.http.preset.resourceId",
                self.resource_id.is_some(),
                &[K::Ckan],
            ),
            (
                "spec.http.preset.objectIdField",
                self.object_id_field.is_some(),
                &[K::ArcgisFeature],
            ),
            (
                "spec.http.preset.outFields",
                !self.out_fields.is_empty(),
                &[K::ArcgisFeature],
            ),
            (
                "spec.http.preset.typeNames",
                self.type_names.is_some(),
                &[K::Wfs],
            ),
            (
                "spec.http.preset.srsName",
                self.srs_name.is_some(),
                &[K::Wfs],
            ),
            (
                "spec.http.preset.expand",
                self.expand.is_some(),
                &[K::SensorThings],
            ),
        ];
        if let Some((field, _, _)) = fields
            .iter()
            .find(|(_, set, kinds)| *set && !kinds.contains(&self.kind))
        {
            return refuse(
                field,
                self.kind.as_str().to_string(),
                "this field belongs to another preset kind",
            );
        }
        if let Some(size) = self.page_size {
            if !(1..=Self::MAX_PAGE_SIZE).contains(&size) {
                return refuse(
                    "spec.http.preset.pageSize",
                    size.to_string(),
                    "pageSize is 1 to 10000",
                );
            }
        }
        let names = [
            (
                "spec.http.preset.organization",
                &self.organization,
                100,
                "_-",
            ),
            ("spec.http.preset.resourceId", &self.resource_id, 100, "_-"),
            (
                "spec.http.preset.objectIdField",
                &self.object_id_field,
                64,
                "_",
            ),
            ("spec.http.preset.typeNames", &self.type_names, 128, "_-.:"),
            ("spec.http.preset.srsName", &self.srs_name, 64, "_-.:"),
            ("spec.http.preset.expand", &self.expand, 256, "/,"),
        ];
        for (field, name, max, extra) in names {
            if let Some(name) = name {
                if !plain(name, max, extra) {
                    return refuse(field, name.clone(), "a name of letters, digits and the separators of its API only, such as avoindata:Helsinki_osoiteluettelo, EPSG:4326 or Datastreams/Observations");
                }
            }
        }
        if let Some(name) = self.out_fields.iter().find(|n| !plain(n, 64, "_")) {
            return refuse(
                "spec.http.preset.outFields",
                name.clone(),
                "each out field is one field name of letters, digits and '_', never *",
            );
        }
        let path = http
            .url
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .trim_end_matches('/');
        let (ends, needs): (&str, Option<(&'static str, bool, &'static str)>) = match self.kind {
            K::Ckan if self.resource_id.is_some() => ("/action/datastore_search", None),
            K::Ckan => ("/action/package_search", None),
            K::ArcgisFeature => (
                "/query",
                Some((
                    "spec.http.preset.outFields",
                    !self.out_fields.is_empty(),
                    "an arcgisFeature preset names the fields it reads in outFields",
                )),
            ),
            K::Wfs => (
                "",
                Some((
                    "spec.http.preset.typeNames",
                    self.type_names.is_some(),
                    "a wfs preset names the feature type it reads in typeNames",
                )),
            ),
            K::OgcFeatures => ("/items", None),
            K::Odata | K::SensorThings => ("", None),
        };
        if !path.ends_with(ends) {
            return refuse(
                "spec.http.url",
                http.url.clone(),
                match self.kind {
                    K::Ckan if self.resource_id.is_some() => "a ckan preset with resourceId reads .../api/3/action/datastore_search",
                    K::Ckan => "a ckan preset reads .../api/3/action/package_search",
                    K::ArcgisFeature => "an arcgisFeature preset reads a layer's query, .../FeatureServer/0/query",
                    _ => "an ogcFeatures preset reads a collection's items, .../collections/{id}/items",
                },
            );
        }
        if let Some((field, true_, reason)) = needs {
            if !true_ {
                return refuse(field, self.kind.as_str().to_string(), reason);
            }
        }
        Ok(())
    }

    /// The connection with this preset's query parameters, `paging` and `records`.
    fn apply(&self, http: &HttpConnection) -> HttpConnection {
        use HttpPresetKind as K;
        let size = self.page_size();
        let mut params: Vec<(&str, String)> = Vec::new();
        let offset = |offset: &str, limit: &str| HttpPaging {
            mode: HttpPagingMode::Offset,
            offset_param: Some(offset.to_string()),
            limit_param: Some(limit.to_string()),
            limit: Some(size),
            ..HttpPaging::default()
        };
        let next_url = |next: &str| HttpPaging {
            mode: HttpPagingMode::NextUrl,
            next: Some(next.to_string()),
            ..HttpPaging::default()
        };
        let (records, paging) = match self.kind {
            K::Ckan => match &self.resource_id {
                Some(resource) => {
                    params.push(("resource_id", resource.clone()));
                    (
                        "result.records",
                        HttpPaging {
                            total: Some("result.total".to_string()),
                            ..offset("offset", "limit")
                        },
                    )
                }
                None => {
                    if let Some(org) = &self.organization {
                        params.push(("fq", format!("organization:{org}")));
                    }
                    (
                        "result.results",
                        HttpPaging {
                            total: Some("result.count".to_string()),
                            ..offset("start", "rows")
                        },
                    )
                }
            },
            K::ArcgisFeature => {
                if !query_has(&http.url, "where") {
                    params.push(("where", "1=1".to_string()));
                }
                params.push(("outFields", self.out_fields.join(",")));
                params.push(("outSR", "4326".to_string()));
                params.push(("f", "geojson".to_string()));
                (
                    "features",
                    HttpPaging {
                        more: Some("properties.exceededTransferLimit".to_string()),
                        order_by: Some(HttpPagingOrder {
                            param: "orderByFields".to_string(),
                            value: self
                                .object_id_field
                                .clone()
                                .unwrap_or_else(|| "OBJECTID".to_string()),
                        }),
                        ..offset("resultOffset", "resultRecordCount")
                    },
                )
            }
            K::Odata if self.page_size.is_some() => ("value", offset("$skip", "$top")),
            K::Odata => ("value", next_url("@odata.nextLink")),
            K::Wfs => {
                params.push(("service", "WFS".to_string()));
                params.push(("version", "2.0.0".to_string()));
                params.push(("request", "GetFeature".to_string()));
                params.push(("typeNames", self.type_names.clone().unwrap_or_default()));
                params.push((
                    "srsName",
                    self.srs_name
                        .clone()
                        .unwrap_or_else(|| "EPSG:4326".to_string()),
                ));
                params.push(("outputFormat", "application/json".to_string()));
                (
                    "features",
                    HttpPaging {
                        total: Some("numberMatched".to_string()),
                        ..offset("startIndex", "count")
                    },
                )
            }
            K::OgcFeatures => {
                params.push(("limit", size.to_string()));
                ("features", next_url("links[rel=next].href"))
            }
            K::SensorThings => {
                params.push(("$top", size.to_string()));
                if let Some(expand) = &self.expand {
                    params.push(("$expand", expand.clone()));
                }
                ("value", next_url("@iot.nextLink"))
            }
        };
        HttpConnection {
            url: with_query(&http.url, &params),
            records: Some(records.to_string()),
            paging: Some(paging),
            preset: None,
            ..http.clone()
        }
    }
}

/// The query of `url` names `name`.
fn query_has(url: &str, name: &str) -> bool {
    let query = url.split('#').next().unwrap_or_default();
    query
        .split_once('?')
        .is_some_and(|(_, q)| form_urlencoded::parse(q.as_bytes()).any(|(key, _)| key == name))
}

/// `url` with each of `params` set: a parameter of the same name the URL already holds is
/// replaced, every other one kept as written.
fn with_query(url: &str, params: &[(&str, String)]) -> String {
    let (url, fragment) = match url.split_once('#') {
        Some((url, fragment)) => (url, Some(fragment)),
        None => (url, None),
    };
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut out = form_urlencoded::Serializer::new(String::new());
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        if !params.iter().any(|(name, _)| *name == key) {
            out.append_pair(&key, &value);
        }
    }
    for (name, value) in params {
        out.append_pair(name, value);
    }
    let query = out.finish();
    let mut url = base.to_string();
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query);
    }
    if let Some(fragment) = fragment {
        url.push('#');
        url.push_str(fragment);
    }
    url
}

/// `value` is 1 to `max` bytes of ASCII letters, digits and `extra`.
fn plain(value: &str, max: usize, extra: &str) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// A whole number of `ms`, `s` or `m` up to one minute.
fn short_duration(value: &str) -> bool {
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let (digits, unit) = value.split_at(split);
    let Ok(n) = digits.parse::<u64>() else {
        return false;
    };
    let ms = match unit {
        "ms" => n,
        "s" => n.saturating_mul(1_000),
        "m" => n.saturating_mul(60_000),
        _ => return false,
    };
    ms <= 60_000
}

/// The archive formats an `http` connection reads (PL-72).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveFormat {
    /// A zip archive: stored or deflated members, no Zip64.
    Zip,
}

/// A WebSocket the runner keeps open (Architecture/08 §6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebSocketConnection {
    /// The socket URL, `ws://` or `wss://`.
    pub url: String,
    /// A message sent once the socket opens, usually a subscription request.
    ///
    /// A socket that needs a credential authenticates inside this message: Bento's websocket
    /// input sends no `Authorization` header, and a field the renderer cannot honour would be
    /// a promise the runner breaks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_message: Option<String>,
}

/// A GTFS-realtime feed (Architecture/08 §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GtfsRtConnection {
    /// The protobuf feed URL, `http://` or `https://`.
    pub url: String,
    /// Which of the three GTFS-realtime feeds this URL serves.
    pub feed: GtfsFeed,
}

/// The three feeds GTFS-realtime defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GtfsFeed {
    /// Where the vehicles are.
    VehiclePositions,
    /// How the trips are running against the timetable.
    TripUpdates,
    /// Service alerts.
    Alerts,
}

impl GtfsFeed {
    /// The camelCase wire name, as written in `spec.gtfsRt.feed`.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::VehiclePositions => "vehiclePositions",
            Self::TripUpdates => "tripUpdates",
            Self::Alerts => "alerts",
        }
    }
}

/// An `Authorization` header whose value is assembled from a secret (CC-06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Authorization {
    /// The header the credential is written into, `Authorization` when absent.
    ///
    /// An open-data feed that authenticates with an API key usually wants it in a header of
    /// its own; without this the only way to reach such a feed would be to write the key into
    /// `headers` as a value, which is what MF-35 exists to prevent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// The scheme written before the secret.
    ///
    /// `Bearer` when absent on `Authorization`, and nothing at all on any other header: a
    /// vendor key header takes the bare key and refuses a scheme in front of it. An explicit
    /// scheme always wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    /// Where the credential comes from.
    pub header_ref: SecretRef,
}

impl Authorization {
    /// The header this credential is written into.
    pub fn header_name(&self) -> &str {
        self.header.as_deref().unwrap_or("Authorization")
    }

    /// The scheme written in front of the credential, empty when the header takes a bare value.
    pub fn scheme_prefix(&self) -> &str {
        match &self.scheme {
            Some(scheme) => scheme,
            None if self.header_name().eq_ignore_ascii_case("Authorization") => "Bearer",
            None => "",
        }
    }
}

/// A pushed feed: the runner's shared ingest listener answers its POSTs (PL-122, ADR-N-071).
///
/// Only the reconciler renders the `http_server` input this becomes, so the managed door keeps
/// authentication and both caps whatever a manifest says; an author-written `http_server` stays
/// refused (PL-50).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebhookConnection {
    /// How a sender proves it may post: exactly one of `hmac` and `bearer`.
    pub auth: WebhookAuth,
    /// The largest body accepted, in bytes (1 to 16777216; 1048576 when absent). A larger one
    /// answers 413 before any step runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_bytes: Option<u64>,
    /// How many requests may wait for their answer at once (1 to 1024; 64 when absent); more
    /// answer 503.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_in_flight: Option<u32>,
}

impl WebhookConnection {
    /// `maxBodyBytes` when absent: 1 MiB, the runner's own default.
    pub const DEFAULT_MAX_BODY_BYTES: u64 = 1024 * 1024;
    /// The largest `maxBodyBytes` the runner accepts: 16 MiB.
    pub const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;
    /// `maxInFlight` when absent.
    pub const DEFAULT_MAX_IN_FLIGHT: u32 = 64;
    /// The largest `maxInFlight` accepted.
    pub const MAX_IN_FLIGHT: u32 = 1024;

    /// The body cap in force.
    pub fn max_body_bytes(&self) -> u64 {
        self.max_body_bytes.unwrap_or(Self::DEFAULT_MAX_BODY_BYTES)
    }

    /// The in-flight cap in force.
    pub fn max_in_flight(&self) -> u32 {
        self.max_in_flight.unwrap_or(Self::DEFAULT_MAX_IN_FLIGHT)
    }

    /// The one credential reference of `auth`, if exactly one is set.
    pub fn credential(&self) -> Option<&SecretRef> {
        match (&self.auth.hmac, &self.auth.bearer) {
            (Some(hmac), None) => Some(&hmac.secret_ref),
            (None, Some(bearer)) => Some(&bearer.token_ref),
            _ => None,
        }
    }
}

/// A webhook's authentication; there is no `public` option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebhookAuth {
    /// The body's hex SHA-256 HMAC in the provider's header, with or without `sha256=`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<WebhookHmac>,
    /// `Authorization: Bearer <token>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer: Option<WebhookBearer>,
}

/// An HMAC-signed webhook (GitHub's `X-Hub-Signature-256`, a form tool's own header).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebhookHmac {
    /// The header the provider writes its signature into, such as `X-Hub-Signature-256`.
    pub header: String,
    /// The shared signing secret.
    pub secret_ref: SecretRef,
}

/// A webhook that sends a fixed bearer token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebhookBearer {
    /// The token the sender presents.
    pub token_ref: SecretRef,
}

/// How the runner trusts the feed's certificate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TlsSettings {
    /// Kept so a manifest can state the choice; only `false` is accepted (see [`DataSourceSpec::validate`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure_skip_verify: bool,
    /// A private certificate authority the feed's certificate chains to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert_ref: Option<SecretRef>,
}

/// Desired state of a `DataSource`: one connection and its credentials (MF-35).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataSourceSpec {
    /// Which connection block below is the live one.
    #[serde(rename = "type")]
    pub source_type: DataSourceType,
    /// The MQTT connection, present when `type` is `mqtt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mqtt: Option<MqttConnection>,
    /// The HTTP connection, present when `type` is `http`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpConnection>,
    /// The WebSocket connection, present when `type` is `websocket`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_socket: Option<WebSocketConnection>,
    /// The GTFS-realtime connection, present when `type` is `gtfs-rt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gtfs_rt: Option<GtfsRtConnection>,
    /// The webhook a sender pushes to, present when `type` is `webhook`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookConnection>,
    /// Transport security for the connection above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsSettings>,
    /// Verbatim input configuration document for runner-provided inputs (PL-50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    /// Secret references for runner-provided inputs (PL-50, MF-35).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretRef>,
    /// The fields of this source's records that hold personal data (AG-158): a sample of the
    /// source answered to an MCP client or an agent run leaves them out and names them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = 256))]
    pub personal_fields: Vec<String>,
}

/// The most bytes one `spec.personalFields` entry may hold.
const PERSONAL_FIELD_BYTES: usize = 256;

impl Kind for DataSourceSpec {
    const KIND: &'static str = "DataSource";
    const PLURAL: &'static str = "datasources";
    const SCOPE: Scope = Scope::Project;
    const PATH_TEMPLATE: &'static str = "projects/{project}/datasources/{name}.yaml";

    fn validate_spec(&self, meta: &ObjectMeta) -> Result<()> {
        names::validate_dns1123_label(&meta.name)?;
        // One runner may carry every project's streams: a file reader keeps to its own
        // project's folder of the files volume (PL-50, T-5337).
        match &meta.namespace {
            Some(project) => self.validate_reading_files(&format!("/data/{project}/")),
            None => self.validate(),
        }
    }
}

/// The URL schemes each connection type may reach, so a manifest cannot point an MQTT
/// subscription at a file or an HTTP poll at a broker.
const MQTT_SCHEMES: &[&str] = &["tcp://", "tls://", "ws://", "wss://"];
const HTTP_SCHEMES: &[&str] = &["http://", "https://"];
const WS_SCHEMES: &[&str] = &["ws://", "wss://"];

impl DataSourceSpec {
    /// Whether this input ends on its own once it has read what there is (PL-04, PL-50).
    ///
    /// Scheduled pipelines may only run terminating inputs; socket and broker inputs never
    /// terminate and must run resident.
    pub fn terminates(&self) -> bool {
        match &self.source_type {
            DataSourceType::Http | DataSourceType::GtfsRt => true,
            DataSourceType::Mqtt | DataSourceType::WebSocket | DataSourceType::Webhook => false,
            DataSourceType::Runner(name) => bento_inputs::TERMINATING.contains(&name.as_str()),
        }
    }

    /// Validates that exactly the declared connection is present and reachable as written. A
    /// file reader may name any path of the runner's files volume, `/data/`; the manifest's own
    /// check holds it to its project's folder.
    pub fn validate(&self) -> Result<()> {
        self.validate_reading_files("/data/")
    }

    /// Each `spec.personalFields` entry a field name: 1 to 256 bytes, no control character,
    /// named once (AG-158).
    fn validate_personal_fields(&self) -> Result<()> {
        if self.personal_fields.len() > 256 {
            return Err(Error::Invalid {
                field: "spec.personalFields".to_owned(),
                reason: format!("names {} fields; at most 256", self.personal_fields.len()),
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for (i, field) in self.personal_fields.iter().enumerate() {
            let reason = if field.is_empty() || field.len() > PERSONAL_FIELD_BYTES {
                "a field name of 1 to 256 bytes, such as `rider`"
            } else if field.chars().any(char::is_control) {
                "a field name holds no control character"
            } else if !seen.insert(field.as_str()) {
                "each field is named once"
            } else {
                continue;
            };
            return Err(Error::Invalid {
                field: format!("spec.personalFields[{i}]"),
                reason: reason.to_owned(),
            });
        }
        Ok(())
    }

    /// [`Self::validate`] with every file a runner input reads held under `files`.
    fn validate_reading_files(&self, files: &str) -> Result<()> {
        let present: Vec<&'static str> = [
            self.mqtt.is_some().then_some("mqtt"),
            self.http.is_some().then_some("http"),
            self.web_socket.is_some().then_some("webSocket"),
            self.gtfs_rt.is_some().then_some("gtfsRt"),
            self.webhook.is_some().then_some("webhook"),
        ]
        .into_iter()
        .flatten()
        .collect();

        match &self.source_type {
            DataSourceType::Mqtt
            | DataSourceType::Http
            | DataSourceType::WebSocket
            | DataSourceType::GtfsRt
            | DataSourceType::Webhook => {
                let expected = match self.source_type {
                    DataSourceType::Mqtt => "mqtt",
                    DataSourceType::Http => "http",
                    DataSourceType::WebSocket => "webSocket",
                    DataSourceType::GtfsRt => "gtfsRt",
                    DataSourceType::Webhook => "webhook",
                    DataSourceType::Runner(_) => unreachable!(),
                };
                if present != [expected] {
                    return Err(Error::Name {
                        field: "spec",
                        value: present.join(", "),
                        reason: "exactly the connection block named by `type` is present (MF-35)",
                    });
                }
            }
            DataSourceType::Runner(name) => {
                if !bento_inputs::INPUTS.contains(&name.as_str()) {
                    let mut accepted = vec!["mqtt", "http", "websocket", "gtfs-rt", "webhook"];
                    accepted.extend(runner_inputs());
                    return Err(Error::Invalid {
                        field: "spec.type".to_string(),
                        reason: format!(
                            "unknown type `{name}`; accepted types are: {}",
                            accepted.join(", ")
                        ),
                    });
                }
                refused_input("spec.type", name)?;
                unregistered_input("spec.type", name)?;
                if !present.is_empty() {
                    return Err(Error::Name {
                        field: "spec",
                        value: present.join(", "),
                        reason: "runner data sources do not use typed connection blocks; configure `spec.input` instead",
                    });
                }
            }
        }

        if let Some(tls) = &self.tls {
            if self.source_type.runner_name().is_some() {
                return Err(Error::Name {
                    field: "spec.tls",
                    value: String::new(),
                    reason: "runner data sources carry their own tls configuration inside `spec.input`, not `spec.tls`",
                });
            }
            if self.source_type == DataSourceType::Webhook {
                return Err(Error::Name {
                    field: "spec.tls",
                    value: String::new(),
                    reason: "a webhook is called over the installation's own HTTPS edge; `spec.tls` belongs to a connection the runner opens",
                });
            }
            if tls.insecure_skip_verify {
                return Err(Error::Name {
                    field: "spec.tls.insecureSkipVerify",
                    value: "true".to_owned(),
                    reason: "a feed's certificate is always verified; use `caCertRef` for a private authority",
                });
            }
        }

        self.validate_secret_references()?;
        self.validate_personal_fields()?;

        // Every string of a typed connection reaches the runner as written, and the runner fills
        // `${NAME}` from its environment: a header or a topic is the same door as a URL (PL-16,
        // T-3163). A runner input names only its own `spec.secrets`, checked below.
        // Keys too: the runner interpolates the raw configuration, so the block is read as text.
        let typed = [
            ("spec.mqtt", serde_json::to_string(&self.mqtt)),
            ("spec.http", serde_json::to_string(&self.http)),
            ("spec.webSocket", serde_json::to_string(&self.web_socket)),
            ("spec.gtfsRt", serde_json::to_string(&self.gtfs_rt)),
            ("spec.webhook", serde_json::to_string(&self.webhook)),
        ];
        for (field, text) in typed {
            if let Some(reason) = text.ok().as_deref().and_then(refused_runner_read) {
                return Err(Error::Invalid {
                    field: field.to_owned(),
                    reason: reason.to_owned(),
                });
            }
        }

        match &self.source_type {
            DataSourceType::Mqtt => {
                self.validate_typed_common()?;
                self.validate_mqtt()
            }
            DataSourceType::Http => {
                self.validate_typed_common()?;
                let written = self.http.as_ref().expect("checked above");
                if let Some(preset) = &written.preset {
                    preset.validate(written)?;
                }
                let http = &*written.effective();
                url(&http.url, HTTP_SCHEMES, "spec.http.url")?;
                if let Some(authorization) = &http.authorization {
                    header_name(authorization.header_name())?;
                }
                if let Some(archive) = &http.archive {
                    archive.validate(http)?;
                }
                if let Some(records) = &http.records {
                    records_mapping(records).map_err(|reason| Error::Name {
                        field: "spec.http.records",
                        value: records.clone(),
                        reason,
                    })?;
                    if http.format.is_some() {
                        return Err(Error::Name {
                            field: "spec.http.records",
                            value: records.clone(),
                            reason: "records splits a JSON body; a body of format csv or xlsx is split by its rows already",
                        });
                    }
                }
                if let Some(paging) = &http.paging {
                    paging.validate(http)?;
                }
                if !(1..=HttpConnection::MAX_BYTES).contains(&http.max_bytes()) {
                    return Err(Error::Name {
                        field: "spec.http.maxBytes",
                        value: http.max_bytes().to_string(),
                        reason: "maxBytes is 1 to 268435456 (256 MiB)",
                    });
                }
                verb(http.verb.as_deref())
            }
            DataSourceType::WebSocket => {
                self.validate_typed_common()?;
                url(
                    &self.web_socket.as_ref().expect("checked above").url,
                    WS_SCHEMES,
                    "spec.webSocket.url",
                )
            }
            DataSourceType::GtfsRt => {
                self.validate_typed_common()?;
                url(
                    &self.gtfs_rt.as_ref().expect("checked above").url,
                    HTTP_SCHEMES,
                    "spec.gtfsRt.url",
                )
            }
            DataSourceType::Webhook => {
                self.validate_typed_common()?;
                validate_webhook(self.webhook.as_ref().expect("checked above"))
            }
            DataSourceType::Runner(name) => self.validate_runner(name, files),
        }
    }

    /// Every credential reference this source carries, by the field a person typed it into.
    ///
    /// Until T-2238 only the runner path checked anything, and it checked the name alone: a typed
    /// `passwordRef` naming a GitHub token was checked green and would have been committed to the
    /// configuration repository in the clear. Each block is walked whatever `type` says, because a
    /// block that does not belong to the type is refused elsewhere and a reference in it is still a
    /// reference a person wrote.
    fn validate_secret_references(&self) -> Result<()> {
        if let Some(mqtt) = &self.mqtt {
            if let Some(password) = &mqtt.password_ref {
                password.validate("spec.mqtt.passwordRef")?;
            }
        }
        if let Some(http) = &self.http {
            if let Some(authorization) = &http.authorization {
                authorization
                    .header_ref
                    .validate("spec.http.authorization.headerRef")?;
            }
        }
        if let Some(webhook) = &self.webhook {
            if let Some(hmac) = &webhook.auth.hmac {
                hmac.secret_ref
                    .validate("spec.webhook.auth.hmac.secretRef")?;
            }
            if let Some(bearer) = &webhook.auth.bearer {
                bearer
                    .token_ref
                    .validate("spec.webhook.auth.bearer.tokenRef")?;
            }
        }
        if let Some(tls) = &self.tls {
            if let Some(ca) = &tls.ca_cert_ref {
                ca.validate("spec.tls.caCertRef")?;
            }
        }
        for (index, secret) in self.secrets.iter().enumerate() {
            secret.validate(&format!("spec.secrets[{index}]"))?;
        }
        Ok(())
    }

    fn validate_typed_common(&self) -> Result<()> {
        if self.input.is_some() {
            return Err(Error::Name {
                field: "spec.input",
                value: String::new(),
                reason: "typed data sources do not declare `input`; use the connection block named by `type`",
            });
        }
        if !self.secrets.is_empty() {
            return Err(Error::Name {
                field: "spec.secrets",
                value: String::new(),
                reason: "typed data sources do not declare `secrets`; credentials belong in the connection block",
            });
        }
        Ok(())
    }

    fn validate_runner(&self, runner_name: &str, files: &str) -> Result<()> {
        let input_obj = match &self.input {
            Some(serde_json::Value::Object(map)) if !map.is_empty() => map,
            _ => {
                return Err(Error::Name {
                    field: "spec.input",
                    value: String::new(),
                    reason: "`spec.input` must be present and a non-empty mapping for runner data sources",
                });
            }
        };

        for sref in &self.secrets {
            // The name and the key are checked for every type by `validate_secret_references`,
            // which names the field and never repeats what it refused (T-2238).
            match &sref.env_var {
                Some(ev) if ENV_VAR_RE.is_match(ev) => {}
                Some(ev) => {
                    return Err(Error::Name {
                        field: "spec.secrets.envVar",
                        value: ev.clone(),
                        reason: "envVar must match [A-Z][A-Z0-9_]*",
                    });
                }
                None => {
                    return Err(Error::Name {
                        field: "spec.secrets.envVar",
                        value: String::new(),
                        reason: "envVar is required for runner secrets (PL-50)",
                    });
                }
            }
        }

        let known_vars: std::collections::HashSet<&str> = self
            .secrets
            .iter()
            .filter_map(|s| s.env_var.as_deref())
            .collect();

        let secret_paths = bento_inputs::SECRET_FIELDS
            .iter()
            .find(|(n, _)| *n == runner_name)
            .map(|(_, paths)| *paths)
            .unwrap_or(&[]);

        let input_val = self.input.as_ref().expect("checked above");
        for path in secret_paths {
            let segments: Vec<&str> = path.split('.').collect();
            let mut targets = Vec::new();
            collect_secret_targets(input_val, &segments, "", &mut targets);
            for (concrete_path, target_val) in targets {
                let valid = if let serde_json::Value::String(s) = target_val {
                    if let Some(var) = is_exact_var_interpolation(s) {
                        known_vars.contains(var)
                    } else {
                        false
                    }
                } else {
                    false
                };

                if !valid {
                    return Err(Error::Invalid {
                        field: format!("spec.input.{concrete_path}"),
                        reason: "a runner-documented secret field holds a `${VAR}` interpolation naming an entry of spec.secrets, never a value (MF-35, PL-16)".to_string(),
                    });
                }
            }
        }

        let mut named = Vec::new();
        interpolated_names(input_val, &mut named);
        if let Some(name) = named.into_iter().find(|n| !known_vars.contains(n)) {
            return Err(Error::Invalid {
                field: "spec.input".to_string(),
                reason: format!(
                    "`${{{name}}}` names no envVar of spec.secrets; the runner's own environment is not reachable from a manifest (PL-50, PL-16)"
                ),
            });
        }

        validate_input_strings(input_val)?;

        let own: Vec<&str> = known_vars.into_iter().collect();
        validate_input_tree(runner_name, input_obj, &own, files)
    }

    fn validate_mqtt(&self) -> Result<()> {
        let mqtt = self.mqtt.as_ref().expect("checked by the caller");
        if let Some(broker) = &mqtt.broker_ref {
            let refuse = |field: &'static str, reason: &'static str| Error::Name {
                field,
                value: broker.name.clone(),
                reason,
            };
            if broker.kind != "MqttBroker" {
                return Err(refuse(
                    "spec.mqtt.brokerRef.kind",
                    "a brokerRef names an MqttBroker",
                ));
            }
            if broker.namespace.is_some() {
                return Err(refuse(
                    "spec.mqtt.brokerRef.namespace",
                    "a DataSource reads a broker of its own project only (PL-76)",
                ));
            }
            names::validate_dns1123_label(&broker.name)?;
            if !mqtt.urls.is_empty() || mqtt.username.is_some() || mqtt.password_ref.is_some() {
                return Err(refuse(
                    "spec.mqtt.brokerRef",
                    "a brokerRef takes the place of urls, username and passwordRef: the runner reads a platform broker with no credential (PL-76)",
                ));
            }
        } else if mqtt.urls.is_empty() {
            return Err(Error::Name {
                field: "spec.mqtt.urls",
                value: String::new(),
                reason: "an MQTT connection needs at least one broker URL",
            });
        }
        if mqtt.topics.is_empty() {
            return Err(Error::Name {
                field: "spec.mqtt.topics",
                value: String::new(),
                reason: "an MQTT connection needs at least one topic filter",
            });
        }
        for broker in &mqtt.urls {
            url(broker, MQTT_SCHEMES, "spec.mqtt.urls")?;
        }
        if let Some(user) = mqtt
            .username
            .as_ref()
            .filter(|user| runner_user(user) || user.contains("${"))
        {
            return Err(Error::Name {
                field: "spec.mqtt.username",
                value: user.clone(),
                reason: RUNNER_USER,
            });
        }
        if let Some(qos) = mqtt.qos {
            if qos > 2 {
                return Err(Error::Name {
                    field: "spec.mqtt.qos",
                    value: qos.to_string(),
                    reason: "MQTT quality of service is 0, 1 or 2",
                });
            }
        }
        Ok(())
    }

    /// The connection's secret references, in the order the runner needs them injected.
    ///
    /// The reconciler resolves these and nothing else: a secret nobody references is never
    /// read, and a credential is never carried in the manifest itself (PL-15).
    pub fn secret_refs(&self) -> Vec<&SecretRef> {
        match &self.source_type {
            DataSourceType::Mqtt => {
                let mut refs = Vec::new();
                if let Some(m) = &self.mqtt {
                    if let Some(p) = &m.password_ref {
                        refs.push(p);
                    }
                }
                if let Some(tls) = &self.tls {
                    if let Some(ca) = &tls.ca_cert_ref {
                        refs.push(ca);
                    }
                }
                refs
            }
            DataSourceType::Http => {
                let mut refs = Vec::new();
                if let Some(h) = &self.http {
                    if let Some(a) = &h.authorization {
                        refs.push(&a.header_ref);
                    }
                }
                if let Some(tls) = &self.tls {
                    if let Some(ca) = &tls.ca_cert_ref {
                        refs.push(ca);
                    }
                }
                refs
            }
            DataSourceType::WebSocket | DataSourceType::GtfsRt => {
                let mut refs = Vec::new();
                if let Some(tls) = &self.tls {
                    if let Some(ca) = &tls.ca_cert_ref {
                        refs.push(ca);
                    }
                }
                refs
            }
            DataSourceType::Webhook => self
                .webhook
                .as_ref()
                .and_then(WebhookConnection::credential)
                .into_iter()
                .collect(),
            DataSourceType::Runner(_) => self.secrets.iter().collect(),
        }
    }
}

/// `spec.webhook`: one credential, a header name for `hmac`, both caps in range (PL-122).
fn validate_webhook(webhook: &WebhookConnection) -> Result<()> {
    match (&webhook.auth.hmac, &webhook.auth.bearer) {
        (Some(hmac), None) => header_name(&hmac.header).map_err(|_| Error::Name {
            field: "spec.webhook.auth.hmac.header",
            value: hmac.header.clone(),
            reason: "a header name is a token: letters, digits and !#$%&'*+-.^_`|~, such as X-Hub-Signature-256",
        })?,
        (None, Some(_)) => {}
        _ => {
            return Err(Error::Name {
                field: "spec.webhook.auth",
                value: String::new(),
                reason: "a webhook names exactly one of `hmac` and `bearer`; anyone could post to one without",
            })
        }
    }
    if !(1..=WebhookConnection::MAX_BODY_BYTES).contains(&webhook.max_body_bytes()) {
        return Err(Error::Name {
            field: "spec.webhook.maxBodyBytes",
            value: webhook.max_body_bytes().to_string(),
            reason: "maxBodyBytes is 1 to 16777216 (16 MiB), such as 1048576",
        });
    }
    if !(1..=WebhookConnection::MAX_IN_FLIGHT).contains(&webhook.max_in_flight()) {
        return Err(Error::Name {
            field: "spec.webhook.maxInFlight",
            value: webhook.max_in_flight().to_string(),
            reason: "maxInFlight is 1 to 1024, such as 64",
        });
    }
    Ok(())
}

/// Validates that the pipeline's execution class is compatible with the data source (PL-04, PL-50).
///
/// A scheduled pipeline (explicit `class: scheduled` or `auto` with period >= 30s) must only
/// read terminating data sources (e.g. files, queries, batch fetches). Sockets, brokers, and
/// streams never terminate on their own and must execute as resident pipelines.
/// A `sourceDeletes` snapshot needs such a terminating source too (PL-99).
pub fn check_class(pipeline: &PipelineSpec, source: &DataSourceSpec) -> Result<()> {
    if pipeline.is_scheduled() && !source.terminates() {
        return Err(Error::Invalid {
            field: "spec.class".to_string(),
            reason: format!(
                "{} never ends on its own; a broker or socket input is resident (PL-04, PL-50)",
                source.source_type
            ),
        });
    }
    // A snapshot deletes what complete runs stopped reading (PL-97); an input that never ends
    // has no complete run, so absence in it means nothing.
    let snapshot = pipeline
        .source_deletes
        .as_ref()
        .is_some_and(|rule| rule.mode == super::SourceDeleteMode::Snapshot);
    if snapshot && !source.terminates() {
        return Err(Error::Invalid {
            field: "spec.sourceDeletes.mode".to_string(),
            reason: format!(
                "{} never ends on its own, so no run reads it completely and a snapshot cannot \
                 tell a removed record from a quiet one; use events or lease (PL-99)",
                source.source_type
            ),
        });
    }
    Ok(())
}

/// The runner inputs no DataSource names (T-3833): each works on the project's runner instead of
/// reading a feed. `subprocess` runs a program and `stdin` reads the runner's own input;
/// `kubernetes_watch` calls the cluster API as the runner's ServiceAccount; `inproc` and
/// `resource` read what another stream of the runner declares; `dynamic`, `http_server` and
/// `socket_server` open an API or a listener in the runner's pod. A feed that pushes writes to an
/// Endpoint instead.
const REFUSED_INPUTS: &[&str] = &[
    "dynamic",
    "http_server",
    "inproc",
    "kubernetes_watch",
    "resource",
    "socket_server",
    "stdin",
    "subprocess",
];

/// The inputs that carry inputs of their own, and the field that holds them.
const WRAPPER_INPUTS: &[(&str, &str)] = &[
    ("batched", "child"),
    ("broker", "inputs"),
    ("read_until", "input"),
    ("sequence", "inputs"),
];

fn refused_input(field: &'static str, name: &str) -> Result<()> {
    if REFUSED_INPUTS.contains(&name) {
        return Err(Error::Invalid {
            field: field.to_owned(),
            reason: format!(
                "input `{name}` works on the project's runner instead of reading a feed: it runs a \
                 program, reads the runner's own process or the cluster API, or reaches another \
                 stream; a feed that pushes writes to an Endpoint (PL-50, T-3833)"
            ),
        });
    }
    Ok(())
}

/// The runner inputs an author may name: the Bento catalog's, less the refused ones, that the
/// pipeline runner (ekiben) also builds (T-6825).
pub fn runner_inputs() -> impl Iterator<Item = &'static str> {
    bento_inputs::INPUTS.iter().copied().filter(|name| {
        !REFUSED_INPUTS.contains(name) && super::ekiben_components::INPUTS.contains(name)
    })
}

/// Refuses an input the Bento catalog knows and the pipeline runner does not build, at save
/// instead of at the stream's first run (T-6825).
fn unregistered_input(field: &'static str, name: &str) -> Result<()> {
    if super::ekiben_components::INPUTS.contains(&name) {
        return Ok(());
    }
    Err(Error::Invalid {
        field: field.to_owned(),
        reason: format!(
            "input `{name}` is not one the pipeline runner carries, so the stream would stop at \
             its first run; accepted inputs are: {}",
            runner_inputs().collect::<Vec<_>>().join(", ")
        ),
    })
}

/// One runner input and every input nested in it (T-3833): no refused input, a file reader (and
/// `fsevent`, which watches paths) under `files`, and its `processors` held to the rules of a
/// pipeline step. `own` is the source's declared `envVar`s.
fn validate_input_tree(
    name: &str,
    config: &serde_json::Map<String, serde_json::Value>,
    own: &[&str],
    files: &str,
) -> Result<()> {
    if bento_inputs::FILE_READERS.contains(&name) || name == "fsevent" {
        let check_file_path = |field: &'static str, val: &str| -> Result<()> {
            if !val.starts_with(files) || val.split('/').any(|seg| seg == "..") {
                return Err(Error::Name {
                    field,
                    value: val.to_string(),
                    reason: "a file-reading input reads its own project's folder of the runner's \
                             files volume, /data/<project>/ (PL-50)",
                });
            }
            Ok(())
        };
        match config.get("paths") {
            Some(serde_json::Value::Array(items)) => {
                for s in items.iter().filter_map(serde_json::Value::as_str) {
                    check_file_path("spec.input.paths", s)?;
                }
            }
            Some(serde_json::Value::String(s)) => check_file_path("spec.input.paths", s)?,
            _ => {}
        }
        if let Some(s) = config.get("path").and_then(serde_json::Value::as_str) {
            check_file_path("spec.input.path", s)?;
        }
    }
    validate_input_processors(config, own)?;
    let Some((_, at)) = WRAPPER_INPUTS.iter().find(|(wrapper, _)| *wrapper == name) else {
        return Ok(());
    };
    let children: Vec<&serde_json::Value> = match config.get(*at) {
        Some(serde_json::Value::Array(items)) => items.iter().collect(),
        Some(child) => vec![child],
        None => Vec::new(),
    };
    for child in children {
        let Some(child) = child.as_object() else {
            return Err(Error::Invalid {
                field: "spec.input".to_owned(),
                reason: format!(
                    "an input inside `{name}` is a map of its name to its configuration"
                ),
            });
        };
        validate_input_processors(child, own)?;
        for (child_name, child_config) in child
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "label" | "processors"))
        {
            if !bento_inputs::INPUTS.contains(&child_name.as_str()) {
                return Err(Error::Invalid {
                    field: "spec.input".to_owned(),
                    reason: format!("unknown input `{child_name}` inside `{name}`"),
                });
            }
            refused_input("spec.input", child_name)?;
            unregistered_input("spec.input", child_name)?;
            let empty = serde_json::Map::new();
            validate_input_tree(
                child_name,
                child_config.as_object().unwrap_or(&empty),
                own,
                files,
            )?;
        }
    }
    Ok(())
}

/// An input's own `processors` run in the stream like a step's, so they pass the step check.
fn validate_input_processors(
    config: &serde_json::Map<String, serde_json::Value>,
    own: &[&str],
) -> Result<()> {
    let Some(processors) = config.get("processors") else {
        return Ok(());
    };
    let Some(processors) = processors.as_array() else {
        return Err(Error::Invalid {
            field: "spec.input.processors".to_owned(),
            reason: "an input's processors are a list".to_owned(),
        });
    };
    for processor in processors {
        super::pipeline::validate_processor_reading("spec.input.processors", processor, own)?;
    }
    Ok(())
}

fn collect_secret_targets<'a>(
    val: &'a serde_json::Value,
    segments: &[&str],
    current: &str,
    out: &mut Vec<(String, &'a serde_json::Value)>,
) {
    if segments.is_empty() {
        out.push((current.to_string(), val));
        return;
    }
    let seg = segments[0];
    let rest = &segments[1..];
    if let Some(array_field) = seg.strip_suffix("[]") {
        if let Some(serde_json::Value::Array(items)) = val.get(array_field) {
            for (idx, item) in items.iter().enumerate() {
                let next_prefix = if current.is_empty() {
                    format!("{array_field}[{idx}]")
                } else {
                    format!("{current}.{array_field}[{idx}]")
                };
                collect_secret_targets(item, rest, &next_prefix, out);
            }
        }
    } else if let Some(child) = val.get(seg) {
        let next_prefix = if current.is_empty() {
            seg.to_string()
        } else {
            format!("{current}.{seg}")
        };
        collect_secret_targets(child, rest, &next_prefix, out);
    }
}

/// Every string value in the document with its dotted path, for checks that read text.
fn strings_of<'a>(val: &'a serde_json::Value, path: String, out: &mut Vec<(String, &'a str)>) {
    match val {
        serde_json::Value::String(s) => out.push((path, s)),
        serde_json::Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                strings_of(v, format!("{path}[{i}]"), out);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                strings_of(v, format!("{path}.{k}"), out);
            }
        }
        _ => {}
    }
}

/// Every name a `${NAME}` or `${NAME:default}` in the document interpolates, keys included:
/// the runner replaces them in the raw configuration before it parses it.
fn interpolated_names<'a>(val: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    fn scan<'a>(mut rest: &'a str, out: &mut Vec<&'a str>) {
        while let Some(start) = rest.find("${") {
            let tail = &rest[start + 2..];
            let end = tail.find(['}', ':']).unwrap_or(tail.len());
            out.push(&tail[..end]);
            rest = &tail[end..];
        }
    }
    match val {
        serde_json::Value::String(s) => scan(s, out),
        serde_json::Value::Array(items) => items.iter().for_each(|v| interpolated_names(v, out)),
        serde_json::Value::Object(map) => map.iter().for_each(|(k, v)| {
            scan(k, out);
            interpolated_names(v, out);
        }),
        _ => {}
    }
}

fn is_exact_var_interpolation(s: &str) -> Option<&str> {
    if s.starts_with("${") && s.ends_with('}') && s.len() > 3 {
        let var = &s[2..s.len() - 1];
        if !var.is_empty()
            && !var.contains('$')
            && !var.contains('{')
            && !var.contains('}')
            && !var.contains(' ')
        {
            return Some(var);
        }
    }
    None
}

/// The environment variable a secret reference is injected as (PL-15, PL-16).
///
/// `envVar` wins where the manifest states one, so a `bento.yaml` that already expects a name
/// keeps working; otherwise the name is derived from the source and the key and cannot collide
/// with another source's in the same runner.
pub fn env_var_of(source_name: &str, reference: &SecretRef) -> String {
    if let Some(explicit) = &reference.env_var {
        return explicit.clone();
    }
    let part = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    match &reference.key {
        Some(key) => format!("DS_{}_{}", part(source_name), part(key)),
        None => format!("DS_{}", part(source_name)),
    }
}

fn url(value: &str, schemes: &'static [&'static str], field: &'static str) -> Result<()> {
    let refuse = |reason| {
        Err(Error::Name {
            field,
            value: value.to_owned(),
            reason,
        })
    };
    if !(schemes.iter().any(|s| value.starts_with(s)) && value.len() > 8) {
        return refuse("the URL scheme is not one this connection type speaks");
    }
    if let Some(reason) = refused_runner_read(value).or_else(|| refused_target(value)) {
        return refuse(reason);
    }
    Ok(())
}

/// The runner variables a typed URL may name: where the gateway and the token service are, and
/// the organisation's domain. None of them is a credential.
pub const URL_VARIABLES: &[&str] = &[
    "JC_GATEWAY_HOST",
    "JC_GATEWAY_URL",
    "JC_TOKEN_URL",
    "JC_ORG_DOMAIN",
    "JC_ORG_ID",
];

/// Why a typed URL reads the runner's environment, if it does (T-3162, PL-16).
///
/// The runner replaces every `${NAME}` in a stream with its own environment when the stream is
/// loaded, and that environment holds every project's credentials, so `https://x/?k=${JC_…}`
/// would send one of them to `x`. A URL names only [`URL_VARIABLES`]; a `${` the feed needs is
/// written `%24{`. A `$` without a brace is left alone, as the runner leaves it.
pub fn refused_runner_read(url: &str) -> Option<&'static str> {
    let mut rest = url;
    while let Some(start) = rest.find("${") {
        let tail = &rest[start + 1..];
        let name = tail
            .strip_prefix('{')
            .and_then(|inner| inner.split_once('}'))
            .map(|(name, _)| name);
        match name {
            Some(name) if URL_VARIABLES.contains(&name) => {
                rest = &tail[name.len() + 2..];
            }
            _ => {
                return Some(
                    "a URL names no runner variable but JC_GATEWAY_HOST, JC_GATEWAY_URL, \
                     JC_TOKEN_URL or JC_ORG_DOMAIN: the runner fills `${…}` from an environment \
                     that holds every project's credentials; write a literal `${` as `%24{` \
                     (PL-16)",
                )
            }
        }
    }
    None
}

/// Why a URL somewhere in `text` reaches the runner's own pod or a private address, if one does
/// (T-3162, PL-07).
///
/// One runner pod runs every project's streams, and its stream API and token sidecar answer on
/// loopback, where neither the NetworkPolicy nor the mesh sees the call. So a host that is
/// loopback, unspecified, link-local, private or carrier-grade NAT, `localhost`, a numeric form
/// other than a dotted quad, or one taken from the message (`${! … }`) is refused, and so are the
/// token Services by name (T-4312). Other cluster Services by name stay allowed: the NetworkPolicy
/// and the mesh decide those.
// ponytail: a public name that resolves to loopback (`127.0.0.1.nip.io`) is not caught here; one
// runner per project (T-1508 option c) is the boundary that covers it.
pub fn refused_target(text: &str) -> Option<&'static str> {
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let after = &rest[at + 3..];
        let end = after
            .find(|c: char| matches!(c, '/' | '?' | '#' | '"' | '\'' | '`') || c.is_whitespace())
            .unwrap_or(after.len());
        let authority = &after[..end];
        let host_port = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        if let Some(reason) = refused_host(host_port) {
            return Some(reason);
        }
        rest = &after[end..];
    }
    None
}

/// Why a `host:port` field of an input reaches the runner's own pod or a private address, if it
/// does (F-PLT2-008). Kafka `addresses`, `seed_brokers`, a socket `address`, Cassandra and NSQ
/// `*_addresses` name hosts without a scheme, so [`refused_target`], which reads what follows a
/// `://`, never saw them. A field may list several, comma-separated.
fn refused_address(path: &str, text: &str) -> Option<&'static str> {
    let key = path.rsplit('.').next().unwrap_or_default();
    let key = key.split('[').next().unwrap_or_default();
    let names_hosts = matches!(key, "address" | "addresses" | "seed_brokers" | "hosts")
        || key.ends_with("_addresses");
    if !names_hosts {
        return None;
    }
    text.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty() && !part.contains("://"))
        .find_map(refused_host)
}

/// The cluster Services of the token sidecars (deployment `components/pipeline-runner` and
/// `components/wasm-host`, `values/tokens`: fullnameOverride and nameOverride).
const TOKEN_SERVICES: &[&str] = &[
    "pipeline-tokens",
    "pipeline-runner-tokens",
    "wasm-host-tokens",
];

/// The host, runner-user and connection-string rules over every string of a runner input.
fn validate_input_strings(input_val: &serde_json::Value) -> Result<()> {
    let mut strings = Vec::new();
    strings_of(input_val, "spec.input".to_string(), &mut strings);
    for (path, text) in strings {
        // An input's Bloblang (`generate.mapping`, a `${! … }` in any field) runs in the same
        // runner as a step's, so it is held to the same host rule (T-3833).
        super::pipeline::validate_author_bloblang("spec.input", text)?;
        if let Some(reason) = refused_target(text).or_else(|| refused_address(&path, text)) {
            return Err(Error::Invalid {
                field: path,
                reason: reason.to_owned(),
            });
        }
        if path.ends_with(".user")
            && (runner_user(text) || (path.contains(".mqtt.") && text.contains("${")))
        {
            return Err(Error::Invalid {
                field: path,
                reason: RUNNER_USER.to_owned(),
            });
        }
        for caps in EMBEDDED_PASSWORD_RE.captures_iter(text) {
            let password = caps
                .get(1)
                .or_else(|| caps.get(2))
                .map_or("", |m| m.as_str());
            if is_exact_var_interpolation(password).is_none() {
                return Err(Error::Invalid {
                    field: path,
                    reason: "a password inside a connection string is a `${VAR}` naming an envVar of spec.secrets, never a literal (PL-50, MF-35)".to_string(),
                });
            }
        }
    }
    Ok(())
}

const RUNNER_USER: &str = "an MQTT username is literal text, never a `${VAR}`, and the pipeline \
                           runner's name is the platform's: a broker of the project is read by \
                           spec.mqtt.brokerRef, which signs in as the runner (PL-76)";

/// Whether a username is one a broker reads as the pipeline runner's, `pipeline-runner` or
/// `pipeline-runner-{project}`, which only a `brokerRef` renders (T-5561, PL-76).
fn runner_user(user: &str) -> bool {
    user.to_ascii_lowercase()
        .starts_with(super::mqtt_broker::RUNNER_USERNAME)
}

/// Whether a host names a platform MQTT broker by its cluster Service, `mqtt-{project}-{name}`
/// bare, in its namespace or under `.svc` (ADR-N-048 §2.4, T-5561). A public name such as
/// `mqtt-eu.example.org` is not one.
fn platform_broker(host: &str) -> bool {
    let mut labels = host.split('.');
    let service = labels.next().unwrap_or_default();
    let third = labels.nth(1);
    service.starts_with("mqtt-") && third.is_none_or(|label| label == "svc")
}

fn refused_host(host_port: &str) -> Option<&'static str> {
    use std::net::IpAddr;
    const OWN: &str = "a pipeline does not connect to the runner's own pod or a private address: \
                       the runner's stream API and token sidecar answer there for every project \
                       (PL-07)";
    if host_port.contains("${!") {
        return Some(
            "the host of a URL is written out, never taken from a message, so where a step \
             connects is known before it runs (PL-07)",
        );
    }
    let host = match host_port.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => host_port.split(':').next().unwrap_or_default(),
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.contains('%') {
        return Some("a host is written without percent-encoding (PL-07)");
    }
    // The token Services mint a token for whichever pipeline a caller names; the runner reaches
    // them by design, so a step may not name them in any namespace or `.svc` form (T-4312).
    let first_label = host.split('.').next().unwrap_or_default();
    if TOKEN_SERVICES.contains(&first_label) {
        return Some(
            "a pipeline does not connect to a token Service: it mints tokens for every project's \
             pipelines (PL-07)",
        );
    }
    if platform_broker(&host) {
        return Some(
            "a pipeline reads a platform broker by spec.mqtt.brokerRef, which names one of its \
             own project, never by its Service name (PL-76)",
        );
    }
    if host == "localhost"
        || host.ends_with(".localhost")
        || host == "localhost.localdomain"
        || host == "ip6-localhost"
        || host == "ip6-loopback"
    {
        return Some(OWN);
    }
    // One rule for which addresses are off the internet, the outbound guard's (F-R2PLT3-012).
    let refused = match host.parse::<IpAddr>() {
        Ok(ip) => !crate::address::is_public(ip),
        // `127.1`, `2130706433`, `0x7f.0.0.1`: a C resolver reads them as addresses.
        Err(_) => {
            !host.is_empty()
                && host.split('.').all(|label| {
                    !label.is_empty()
                        && (label.bytes().all(|b| b.is_ascii_digit())
                            || label
                                .strip_prefix("0x")
                                .is_some_and(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit())))
                })
        }
    };
    refused.then_some(OWN)
}

/// An RFC 9110 field name: a non-empty token, so a manifest cannot smuggle a second header or
/// a value past the colon into the name.
fn header_name(value: &str) -> Result<()> {
    const TOKEN_PUNCTUATION: &str = "!#$%&'*+-.^_`|~";
    let is_token = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || TOKEN_PUNCTUATION.contains(c));
    match is_token {
        true => Ok(()),
        false => Err(Error::Name {
            field: "spec.http.authorization.header",
            value: value.to_owned(),
            reason: "a header name is a token: letters, digits and !#$%&'*+-.^_`|~",
        }),
    }
}

fn verb(value: Option<&str>) -> Result<()> {
    match value {
        None | Some("GET") | Some("POST") => Ok(()),
        Some(other) => Err(Error::Name {
            field: "spec.http.verb",
            value: other.to_owned(),
            reason: "a polled feed is read with GET or POST",
        }),
    }
}

#[cfg(test)]
mod address_tests {
    use super::validate_input_strings;
    use serde_json::json;

    /// F-PLT2-008: a scheme-less `host:port` field is held to the host rule as a URL is.
    #[test]
    fn a_private_host_port_is_refused_and_a_public_one_passes() {
        for refused in [
            json!({ "kafka": { "addresses": ["127.0.0.1:4195"] } }),
            json!({ "kafka": { "addresses": ["broker.example.org:9092,10.0.0.5:9092"] } }),
            json!({ "socket": { "address": "10.0.0.1:9000" } }),
            json!({ "kafka_franz": { "seed_brokers": ["localhost:9092"] } }),
            json!({ "nsq": { "lookupd_http_addresses": ["192.168.1.2:4161"] } }),
            json!({ "cassandra": { "addresses": ["[::1]:9042"] } }),
        ] {
            assert!(validate_input_strings(&refused).is_err(), "{refused}");
        }
        for allowed in [
            json!({ "kafka": { "addresses": ["broker.example.org:9092"] } }),
            json!({ "socket": { "address": "1.1.1.1:9000" } }),
        ] {
            assert!(validate_input_strings(&allowed).is_ok(), "{allowed}");
        }
    }
}

#[cfg(test)]
mod webhook_tests {
    use super::{DataSourceSpec, DataSourceType};
    use serde_json::{json, Value};

    fn spec(webhook: Value) -> DataSourceSpec {
        serde_json::from_value(json!({ "type": "webhook", "webhook": webhook })).expect("parses")
    }

    fn refusal(source: &DataSourceSpec) -> String {
        source.validate().expect_err("refused").to_string()
    }

    const HMAC: &str = "X-Hub-Signature-256";

    /// PL-122: an hmac or a bearer webhook is valid, resident, and its one credential is the
    /// reference the reconciler resolves.
    #[test]
    fn an_hmac_or_a_bearer_webhook_is_a_resident_source_with_one_credential() {
        let hmac = spec(json!({
            "auth": { "hmac": { "header": HMAC, "secretRef": { "name": "ttn", "key": "secret" } } }
        }));
        hmac.validate().expect("valid");
        assert_eq!(hmac.source_type, DataSourceType::Webhook);
        assert!(!hmac.terminates());
        assert_eq!(hmac.secret_refs().len(), 1);
        assert_eq!(hmac.secret_refs()[0].name, "ttn");
        let webhook = hmac.webhook.as_ref().expect("block");
        assert_eq!(
            (webhook.max_body_bytes(), webhook.max_in_flight()),
            (1_048_576, 64)
        );

        let bearer = spec(json!({
            "auth": { "bearer": { "tokenRef": { "name": "chirpstack", "key": "token" } } },
            "maxBodyBytes": 16_777_216, "maxInFlight": 1
        }));
        bearer.validate().expect("valid");
        assert_eq!(bearer.secret_refs()[0].name, "chirpstack");
    }

    /// No webhook without authentication, and none with two: a sender is one kind.
    #[test]
    fn a_webhook_names_exactly_one_credential_and_no_public_option() {
        for auth in [
            json!({}),
            json!({
                "hmac": { "header": HMAC, "secretRef": { "name": "a" } },
                "bearer": { "tokenRef": { "name": "b" } }
            }),
        ] {
            let err = refusal(&spec(json!({ "auth": auth })));
            assert!(
                err.contains("spec.webhook.auth") && err.contains("exactly one"),
                "{err}"
            );
        }
        assert!(serde_json::from_value::<DataSourceSpec>(json!({
            "type": "webhook", "webhook": { "auth": { "public": true } }
        }))
        .is_err());
        assert!(serde_json::from_value::<DataSourceSpec>(json!({
            "type": "webhook", "webhook": { "auth": { "bearer": { "tokenRef": { "name": "b" } } }, "address": "0.0.0.0:4195" }
        }))
        .is_err());
    }

    #[test]
    fn caps_header_and_credential_are_checked_naming_the_field() {
        let bearer = json!({ "bearer": { "tokenRef": { "name": "b" } } });
        for (webhook, field) in [
            (
                json!({ "auth": bearer, "maxBodyBytes": 0 }),
                "spec.webhook.maxBodyBytes",
            ),
            (
                json!({ "auth": bearer, "maxBodyBytes": 16_777_217 }),
                "spec.webhook.maxBodyBytes",
            ),
            (
                json!({ "auth": bearer, "maxInFlight": 0 }),
                "spec.webhook.maxInFlight",
            ),
            (
                json!({ "auth": bearer, "maxInFlight": 1025 }),
                "spec.webhook.maxInFlight",
            ),
            (
                json!({ "auth": { "hmac": { "header": "X Sig", "secretRef": { "name": "a" } } } }),
                "spec.webhook.auth.hmac.header",
            ),
            (
                json!({ "auth": { "bearer": { "tokenRef": { "name": "ghp_0123456789abcdefghijklmnopqrstuvwxyz" } } } }),
                "spec.webhook.auth.bearer.tokenRef",
            ),
        ] {
            let err = refusal(&spec(webhook));
            assert!(err.contains(field), "{field}: {err}");
        }
    }

    /// The managed listener is the only one: `http_server` stays refused as an author's input,
    /// and a webhook carries no other block, input, secrets or tls.
    #[test]
    fn http_server_stays_refused_and_a_webhook_carries_nothing_else() {
        let server: DataSourceSpec = serde_json::from_value(json!({
            "type": "http_server", "input": { "path": "/hook" }
        }))
        .expect("parses");
        let err = refusal(&server);
        assert!(err.contains("http_server"), "{err}");

        let auth = json!({ "bearer": { "tokenRef": { "name": "b" } } });
        for extra in [
            json!({ "http": { "url": "https://feeds.example/x" } }),
            json!({ "input": { "path": "/x" } }),
            json!({ "secrets": [{ "name": "b", "envVar": "B" }] }),
            json!({ "tls": { "caCertRef": { "name": "ca" } } }),
        ] {
            let mut value = json!({ "type": "webhook", "webhook": { "auth": auth } });
            value
                .as_object_mut()
                .expect("object")
                .extend(extra.as_object().expect("object").clone());
            let source: DataSourceSpec = serde_json::from_value(value).expect("parses");
            assert!(source.validate().is_err(), "{extra}");
        }
        let missing: DataSourceSpec =
            serde_json::from_value(json!({ "type": "webhook" })).expect("parses");
        assert!(refusal(&missing).contains("spec"));
    }
}

#[cfg(test)]
mod runner_registry_tests {
    fn source(input: &str) -> String {
        format!(
            "apiVersion: joinedcontext.com/v1alpha1\nkind: DataSource\nmetadata:\n  name: feed\n  namespace: helsinki\nspec:\n{input}"
        )
    }

    /// T-6825: an input the Bento catalog lists and the runner does not build is refused at
    /// save, naming the field and the input, instead of failing at the stream's first run.
    #[test]
    fn an_input_the_runner_does_not_build_is_refused_naming_the_field() {
        let kafka = source(
            "  type: kafka\n  input:\n    addresses: [\"broker.example.fi:9092\"]\n    topics: [\"vehicles\"]\n",
        );
        let err = crate::registry::validate_yaml("DataSource", &kafka)
            .expect("a kind")
            .expect_err("kafka is not built by the runner")
            .to_string();
        assert!(
            err.contains("spec.type") && err.contains("`kafka`"),
            "{err}"
        );
        assert!(
            err.contains("nats") && !err.contains("kafka_franz"),
            "{err}"
        );

        let nested = source(
            "  type: broker\n  input:\n    inputs:\n      - nats: { urls: [\"nats://broker.example.fi:4222\"], subject: a }\n      - amqp_0_9: { urls: [\"amqp://broker.example.fi:5672\"], queue: a }\n",
        );
        let err = crate::registry::validate_yaml("DataSource", &nested)
            .expect("a kind")
            .expect_err("amqp_0_9 is not built by the runner")
            .to_string();
        assert!(
            err.contains("spec.input") && err.contains("`amqp_0_9`"),
            "{err}"
        );

        let nats = source(
            "  type: nats\n  input:\n    urls: [\"nats://broker.example.fi:4222\"]\n    subject: vehicles\n",
        );
        crate::registry::validate_yaml("DataSource", &nats)
            .expect("a kind")
            .expect("nats is built by the runner");
    }

    /// Every input offered is one the Bento catalog draws a form for and the runner builds.
    #[test]
    fn runner_inputs_are_both_catalogs() {
        let offered: Vec<&str> = super::runner_inputs().collect();
        assert!(offered.contains(&"http_client") && offered.contains(&"nats"));
        for name in &offered {
            assert!(
                super::super::ekiben_components::INPUTS.contains(name),
                "{name}"
            );
            assert!(super::bento_inputs::INPUTS.contains(name), "{name}");
        }
        assert!(!offered.contains(&"kafka") && !offered.contains(&"stdin"));
    }
}
