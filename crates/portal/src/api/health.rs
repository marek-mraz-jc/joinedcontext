use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
pub struct Health {
    pub name: String,
    pub version: String,
    pub status: &'static str,
}

#[utoipa::path(
    get,
    path = "/api/v1/health",
    summary = "Liveness",
    description = "Whether this process answers at all. Public and cheap; it says nothing about the repository or the cluster.",
    tag = "system",
    responses(
        (status = 200, description = "Service is healthy", body = Health)
    )
)]
pub async fn health() -> Json<Health> {
    Json(Health {
        name: crate::APP_NAME.to_string(),
        version: crate::APP_VERSION.to_string(),
        status: "ok",
    })
}

/// Whether this replica serves the repository yet: `ready` or `loading`, nothing more (OPS-51).
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
pub struct Readiness {
    pub status: &'static str,
}

#[utoipa::path(
    get,
    path = "/api/v1/ready",
    summary = "Readiness",
    description = "Whether this replica serves the repository yet: `ready` or `loading`, nothing more (OPS-51). Answers 503 until the mirror has loaded.",
    tag = "system",
    responses(
        (status = 200, description = "The mirror holds the repository", body = Readiness),
        (status = 503, description = "The mirror has not loaded yet", body = Readiness)
    )
)]
pub async fn ready(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
) -> (axum::http::StatusCode, Json<Readiness>) {
    // Without a forge there is no repository to wait for.
    if state.syncer.as_ref().is_none_or(|syncer| syncer.is_ready()) {
        (
            axum::http::StatusCode::OK,
            Json(Readiness { status: "ready" }),
        )
    } else {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(Readiness { status: "loading" }),
        )
    }
}

/// One deploy artifact: the commit it was built from and the OpenAPI document it speaks (T-2485).
#[derive(Debug, Clone, Serialize, serde::Deserialize, ToSchema, PartialEq, Eq)]
pub struct Build {
    /// Empty when the build named none (a local `cargo run` or `pnpm build`).
    #[serde(default)]
    pub commit: String,
    /// The sha256 of the OpenAPI document, hex; empty when unknown.
    #[serde(default)]
    pub openapi: String,
}

/// Both halves of the Portal and whether the UI was built against this binary's API (T-2485).
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
pub struct Version {
    pub portal: Build,
    /// `None` when the bundle is unreachable or carries no `version.json`.
    pub ui: Option<Build>,
    /// `match`, `mismatch`, or `unknown` when the bundle names no hash.
    pub contract: &'static str,
}

/// The sha256 of the spec as `ui/openapi.json` commits it: pretty JSON and one newline.
fn openapi_sha() -> &'static str {
    static SHA: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SHA.get_or_init(|| {
        use sha2::Digest;
        let mut json = crate::openapi::document_text();
        json.push('\n');
        format!("{:x}", sha2::Sha256::digest(json.as_bytes()))
    })
}

/// `match` only when both hashes are known and equal.
pub fn contract(portal: &Build, ui: Option<&Build>) -> &'static str {
    match ui.map(|ui| ui.openapi.as_str()) {
        None | Some("") => "unknown",
        Some(sha) if sha == portal.openapi => "match",
        Some(_) => "mismatch",
    }
}

/// One client for every health read of the `portal-ui` bundle, not one per request (T-4965).
static UI_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(reqwest::Client::new);
/// A `version.json` holds a few hashes; anything longer is not one.
const VERSION_BYTES: usize = 64 * 1024;

/// The bundle's `version.json`: from `portal-ui` when configured, else the embedded build's.
async fn ui_build(ui_url: Option<&str>) -> Option<Build> {
    let bytes = match ui_url {
        Some(ui) => {
            let answer = UI_CLIENT
                .get(format!("{ui}/version.json"))
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?;
            crate::capped::bytes(answer, VERSION_BYTES).await.ok()?
        }
        None => crate::assets::Assets::get("version.json")?
            .data
            .into_owned(),
    };
    serde_json::from_slice(&bytes).ok()
}

#[utoipa::path(
    get,
    path = "/api/v1/version",
    summary = "Build of both artifacts",
    description = "The commit and OpenAPI hash of the binary and of the UI bundle it serves, and whether the bundle was built against this API (`match`, `mismatch`, `unknown`). Public (T-2485).",
    tag = "system",
    responses(
        (status = 200, description = "Both builds", body = Version)
    )
)]
pub async fn version(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
) -> Json<Version> {
    let portal = Build {
        commit: option_env!("JC_PORTAL_COMMIT")
            .unwrap_or_default()
            .trim()
            .to_owned(),
        openapi: openapi_sha().to_owned(),
    };
    let ui = ui_build(state.config.ui_url.as_deref()).await;
    let contract = contract(&portal, ui.as_ref());
    Json(Version {
        portal,
        ui,
        contract,
    })
}

pub fn router() -> axum::Router<crate::state::AppState> {
    axum::Router::new()
        .route("/health", axum::routing::get(health))
        .route("/ready", axum::routing::get(ready))
        .route("/version", axum::routing::get(version))
}
