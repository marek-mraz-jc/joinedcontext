// SPDX-License-Identifier: EUPL-1.2
//! The operation layer every surface serves through (ADR-0027 section 1,
//! surfaces-threat-model 9.1).
//!
//! A surface (`OData`, MCP, the NGSI-LD view root and the others) never holds the
//! broker's state, its store or its SQL: it gets an [`Ops`], and every
//! operation it serves is one [`OperationId`] run through [`Ops::run`], which
//! is the broker's own NGSI-LD handler with its policy seam, bounds, tenancy,
//! history and notifications, exactly as an NGSI-LD client would reach it
//! (`AppState::call`).
//!
//! ```
//! use antares_ops::{Caller, Ops, OperationId, OpsRequest, Surface};
//! use axum::{routing::get, Router};
//!
//! struct Hello;
//!
//! impl Surface for Hello {
//!     fn name(&self) -> &str { "hello" }
//!     fn prefix(&self) -> &str { "/x/hello" }
//!     fn version_info(&self) -> serde_json::Value { serde_json::json!({}) }
//!     fn router(&self, ops: Ops) -> Router {
//!         Router::new().route("/types", get(move |caller: Caller| async move {
//!             let req = OpsRequest::new(axum::http::Method::GET, "/ngsi-ld/v1/types");
//!             match ops.run(OperationId::RetrieveEntityTypes, &caller, req).await {
//!                 Ok(answer) => answer,
//!                 Err(refusal) => refusal.ngsi_ld(),
//!             }
//!         }))
//!     }
//! }
//!
//! let mounted = antares_ops::mount(Hello);
//! assert_eq!(mounted.prefix(), "/x/hello");
//! ```
#![forbid(unsafe_code)]

mod error;
pub mod guide;
mod mapped;
pub mod mapping;
mod names;
mod operation;
pub mod outputs;
pub mod percent;
mod scan;
pub mod view_cache;
pub mod views;

/// The one `GeoJSON` geometry rule of the broker (T-5208): the default instance, valid, RFC 7946
/// winding, for every surface that writes `GeoJSON`.
pub use antares_api::{geometry_of, rewound};
/// The NGSI-LD primer a surface tells a model (T-4532), through `antares-ops` alone (rule M2).
pub use antares_ql::PRIMER;
/// The residual of a standard query (ADR-0028 rule 2), re-exported so a surface compiles into
/// it through `antares-ops` alone (rule M2).
pub use antares_residual as residual;
pub use error::{ngsi_problem, Problem, Refusal, CONTEXT_DETAIL, INNER_DETAIL, TIMEOUT_DETAIL};
pub use operation::OperationId;
pub use scan::{pickable, Page, Pushdown, Scanned, RESIDUAL_SCAN, TOO_COMPLEX};

use antares_api::AppState;
use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use axum::Router;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The policy seam's public types (ADR-0020), so a surface names no
/// `antares-api` item.
pub mod policy {
    /// The words a refusal of a query on a stripped metadata member carries.
    pub use antares_api::policy::metadata::STRIPPED as METADATA_STRIPPED;
    pub use antares_api::policy::{
        Decision, DecisionFuture, Filter, Metadata, NotifyDecision, Operation, PolicyEngine, Rule,
        Subject,
    };
}

/// The notification sink contract (ADR-0027 section 1), for a surface that
/// delivers notifications (WebSocket).
pub use antares_api::{build_message, DeliveryError, DeliveryFuture, NotificationSink, Outbound};
/// The error a sink's endpoint check answers with.
pub use antares_model::{EntityId, NgsiError, TenantId};

/// The request bounds a surface applies to what it parses itself (a
/// WebSocket message never passes the HTTP bounds layer).
pub mod bounds {
    pub use antares_api::bounds::{
        json_depth, temporal_default_instances, temporal_instance_limit, AnswerCharge, HeldAnswer,
        ANSWER_POOL_RETRY_SECONDS, ANSWER_TOO_LARGE, DEFAULT_LIMIT, MAX_ANSWER_BYTES,
        MAX_BATCH_ITEMS, MAX_BODY_BYTES, MAX_JSON_DEPTH, MAX_LIMIT, MEMORY_OVERLOADED,
    };
}

/// The federation transport contract (ADR-0025 rule 8, ADR-0027 section 9),
/// for a crate that serves some forwards in process.
pub use antares_api::{
    AdmitFuture, BuiltForward, ForwardAnswer, ForwardFuture, ForwardTransport, NotifyFuture,
};

/// The URL this broker is reached at (`ANTARES_PUBLIC_URL`, else the
/// request's `Host`), for a surface whose documents name its resources by
/// absolute URL.
pub use antares_api::public_base;

/// The headers a surface may never set on an operation: the tenant, the
/// snapshot and the policy subject come from the outer request alone (G2).
const OUTER_ONLY: [&str; 2] = ["ngsild-tenant", "ngsild-snapshot"];

fn outer_only(name: &HeaderName) -> bool {
    OUTER_ONLY.contains(&name.as_str())
        || antares_api::policy::SUBJECT_HEADERS
            .iter()
            .any(|s| s == name.as_str())
}

/// How many bytes of an inner failure's body reach the log.
const LOGGED_DETAIL_BYTES: usize = 4096;

/// The handle a surface serves with. Its fields are private: the broker's
/// state inside is never reachable from a surface (threat F1).
#[derive(Clone)]
pub struct Ops {
    state: AppState,
    /// The narrowed handle of each view served lately, so a request does
    /// not build the narrowed state and its in-process router anew; an
    /// idle one is dropped ([`view_cache`]).
    views: Arc<view_cache::ViewCache<ViewOps>>,
    /// The surface these operations are run for, named on each one's log
    /// line (threat G10); `None` for the broker's own in-process forwards.
    surface: Option<Arc<str>>,
}

/// The operations of one loaded view (ADR-0023): every one runs composed
/// with the view's narrowing, in the view's one Tenant, and only when the
/// view allows it. Made by [`Ops::view`].
#[derive(Clone)]
pub struct ViewOps {
    inner: Ops,
    view: Arc<antares_api::views::CompiledView>,
    tenant: antares_model::TenantId,
    operations: Vec<OperationId>,
    mapping: Option<Arc<mapping::Mapping>>,
    /// The core @context with the view's own merged in: what a name a caller
    /// writes in an NGSI-LD parameter means here (`names.rs`).
    names: Arc<antares_jsonld::Context>,
    representations: Option<Arc<serde_json::Map<String, serde_json::Value>>>,
}

/// The Entity types a caller may see through a view and the Attributes
/// each may show, as IRIs (ADR-0023 rule 5): made by [`ViewOps::visible`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Visible {
    /// Each rule's types and the Attributes it shows; `None` for an
    /// engine's grant over the whole Entity (`Rule::every_attribute`), which
    /// shows every Attribute of its types.
    rules: Vec<(BTreeSet<String>, Option<BTreeSet<String>>)>,
    /// The filter's `pick` (empty keeps every name) and `omit`, applied to
    /// every rule.
    pick: BTreeSet<String>,
    omit: BTreeSet<String>,
    /// Attributes removed per type, on top of the rules.
    omit_by_type: BTreeMap<String, BTreeSet<String>>,
    /// A filter without rules narrows by none (a `*` view, ADR-0023 rule
    /// 12): every type and every Attribute, less `pick` and `omit`.
    every: bool,
}

impl Visible {
    /// Whether an Entity of type `name` (an IRI or a core-context term) may
    /// be seen.
    ///
    /// ```
    /// assert!(!antares_ops::Visible::default().shows_type("Car"));
    /// ```
    #[must_use]
    pub fn shows_type(&self, name: &str) -> bool {
        let ty = expand(name);
        self.every || self.rules.iter().any(|(types, _)| types.contains(&ty))
    }

    /// Whether an Entity of type `ty` may show Attribute `attr`.
    ///
    /// ```
    /// assert!(!antares_ops::Visible::default().shows_attribute("Car", "speed"));
    /// ```
    #[must_use]
    pub fn shows_attribute(&self, ty: &str, attr: &str) -> bool {
        let (ty, attr) = (expand(ty), expand(attr));
        let kept = (self.pick.is_empty() || self.pick.contains(&attr))
            && !self.omit.contains(&attr)
            && !self
                .omit_by_type
                .get(&ty)
                .is_some_and(|gone| gone.contains(&attr));
        kept && (self.every
            || self.rules.iter().any(|(types, attrs)| {
                types.contains(&ty) && attrs.as_ref().is_none_or(|a| a.contains(&attr))
            }))
    }

    /// What `filter`'s rules show: each rule's types with its Attributes
    /// (every one for a rule over the whole Entity), less what `pick` leaves
    /// out and `omit` and `omit_by_type` remove; a filter without rules
    /// shows every type and Attribute, less the same. A rule's condition is
    /// not read: it decides which Entities show the Attributes, not which
    /// Attributes there are.
    ///
    /// ```
    /// use antares_ops::policy::{Filter, Rule};
    /// let visible = antares_ops::Visible::from_filter(&Filter {
    ///     rules: vec![Rule {
    ///         types: vec!["https://e.org/Car".into()],
    ///         attributes: vec!["https://e.org/speed".into(), "https://e.org/vin".into()],
    ///         ..Rule::default()
    ///     }],
    ///     omit: vec!["https://e.org/vin".into()],
    ///     ..Filter::default()
    /// });
    /// assert!(visible.shows_attribute("https://e.org/Car", "https://e.org/speed"));
    /// assert!(!visible.shows_attribute("https://e.org/Car", "https://e.org/vin"));
    /// ```
    #[must_use]
    pub fn from_filter(filter: &policy::Filter) -> Self {
        let set =
            |names: &[String]| -> BTreeSet<String> { names.iter().map(|n| expand(n)).collect() };
        let rules = filter
            .rules
            .iter()
            .map(|rule| {
                let attrs = (!rule.every_attribute).then(|| set(&rule.attributes));
                (set(&rule.types), attrs)
            })
            .collect();
        Self {
            rules,
            pick: set(&filter.pick),
            omit: set(&filter.omit),
            omit_by_type: filter
                .omit_by_type
                .iter()
                .map(|(t, attrs)| (expand(t), set(attrs)))
                .collect(),
            every: filter.rules.is_empty(),
        }
    }
}

