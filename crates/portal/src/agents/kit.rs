//! What the Portal embeds of the SDK's build (`sdk/dist`): the map's worker every code preview
//! carries inline, and the server module every `jc-functions` invocation is sent with.
//!
//! The spec.json kit this module was named for is retired (T-0681, AP-56): every run that builds
//! writes code on the App SDK.

use base64::Engine;
use rust_embed::RustEmbed;
use sha2::{Digest, Sha256};

/// `sdk/dist`, built with the Portal. Empty in a plain `cargo test`, which is why every getter
/// here answers `None` rather than failing.
#[derive(RustEmbed)]
#[folder = "sdk/dist"]
#[exclude = "runtime/*"]
#[exclude = "demos/*"]
struct Dist;

/// `kit-worker.js` as base64, for any document that carries the map's worker inline.
pub fn worker() -> Option<String> {
    let worker = Dist::get("kit-worker.js")?;
    Some(base64::engine::general_purpose::STANDARD.encode(&worker.data))
}

/// The SDK's server module for `jc-functions` (`sdk/dist/functions-server.js`), sent with every
/// invocation as `@joinedcontext/sdk/server`; `None` in a Portal built without the SDK.
pub fn functions_server() -> Option<String> {
    let module = Dist::get("functions-server.js")?;
    Some(String::from_utf8_lossy(&module.data).into_owned())
}

/// For a test that needs `file` of the SDK's build: `built` back, with a printed reason when it
/// skips. Under CI (`CI=true`, as GitHub Actions sets it) a missing build fails instead, since a
/// test that returns early there passes without testing anything (T-7826).
#[doc(hidden)]
pub fn sdk_built_or_skip(built: bool, file: &str) -> bool {
    built || skip_unless_ci(file, std::env::var("CI").as_deref() == Ok("true"))
}

fn skip_unless_ci(file: &str, ci: bool) -> bool {
    let build = "cd crates/portal/sdk && pnpm install --frozen-lockfile && pnpm build";
    assert!(
        !ci,
        "sdk/dist/{file} is missing in CI: run `{build}` before cargo test"
    );
    eprintln!("skipped: sdk/dist/{file} is not built (`{build}`)");
    false
}

/// The CSP source of one inline script: its SHA-256, so the policy needs no `'unsafe-inline'`.
pub fn script_hash(js: &str) -> String {
    format!(
        "'sha256-{}'",
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(js.as_bytes()))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each getter answers the embedded file exactly, or `None` when the build carried no SDK.
    #[test]
    fn the_getters_answer_the_embedded_files_or_none() {
        match Dist::get("kit-worker.js") {
            Some(file) => assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(worker().expect("the worker"))
                    .expect("base64"),
                file.data.as_ref()
            ),
            None => assert!(worker().is_none()),
        }
        match Dist::get("functions-server.js") {
            Some(file) => assert_eq!(
                functions_server().expect("the module").as_bytes(),
                file.data.as_ref()
            ),
            None => assert!(functions_server().is_none()),
        }
        assert_eq!(
            script_hash(""),
            "'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='"
        );
    }

    /// A missing build skips locally and fails under CI, naming the file and the build step.
    #[test]
    fn a_missing_sdk_build_skips_locally_and_fails_in_ci() {
        assert!(sdk_built_or_skip(true, "functions-server.js"));
        assert!(!skip_unless_ci("functions-server.js", false));
        let failed = std::panic::catch_unwind(|| skip_unless_ci("functions-server.js", true))
            .expect_err("a missing build fails in CI");
        let message = failed.downcast_ref::<String>().expect("a message");
        assert!(
            message.contains("sdk/dist/functions-server.js") && message.contains("pnpm build"),
            "{message}"
        );
    }
}
