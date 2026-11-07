// SPDX-License-Identifier: EUPL-1.2
//! `Caller::subject`: the policy subject headers a caller sent, in the
//! order `ANTARES_POLICY_SUBJECT_HEADERS` names them, each with every
//! value; no other header. A binary of its own: the list is read once per
//! process.

use antares_ops::Caller;
use axum::extract::FromRequestParts as _;

#[tokio::test]
async fn a_caller_names_its_subject_headers_and_nothing_else() -> Result<(), String> {
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "X-User, x-group");
    let (mut parts, ()) = axum::http::Request::builder()
        .header("x-group", "g")
        .header("x-user", "u")
        .header("x-user", "v")
        .header("authorization", "Bearer token")
        .body(())
        .map_err(|e| e.to_string())?
        .into_parts();
    let Ok(caller) = Caller::from_request_parts(&mut parts, &()).await;
    assert_eq!(
        caller.subject(),
        vec![
            ("x-user".to_owned(), b"u".to_vec()),
            ("x-user".to_owned(), b"v".to_vec()),
            ("x-group".to_owned(), b"g".to_vec()),
        ]
    );
    Ok(())
}
