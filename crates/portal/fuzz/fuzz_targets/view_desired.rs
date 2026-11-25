//! What a repository's manifests make of a broker view (T-7808): any Endpoint spec, Policies,
//! ModelProjection and LinkML model a push can carry compile to views and rule documents or to a
//! refusal, never a panic; a written view id stays a broker view id and a view holds at most
//! 64 rules.
#![no_main]

use joinedcontext_portal::reconciler::views::{desired, EndpointSource, ModelSource, Sources};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let read = |key: &str| input.get(key).cloned().unwrap_or(Value::Null);
    let Ok(spec) = serde_json::from_value(read("endpoint")) else {
        return;
    };
    let space = ("city".to_owned(), "air".to_owned());
    let mut sources = Sources::default();
    sources
        .spaces
        .push((space.0.clone(), space.1.clone(), "city-air".to_owned()));
    sources.endpoints.push(EndpointSource {
        project: space.0.clone(),
        name: "air-public".to_owned(),
        tenant: "city-air".to_owned(),
        spec,
    });
    let policies = read("policies")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| serde_json::from_value(p.clone()).ok())
        .enumerate()
        .map(|(n, p)| (format!("p{n}"), p))
        .collect();
    sources.policies.insert(space.clone(), policies);
    if let Ok(projection) = serde_json::from_value(read("projection")) {
        sources
            .projections
            .insert((space.0.clone(), "cars-only".to_owned()), projection);
    }
    if let Some(text) = read("model").as_str() {
        let model = ModelSource {
            model: "fuzz".to_owned(),
            source: text.to_owned(),
            imports: Default::default(),
        };
        sources.models.insert(space, Ok(model));
    }
    let wanted = desired(&sources);
    for (id, (_, view)) in &wanted.views {
        assert!(!id.is_empty() && id.len() <= 63, "view id {id:?}");
        let rules = view
            .get("rules")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        assert!(rules <= 64, "{rules} rules");
    }
});