/// `name` as an IRI under the core @context, as a view's names are kept.
fn expand(name: &str) -> String {
    static CORE: std::sync::LazyLock<antares_jsonld::Context> =
        std::sync::LazyLock::new(antares_jsonld::core_context);
    CORE.expand_key(name)
}

/// The headers of the request a surface is answering, as the HTTP layer
/// received them. Only an extractor makes one, so a surface cannot invent
/// the tenant or the subject an operation runs as.
#[derive(Debug, Clone)]
pub struct Caller(HeaderMap);

impl Caller {
    /// The policy subject headers the caller sent, in the order
    /// `ANTARES_POLICY_SUBJECT_HEADERS` names them, each with every value:
    /// what a surface compares when it binds something to one subject
    /// (a resumed WebSocket connection, ADR-0026 rule 7).
    #[must_use]
    pub fn subject(&self) -> Vec<(String, Vec<u8>)> {
        antares_api::policy::SUBJECT_HEADERS
            .iter()
            .flat_map(|name| {
                self.0
                    .get_all(name.as_str())
                    .iter()
                    .map(move |v| (name.clone(), v.as_bytes().to_vec()))
            })
            .collect()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(Ok(Self(parts.headers.clone())))
    }
}

/// The NGSI-LD request a surface builds for one operation. It has no way to
/// carry `NGSILD-Tenant`, `NGSILD-Snapshot` or a policy subject header:
/// [`OpsRequest::header`] drops them, and the fields are private, so it is
/// the only way a header gets in.
#[derive(Debug)]
pub struct OpsRequest {
    method: Method,
    path_and_query: String,
    headers: HeaderMap,
    body: Body,
    context: Option<Arc<serde_json::Value>>,
}

impl OpsRequest {
    /// A request for `path_and_query` under `/ngsi-ld/v1`.
    ///
    /// ```
    /// let req = antares_ops::OpsRequest::new(axum::http::Method::GET, "/ngsi-ld/v1/types");
    /// ```
    #[must_use]
    pub fn new(method: Method, path_and_query: impl Into<String>) -> Self {
        Self {
            method,
            path_and_query: path_and_query.into(),
            headers: HeaderMap::new(),
            body: Body::empty(),
            context: None,
        }
    }

    /// One more header. The tenant, the snapshot and the policy subject are
    /// the outer request's, so a header naming one of them is dropped.
    ///
    /// ```
    /// use axum::http::{HeaderName, HeaderValue, Method};
    /// let req = antares_ops::OpsRequest::new(Method::GET, "/ngsi-ld/v1/types")
    ///     .header(HeaderName::from_static("accept"), HeaderValue::from_static("application/json"));
    /// ```
    #[must_use]
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        if !outer_only(&name) {
            self.headers.append(name, value);
        }
        self
    }

    /// Every header of `all` as [`Self::header`] adds one, moved rather than
    /// cloned one by one (R3-024).
    #[must_use]
    pub fn headers(mut self, all: HeaderMap) -> Self {
        let mut last: Option<HeaderName> = None;
        for (name, value) in all {
            // a repeated header's later values come without their name
            if let Some(name) = name {
                last = Some(name);
            }
            if let Some(name) = last.as_ref().filter(|n| !outer_only(n)) {
                self.headers.append(name, value);
            }
        }
        self
    }

    /// The @context the request's names are read under, in place of the
    /// outer request's `Link` header (a read's `q`, `attrs`, `type`): a
    /// surface that names Attributes outside the core @context defines its
    /// own terms, and the outer client cannot redefine them.
    ///
    /// ```
    /// let req = antares_ops::OpsRequest::new(axum::http::Method::GET, "/ngsi-ld/v1/entities?type=Car&q=speed>1")
    ///     .context(serde_json::json!({"Car": "https://example.org/Car", "speed": "https://example.org/speed"}));
    /// ```
    #[must_use]
    pub fn context(mut self, context: serde_json::Value) -> Self {
        self.context = Some(Arc::new(context));
        self
    }

    /// The request body.
    ///
    /// ```
    /// let req = antares_ops::OpsRequest::new(axum::http::Method::POST, "/ngsi-ld/v1/entities")
    ///     .body(r#"{"id":"urn:ngsi-ld:Car:1","type":"Car"}"#);
    /// ```
    #[must_use]
    pub fn body(mut self, body: impl Into<Body>) -> Self {
        self.body = body.into();
        self
    }
}

impl Ops {
    fn new(state: AppState, surface: Option<Arc<str>>) -> Self {
        Self {
            state,
            views: Arc::default(),
            surface,
        }
    }

    /// The most bytes one answer may hold (`max_answer_bytes`, [`bounds::MAX_ANSWER_BYTES`]):
    /// the ceiling a surface that assembles a file itself writes under (T-4465).
    #[must_use]
    pub fn max_answer_bytes(&self) -> usize {
        self.state.max_answer_bytes
    }

    /// Whether `headers` carry the broker's `/q` admin bearer token: an
    /// internal route of a surface (the iCalendar preview, T-4901) serves
    /// the Portal alone. `false` when no admin token is configured, so
    /// such a route fails closed where `/q` would be open.
    #[must_use]
    pub fn admits_admin(&self, headers: &HeaderMap) -> bool {
        let presented = headers
            .get(axum::http::header::AUTHORIZATION)
            .map(HeaderValue::as_bytes);
        self.state
            .admin_auth
            .as_deref()
            .is_some_and(|auth| auth.admits(presented))
    }

    /// An answer's body read whole, up to the broker's `max_answer_bytes`
    /// ([`bounds::MAX_ANSWER_BYTES`]): the page size bounds the Entities, not the
    /// bytes, and every concurrent request holds its buffer.
    ///
    /// The answer holds its share of the broker's buffered-answer pool until
    /// it drops (T-5895).
    ///
    /// # Errors
    /// [`Refusal::Limit`] with [`bounds::ANSWER_TOO_LARGE`] past the bound,
    /// [`Refusal::Overloaded`] while the pool has no room.
    pub async fn read_answer(
        &self,
        body: Body,
        surface: &'static str,
    ) -> Result<bounds::HeldAnswer, Refusal> {
        self.state
            .read_answer(body, surface)
            .await
            .map_err(|r| match r {
                antares_api::bounds::AnswerRefusal::OverBudget => Refusal::Overloaded {
                    retry_after: Some(HeaderValue::from(
                        antares_api::bounds::ANSWER_POOL_RETRY_SECONDS,
                    )),
                },
                _ => Refusal::Limit(bounds::ANSWER_TOO_LARGE),
            })
    }

    /// The operations of the loaded view `id`, or `None` when no view of
    /// that id is loaded: an unknown id, a view that failed validation and
    /// a removed one are the same answer.
    #[must_use]
    ///
    /// A view of another model (one with a `mapping`) is `None` here too:
    /// only the NGSI-LD view root rebuilds its answers, so every other
    /// surface answers it as an unknown view ([`Self::mapped_view`]).
    pub fn view(&self, id: &str) -> Option<ViewOps> {
        self.mapped_view(id).filter(|v| v.mapping.is_none())
    }

    /// [`Self::view`], a view of another model included: for the NGSI-LD
    /// view root, which inverts each request and rebuilds each answer
    /// through [`ViewOps::mapping`].
    #[must_use]
    pub fn mapped_view(&self, id: &str) -> Option<ViewOps> {
        let table = self.state.views.snapshot();
        let Some(antares_api::views::ViewEntry::Loaded {
            view,
            tenant,
            operations,
            mapping,
            context,
            representations,
            ..
        }) = table.get(id)
        else {
            return None;
        };
        // The loader stored the tenant it validated, so this never refuses.
        let tenant = antares_model::TenantId::new(tenant).ok()?;
        // the loader attaches nothing else, so a failed downcast is a view
        // no surface serves
        let mapping = match mapping {
            None => None,
            Some(a) => Some(a.0.clone().downcast::<mapping::Mapping>().ok()?),
        };
        let build = || {
            Some(ViewOps {
                inner: Self::new(self.state.with_view(view.clone()), self.surface.clone()),
                view: view.clone(),
                tenant,
                operations: operations
                    .iter()
                    .filter_map(|o| OperationId::from_id(o))
                    .collect(),
                mapping,
                names: Arc::new(names::of_view(context.as_deref())?),
                representations: representations.clone(),
            })
        };
        // Only views loaded now stay cached: a removed one goes with the
        // next sweep. A changed view is a new `CompiledView`, so a handle
        // of the old one is never served again.
        let (made, gone) = self.views.get_or_build(
            id,
            |held| Arc::ptr_eq(&held.view, view),
            |k| table.contains_key(k),
            std::time::Instant::now(),
            view_cache::idle(),
            build,
        );
        view_cache::release(gone);
        made
    }

    /// Run `op` as the caller of the outer request.
    ///
    /// The request must be the operation it names: its method, path and
    /// query match `op`'s NGSI-LD binding (`OperationId::of`), so a surface
    /// reaches the NGSI-LD API only through the operations of the matrix and
    /// never the admin or another surface's routes. A failure inside the
    /// broker (5xx) never reaches the surface with its detail: it becomes
    /// [`Refusal::Inner`] with a request id, and the detail goes to the log
    /// under that id.
    ///
    /// # Errors
    /// [`Refusal::NotAllowed`] when the request is not `op`,
    /// [`Refusal::Malformed`] when its target is not a URI,
    /// [`Refusal::Inner`] or [`Refusal::Overloaded`] when the broker failed.
    pub async fn run(
        &self,
        op: OperationId,
        caller: &Caller,
        req: OpsRequest,
    ) -> Result<Response, Refusal> {
        self.run_in(op, caller, req, None).await
    }

