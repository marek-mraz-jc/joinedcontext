//! A caller who may not propose a kind is refused before the manifest is read (T-2576, PF-50,
//! PF-51): a viewer's create answers 403 whatever it sends, and nothing of the kind's schema
//! (which field is missing, what shape it wants) reaches a caller who may not write it.

mod common;

use axum::http::StatusCode;
use common::doors::{state, viewer, PROJECT};
use serde_json::json;

#[tokio::test]
async fn a_viewers_create_is_refused_whatever_its_spec_says() {
    let state = state();
    for kind in ["ServiceAccount", "SharedSpaceReference", "Subscription"] {
        let plural = joinedcontext_portal::resource::by_kind(kind)
            .expect("a kind the Portal writes")
            .plural;
        for dry in ["", "?dryRun=All"] {
            let answer = common::send(
                &state,
                viewer(),
                "POST",
                &format!("/api/v1/projects/{PROJECT}/{plural}{dry}"),
                Some(json!({
                    "apiVersion": "joinedcontext.com/v1alpha1",
                    "kind": kind,
                    "metadata": { "name": "t2576-viewer", "namespace": PROJECT },
                    "spec": {},
                })),
            )
            .await;
            assert_eq!(
                answer.status,
                StatusCode::FORBIDDEN,
                "{kind}{dry}: {}",
                answer.text
            );
            for schema in ["missing field", "does not parse", "not a valid"] {
                assert!(
                    !answer.text.contains(schema),
                    "{kind}{dry} told a viewer the schema: {}",
                    answer.text
                );
            }
        }
    }
}
