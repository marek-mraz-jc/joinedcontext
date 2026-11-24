//! Input: an operation call as the API, MCP and jcctl hand it over: the operation name (first
//! line) and its JSON input (the rest), run by the catalogue's admin on a memory store. Every
//! call answers a value or an error, never a panic.
#![no_main]

use std::sync::Arc;

use dataroom_core::{
    execute_operation, Action, Catalogue, CatalogueId, GrantRule, OpContext, OrgId,
    PolicyAuthorizer, RequestCtx, Store, Subject,
};
use dataroom_store_memory::MemoryStore;
use libfuzzer_sys::fuzz_target;
use serde_json::{json, Value};
use uuid::Uuid;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let (op, raw) = text.split_once('\n').unwrap_or((text, "{}"));
    let Ok(input) = serde_json::from_str::<Value>(raw) else {
        return;
    };
    let org = OrgId::from_uuid(Uuid::from_u128(1));
    let cat = CatalogueId::from_uuid(Uuid::from_u128(10));
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let store = Arc::new(MemoryStore::new());
        let catalogue: Catalogue = serde_json::from_value(json!({
            "orgId": org, "id": cat, "name": "city", "host": "data.city.example",
            "title": {"sk": "Mesto"}, "description": {}, "defaultLanguage": "sk",
            "languages": ["sk"], "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
            "modified": "2026-11-01T10:00:00Z"
        }))
        .expect("catalogue");
        store.upsert_catalogue(catalogue).await.expect("catalogue");
        let ops = OpContext {
            store,
            authorizer: Arc::new(PolicyAuthorizer::new(vec![GrantRule {
                org_id: org,
                catalogue_id: Some(cat),
                principal: "admin".into(),
                actions: vec![Action::Admin],
                publisher_id: None,
            }])),
            validator: None,
            harvester: None,
            sources_by_portal: false,
            branding: None,
        };
        let ctx = RequestCtx::new(
            org,
            cat,
            Subject::authenticated("admin", vec![org], Vec::new()),
        );
        let _ = execute_operation(&ops, &ctx, op.trim(), input).await;
    });
});