    /// [`Self::run`], logged with the view (id and Tenant) it runs through.
    async fn run_in(
        &self,
        op: OperationId,
        caller: &Caller,
        req: OpsRequest,
        view: Option<(&str, &str)>,
    ) -> Result<Response, Refusal> {
        let (path, query) = match req.path_and_query.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (req.path_and_query.as_str(), None),
        };
        if OperationId::of(req.method.as_str(), path, query) != Some(op) {
            return Err(Refusal::NotAllowed { allow: Vec::new() });
        }
        let mut inner = axum::http::Request::builder()
            .method(req.method)
            .uri(req.path_and_query.as_str())
            .body(req.body)
            .map_err(|_| Refusal::Malformed("the operation's target is not a URI"))?;
        *inner.headers_mut() = req.headers;
        let call = async {
            match req.context {
                Some(context) => self.state.call_in_context(&caller.0, inner, context).await,
                None => self.state.call(&caller.0, inner).await,
            }
        };
        // The engine learns the view from the in-process scope, never from
        // a header a client could send (ADR-0020 amendment, T-3702).
        let answer = match view {
            Some((id, _)) => antares_api::policy::through_view(id, call).await,
            None => call.await,
        };
        if let Some(surface) = &self.surface {
            // Threat G10: which surface and view ran it; never the subject
            // or an argument (ADR-0020).
            let status = answer.status().as_u16();
            if let Some((view, tenant)) = view {
                tracing::info!(
                    surface = &**surface,
                    view,
                    tenant,
                    operation = op.id(),
                    status,
                    "surface operation"
                );
            } else {
                tracing::info!(
                    surface = &**surface,
                    operation = op.id(),
                    status,
                    "surface operation"
                );
            }
        }
        answered(op, answer).await
    }
}

/// The API's answer as a surface gets it: a 5xx becomes a [`Refusal`], and
/// only 503 and the API's own 504 `LdContextNotAvailable` keep their status.
async fn answered(op: OperationId, answer: Response) -> Result<Response, Refusal> {
    if !answer.status().is_server_error() {
        return Ok(answer);
    }
    if answer.status() == StatusCode::SERVICE_UNAVAILABLE {
        return Err(Refusal::Overloaded {
            retry_after: answer
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .cloned(),
        });
    }
    let status = answer.status();
    let detail = axum::body::to_bytes(answer.into_body(), LOGGED_DETAIL_BYTES)
        .await
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    // 6.3.2: a remote @context the caller named is the caller's to know
    // about; a forward that timed out (also 504) stays an inner failure.
    if context_not_available(status, &detail) {
        return Err(Refusal::ContextNotAvailable);
    }
    // antares-sql logged the cancelled statement already
    if statement_timeout(status, &detail) {
        return Err(Refusal::StatementTimeout);
    }
    Err(inner_failure(op, status, &detail))
}

/// Whether an inner answer is the NGSI-LD API's 500 for a query the database
/// cancelled at the statement timeout (`antares-sql` maps SQLSTATE 57014).
fn statement_timeout(status: StatusCode, body: &str) -> bool {
    status == StatusCode::INTERNAL_SERVER_ERROR
        && serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|p| p.get("detail")?.as_str().map(str::to_owned))
            .is_some_and(|d| d == error::TIMEOUT_DETAIL)
}

/// Whether an inner answer is the NGSI-LD API's 504 `LdContextNotAvailable`
/// (6.3.2), told apart from a forward's 504 by its problem type.
fn context_not_available(status: StatusCode, body: &str) -> bool {
    status == StatusCode::GATEWAY_TIMEOUT
        && serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|p| p.get("type")?.as_str().map(str::to_owned))
            .is_some_and(|t| t == "https://uri.etsi.org/ngsi-ld/errors/LdContextNotAvailable")
}

impl ViewOps {
    /// The compiled Mapping of a view of another model, `None` for a view
    /// of the store's own model.
    #[must_use]
    pub fn mapping(&self) -> Option<&mapping::Mapping> {
        self.mapping.as_deref()
    }

    /// [`Ops::read_answer`] for an answer of this view.
    ///
    /// # Errors
    /// [`Refusal::Limit`] past the bound.
    pub async fn read_answer(
        &self,
        body: Body,
        surface: &'static str,
    ) -> Result<bounds::HeldAnswer, Refusal> {
        self.inner.read_answer(body, surface).await
    }

    /// Whether the view allows `op`.
    #[must_use]
    pub fn allows(&self, op: OperationId) -> bool {
        self.operations.contains(&op)
    }

    /// [`Ops::forward_in_process`] through this view: the forward is
    /// served as a request to the view's Tenant through the view is, the
    /// view's rules composed with that Tenant's engine (ADR-0023 rule 5,
    /// ADR-0025 rule 5).
    pub async fn forward_in_process(&self, req: BuiltForward<'_>, self_url: &str) -> ForwardAnswer {
        self.inner
            .forward_through(Some(self.view.id()), req, self_url)
            .await
    }

    /// A handle on the view compilation this request reads through (T-5909): it compares
    /// equal ([`std::sync::Weak::ptr_eq`]) only to the same compilation, so a surface cache
    /// keyed by it is not served after the view's rules change.
    #[must_use]
    pub fn generation(&self) -> std::sync::Weak<dyn std::any::Any + Send + Sync> {
        let any: Arc<dyn std::any::Any + Send + Sync> = self.view.clone();
        Arc::downgrade(&any)
    }

    /// The one Tenant the view serves (ADR-0023 rule 10).
    #[must_use]
    pub fn tenant(&self) -> &str {
        self.tenant.as_str()
    }

    /// The view's `representations` member for surface `name` as written
    /// (`views::REPRESENTATIONS`), for that surface to parse; `None` when
    /// the view names none.
    #[must_use]
    pub fn representation(&self, name: &str) -> Option<&serde_json::Value> {
        self.representations.as_deref()?.get(name)
    }

    /// The `LinkML` schema of the view's Tenant as written to
    /// `/q/schemas/{tenant}`, with its revision, if one is stored (T-3628).
    /// The surface parses it under its own limits and caches the parse per
    /// revision; another Tenant's schema never reaches it.
    #[must_use]
    pub fn schema(&self) -> Option<(u64, Arc<[u8]>)> {
        self.inner
            .state
            .views
            .schema(self.tenant.as_str())
            .map(|s| (s.revision, s.bytes))
    }

    /// The Entity types and the Attributes the view shows, compacted under
    /// the core @context, sorted: the names a surface may offer the caller.
    #[must_use]
    pub fn shown_names(&self) -> (Vec<String>, Vec<String>) {
        self.view.shown_names()
    }

    /// What `caller` may see through the view for `op`, a read the view
    /// allows: the view's rules composed with the engine's answer for the
    /// operation's clause and the expanded `types` and `attrs` it names
    /// (`effective = view ∧ engine`, ADR-0023 rule 5), asked once, in the
    /// view's Tenant, failing closed as every gate does.
    ///
    /// # Errors
    /// [`Refusal::NotAllowed`] when the view does not allow `op`,
    /// [`Refusal::Denied`] when the engine refuses it.
    pub async fn visible(
        &self,
        caller: &Caller,
        op: OperationId,
        types: &[String],
        attrs: &[String],
    ) -> Result<Visible, Refusal> {
        let Some(clause) = op.gate_clause().filter(|_| self.allows(op)) else {
            return Err(Refusal::NotAllowed { allow: Vec::new() });
        };
        let engine = match self.inner.state.policy.as_deref() {
            None => Ok(antares_api::policy::unnarrowed()),
            Some(engine) => {
                let asked = policy::Operation {
                    types,
                    attrs,
                    ..policy::Operation::new(clause)
                };
                let gated = antares_api::policy::gate(engine, &self.tenant, &caller.0, &asked);
                // The engine learns the view in process (T-3702), as for a run.
                antares_api::policy::through_view(self.view.id(), gated).await
            }
        };
        antares_api::views::with_view(Some(&self.view), clause, engine)
            .map(|f| Visible::from_filter(&f))
            .map_err(|d| Refusal::Denied(d.0))
    }

    /// The `type=` a surface's Query Entities pushes for the local type
    /// names `wanted` (the last IRI segment): the view's shown types of
    /// those names, or for a view of every type the Tenant's stored types of
    /// those names (F-ANT5-010, F-ANT5-011), so the read never walks the
    /// whole Tenant into the residual-scan bound. `None` when a view that
    /// names its types shows none of `wanted`; `local=true`, the whole
    /// local store, only when the store keeps no type list.
    pub async fn type_selector(&self, wanted: &[&str]) -> Option<String> {
        let local = |n: &str| n.rsplit(['/', '#']).next().unwrap_or(n).to_owned();
        let named = |names: &[String]| -> Vec<String> {
            names
                .iter()
                .filter(|t| wanted.contains(&local(t).as_str()))
                .map(|t| percent::encode(t))
                .collect()
        };
        let matching = if self.shows_every_name() {
            match self.inner.state.stored_type_names(&self.tenant).await {
                Ok(Some(stored)) => match named(&stored) {
                    // no Entity of those types yet: a selector that matches none
                    m if m.is_empty() => wanted.iter().map(|t| percent::encode(t)).collect(),
                    m => m,
                },
                _ => return Some("local=true".to_owned()),
            }
        } else {
            named(&self.shown_names().0)
        };
        (!matching.is_empty()).then(|| format!("type={}", matching.join(",")))
    }

