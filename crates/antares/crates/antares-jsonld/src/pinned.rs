// SPDX-License-Identifier: EUPL-1.2
//! The @contexts this crate carries and the resolution that needs nothing
//! else (T-3911): the pinned core contexts, compiled in, and a
//! [`ContextLoader`] a caller implements to supply any other document. No
//! network: the caching HTTP loader is `loader`, behind the `fetch` feature.

use crate::context::Context;
use antares_model::NgsiError;
use serde_json::Value;

/// Core context versions served from the build, never the network
/// (uri.etsi.org serves an HTML landing page to plain HTTP clients).
pub(crate) static PINNED: &[(&str, &str)] = &[
    (
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.3.jsonld",
        include_str!("../contexts/core-v1.6.jsonld"),
    ),
    (
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.4.jsonld",
        include_str!("../contexts/core-v1.6.jsonld"),
    ),
    (
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.5.jsonld",
        include_str!("../contexts/core-v1.6.jsonld"),
    ),
    (
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.6.jsonld",
        include_str!("../contexts/core-v1.6.jsonld"),
    ),
    (
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.7.jsonld",
        include_str!("../contexts/core-v1.7.jsonld"),
    ),
    (
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.8.jsonld",
        include_str!("../contexts/core-v1.8.jsonld"),
    ),
    (
        "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld",
        include_str!("../contexts/core-v1.9.jsonld"),
    ),
];

/// 4.4: "The NGSI-LD Core @context is publicly available at
/// `https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld` and
/// shall contain all the terms as mandated by annex B." It is what this broker
/// advertises (the Link header of a `application/json` answer, the `@context`
/// of an `ld+json` one, the context a forwarded request carries) and the
/// document merged last when a request names no @context of its own. Older
/// core versions stay in `PINNED` because a client may still reference one.
pub const CORE_CONTEXT: &str = "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld";

/// @context responses above this size are refused.
pub(crate) const MAX_CONTEXT_BYTES: usize = 5 * 1024 * 1024;

/// Fetch-count cap per @context resolution — a hostile context tree must
/// not turn one request into an unbounded crawl. Checked BEFORE each
/// fetch, so at most this many URLs are ever contacted. Public because
/// `/q/health` publishes the caps a request runs under, and a second
/// constant carrying the same number is one that can drift from the one
/// actually enforced.
pub const MAX_CONTEXT_URLS: usize = 32;

/// The pinned core `@context`, parsed and frozen, with no loader, client or
/// cache behind it: the value every `Loader` pins, and what a test of
/// expansion or matching needs on its own.
#[allow(clippy::expect_used)]
pub fn core_context() -> Context {
    let mut core = Context::default();
    // PINNED holds CORE_CONTEXT and every entry parses: pinned by
    // `every_pinned_context_parses_and_carries_an_at_context`
    merge_context_value(&mut core, &pinned(CORE_CONTEXT).expect("pinned core"));
    core.freeze();
    core.source = Value::String(CORE_CONTEXT.to_owned());
    core
}

/// The core @context URL of NGSI-LD version `major.minor` (TS 104 175
/// 13.4.3 `core`), for the versions whose core is pinned in this build;
/// `None` for anything else, a malformed version included.
pub fn core_context_url(version: &str) -> Option<&'static str> {
    let (major, minor) = version.split_once('.')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(major) || !digits(minor) {
        return None;
    }
    let want = format!("https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v{version}.jsonld");
    PINNED.iter().map(|(u, _)| *u).find(|u| *u == want)
}

/// The `@context` of a compiled-in document, or `None` for a URL that is not
/// pinned. The bodies are `include_str!`-ed at build time, so a body that
/// does not parse or carries no `@context` is a build defect rather than
/// anything a request can cause —
/// `every_pinned_context_parses_and_carries_an_at_context` fails on it.
pub(crate) fn pinned(url: &str) -> Option<Value> {
    let (_, body) = PINNED.iter().find(|(u, _)| *u == url)?;
    let doc: Value = serde_json::from_str(body).ok()?;
    doc.get("@context").cloned()
}

