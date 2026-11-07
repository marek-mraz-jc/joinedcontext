//! Who writes an Endpoint's `spec.slug` (EP-02, EP-75): the platform mints it, nobody types it.
//!
//! Every door that takes an Endpoint manifest from a caller runs [`settle`] on it before the
//! manifest is checked: a manifest without a slug gets the stored one (an update) or a fresh one
//! (a create), and a slug the caller wrote is refused unless it is the stored one. The Portal's
//! own endpoint form is the one writer allowed to bring a slug to a create: it mints it in the
//! browser with the platform CSPRNG and shows the address before the change is proposed.

use serde_json::Value;

use crate::state::AppState;

/// The JSON pointer a refusal names.
pub const PATH: &str = "/spec/slug";

/// Fills in or refuses `spec.slug` of an Endpoint manifest; any other kind passes untouched.
/// `live` is the slug the Endpoint of the same project and name holds now, `drafted` the one
/// its shared draft holds; `typed_allowed` is true only for a person's Portal session, whose
/// form mints the slug itself.
pub fn settle(
    live: Option<String>,
    drafted: Option<String>,
    typed_allowed: bool,
    manifest: &mut Value,
) -> Result<(), String> {
    if manifest.get("kind").and_then(Value::as_str) != Some("Endpoint") {
        return Ok(());
    }
    // A spec that is no object is the manifest parse's to refuse, with its own message.
    let Some(spec) = manifest.get_mut("spec").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    match spec.get("slug").and_then(Value::as_str) {
        Some(given) => match live {
            Some(live) if live == given => Ok(()),
            Some(_) => Err(
                "an Endpoint keeps the slug the platform minted for it: leave spec.slug out and \
                 the change keeps the stored one"
                    .into(),
            ),
            None if typed_allowed || drafted.as_deref() == Some(given) => Ok(()),
            None => Err(
                "the platform mints a new Endpoint's slug (EP-75): leave spec.slug out and the \
                 answer shows the one minted"
                    .into(),
            ),
        },
        None => {
            let slug = live.or(drafted).unwrap_or_else(crate::agents::share::slug);
            spec.insert("slug".into(), Value::String(slug));
            Ok(())
        }
    }
}