    /// The Entity types a Subscription through the view names to watch everything it shows:
    /// the shown types, or for a view of every type the Tenant's stored types. Empty when the
    /// store keeps no type list or holds no Entity yet.
    pub async fn watched_types(&self) -> Vec<String> {
        if !self.shows_every_name() {
            return self.shown_names().0;
        }
        self.inner
            .state
            .stored_type_names(&self.tenant)
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// Whether the view shows every type and Attribute (ADR-0023 rule 12):
    /// [`Self::shown_names`] and [`Self::shown_by_type`] are then empty and a
    /// surface offers a name by its syntax alone.
    #[must_use]
    pub fn shows_every_name(&self) -> bool {
        self.view.shows_every_name()
    }

    /// The type and Attribute names a caller of a view of another model
    /// uses: the target model's, limited to what the view's rules show of
    /// the source class ([`mapping::Mapping::target_names`]); `None` for a
    /// view of its stored model.
    #[must_use]
    pub fn target_names(&self) -> Option<(Vec<String>, Vec<String>)> {
        let mapping = self.mapping.as_deref()?;
        // a mapped view selects its source class alone, and names what it
        // shows of it: never the `*` form
        let shown: std::collections::BTreeSet<String> = self
            .view
            .narrowing()
            .rules
            .iter()
            .filter(|rule| rule.types.iter().any(|t| t == mapping.source_class()))
            .flat_map(|rule| rule.attributes.iter().cloned())
            .collect();
        Some(mapping.target_names(&shown))
    }

    /// Each Entity type the view shows with the Attributes it shows of it,
    /// as expanded IRIs: the union of the view's rules, type by type. A
    /// surface that builds a model from a schema intersects it with this.
    #[must_use]
    pub fn shown_by_type(
        &self,
    ) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
        let mut out: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
            std::collections::BTreeMap::new();
        for rule in &self.view.narrowing().rules {
            for ty in &rule.types {
                out.entry(ty.clone())
                    .or_default()
                    .extend(rule.attributes.iter().cloned());
            }
        }
        out
    }

    /// For each of `ops`, whether the view allows it and the policy engine
    /// could allow a request of it for `caller`, asked in one probe in the
    /// view's Tenant (MCP D-3). The probe fails closed: an engine that fails
    /// it offers nothing.
    pub async fn offered(&self, caller: &Caller, ops: &[OperationId]) -> Vec<bool> {
        let asked = |op: &OperationId| self.allows(*op) && op.gate_clause().is_some();
        let clauses: Vec<&'static str> = ops
            .iter()
            .filter(|op| asked(op))
            .filter_map(|op| op.gate_clause())
            .collect();
        let mut answers = antares_api::policy::through_view(
            self.view.id(),
            antares_api::policy::offers(
                self.inner.state.policy.as_deref(),
                &self.tenant,
                &caller.0,
                &clauses,
            ),
        )
        .await
        .into_iter();
        // The answers come in the order asked; a short answer offers nothing.
        ops.iter()
            .map(|op| asked(op) && answers.next().unwrap_or(false))
            .collect()
    }

    /// Run `op` through the view, as [`Ops::run`] does, in the view's
    /// Tenant (ADR-0023 rule 10): a caller's `NGSILD-Tenant` naming another
    /// Tenant is refused, never a switch, and the answer names the Tenant
    /// only to a caller who named it (threat F4/V18).
    ///
    /// # Errors
    /// [`Refusal::NotAllowed`] when the view does not allow `op`,
    /// [`Refusal::Malformed`] when the caller names another Tenant, and
    /// everything [`Ops::run`] refuses.
    pub async fn run(
        &self,
        op: OperationId,
        caller: &Caller,
        req: OpsRequest,
    ) -> Result<Response, Refusal> {
        match &self.mapping {
            // a view of another model inverts the request and rebuilds the
            // answer, for every surface alike (G18)
            Some(mapping) if self.allows(op) => mapped::serve(self, mapping, op, caller, req).await,
            Some(_) => Err(Refusal::NotAllowed { allow: Vec::new() }),
            None => self.run_unmapped(op, caller, req).await,
        }
    }

    /// [`Self::run`] of a request already in the view's stored model.
    pub(crate) async fn run_unmapped(
        &self,
        op: OperationId,
        caller: &Caller,
        req: OpsRequest,
    ) -> Result<Response, Refusal> {
        const TENANT: HeaderName = HeaderName::from_static("ngsild-tenant");
        const OTHER: &str = "a view serves one tenant";
        if !self.allows(op) {
            return Err(Refusal::NotAllowed { allow: Vec::new() });
        }
        let mut sent = caller.0.get_all(&TENANT).iter();
        let named = match (sent.next(), sent.next()) {
            (None, _) => false,
            (Some(one), None) => {
                let same = one
                    .to_str()
                    .ok()
                    .and_then(|t| antares_model::TenantId::new(t).ok())
                    .is_some_and(|t| t.as_str() == self.tenant.as_str());
                if !same {
                    return Err(Refusal::Malformed(OTHER));
                }
                true
            }
            (Some(_), Some(_)) => return Err(Refusal::Malformed(OTHER)),
        };
        let mut headers = caller.0.clone();
        headers.insert(
            TENANT,
            HeaderValue::from_str(self.tenant.as_str()).map_err(|_| Refusal::Malformed(OTHER))?,
        );
        let mut answer = self
            .inner
            .run_in(
                op,
                &Caller(headers),
                req,
                Some((self.view.id(), self.tenant.as_str())),
            )
            .await?;
        if !named {
            answer.headers_mut().remove(TENANT);
        }
        Ok(answer)
    }
}

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

/// Log an inner failure under a fresh id and answer with the id alone.
fn inner_failure(op: OperationId, status: StatusCode, detail: &str) -> Refusal {
    let request_id = format!(
        "ops-{:x}-{:x}",
        std::process::id(),
        NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
    );
    tracing::warn!(
        request_id,
        operation = op.id(),
        status = status.as_u16(),
        detail,
        "an operation failed inside the broker"
    );
    Refusal::Inner { request_id }
}

/// What a surface implements: its routes, given the [`Ops`] it serves with.
pub trait Surface: Send + Sync + 'static {
    /// The name `ANTARES_API_SURFACES` selects it by.
    fn name(&self) -> &str;
    /// Where it mounts: `/x` or below it.
    fn prefix(&self) -> &str;
    /// The routes, relative to the prefix.
    fn router(&self, ops: Ops) -> Router;
    /// What `/q/health` reports about it.
    fn version_info(&self) -> serde_json::Value;
    /// Whether it serves only while a view is loaded (fail-closed, T-3496).
    /// A surface of the NGSI-LD root itself, which runs every operation
    /// through the policy seam unnarrowed (the WebSocket channel of a
    /// Tenant, ADR-0026 rule 2), answers `false`.
    fn needs_view(&self) -> bool {
        true
    }
}

/// The name an NGSI-LD answer gives the expanded IRI `iri` when the
/// request names no `@context`: its compaction under the core context, the
/// IRI itself when the core context has no term for it. A surface that
/// reads an answer by IRI maps its keys through this.
///
/// ```
/// assert_eq!(antares_ops::core_name("https://uri.etsi.org/ngsi-ld/location"), "location");
/// assert_eq!(antares_ops::core_name("https://example.org/speed"), "https://example.org/speed");
/// ```
#[must_use]
pub fn core_name(iri: &str) -> String {
    CORE.compact_iri(iri)
}

/// Whether the core @context defines the term `name` (its keyword aliases `id` and `type`
/// included), which a surface's own @context must not define again.
///
/// ```
/// assert!(antares_ops::is_core_term("location"));
/// assert!(antares_ops::is_core_term("value"));
/// assert!(!antares_ops::is_core_term("speed"));
/// ```
#[must_use]
pub fn is_core_term(name: &str) -> bool {
    CORE.term(name).is_some()
}

/// The expanded IRI of the short name `name` under the core @context, as a view's names are
/// read (ADR-0020); a surface rebases a schema onto a view's names with it (T-3752).
///
/// ```
/// assert_eq!(antares_ops::core_iri("location"), "https://uri.etsi.org/ngsi-ld/location");
/// assert_eq!(antares_ops::core_iri("Car"), "https://uri.etsi.org/ngsi-ld/default-context/Car");
/// ```
#[must_use]
pub fn core_iri(name: &str) -> String {
    CORE.expand_key(name)
}

/// The core @context, read once.
static CORE: std::sync::LazyLock<antares_jsonld::Context> =
    std::sync::LazyLock::new(antares_jsonld::core_context);

/// `surface` as the broker mounts it.
///
/// ```
/// # use antares_ops::{Ops, Surface};
/// # struct Empty;
/// # impl Surface for Empty {
/// #     fn name(&self) -> &str { "empty" }
/// #     fn prefix(&self) -> &str { "/x/empty" }
/// #     fn router(&self, _ops: Ops) -> axum::Router { axum::Router::new() }
/// #     fn version_info(&self) -> serde_json::Value { serde_json::Value::Null }
/// # }
/// let mounted = antares_ops::mount(Empty);
/// assert_eq!(mounted.name(), "empty");
/// ```
pub fn mount<S: Surface>(surface: S) -> Box<dyn antares_api::ApiSurface> {
    Box::new(Mounted {
        surface,
        views: std::sync::OnceLock::new(),
    })
}

/// The methods a read-only surface answers.
pub const READ_ONLY_METHODS: &str = "GET, HEAD, OPTIONS";