/// Merge a pre-parsed Core @context value (4.4: merged last, so it wins).
pub(crate) fn merge_context_value(ctx: &mut Context, v: &Value) {
    match v {
        Value::Object(o) => {
            let _ = ctx.merge_core_object(o);
        }
        Value::Array(items) => {
            for i in items {
                merge_context_value(ctx, i);
            }
        }
        _ => {}
    }
}

/// Supplies the documents a resolution names beyond the pinned core
/// contexts. `None` means this loader has no such document; the resolution
/// then answers `LdContextNotAvailable`, as the broker does for a URL it
/// cannot fetch. Synchronous: expansion never awaits, and a caller with a
/// network fetches before it resolves.
pub trait ContextLoader {
    /// The `@context` member of the document at `url` (absolute).
    fn load(&self, url: &str) -> Option<Value>;
}

/// The loader that knows the pinned core contexts and nothing else.
pub struct PinnedOnly;

impl ContextLoader for PinnedOnly {
    fn load(&self, _url: &str) -> Option<Value> {
        None
    }
}

/// Resolve a user `@context` (a URL, an inline object, or an array of
/// both) the way the broker's `Loader` does, without a network or a cache:
/// pinned core URLs first, every other URL from `loader`, the Core
/// `@context` merged last so its terms win (CIM 009 4.4). The same caps
/// apply: nesting deeper than 8 and more than [`MAX_CONTEXT_URLS`]
/// referenced URLs are `BadRequestData`.
pub fn resolve_with(loader: &dyn ContextLoader, user: &Value) -> Result<Context, NgsiError> {
    let mut ctx = Context::default();
    let mut urls = 0usize;
    merge_entry(loader, &mut ctx, user, 0, &mut urls, None)?;
    let core = pinned(CORE_CONTEXT)
        .ok_or_else(|| NgsiError::InternalError("the core @context is not pinned".into()))?;
    merge_context_value(&mut ctx, &core);
    ctx.freeze();
    ctx.source = user.clone();
    Ok(ctx)
}

fn merge_entry(
    loader: &dyn ContextLoader,
    ctx: &mut Context,
    entry: &Value,
    depth: usize,
    urls: &mut usize,
    base: Option<&str>,
) -> Result<(), NgsiError> {
    if depth > 8 {
        return Err(NgsiError::BadRequestData(
            "@context nesting too deep".into(),
        ));
    }
    match entry {
        Value::Array(items) => items
            .iter()
            .try_for_each(|i| merge_entry(loader, ctx, i, depth.saturating_add(1), urls, base)),
        Value::String(url) => {
            if *urls >= MAX_CONTEXT_URLS {
                return Err(NgsiError::BadRequestData(format!(
                    "@context resolution exceeds {MAX_CONTEXT_URLS} referenced URLs"
                )));
            }
            *urls = urls.saturating_add(1);
            let resolved = match base {
                Some(b) if !url.starts_with("http://") && !url.starts_with("https://") => {
                    join(b, url)
                }
                _ => url.clone(),
            };
            let doc = pinned(&resolved)
                .or_else(|| loader.load(&resolved))
                .ok_or_else(|| {
                    NgsiError::LdContextNotAvailable(format!(
                        "@context {resolved} is not available"
                    ))
                })?;
            merge_entry(
                loader,
                ctx,
                &doc,
                depth.saturating_add(1),
                urls,
                Some(&resolved),
            )
        }
        // 5.5.7: a document fetched from a pinned core URL is a Core
        // @context and merges under the Core rule; the rest is the user's.
        Value::Object(obj) => match base {
            Some(b) if pinned(b).is_some() => ctx.merge_core_object(obj),
            _ => ctx.merge_object(obj),
        },
        Value::Null => Ok(()),
        _ => Err(NgsiError::BadRequestData("invalid @context entry".into())),
    }
}

