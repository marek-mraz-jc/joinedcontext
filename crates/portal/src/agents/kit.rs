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
}