/// A read-only surface's answer to `method`: `OPTIONS` lists [`READ_ONLY_METHODS`], any method but
/// `GET` and `HEAD` is the surface's own `refused` answer with that list in `Allow`, and `HEAD` is
/// `get`'s answer without its body.
pub async fn read_only(
    method: &Method,
    refused: impl FnOnce() -> Response,
    get: impl std::future::Future<Output = Response>,
) -> Response {
    let allow = HeaderValue::from_static(READ_ONLY_METHODS);
    if method == Method::OPTIONS {
        return axum::response::IntoResponse::into_response((
            StatusCode::NO_CONTENT,
            [(axum::http::header::ALLOW, allow)],
        ));
    }
    if method != Method::GET && method != Method::HEAD {
        let mut reply = refused();
        reply.headers_mut().insert(axum::http::header::ALLOW, allow);
        return reply;
    }
    let reply = get.await;
    if method == Method::HEAD {
        let (parts, _) = reply.into_parts();
        return Response::from_parts(parts, Body::empty());
    }
    reply
}

/// Register the forward transport `make` builds over the broker's
/// operations: every federation forward asks it first, and a forward it
/// does not serve goes over HTTP (ADR-0025 rule 8). The composition root
/// calls this once, before the state serves.
///
/// ```
/// use antares_ops::{BuiltForward, ForwardFuture, ForwardTransport};
///
/// /// Serves nothing: every forward stays HTTP.
/// struct Nothing;
///
/// impl ForwardTransport for Nothing {
///     fn forward<'a>(&'a self, _req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
///         None
///     }
/// }
///
/// let transport: std::sync::Arc<dyn ForwardTransport> = std::sync::Arc::new(Nothing);
/// # let _ = transport;
/// ```
///
/// # Errors
/// When a transport is already registered.
pub fn with_forward_transport(
    state: AppState,
    make: impl FnOnce(Ops) -> Arc<dyn ForwardTransport>,
) -> Result<AppState, String> {
    let transport = make(Ops::new(state.clone(), None));
    state.with_forward_transport(transport)
}

impl Ops {
    /// Serve the forward `req` through this broker's own router, in process
    /// (ADR-0025 rule 2). `self_url` is the self URL its endpoint starts
    /// with; the rest of its target is the path the router serves. The
    /// inner request carries the forward's headers and payload and nothing
    /// else of the triggering request: the caller map is its subject headers
    /// alone (rules 2 and 4), so a registration without a `tenant` lands in
    /// the default Tenant and a snapshot is never inherited. The answer is
    /// read as an HTTP one is: the body up to the federation response cap,
    /// the source's `NGSILD-Warning` values up to their cap.
    ///
    /// Past [`antares_api::bounds::MAX_IN_PROCESS_CALL_DEPTH`] the forward
    /// is not made, in process or over HTTP (rule 3): it answers 508 for an
    /// `exclusive` or `redirect` registration, whose failure is the
    /// operation's answer, and 504 otherwise, which the merge reports as
    /// code 199 (6.3.17), as a loop is.
    pub async fn forward_in_process(&self, req: BuiltForward<'_>, self_url: &str) -> ForwardAnswer {
        self.forward_through(None, req, self_url).await
    }

    /// [`Self::forward_in_process`] through the view `view` (T-3935): the
    /// engine of the target learns it as for a request through that view.
    async fn forward_through(
        &self,
        view: Option<&str>,
        req: BuiltForward<'_>,
        self_url: &str,
    ) -> ForwardAnswer {
        if antares_api::state::in_process_depth() >= antares_api::bounds::MAX_IN_PROCESS_CALL_DEPTH
        {
            let status = if req.proxied() { 508 } else { 504 };
            return (status, serde_json::Value::Null, Vec::new());
        }
        // A target outside `self_url` is the transport's mistake, and a
        // header 5.9.2 let through that HTTP could not send either: the
        // forward is not made in process, and the answer says so (502).
        let Some(inner) = in_process_request(&req, self_url) else {
            return (502, serde_json::Value::Null, Vec::new());
        };
        let answer = self
            .state
            .call_on_task(view, req.subject().clone(), inner)
            .await;
        let status = answer.status().as_u16();
        let warnings = answer
            .headers()
            .get_all("ngsild-warning")
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_owned))
            .take(antares_api::bounds::MAX_PEER_WARNINGS)
            .collect();
        let cap = *antares_api::bounds::MAX_FED_RESPONSE_BYTES;
        let body = axum::body::to_bytes(answer.into_body(), cap)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(serde_json::Value::Null);
        (status, body, warnings)
    }
}

/// The notifications an internal transport delivers in process (ADR-0025
/// rule 7).
impl Ops {
    /// The Tenant receiving the notifications of the distributed
    /// Subscription copy `subscription_id` (5.8.1.4), from this broker's
    /// inbound index; `None` when no Subscription maps it.
    pub async fn notification_receiver(&self, subscription_id: &str) -> Option<String> {
        self.state.notification_receiver(subscription_id).await
    }

    /// POST `notification` to `path` through this broker's own router, as
    /// the HTTP sink would send it to the self URL, with no subject: a
    /// notification is broker-initiated (ADR-0020). Past
    /// [`antares_api::bounds::MAX_IN_PROCESS_CALL_DEPTH`] it is not made.
    ///
    /// # Errors
    /// The receiver's status when it is not a success, or why no request
    /// was made.
    pub async fn notify_in_process(
        &self,
        path: &str,
        notification: &serde_json::Value,
    ) -> Result<(), String> {
        if antares_api::state::in_process_depth() >= antares_api::bounds::MAX_IN_PROCESS_CALL_DEPTH
        {
            return Err("in-process call depth exceeded".into());
        }
        let body = notification.to_string();
        let req = axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri(path)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .header(axum::http::header::CONTENT_LENGTH, body.len())
            .body(Body::from(body))
            .map_err(|e| e.to_string())?;
        let answer = self
            .state
            .call_on_task(None, axum::http::HeaderMap::new(), req)
            .await;
        if answer.status().is_success() {
            Ok(())
        } else {
            Err(format!(
                "the receiver answered {}",
                answer.status().as_u16()
            ))
        }
    }
}

impl Ops {
    /// GET `path_and_query` through this broker's own router as `caller` sent it over HTTP: the
    /// answer a surface re-reads for a push channel, so a pushed message is the HTTP read byte
    /// for byte (the `SensorThings` MQTT extension, T-5911). The status and the body up to `max`
    /// bytes; an empty body past `max`.
    pub async fn get_as(
        &self,
        caller: &Caller,
        path_and_query: &str,
        max: usize,
    ) -> (u16, axum::body::Bytes) {
        let mut req = axum::http::Request::builder()
            .method(Method::GET)
            .uri(path_and_query);
        // the links an answer writes start from the Host the caller reached, as over HTTP
        if let Some(host) = caller.0.get(axum::http::header::HOST) {
            req = req.header(axum::http::header::HOST, host);
        }
        let Ok(req) = req.body(Body::empty()) else {
            return (400, axum::body::Bytes::new());
        };
        let answer = self.state.call_on_task(None, caller.0.clone(), req).await;
        let status = answer.status().as_u16();
        let body = axum::body::to_bytes(answer.into_body(), max)
            .await
            .unwrap_or_default();
        (status, body)
    }
}

/// The request [`Ops::forward_in_process`] hands the router for `req`: its
/// method, the path and query of its target after `self_url`, its headers
/// and its payload with the length the HTTP client would state, and nothing
/// else (ADR-0025 equivalence layer 1). `None` when the target does not lie
/// under `self_url` or a header is one no request can carry.
#[must_use]
pub fn in_process_request(
    req: &BuiltForward<'_>,
    self_url: &str,
) -> Option<axum::http::Request<Body>> {
    let path = req.target().and_then(|target| {
        target
            .strip_prefix(self_url)
            .filter(|rest| rest.starts_with('/'))
            .map(str::to_owned)
    })?;
    let mut inner = axum::http::Request::builder()
        .method(req.method().clone())
        .uri(path);
    for (name, value) in req.headers() {
        inner = inner.header(name.as_str(), value.as_str());
    }
    // The HTTP client states the length of what it sends.
    let body = match req.body() {
        Some(bytes) => {
            inner = inner.header(axum::http::header::CONTENT_LENGTH, bytes.len());
            Body::from(bytes.to_vec())
        }
        None => Body::empty(),
    };
    inner.body(body).ok()
}

/// The export grants of internal context sources (ADR-0025 rule 5), kept
/// under their reserved tenant, and the admin routes a module adds beside
/// `/q/views`.
impl Ops {
    /// Every stored grant document.
    ///
    /// # Errors
    /// The store's.
    pub async fn grant_docs(&self) -> Result<Vec<serde_json::Value>, String> {
        self.state.grant_docs().await
    }

    /// The stored grant document `id`, if there is one.
    ///
    /// # Errors
    /// The store's.
    pub async fn grant_doc(&self, id: &str) -> Result<Option<serde_json::Value>, String> {
        views::count_doc_reads("grant", 1);
        self.state.grant_doc(id).await
    }

    /// Store grant document `id`; `true` when it replaced one.
    ///
    /// # Errors
    /// The store's.
    pub async fn put_grant_doc(&self, id: &str, doc: serde_json::Value) -> Result<bool, String> {
        self.state.put_grant_doc(id, doc).await
    }

    /// Remove grant document `id`; `false` when there was none.
    ///
    /// # Errors
    /// The store's.
    pub async fn delete_grant_doc(&self, id: &str) -> Result<bool, String> {
        self.state.delete_grant_doc(id).await
    }

    /// Serve `routes` on the admin surface, relative to `/q` and behind its
    /// credential. Before the router is built.
    pub fn add_admin(&self, routes: Router) {
        self.state.views.add_admin(routes);
    }

    /// Hold every NGSI-LD write of a Tenant with a schema to `check`
    /// (T-5912, [`antares_api::schema_writes`]): a surface that reads
    /// schemas registers it; the first registration holds.
    pub fn check_writes(&self, check: antares_api::schema_writes::SchemaCheck) {
        self.state.views.schema_writes.set(check);
    }
}

