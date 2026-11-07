//! Edge cases of the installation's validation mode (T-2105; PF-51, PF-57, AG-59).
//!
//! **The contract, in one sentence:** an installation is strict unless its branding file says the
//! single word `lax`, and nothing else — a near miss, a broken file, a half-written file, a query
//! string, a header — may ever relax it.
//!
//! That sentence is a security property, not a cosmetic one. `platform.validation: lax` is what
//! lets a proposal skip the fresh green verdict (PF-57) and what lets an open project merge itself
//! with nobody to approve it (`src/api/projects.rs:382`). A mode read wrongly in the relaxing
//! direction removes a gate; read wrongly in the other direction it only asks for one check more.
//! So every case below drives the input at `Validation` and asserts which way it fails.
//!
//! The mode is also a cross-language string: the JSON the public branding endpoint serves is
//! compared in the UI as `validation === "lax"` (`ui/src/components/ResourceFormDialog.tsx:266`).
//! Two of the cases pin those two words, because a rename on the Rust side would silently make
//! every browser think the installation is strict.
//!
//! Tests only (the family's rule). Every case here is green; a red one would become its own task.
//! The one thing this task did change is named in its body: `Validation::as_str` was a third
//! spelling of those two words with no caller anywhere in the crate, so it is gone and the wire
//! form serde writes is the only one left to drift from.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::branding::{Branding, Validation};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops::verdict::get_validation_mode;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use tower::ServiceExt;

/// A branding file of its own for one case, so no case can read another's leftovers.
fn branding_file(text: &[u8]) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "jc-branding-{}-{}.yaml",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::write(&path, text).expect("write the branding file");
    path
}

fn mode_of(text: &str) -> Validation {
    let path = branding_file(text.as_bytes());
    let mode = Branding::load(Some(&path.to_string_lossy())).validation;
    let _ = std::fs::remove_file(&path);
    mode
}

// -------------------------------------------------------------------------------------------------
// What relaxes an installation, and what only looks like it
// -------------------------------------------------------------------------------------------------

/// PF-57: the word, quoted or bare, with a comment or trailing spaces beside it, is the mode.
#[test]
fn the_one_word_that_relaxes_an_installation_is_lax() {
    for text in [
        "validation: lax\n",
        "validation: \"lax\"\n",
        "validation: 'lax'\n",
        "validation: lax   \n",
        "validation: lax # the owner asked for it\n",
        "instanceName: \"Helsinki\"\nvalidation: lax\n",
        // The key is camelCase like the rest of the block, and a one-word key has no other spelling.
        "validation: lax",
    ] {
        assert_eq!(
            mode_of(text),
            Validation::Lax,
            "an installation that asked to be lax was left strict by {text:?}",
        );
    }
}

/// PF-57, PF-51: every near miss leaves the gate in place. A mode is not trimmed into existence,
/// not lower-cased into existence, and not decoded into existence.
#[test]
fn nothing_but_that_word_relaxes_an_installation() {
    for text in [
        // Case: the file is written by a person, and `Lax` is what a person types.
        "validation: Lax\n",
        "validation: LAX\n",
        "validation: lAx\n",
        // One character too many, one too few, and one beside it.
        "validation: laxx\n",
        "validation: la\n",
        "validation: lax lax\n",
        "validation: \"lax \"\n",
        "validation: \" lax\"\n",
        "validation: \"lax\\n\"\n",
        "validation: \"lax\\0\"\n",
        "validation: \"lax\\r\\nvalidation: lax\"\n",
        // Percent-encoded once and twice: this is a file, not a URL, and nothing decodes it.
        "validation: \"%6cax\"\n",
        "validation: \"%256cax\"\n",
        // A Cyrillic а in the middle, which reads as `lax` and is not.
        "validation: \"l\u{0430}x\"\n",
        // The wrong JSON/YAML type: null, a number, a boolean, a list, a map.
        "validation:\n",
        "validation: ~\n",
        "validation: null\n",
        "validation: 0\n",
        "validation: 1\n",
        "validation: true\n",
        "validation: []\n",
        "validation: [lax]\n",
        "validation: {}\n",
        "validation: {lax: true}\n",
        "validation: \"\"\n",
        // The mode of some other key is not this installation's mode.
        "Validation: lax\n",
        "VALIDATION: lax\n",
        "platform:\n  validation: lax\n",
        "languages:\n  validation: lax\n",
        "instanceName: lax\n",
        // Said twice, once each way: a file nobody can read one way is not read the lax way.
        "validation: lax\nvalidation: strict\n",
        "validation: strict\nvalidation: lax\n",
        // And the word that means the gate stays.
        "validation: strict\n",
    ] {
        assert_eq!(
            mode_of(text),
            Validation::Strict,
            "{text:?} relaxed the installation's validation",
        );
    }
}