/// A relative @context reference against the URL of the document holding
/// it (JSON-LD 1.1 section 3.1).
// ponytail: path-relative and root-relative references only, no `..`
// removal; the fetch loader uses a full URL parser.
fn join(base: &str, url: &str) -> String {
    let after_scheme = base.find("://").map_or(0, |i| i.saturating_add(3));
    let root = base
        .get(after_scheme..)
        .and_then(|rest| rest.find('/'))
        .map_or(base.len(), |i| i.saturating_add(after_scheme));
    match url.strip_prefix('/') {
        Some(_) => format!("{}{url}", base.get(..root).unwrap_or(base)),
        None => {
            let dir = base
                .rfind('/')
                .filter(|i| *i >= root)
                .map_or(base.len(), |i| i.saturating_add(1));
            let prefix = base.get(..dir).unwrap_or(base);
            if dir == base.len() && !prefix.ends_with('/') {
                format!("{prefix}/{url}")
            } else {
                format!("{prefix}{url}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A loader that serves one document and counts every call.
    struct Counting {
        calls: Cell<usize>,
        url: &'static str,
        doc: Value,
    }

    impl ContextLoader for Counting {
        fn load(&self, url: &str) -> Option<Value> {
            self.calls.set(self.calls.get().saturating_add(1));
            (url == self.url).then(|| self.doc.clone())
        }
    }

    fn counting() -> Counting {
        Counting {
            calls: Cell::new(0),
            url: "https://example.org/ctx.jsonld",
            doc: serde_json::json!({"speed": "https://example.org/speed"}),
        }
    }

    #[test]
    fn a_document_on_the_core_context_alone_expands_without_a_loader() {
        let ctx = resolve_with(&PinnedOnly, &Value::String(CORE_CONTEXT.into())).expect("resolve");
        let entity = serde_json::json!({
            "id": "urn:ngsi-ld:Car:1", "type": "Car",
            "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [1.0, 2.0]}}
        });
        let entity = entity.as_object().cloned().expect("an object literal");
        let out =
            crate::expand_entity(&entity, &ctx, crate::ExpandOpts::default()).expect("expand");
        assert!(
            out.get("https://uri.etsi.org/ngsi-ld/location").is_some(),
            "{out}"
        );
        assert_eq!(
            ctx.expand_key("location"),
            "https://uri.etsi.org/ngsi-ld/location"
        );
    }

    #[test]
    fn an_unknown_context_is_not_available_after_one_call_and_a_pinned_one_calls_nothing() {
        let l = counting();
        let err = resolve_with(&l, &serde_json::json!("https://example.org/other.jsonld"))
            .expect_err("unknown");
        assert!(
            matches!(err, NgsiError::LdContextNotAvailable(_)),
            "{err:?}"
        );
        assert_eq!(l.calls.get(), 1);
        let l = counting();
        resolve_with(&l, &serde_json::json!([CORE_CONTEXT, {"a": "https://x/a"}])).expect("pinned");
        assert_eq!(l.calls.get(), 0, "a pinned URL never reaches the loader");
    }

    #[test]
    fn a_loaded_context_applies_and_the_core_terms_still_win() {
        let l = counting();
        let user = serde_json::json!(["https://example.org/ctx.jsonld",
            {"Property": "https://evil.example/Property"}]);
        let ctx = resolve_with(&l, &user).expect("resolve");
        assert_eq!(ctx.expand_key("speed"), "https://example.org/speed");
        assert_eq!(
            ctx.expand_key("Property"),
            "https://uri.etsi.org/ngsi-ld/Property"
        );
        assert_eq!(ctx.source, user);
    }

    #[test]
    fn the_caps_hold_without_a_network() {
        let many: Vec<Value> = (0..=MAX_CONTEXT_URLS)
            .map(|_| Value::String(CORE_CONTEXT.into()))
            .collect();
        assert!(matches!(
            resolve_with(&PinnedOnly, &Value::Array(many)),
            Err(NgsiError::BadRequestData(_))
        ));
        let mut deep = serde_json::json!({});
        for _ in 0..10 {
            deep = Value::Array(vec![deep]);
        }
        assert!(matches!(
            resolve_with(&PinnedOnly, &deep),
            Err(NgsiError::BadRequestData(_))
        ));
        assert!(matches!(
            resolve_with(&PinnedOnly, &serde_json::json!(7)),
            Err(NgsiError::BadRequestData(_))
        ));
        assert!(resolve_with(&PinnedOnly, &Value::Null).is_ok());
    }

    #[test]
    fn relative_references_join_against_their_document() {
        assert_eq!(
            join("https://h.example/a/b.jsonld", "c.jsonld"),
            "https://h.example/a/c.jsonld"
        );
        assert_eq!(
            join("https://h.example/a/b.jsonld", "/c.jsonld"),
            "https://h.example/c.jsonld"
        );
        assert_eq!(
            join("https://h.example", "c.jsonld"),
            "https://h.example/c.jsonld"
        );
        assert_eq!(
            join("https://h.example/", "c.jsonld"),
            "https://h.example/c.jsonld"
        );
    }

    /// `core_context` and the merge tail unwrap `pinned(CORE_CONTEXT)`,
    /// because a broker without the Core @context cannot expand anything and
    /// has no useful degraded mode. That is only safe while the compiled-in
    /// table really does carry a parseable `@context` for every URL in it —
    /// which is a property of the build, so it is checked here rather than
    /// left to a comment. A pinned document that stops parsing fails this
    /// test instead of the first request after deploy.
    #[test]
    fn every_pinned_context_parses_and_carries_an_at_context() {
        assert!(!PINNED.is_empty(), "the pinned table is empty");
        for (url, _) in PINNED {
            assert!(
                pinned(url).is_some(),
                "pinned document {url} does not parse, or carries no @context"
            );
        }
        let core = pinned(CORE_CONTEXT).expect("the Core @context is pinned");
        assert!(
            matches!(core, Value::Object(_) | Value::Array(_) | Value::String(_)),
            "the Core @context is not a usable @context value: {core}"
        );
        assert!(
            PINNED.iter().any(|(u, _)| *u == CORE_CONTEXT),
            "CORE_CONTEXT must be in the pinned table by URL, not merely resolvable"
        );
    }

    /// The edges of the pinned set (T-3912): which versions have a core URL, a relative
    /// reference resolved against the document that holds it, an array merged member by
    /// member, and a URL no loader serves.
    #[test]
    fn versions_relative_references_and_arrays() {
        assert_eq!(
            core_context_url("1.3"),
            Some("https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.3.jsonld")
        );
        for refused in ["1", "1.99", "x.1", ".1", "1.", "1.3a"] {
            assert_eq!(core_context_url(refused), None, "{refused}");
        }

        struct Two;
        impl ContextLoader for Two {
            fn load(&self, url: &str) -> Option<Value> {
                match url {
                    "https://example.org/a/outer.jsonld" => Some(serde_json::json!([
                        "inner.jsonld",
                        "https://example.org/abs.jsonld",
                        "http://example.org/plain.jsonld"
                    ])),
                    "https://example.org/a/inner.jsonld" => {
                        Some(serde_json::json!({"speed": "https://example.org/speed"}))
                    }
                    "https://example.org/abs.jsonld" => {
                        Some(serde_json::json!({"size": "https://example.org/size"}))
                    }
                    "http://example.org/plain.jsonld" => {
                        Some(serde_json::json!({"plain": "https://example.org/plain"}))
                    }
                    _ => None,
                }
            }
        }
        let ctx = resolve_with(
            &Two,
            &Value::String("https://example.org/a/outer.jsonld".into()),
        )
        .expect("resolve");
        assert_eq!(ctx.expand_key("speed"), "https://example.org/speed");
        assert_eq!(ctx.expand_key("size"), "https://example.org/size");
        assert_eq!(ctx.expand_key("plain"), "https://example.org/plain");
        assert!(resolve_with(&Two, &Value::String("https://example.org/b.jsonld".into())).is_err());
        let missing = resolve_with(
            &PinnedOnly,
            &Value::String("https://example.org/none.jsonld".into()),
        );
        assert!(
            matches!(missing, Err(NgsiError::LdContextNotAvailable(_))),
            "{missing:?}"
        );

        let mut ctx = Context::default();
        merge_context_value(
            &mut ctx,
            &serde_json::json!([{"speed": "https://example.org/speed"}, 5]),
        );
        assert_eq!(ctx.expand_key("speed"), "https://example.org/speed");
    }
}