/// A surface as the broker mounts it: closed, every route 404, while no
/// view is loaded (rule M4), and reported so in `/q/health`.
struct Mounted<S> {
    surface: S,
    /// The table of the state it was mounted on, for `version_info`.
    views: std::sync::OnceLock<std::sync::Arc<antares_api::views::ViewTable>>,
}

impl<S: Surface> antares_api::ApiSurface for Mounted<S> {
    fn name(&self) -> &str {
        self.surface.name()
    }

    fn prefix(&self) -> &str {
        self.surface.prefix()
    }

    fn router(&self, st: AppState) -> Router<AppState> {
        let table = st.views.clone();
        // The first state it is mounted on is the one /q/health reports.
        let _ = self.views.set(table.clone());
        let routes = self
            .surface
            .router(Ops::new(st, Some(Arc::from(self.surface.name()))));
        if !self.surface.needs_view() {
            return routes.with_state(());
        }
        routes
            .layer(axum::middleware::from_fn(
                move |req: axum::extract::Request, next: axum::middleware::Next| {
                    let open = table.any_loaded();
                    async move {
                        if open {
                            next.run(req).await
                        } else {
                            Refusal::Closed.ngsi_ld()
                        }
                    }
                },
            ))
            .with_state(())
    }

    fn version_info(&self) -> serde_json::Value {
        let info = self.surface.version_info();
        if !self.surface.needs_view() || self.views.get().is_some_and(|t| t.any_loaded()) {
            serde_json::json!({"state": "open", "info": info})
        } else {
            serde_json::json!({"state": "closed", "reason": "no view is loaded", "info": info})
        }
    }
}

/// The length of `v`'s compact JSON text (what `v.to_string()` holds), counted as it is written
/// so no text is built: a surface sizing each row against its answer budget serialised it once
/// to measure and again to answer (T-6612, R3-016).
#[must_use]
pub fn json_len(v: &serde_json::Value) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    // A Value always serialises; a write into the counter cannot fail.
    let _ = serde_json::to_writer(&mut count, v);
    count.0
}

/// A broker over the built-in store, for a surface's integration tests.
#[cfg(feature = "test-kit")]
pub mod test_kit {
    use super::{mount, AppState, Surface};
    use std::sync::Arc;

    /// The `/q` credential [`TestBroker::with_admin_auth`] takes, for the
    /// surface crates, which reach the API crate through this one alone.
    pub use antares_api::admin_auth::AdminAuth;

    /// Let this process forward to loopback and private addresses, as a
    /// test's own listener is one (the egress policy refuses them otherwise).
    pub fn allow_private_egress() {
        antares_jsonld::allow_private_egress(true);
    }

    /// One broker: the NGSI-LD API, the admin surface and the surfaces a
    /// test mounts, over one in-memory store.
    pub struct TestBroker {
        state: AppState,
        reload: crate::views::Reload,
    }

    impl TestBroker {
        /// A wired broker with no policy engine, its view table loaded and
        /// `/q/views` served.
        ///
        /// # Errors
        /// The view table's.
        pub async fn new() -> Result<Self, String> {
            Self::with_sinks(Vec::new(), None).await
        }

        /// [`TestBroker::new`] over `state`, a test's own store (the Postgres
        /// arm of the quality sweep's tests, T-4018).
        ///
        /// # Errors
        /// The view table's.
        pub async fn over(mut state: AppState) -> Result<Self, String> {
            antares_api::wire(&mut state).await;
            let reload = crate::views::install(&state).await?;
            Ok(Self { state, reload })
        }

        /// [`TestBroker::new`] with the platform's history retention set
        /// to `keep`, as `ANTARES_RETENTION_HISTORY_DEFAULT` sets it (DM-97).
        ///
        /// # Errors
        /// The view table's.
        pub async fn with_platform_history(keep: std::time::Duration) -> Result<Self, String> {
            let mut state = AppState::new("antares-test".into());
            antares_api::wire(&mut state).await;
            let reload = crate::views::install_with(&state, Some(keep)).await?;
            Ok(Self { state, reload })
        }

        /// [`TestBroker::new`] delivering through `sinks` too (a surface's
        /// own notification binding, ADR-0026) and asking `engine`, both
        /// set before the notifier is wired, as the broker sets them, so
        /// `pre_notify` runs on every notification.
        ///
        /// # Errors
        /// The view table's.
        pub async fn with_sinks(
            sinks: Vec<Box<dyn super::NotificationSink>>,
            engine: Option<Arc<dyn super::policy::PolicyEngine>>,
        ) -> Result<Self, String> {
            let mut state = AppState::new("antares-test".into());
            for sink in sinks {
                state = state.with_sink(sink);
            }
            if let Some(engine) = engine {
                state = state.with_policy(engine);
            }
            Self::over(state).await
        }

        /// One retention sweep at `now` (T-3900), as the minute loop runs it.
        pub async fn sweep_retention(
            &self,
            now: chrono::DateTime<chrono::Utc>,
        ) -> Vec<crate::views::retention::Deleted> {
            self.reload.sweep_retention(now).await
        }

        /// One retention sweep at `now` that starts no statement after `budget`.
        pub async fn sweep_retention_within(
            &self,
            now: chrono::DateTime<chrono::Utc>,
            budget: std::time::Duration,
        ) -> Vec<crate::views::retention::Deleted> {
            self.reload.sweep_retention_within(now, budget).await
        }

        /// One quality sweep at `now` (T-4018), as the minute loop runs it.
        pub async fn sweep_quality(
            &self,
            now: chrono::DateTime<chrono::Utc>,
        ) -> Vec<crate::views::quality::Changed> {
            self.reload.sweep_quality(now).await
        }

        /// Another pod over the same store: its own view table, loaded and
        /// reloaded from the store like this one's.
        ///
        /// # Errors
        /// The view table's.
        pub async fn sibling(&self) -> Result<Self, String> {
            let mut state = AppState::new("antares-test".into());
            state.store = self.state.store.clone();
            antares_api::wire(&mut state).await;
            let reload = crate::views::install(&state).await?;
            Ok(Self { state, reload })
        }

        /// The same broker asking `engine` (ADR-0020). Its view table was
        /// loaded before: `/q/policy` keeps answering as without an engine.
        #[must_use]
        pub fn with_policy(self, engine: Arc<dyn super::policy::PolicyEngine>) -> Self {
            Self {
                state: self.state.with_policy(engine),
                reload: self.reload,
            }
        }

        /// [`TestBroker::new`] answering at most `limit` Entities per Query
        /// Entities page (`max_limit`, 1000 otherwise), so a test pages past it
        /// without writing thousands of Entities.
        ///
        /// # Errors
        /// The view table's.
        pub async fn with_max_limit(limit: usize) -> Result<Self, String> {
            let mut state = AppState::new("antares-test".into());
            state.max_limit = limit;
            antares_api::wire(&mut state).await;
            let reload = crate::views::install(&state).await?;
            Ok(Self { state, reload })
        }

        /// [`TestBroker::new`] buffering at most `bytes` of one in-process
        /// answer ([`crate::bounds::MAX_ANSWER_BYTES`] otherwise), set before
        /// the view table is installed so `/q` previews hold it too; a test
        /// reaches the bound without writing 64 MiB.
        ///
        /// # Errors
        /// The view table's.
        pub async fn with_max_answer_bytes(bytes: usize) -> Result<Self, String> {
            let mut state = AppState::new("antares-test".into());
            state.max_answer_bytes = bytes;
            antares_api::wire(&mut state).await;
            let reload = crate::views::install(&state).await?;
            Ok(Self { state, reload })
        }

        /// [`TestBroker::new`] whose buffered answers share a pool of `bytes`
        /// (the memory budget's share otherwise, T-5895), so a test fills it
        /// without holding 64 MiB.
        ///
        /// # Errors
        /// The view table's.
        pub async fn with_answer_pool(bytes: usize) -> Result<Self, String> {
            let mut state = AppState::new("antares-test".into());
            state.answer_pool = antares_api::bounds::AnswerPool::new(bytes);
            antares_api::wire(&mut state).await;
            let reload = crate::views::install(&state).await?;
            Ok(Self { state, reload })
        }

        /// The broker's buffered-answer pool, for a test reading what it
        /// holds (T-6666).
        #[must_use]
        pub fn answer_pool(&self) -> Arc<antares_api::bounds::AnswerPool> {
            Arc::clone(&self.state.answer_pool)
        }

        /// The same broker with the forward transport `make` builds
        /// registered (ADR-0025 rule 8), as the composition root registers
        /// it.
        ///
        /// # Errors
        /// When a transport is registered already.
        pub fn with_forward_transport(
            self,
            make: impl FnOnce(super::Ops) -> Arc<dyn super::ForwardTransport>,
        ) -> Result<Self, String> {
            Ok(Self {
                state: super::with_forward_transport(self.state, make)?,
                reload: self.reload,
            })
        }

        /// Serve `routes` on the admin surface, as the composition root's
        /// [`super::Ops::add_admin`] does.
        #[must_use]
        pub fn with_admin(self, routes: axum::Router) -> Self {
            self.state.views.add_admin(routes);
            self
        }

        /// The same broker with `/q` behind the admin credential (T-3569).
        #[must_use]
        pub fn with_admin_auth(self, auth: antares_api::admin_auth::AdminAuth) -> Self {
            Self {
                state: self.state.with_admin_auth(auth),
                reload: self.reload,
            }
        }

        /// Store a view document as it is, past `/q/views`' validation (what
        /// another version or a hand edit may have left in the store), and
        /// reload this broker's table.
        ///
        /// # Errors
        /// The store's.
        pub async fn store_view_doc(&self, id: &str, doc: serde_json::Value) -> Result<(), String> {
            self.state.put_view_doc(id, doc).await?;
            self.reload.now().await
        }

