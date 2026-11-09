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
        None => {
            let bytes = crate::assets::Assets::get("version.json")?.data;
            let mut build: Build = serde_json::from_slice(&bytes).ok()?;
            // The embedded bundle was built with this binary: a bundle that names no commit
            // (the jc image keeps version.json commit-free, bins/jc/Dockerfile) is this one's.
            if build.commit.is_empty() {
                build.commit = built_commit();
            }
            return Some(build);
        }
    };
    serde_json::from_slice(&bytes).ok()
}

/// The commit this binary was built from: the `JC_PORTAL_COMMIT` build argument (the Portal's own
/// image), else `JC_COMMIT` from the environment (the jc image sets it in its last layer, so a new
/// commit recompiles nothing); empty when neither names one.
fn built_commit() -> String {
    commit_of(
        option_env!("JC_PORTAL_COMMIT"),
        std::env::var("JC_COMMIT").ok().as_deref(),
    )
}

/// The first of the two that is a commit id (1 to 64 hex digits); anything else is not shown.
fn commit_of(compiled: Option<&str>, runtime: Option<&str>) -> String {
    [compiled, runtime]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|c| !c.is_empty() && c.len() <= 64 && c.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_default()
        .to_owned()
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
        commit: built_commit(),
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

#[cfg(test)]
mod tests {
    use super::commit_of;

    #[test]
    fn the_build_argument_wins_and_the_environment_fills_in() {
        let sha = "2d9a2259aa53b0f6c8b5d4e7f1a2b3c4d5e6f7a8";
        assert_eq!(commit_of(Some("abc123"), Some(sha)), "abc123");
        assert_eq!(commit_of(None, Some(sha)), sha);
        assert_eq!(commit_of(Some(" "), Some(sha)), sha);
        assert_eq!(commit_of(None, None), "");
    }

    #[test]
    fn a_value_that_is_no_commit_is_not_shown() {
        assert_eq!(commit_of(None, Some("<script>")), "");
        assert_eq!(commit_of(None, Some(&"a".repeat(65))), "");
        assert_eq!(commit_of(Some("main"), Some("abc")), "abc");
    }
}
