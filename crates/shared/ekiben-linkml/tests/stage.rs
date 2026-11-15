//! `linkml_validate` in a runner's processor list, as the Portal renders it (G-19).

use std::sync::Arc;

use ekiben_bloblang::Environment;
use ekiben_core::{
    ComponentConfig, OrgId, Part, ProcContext, Registry, ResourceManager, StreamMetrics,
    execute_all,
};
use tokio::sync::watch;

fn make_ctx(org_id: &str) -> (Arc<Registry>, ProcContext) {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    ekiben_linkml::register(&reg);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new(org_id));
    let (_tx, rx) = watch::channel(false);
    let ctx = ProcContext {
        stream_id: "test-stream".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("test-stream", org_id),
        label: "root".to_string(),
        component_type: "root".to_string(),
        path: "pipeline.processors".to_string(),
        file_root: None,
        shutdown_rx: rx,
    };
    (reg, ctx)
}

fn parse_procs(
    json: &str,
    reg: &Registry,
    ctx: &ProcContext,
) -> Vec<Arc<dyn ekiben_core::Processor>> {
    let val: serde_json::Value = serde_json::from_str(json).unwrap();
    let confs: Vec<ComponentConfig> = val
        .as_array()
        .unwrap()
        .iter()
        .map(|v| ComponentConfig::from_json_value(v, "noop", None).unwrap())
        .collect();
    reg.build_processor_list(&confs, ctx).unwrap()
}

#[tokio::test]
async fn linkml_validate_flags_a_record_breaking_the_model_with_its_shacl_component() {
    let (reg, ctx) = make_ctx("org1");
    let model = r#"{"id": "https://e.org/", "name": "air",
      "classes": {"Station": {"slots": ["pm10", "name"]}},
      "slots": {"pm10": {"range": "float", "minimum_value": 0, "required": true},
                "name": {"range": "string"}}}"#;
    let stage = |closed: bool| {
        serde_json::json!([{ "label": "validation",
            "linkml_validate": { "model": model, "revision": "abc", "closed": closed } }])
        .to_string()
    };
    let good =
        r#"{"id":"urn:ngsi-ld:Station:1","type":"Station","pm10":{"type":"Property","value":3.5}}"#;
    let low =
        r#"{"id":"urn:ngsi-ld:Station:2","type":"Station","pm10":{"type":"Property","value":-7}}"#;
    let missing = r#"{"id":"urn:ngsi-ld:Station:3","type":"Station"}"#;
    let extra = r#"{"id":"urn:ngsi-ld:Station:4","type":"Station","pm10":{"type":"Property","value":1},"x":{"type":"Property","value":1}}"#;
    let run = |closed| {
        let procs = parse_procs(&stage(closed), &reg, &ctx);
        let batch = [good, low, missing, extra, "not json"]
            .map(Part::from_str)
            .to_vec();
        let ctx = ctx.clone();
        async move { execute_all(&ctx, &procs, vec![batch]).await.unwrap() }
    };
    let open = run(false).await;
    let flags: Vec<(bool, String)> = open[0]
        .iter()
        .map(|p| (p.is_errored(), p.meta_get_str("jc_rule")))
        .collect();
    assert_eq!(
        flags,
        [
            (false, String::new()),
            (true, "sh:MinInclusiveConstraintComponent".into()),
            (true, "sh:MinCountConstraintComponent".into()),
            (false, String::new()),
            (true, String::new()),
        ]
    );
    let message = open[0][1].error_get().unwrap().to_string();
    assert!(
        message.contains("pm10") && !message.contains("-7"),
        "{message}"
    );
    let closed = run(true).await;
    assert_eq!(
        closed[0][3].meta_get_str("jc_rule"),
        "sh:ClosedConstraintComponent"
    );
    let bad = serde_json::json!([{ "linkml_validate": { "model": "{\"imports\": []}" } }]);
    let conf = ComponentConfig::from_json_value(&bad[0], "noop", None).unwrap();
    assert!(
        reg.build_processor_list(&[conf], &ctx).is_err(),
        "a model the surfaces refuse fails the build"
    );
}