        /// Rebuild this broker's table from the store now, as its interval
        /// would after another pod's write.
        ///
        /// # Errors
        /// The store's.
        pub async fn reload_now(&self) -> Result<(), String> {
            self.reload.now().await
        }

        /// The broker's router: the NGSI-LD API and `/q`.
        pub fn router(&self) -> axum::Router {
            antares_api::router(self.state.clone())
        }

        /// The NGSI-LD API narrowed by the loaded view `id`: every operation
        /// composed with it as the view root will serve it (ADR-0023).
        ///
        /// # Errors
        /// When `id` is not a loaded view.
        pub fn serve_view(&self, id: &str) -> Result<axum::Router, String> {
            match self.state.views.snapshot().get(id) {
                Some(antares_api::views::ViewEntry::Loaded { view, .. }) => {
                    Ok(antares_api::router(self.state.with_view(view.clone())))
                }
                _ => Err(format!("{id:?} is not a loaded view")),
            }
        }

        /// The broker's router with every surface `mounted` (each made by
        /// [`mount`]) beside the NGSI-LD API.
        ///
        /// # Errors
        /// The broker's refusal of a surface's prefix.
        pub fn serve_mounted(
            &self,
            mounted: Vec<Box<dyn antares_api::ApiSurface>>,
        ) -> Result<axum::Router, String> {
            let mut all: Vec<Box<dyn antares_api::ApiSurface>> = vec![Box::new(antares_api::Admin)];
            all.extend(mounted);
            Ok(antares_api::router(self.state.clone().with_surfaces(all)?))
        }

        /// The broker's router with `surface` mounted beside the NGSI-LD API.
        ///
        /// # Errors
        /// The broker's refusal of the surface's prefix.
        pub fn serve<S: Surface>(&self, surface: S) -> Result<axum::Router, String> {
            let state = self
                .state
                .clone()
                .with_surfaces(vec![Box::new(antares_api::Admin), mount(surface)])?;
            Ok(antares_api::router(state))
        }
    }

    /// [`ViewOps::scan`](super::ViewOps::scan) with `residual` under a `budget` of upstream
    /// Entities read in upstream pages of `upstream`, and how many it read: the bound a test
    /// crosses with a handful of Entities instead of [`RESIDUAL_SCAN`](super::RESIDUAL_SCAN).
    ///
    /// # Errors
    /// [`ViewOps::scan`](super::ViewOps::scan)'s.
    pub async fn scan_within(
        view: &super::ViewOps,
        caller: &super::Caller,
        pushdown: &super::Pushdown,
        residual: &super::residual::Program,
        page: super::Page,
        (budget, upstream): (usize, usize),
    ) -> Result<(super::Scanned, usize), super::Refusal> {
        view.fill(caller, pushdown, residual, page, (budget, upstream))
            .await
    }

    /// A policy [`Filter`](super::policy::Filter) conditioned on `q` (4.9),
    /// for a test engine's `AllowIf`: a surface reaches the query language
    /// through this crate alone (ADR-0027 rule M1).
    ///
    /// # Errors
    /// `q` does not parse.
    pub fn q_filter(q: &str) -> Result<super::policy::Filter, String> {
        let q = antares_ql::parse_q(q).map_err(|e| e.to_string())?;
        Ok(super::policy::Filter {
            q: Some(Box::new(q)),
            ..super::policy::Filter::default()
        })
    }
}

/// An Attribute's default instance (TS 104 175 5.2.6.4.1): the one without a `datasetId`,
/// else the first instance, as antares-feed and antares-ical read a member.
#[must_use]
pub fn default_instance<'a>(
    entity: &'a serde_json::Value,
    name: &str,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    let named = |i: &serde_json::Value| i.get("datasetId").is_some();
    match entity.get(name)? {
        serde_json::Value::Array(instances) => instances
            .iter()
            .find(|i| !named(i))
            .or_else(|| instances.first())?
            .as_object(),
        one => one.as_object(),
    }
}

#[cfg(test)]
mod tests {

    /// T-6612, R3-016: the counted length is the text's length, escapes and non-ASCII included.
    #[test]
    fn json_len_is_the_length_of_the_text() {
        for v in [
            serde_json::json!(null),
            serde_json::json!({"a": [1, 2.5, "x\"y\n"], "ä": {"b": true}}),
            serde_json::json!("€ and \u{1}"),
        ] {
            assert_eq!(super::json_len(&v), v.to_string().len(), "{v}");
        }
    }
    use super::*;

    /// F-ANT5-009: a named instance listed first is not the member's value; with only
    /// named instances the first one is.
    #[test]
    fn the_default_instance_is_the_one_without_a_dataset_id() {
        let e = serde_json::json!({"a": [{"datasetId": "urn:x", "value": 1}, {"value": 5}],
            "b": {"datasetId": "urn:x", "value": 1}, "c": {"value": 2},
            "d": [{"datasetId": "urn:x", "value": 1}], "e": 3, "g": []});
        let v = |n| {
            default_instance(&e, n)
                .and_then(|m| m.get("value"))
                .cloned()
        };
        assert_eq!(v("a"), Some(serde_json::json!(5)));
        assert_eq!(v("b"), Some(serde_json::json!(1)));
        assert_eq!(v("c"), Some(serde_json::json!(2)));
        assert_eq!(v("d"), Some(serde_json::json!(1)));
        assert_eq!(v("e"), None);
        assert_eq!(v("g"), None);
        assert_eq!(v("missing"), None);
    }

    /// T-3752: a view's short names expand as the core @context reads them.
    #[test]
    fn core_iri_expands_a_short_name_under_the_core_context() {
        assert_eq!(
            core_iri("location"),
            "https://uri.etsi.org/ngsi-ld/location"
        );
        assert_eq!(
            core_iri("BikeHireDockingStation"),
            "https://uri.etsi.org/ngsi-ld/default-context/BikeHireDockingStation"
        );
        assert_eq!(core_iri("https://sdm.org/Car"), "https://sdm.org/Car");
    }

    #[test]
    fn core_terms_are_the_core_contexts_own() {
        for term in ["id", "type", "value", "location", "observedAt"] {
            assert!(is_core_term(term), "{term}");
        }
        for name in ["speed", "p0", "Location"] {
            assert!(!is_core_term(name), "{name}");
        }
        assert_eq!(
            core_name("https://uri.etsi.org/ngsi-ld/location"),
            "location"
        );
    }

    /// Only the NGSI-LD API's own 504 `LdContextNotAvailable` reaches a
    /// surface as such; a forward's 504, another status or a body that is
    /// no problem stays an inner failure (T-3947).
    #[test]
    fn only_the_apis_ld_context_504_is_passed_on() {
        let ld = r#"{"type":"https://uri.etsi.org/ngsi-ld/errors/LdContextNotAvailable","title":"LdContextNotAvailable","detail":"http://10.0.0.1/x"}"#;
        let forward = r#"{"type":"https://uri.etsi.org/ngsi-ld/errors/InternalError","detail":"https://uri.etsi.org/ngsi-ld/errors/LdContextNotAvailable"}"#;
        assert!(context_not_available(StatusCode::GATEWAY_TIMEOUT, ld));
        assert!(!context_not_available(StatusCode::GATEWAY_TIMEOUT, forward));
        assert!(!context_not_available(StatusCode::GATEWAY_TIMEOUT, ""));
        assert!(!context_not_available(StatusCode::GATEWAY_TIMEOUT, "[1]"));
        assert!(!context_not_available(
            StatusCode::INTERNAL_SERVER_ERROR,
            ld
        ));
    }