/// [`settle`] with the slug this instance holds for the Endpoint: the one live in project
/// `home`, else the one in its shared draft, which holds a slug minted by an earlier [`settle`]
/// (a checked manifest records its verdict there) or by the Portal form. The draft's slug is
/// what lets a dry run's minted slug reach the proposal of the same manifest.
pub async fn settle_in(
    state: &AppState,
    home: &str,
    workspace: Option<&str>,
    typed_allowed: bool,
    manifest: &mut Value,
) -> Result<(), String> {
    if manifest.get("kind").and_then(Value::as_str) != Some("Endpoint") {
        return Ok(());
    }
    let slug_of = |m: &Value| m.pointer(PATH).and_then(Value::as_str).map(str::to_owned);
    let name = manifest
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let live = state.mirror.get(home, "Endpoint", &name).and_then(|env| {
        env.spec
            .get("slug")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let mut drafted = None;
    if live.is_none() && !name.is_empty() {
        drafted = state
            .drafts
            .get_in(workspace, home, "Endpoint", &name)
            .await
            .ok()
            .flatten()
            .and_then(|draft| slug_of(&draft.manifest));
    }
    settle(live, drafted, typed_allowed, manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn endpoint(slug: Option<&str>) -> Value {
        let mut m = json!({
            "kind": "Endpoint",
            "metadata": { "name": "bikes", "namespace": "p" },
            "spec": { "contextSpaceRef": "air", "audience": "public" }
        });
        if let Some(slug) = slug {
            m["spec"]["slug"] = json!(slug);
        }
        m
    }

    const TYPED: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
    const STORED: &str = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

    #[test]
    fn a_create_without_a_slug_is_minted_26_base32_characters() {
        let mut m = endpoint(None);
        settle(None, None, false, &mut m).expect("minted");
        let slug = m["spec"]["slug"].as_str().expect("a slug");
        assert_eq!(slug.len(), 26);
        assert!(slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)));
        let mut other = endpoint(None);
        settle(None, None, false, &mut other).expect("minted");
        assert_ne!(other["spec"]["slug"], m["spec"]["slug"]);
    }

    #[test]
    fn a_create_with_a_typed_slug_is_refused_outside_the_portal_form() {
        let mut m = endpoint(Some(TYPED));
        let why = settle(None, None, false, &mut m).expect_err("refused");
        assert!(why.contains("leave spec.slug out"), "{why}");
        assert!(!why.contains(TYPED), "the refusal repeats no slug: {why}");
        let mut form = endpoint(Some(STORED));
        settle(None, None, true, &mut form).expect("the form's own minted slug");
        assert_eq!(form["spec"]["slug"], STORED);
    }

    #[test]
    fn an_update_keeps_the_stored_slug_and_refuses_another() {
        let mut m = endpoint(None);
        settle(Some(STORED.into()), None, false, &mut m).expect("kept");
        assert_eq!(m["spec"]["slug"], STORED);
        let mut same = endpoint(Some(STORED));
        settle(Some(STORED.into()), None, false, &mut same).expect("the stored slug");
        for typed_allowed in [false, true] {
            let mut moved = endpoint(Some(TYPED));
            settle(Some(STORED.into()), None, typed_allowed, &mut moved).expect_err("refused");
        }
    }

    #[test]
    fn a_create_takes_the_slug_its_draft_holds() {
        let mut m = endpoint(None);
        settle(None, Some(STORED.into()), false, &mut m).expect("filled from the draft");
        assert_eq!(m["spec"]["slug"], STORED);
        let mut resent = endpoint(Some(STORED));
        settle(None, Some(STORED.into()), false, &mut resent).expect("the drafted slug");
        let mut typed = endpoint(Some(TYPED));
        settle(None, Some(STORED.into()), false, &mut typed).expect_err("not the drafted one");
    }

    #[test]
    fn another_kind_passes_untouched() {
        let mut m = json!({ "kind": "Pipeline", "spec": { "slug": TYPED } });
        let before = m.clone();
        settle(None, None, false, &mut m).expect("not an Endpoint");
        assert_eq!(m, before);
    }

    /// The slug an Endpoint holds comes from the mirror for an update and from its draft for a
    /// create a check minted; nothing else a caller writes is taken (T-6285).
    #[tokio::test]
    async fn the_live_slug_then_the_drafted_one_then_a_fresh_one() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let live: crate::resource::ResourceEnvelope = serde_json::from_value(json!({
            "apiVersion": crate::resource::API_VERSION,
            "kind": "Endpoint",
            "metadata": { "name": "bikes", "namespace": "p" },
            "spec": { "contextSpaceRef": "air", "slug": STORED }
        }))
        .expect("an envelope");
        state.mirror.upsert(live);
        let mut update = endpoint(None);
        settle_in(&state, "p", None, false, &mut update)
            .await
            .expect("kept");
        assert_eq!(update["spec"]["slug"], STORED);
        let mut moved = endpoint(Some(TYPED));
        settle_in(&state, "p", None, true, &mut moved)
            .await
            .expect_err("refused");

        let mut first = endpoint(None);
        settle_in(&state, "q", None, false, &mut first)
            .await
            .expect("minted");
        let minted = first["spec"]["slug"].as_str().expect("a slug").to_owned();
        state
            .drafts
            .put("q", "Endpoint", "bikes", first, None, "jana", "mcp")
            .await
            .expect("drafted");
        let mut again = endpoint(None);
        settle_in(&state, "q", None, false, &mut again)
            .await
            .expect("from the draft");
        assert_eq!(again["spec"]["slug"], minted.as_str());
        let mut typed = endpoint(Some(TYPED));
        settle_in(&state, "q", None, false, &mut typed)
            .await
            .expect_err("typed");
    }
}
