// SPDX-License-Identifier: EUPL-1.2
//! Holds every ETSI NGSI-LD test suite Subscription fixture against the suite's Building and
//! Vehicle Entities and prints which of the two each would notify for. The selector types are
//! expanded first, as a broker stores them; a `q`, `geoQ` or `watchedAttributes` applies as-is.
//!
//!     cargo run -p antares-matcher --example suite_subscriptions [ngsi-ld-test-suite]
#![allow(clippy::print_stdout)]
use antares_jsonld::{core_context, expand_entity, Context, ExpandOpts};
use ngsild_matcher::would_notify;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn entity(suite: &Path, name: &str, ctx: &Context) -> Result<Value, Box<dyn std::error::Error>> {
    let Some(Value::Object(mut doc)) = read(&suite.join("data/entities").join(name)) else {
        return Err(format!("{name} is not an Entity").into());
    };
    doc.remove("@context");
    Ok(expand_entity(&doc, ctx, ExpandOpts::default())?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let suite = std::env::args().nth(1).map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ngsi-ld-test-suite"),
        PathBuf::from,
    );
    let terms = read(&suite.join("resources/jsonld-contexts/ngsi-ld-test-suite.jsonld"))
        .ok_or("suite @context")?;
    let mut ctx = core_context();
    ctx.merge_object(
        terms["@context"]
            .as_object()
            .ok_or("the suite @context is not an object")?,
    )?;
    ctx.freeze();
    let building = entity(&suite, "building-minimal.jsonld", &ctx)?;
    let vehicle = entity(&suite, "vehicle-speed-attribute.jsonld", &ctx)?;
    let mut files: Vec<_> = std::fs::read_dir(suite.join("data/subscriptions"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    files.sort();
    for path in files {
        let Some(mut sub @ Value::Object(_)) = read(&path) else {
            continue;
        };
        if let Some(Value::Array(selectors)) = sub.get_mut("entities") {
            for sel in selectors.iter_mut().filter_map(Value::as_object_mut) {
                if let Some(t) = sel
                    .get("type")
                    .and_then(Value::as_str)
                    .map(|t| ctx.expand_key(t))
                {
                    sel.insert("type".into(), t.into());
                }
            }
        }
        println!(
            "{}: Building {}, Vehicle {}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            would_notify(&sub, &building, &ctx, &|_| None),
            would_notify(&sub, &vehicle, &ctx, &|_| None),
        );
    }
    Ok(())
}