    /// What a surface gets for each kind of API answer: success passes, a
    /// 503 keeps its `Retry-After`, the API's 504 `LdContextNotAvailable`
    /// keeps its status, any other 5xx (a forward's 504 included) is an
    /// inner failure with an id and no inner text (T-3947).
    #[tokio::test]
    async fn every_api_answer_maps_to_what_a_surface_may_see() {
        let answer = |status: StatusCode, body: &'static str| {
            let mut r = Response::new(Body::from(body));
            *r.status_mut() = status;
            r
        };
        let op = OperationId::QueryEntities;
        let ok = answered(op, answer(StatusCode::OK, "[]")).await;
        assert!(ok.is_ok_and(|r| r.status() == StatusCode::OK));
        let mut busy = answer(StatusCode::SERVICE_UNAVAILABLE, "");
        busy.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            HeaderValue::from_static("6"),
        );
        assert_eq!(
            answered(op, busy).await.err(),
            Some(Refusal::Overloaded {
                retry_after: Some(HeaderValue::from_static("6"))
            })
        );
        // T-5616: the statement timeout keeps its fixed detail through a view
        let timeout = r#"{"type":"https://uri.etsi.org/ngsi-ld/errors/InternalError","title":"InternalError","status":500,"detail":"database statement timeout"}"#;
        assert_eq!(
            answered(op, answer(StatusCode::INTERNAL_SERVER_ERROR, timeout))
                .await
                .err(),
            Some(Refusal::StatementTimeout)
        );
        let ld = r#"{"type":"https://uri.etsi.org/ngsi-ld/errors/LdContextNotAvailable"}"#;
        assert_eq!(
            answered(op, answer(StatusCode::GATEWAY_TIMEOUT, ld))
                .await
                .err(),
            Some(Refusal::ContextNotAvailable)
        );
        for (status, body) in [
            (
                StatusCode::GATEWAY_TIMEOUT,
                r#"{"type":"https://uri.etsi.org/ngsi-ld/errors/InternalError","detail":"forward to http://10.0.0.1 timed out"}"#,
            ),
            (StatusCode::INTERNAL_SERVER_ERROR, "store said no"),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                r#"{"detail":"database statement timeout, said a forward"}"#,
            ),
        ] {
            match answered(op, answer(status, body)).await.err() {
                Some(Refusal::Inner { request_id }) => {
                    assert!(request_id.starts_with("ops-"), "{status}: {request_id}");
                }
                other => panic!("{status}: {other:?}"),
            }
        }
    }

    #[cfg(feature = "test-kit")]
    #[test]
    fn a_test_engine_condition_is_parsed_or_refused() {
        let f = test_kit::q_filter("colour==\"red\"").unwrap_or_default();
        assert!(f.q.is_some(), "{f:?}");
        assert_eq!(
            f.q.map(|q| q.to_string()).as_deref(),
            Some("colour==\"red\"")
        );
        assert!(test_kit::q_filter("colour==").is_err());
    }

    #[test]
    fn visible_names_are_the_rules_less_pick_omit_and_omit_by_type() {
        let rule = |types: &[&str], attrs: &[&str]| policy::Rule {
            types: types.iter().map(|t| (*t).to_owned()).collect(),
            attributes: attrs.iter().map(|a| (*a).to_owned()).collect(),
            ..policy::Rule::default()
        };
        let iri = |n: &str| format!("https://e.org/{n}");
        let filter = policy::Filter {
            rules: vec![
                rule(&[&iri("Car")], &[&iri("speed"), &iri("owner"), &iri("vin")]),
                rule(&[&iri("Bus")], &[&iri("speed"), &iri("vin")]),
                rule(&[&iri("Car")], &[&iri("colour")]),
            ],
            pick: vec![iri("speed"), iri("owner"), iri("colour"), iri("vin")],
            omit: vec![iri("owner")],
            omit_by_type: BTreeMap::from([(iri("Bus"), vec![iri("vin")])]),
            ..policy::Filter::default()
        };
        let v = Visible::from_filter(&filter);
        assert!(v.shows_type(&iri("Car")));
        assert!(v.shows_type(&iri("Bus")));
        assert!(!v.shows_type(&iri("Tram")));
        assert!(v.shows_attribute(&iri("Car"), &iri("speed")));
        assert!(
            v.shows_attribute(&iri("Car"), &iri("colour")),
            "rules add up"
        );
        assert!(
            !v.shows_attribute(&iri("Car"), &iri("owner")),
            "omit removes"
        );
        assert!(v.shows_attribute(&iri("Car"), &iri("vin")));
        assert!(
            !v.shows_attribute(&iri("Bus"), &iri("vin")),
            "omit_by_type removes"
        );
        assert!(!v.shows_attribute(&iri("Tram"), &iri("speed")));

        let picked = Visible::from_filter(&policy::Filter {
            pick: vec![iri("speed")],
            ..filter
        });
        assert!(
            !picked.shows_attribute(&iri("Car"), &iri("colour")),
            "pick keeps"
        );
        // A core-context term is the IRI it expands to.
        let core = Visible::from_filter(&policy::Filter {
            rules: vec![rule(&["Car"], &["location"])],
            ..policy::Filter::default()
        });
        assert!(core.shows_attribute(
            "https://uri.etsi.org/ngsi-ld/default-context/Car",
            "https://uri.etsi.org/ngsi-ld/location"
        ));
    }

    /// A `*` view (ADR-0023 rule 12) narrows by no rule: every type and
    /// Attribute shows, less `pick`, `omit` and `omit_by_type`; the
    /// default still shows nothing.
    #[test]
    fn a_filter_without_rules_shows_every_name_less_pick_and_omit() {
        let iri = |n: &str| format!("https://e.org/{n}");
        let open = Visible::from_filter(&policy::Filter::default());
        assert!(open.shows_type(&iri("Tram")));
        assert!(open.shows_attribute(&iri("Tram"), &iri("speed")));
        let narrowed = Visible::from_filter(&policy::Filter {
            pick: vec![iri("speed"), iri("vin")],
            omit: vec![iri("vin")],
            omit_by_type: BTreeMap::from([(iri("Bus"), vec![iri("speed")])]),
            ..policy::Filter::default()
        });
        assert_eq!(
            [
                narrowed.shows_attribute(&iri("Car"), &iri("speed")),
                narrowed.shows_attribute(&iri("Car"), &iri("vin")),
                narrowed.shows_attribute(&iri("Car"), &iri("colour")),
                narrowed.shows_attribute(&iri("Bus"), &iri("speed")),
            ],
            [true, false, false, false]
        );
        assert!(!Visible::default().shows_type(&iri("Car")));
    }

    /// T-3798: an engine's grant over the whole Entity (`every_attribute`,
    /// ADR-0023 rule 11 as amended by T-3637) shows every Attribute of its
    /// types, less `pick`, `omit` and `omit_by_type`, and none of a type no
    /// rule names.
    #[test]
    fn a_rule_over_the_whole_entity_shows_every_attribute_of_its_types() {
        let iri = |n: &str| format!("https://e.org/{n}");
        let whole = |types: &[&str]| policy::Rule {
            types: types.iter().map(|t| iri(t)).collect(),
            every_attribute: true,
            ..policy::Rule::default()
        };
        let open = Visible::from_filter(&policy::Filter {
            rules: vec![whole(&["Car"])],
            ..policy::Filter::default()
        });
        assert_eq!(
            [
                open.shows_type(&iri("Car")),
                open.shows_attribute(&iri("Car"), &iri("speed")),
                open.shows_type(&iri("Bus")),
                open.shows_attribute(&iri("Bus"), &iri("speed")),
            ],
            [true, true, false, false]
        );
        let narrowed = Visible::from_filter(&policy::Filter {
            rules: vec![
                whole(&["Car", "Bus"]),
                policy::Rule {
                    types: vec![iri("Tram")],
                    attributes: vec![iri("speed")],
                    ..policy::Rule::default()
                },
            ],
            pick: vec![iri("speed"), iri("vin"), iri("colour")],
            omit: vec![iri("vin")],
            omit_by_type: BTreeMap::from([(iri("Bus"), vec![iri("colour")])]),
            ..policy::Filter::default()
        });
        assert_eq!(
            [
                narrowed.shows_attribute(&iri("Car"), &iri("speed")),
                narrowed.shows_attribute(&iri("Car"), &iri("colour")),
                narrowed.shows_attribute(&iri("Car"), &iri("vin")),
                narrowed.shows_attribute(&iri("Car"), &iri("owner")),
                narrowed.shows_attribute(&iri("Bus"), &iri("colour")),
                narrowed.shows_attribute(&iri("Tram"), &iri("colour")),
                narrowed.shows_type(&iri("Truck")),
            ],
            [true, true, false, false, false, false, false]
        );
    }

    #[test]
    fn an_iri_is_named_as_an_answer_without_a_context_names_it() {
        assert_eq!(
            core_name("https://uri.etsi.org/ngsi-ld/location"),
            "location"
        );
        assert_eq!(
            core_name("https://example.org/speed"),
            "https://example.org/speed"
        );
    }

    #[test]
    fn a_request_keeps_its_own_headers_and_body_and_drops_the_outer_ones() {
        let req = OpsRequest::new(Method::POST, "/ngsi-ld/v1/entities")
            .header(
                HeaderName::from_static("ngsild-tenant"),
                HeaderValue::from_static("evil"),
            )
            .header(
                HeaderName::from_static("ngsild-snapshot"),
                HeaderValue::from_static("urn:ngsi-ld:Snapshot:x"),
            )
            .header(
                HeaderName::from_static("content-type"),
                HeaderValue::from_static("application/json"),
            )
            .body("{}");
        assert_eq!(req.headers.len(), 1);
        assert_eq!(req.headers["content-type"], "application/json");
        assert_eq!(axum::body::HttpBody::size_hint(&req.body).exact(), Some(2));
    }

    /// R3-024: a moved header map keeps what one header at a time kept,
    /// every value of a repeated header, and drops the outer ones.
    #[test]
    fn a_moved_header_map_keeps_what_one_header_at_a_time_keeps() {
        let mut all = HeaderMap::new();
        all.append("accept", HeaderValue::from_static("a"));
        all.append("ngsild-tenant", HeaderValue::from_static("evil"));
        all.append("ngsild-tenant", HeaderValue::from_static("evil2"));
        all.append("accept", HeaderValue::from_static("b"));
        all.append("x-a", HeaderValue::from_static("c"));
        let one_by_one = all
            .iter()
            .fold(OpsRequest::new(Method::GET, "/x"), |r, (n, v)| {
                r.header(n.clone(), v.clone())
            });
        let moved = OpsRequest::new(Method::GET, "/x").headers(all);
        assert_eq!(moved.headers, one_by_one.headers);
        assert_eq!(moved.headers.get_all("accept").iter().count(), 2);
        assert!(!moved.headers.contains_key("ngsild-tenant"));
    }

    /// `read_only`: `OPTIONS` names the methods, a write is the surface's refusal with `Allow`,
    /// `HEAD` keeps `GET`'s headers and drops its body, and `GET` is answered whole.
    #[tokio::test]
    async fn a_read_only_surface_answers_each_method() {
        let get = || async { Response::new(Body::from("doc")) };
        let refused = || {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
            r
        };
        let body = |r: Response| async {
            axum::body::to_bytes(r.into_body(), 64)
                .await
                .map(|b| b.to_vec())
                .unwrap_or_default()
        };

        let options = read_only(&Method::OPTIONS, refused, get()).await;
        assert_eq!(options.status(), StatusCode::NO_CONTENT);
        assert_eq!(options.headers()["allow"], READ_ONLY_METHODS);

        let post = read_only(&Method::POST, refused, get()).await;
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(post.headers()["allow"], READ_ONLY_METHODS);

        let head = read_only(&Method::HEAD, refused, get()).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(body(head).await, b"");

        let got = read_only(&Method::GET, refused, get()).await;
        assert_eq!(body(got).await, b"doc");
    }
}