// -------------------------------------------------------------------------------------------------
// A file that cannot be read fails closed
// -------------------------------------------------------------------------------------------------

/// PF-57: unreadable, unparseable, not a block at all, not even UTF-8 — the gate stays.
#[test]
fn a_file_that_cannot_be_read_leaves_the_installation_strict() {
    // No file configured at all, and a path that is not there.
    assert_eq!(Branding::load(None).validation, Validation::Strict);
    assert_eq!(
        Branding::load(Some("/nonexistent/branding.yaml")).validation,
        Validation::Strict,
    );
    // A directory, which reads as an error rather than as text.
    assert_eq!(
        Branding::load(Some(&std::env::temp_dir().to_string_lossy())).validation,
        Validation::Strict,
    );
    // An empty path is treated as no path by the config, and as no file here either.
    assert_eq!(Branding::load(Some("")).validation, Validation::Strict);

    for text in [
        "",
        "   \n\n",
        "this is not yaml: [unclosed\n",
        "- validation: lax\n",           // a list at the root, not a block
        "\"validation: lax\"\n",         // one string at the root
        "validation: lax\n\tbad: tab\n", // a tab where YAML forbids one
    ] {
        assert_eq!(
            mode_of(text),
            Validation::Strict,
            "{text:?} was read as a branding block",
        );
    }

    // Bytes that are not UTF-8: `read_to_string` refuses them, and the warning is the only effect.
    let path = branding_file(b"validation: lax\ncity: \"\xff\xfe\"\n");
    assert_eq!(
        Branding::load(Some(&path.to_string_lossy())).validation,
        Validation::Strict,
    );
    let _ = std::fs::remove_file(&path);
}

/// PF-57: a ConfigMap is written, not patched, so a reader can see half of one. Half a block that
/// says `lax` is not a block that says `lax`.
#[test]
fn a_half_written_file_is_never_read_as_the_lax_half() {
    let whole = "instanceName: \"Helsinki\"\nvalidation: lax\ncolours:\n  primary: \"#123456\"\n";
    let path = branding_file(whole.as_bytes());
    let full = &path.to_string_lossy().into_owned();
    assert_eq!(Branding::load(Some(full)).validation, Validation::Lax);

    // Every prefix of the file is a half-written ConfigMap. A prefix that stops before the mode, or
    // in the middle of the word, or in the middle of the key after it, is strict; a prefix is only
    // ever lax when the whole word survived the cut. Anything else — a truncated `validation: la`
    // read as the relaxed mode — would be a gate removed by a race.
    for cut in 1..whole.len() {
        let prefix = &whole[..cut];
        std::fs::write(&path, prefix).expect("truncate the branding file");
        if Branding::load(Some(full)).validation == Validation::Lax {
            assert!(
                prefix.contains("validation: lax"),
                "prefix {prefix:?} was read as lax without saying so",
            );
        }
    }
    let _ = std::fs::remove_file(&path);
}

// -------------------------------------------------------------------------------------------------
// The two words the UI compares against
// -------------------------------------------------------------------------------------------------

