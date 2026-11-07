// SPDX-License-Identifier: EUPL-1.2
//! What a view costs (V23, surfaces-test-plan section 5): loading the
//! largest view `/q/views` accepts, composing a view with an engine's
//! narrowing, and the request through a view under such an engine, which
//! hands each request a narrowing of its own (T-3778).
//!
//!     cargo bench -p antares-ops --bench views

use antares_api::policy::{Filter, Rule};
use antares_api::views::{compose, with_view};
use antares_ops::views::{View, MAX_RULES};
use std::hint::black_box;
use std::io::Write as _;
use std::sync::Arc;
use std::time::Instant;

fn largest_view() -> Vec<u8> {
    let rules: Vec<serde_json::Value> = (0..MAX_RULES)
        .map(|i| {
            serde_json::json!({
                "types": ["Car"],
                "q": format!("speed>{i};status==\"moving\""),
                "attributes": ["speed", "status", "location"]
            })
        })
        .collect();
    serde_json::json!({
        "id": "bench", "tenant": "city", "selector": ["Car"], "q": "speed>0",
        "rules": rules, "operations": ["query_entities", "retrieve_entity", "temporal_query"]
    })
    .to_string()
    .into_bytes()
}

/// Runs `f` `n` times and reports nanoseconds per run.
fn time(name: &str, n: u32, mut f: impl FnMut()) -> std::io::Result<()> {
    let start = Instant::now();
    for _ in 0..n {
        f();
    }
    let per = start
        .elapsed()
        .as_nanos()
        .checked_div(u128::from(n))
        .unwrap_or(0);
    writeln!(std::io::stdout(), "{name}: {per} ns/op ({n} runs)")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let doc = largest_view();
    time("load the largest view", 200, || {
        if let Ok(v) = View::parse(black_box(&doc)) {
            black_box(v.load().is_ok());
        }
    })?;
    let loaded = View::parse(&doc)?.load()?;
    let engine = Filter {
        rules: vec![Rule {
            types: vec!["Car".into()],
            attributes: vec!["speed".into()],
            ..Rule::default()
        }],
        ..Filter::default()
    };
    time("compose a view with an engine's rules", 2000, || {
        black_box(compose(loaded.view.narrowing(), black_box(engine.clone())).is_ok());
    })?;
    // `gate` answers each request with a fresh `Filter`, equal to the last
    time(
        "query through the largest view, narrowing engine",
        2000,
        || {
            let answer = Ok(Arc::new(black_box(engine.clone())));
            black_box(with_view(Some(&loaded.view), "5.7.2", answer).is_ok());
        },
    )?;
    let small = View::parse(
        br#"{"id":"small","tenant":"city","selector":["Car"],"q":"speed>0","rules":[{"types":["Car"],"attributes":["speed","location"]}],"operations":["query_entities"]}"#,
    )?
    .load()?;
    time(
        "query through a one-type view, narrowing engine",
        20000,
        || {
            let answer = Ok(Arc::new(black_box(engine.clone())));
            black_box(with_view(Some(&small.view), "5.7.2", answer).is_ok());
        },
    )?;
    Ok(())
}