/// PF-57, UI-30: the JSON spelling is a contract with the browser
/// (`ui/src/components/ResourceFormDialog.tsx:266` reads `validation === "lax"`, and
/// `ui/src/branding.tsx:36` falls back to `"strict"`). Renaming either word on this side would
/// make every browser read a lax installation as strict without a single test going red elsewhere.
#[test]
fn the_mode_is_written_as_the_word_the_browser_compares() {
    assert_eq!(
        serde_json::to_value(Validation::Strict).expect("serialise"),
        serde_json::json!("strict"),
    );
    assert_eq!(
        serde_json::to_value(Validation::Lax).expect("serialise"),
        serde_json::json!("lax"),
    );
    // And back, so the two directions can never drift apart.
    for (word, mode) in [("strict", Validation::Strict), ("lax", Validation::Lax)] {
        let parsed: Validation =
            serde_json::from_value(serde_json::json!(word)).expect("deserialise");
        assert_eq!(parsed, mode);
    }
}

/// PF-57: default is strict, and a block that omits the mode is a block that asked for strict.
#[test]
fn an_installation_that_says_nothing_about_validation_is_strict() {
    assert_eq!(Validation::default(), Validation::Strict);
    assert_eq!(Branding::default().validation, Validation::Strict);
    assert_eq!(
        mode_of("instanceName: \"Helsinki\"\ncity: \"Helsinki\"\n"),
        Validation::Strict
    );
    // Sanitising the block, which rewrites everything the UI must not be handed, leaves the mode.
    assert_eq!(
        Branding {
            validation: Validation::Lax,
            ..Branding::default()
        }
        .sanitised()
        .validation,
        Validation::Lax,
    );
}

// -------------------------------------------------------------------------------------------------
// The mode a request sees
// -------------------------------------------------------------------------------------------------

/// PF-57, AG-59: the file decides, and the caller never does. A query string, a header and a body
/// naming a mode are all ignored, including on the public endpoint anyone may call unauthenticated.
#[tokio::test]
async fn the_mode_comes_from_the_file_and_never_from_the_request() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    for uri in [
        "/api/v1/branding",
        "/api/v1/branding?validation=lax",
        "/api/v1/branding?validation=lax&instanceName=Evil",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("x-validation", "lax")
                    .header("x-forwarded-validation", "lax")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let served: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(served["validation"], serde_json::json!("strict"), "{uri}");
    }
}

/// PF-57: what the gate reads is what the file says, for both words, through the same call the
/// handlers make (`get_validation_mode`).
#[test]
fn the_gate_reads_the_mode_of_the_file_this_installation_was_given() {
    for (text, expected) in [
        ("validation: lax\n", Validation::Lax),
        ("validation: strict\n", Validation::Strict),
        ("instanceName: \"Helsinki\"\n", Validation::Strict),
    ] {
        let path = branding_file(text.as_bytes());
        let mut config = Config::for_tests();
        config.branding_file = Some(path.to_string_lossy().into_owned());
        let state = AppState::new(config, None);
        assert_eq!(get_validation_mode(&state), expected, "{text:?}");
        // Read twice: the file is read on every call so a new ConfigMap needs no restart, and two
        // reads of one unchanged file must not disagree.
        assert_eq!(get_validation_mode(&state), expected, "{text:?} read twice");
        let _ = std::fs::remove_file(&path);
    }
}

/// PF-57, PF-51: a branding file is public — the endpoint serves it to anyone — so a key that is
/// not part of the block is dropped rather than echoed. A ConfigMap that accidentally carries a
/// value nobody should read must not become an anonymous GET that hands it out.
#[tokio::test]
async fn the_public_answer_carries_the_block_and_nothing_else_the_file_holds() {
    let path = branding_file(
        b"instanceName: \"Helsinki\"\nvalidation: lax\nclientSecret: \"not-for-the-browser\"\n\
          keycloakAdminPassword: \"not-for-the-browser-either\"\n",
    );
    let mut config = Config::for_tests();
    config.branding_file = Some(path.to_string_lossy().into_owned());
    let app = server::app(AppState::new(config, None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/branding")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).expect("utf-8 body");
    assert!(
        text.contains("\"validation\":\"lax\""),
        "the mode the UI reads was not served: {text}",
    );
    for dropped in [
        "not-for-the-browser",
        "clientSecret",
        "keycloakAdminPassword",
    ] {
        assert!(
            !text.contains(dropped),
            "the public branding answer echoed {dropped:?}: {text}",
        );
    }
    let _ = std::fs::remove_file(&path);
}
