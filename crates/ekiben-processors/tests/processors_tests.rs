//! Comprehensive test suite for `ekiben-processors`:
//! - `mapping`, `bloblang`, `mutation` (including PART B trap: `throw` keeps message content and sets error flag)
//! - `branch` (request_map, processors, result_map, conditional `deleted()`, and PART B trap: `catch` after `branch` swallows an earlier throw unless guarded by `try`)
//! - `workflow` (automatic DAG resolution from `request_map`/`result_map` targets, cycle detection, `meta_path`)
//! - `switch` (conditional routing, `fallthrough`, `SortGroup` reordering)
//! - `try`, `catch`, `for_each`, `while` (PART B trap: judging a batch by its first message)
//! - `archive` & `unarchive` (`json_array`, `json_documents`, `lines`, `tar`, `zip`, `csv`, `csv:;`, zip-bomb protection)
//! - `split`, `xml` (XXE DTD refusal + `cast`), `sleep`, `log`, `rate_limit` (`local`), `cache` (`memory`), `dedupe`, `group_by_value`
//! - Multi-organization resource isolation (Org A cannot read or reference Org B's cache, rate_limit, or processor resource).

use ekiben_core::Cache;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use ekiben_bloblang::Environment;
use ekiben_core::{
    execute_all, ComponentConfig, OrgId, Part, ProcContext, Registry, ResourceManager,
    StreamMetrics,
};
use tokio::sync::watch;

fn make_ctx(org_id: &str) -> (Arc<Registry>, ProcContext) {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
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
    yaml: &str,
    reg: &Registry,
    ctx: &ProcContext,
) -> Vec<Arc<dyn ekiben_core::Processor>> {
    let val: serde_json::Value = serde_norway::from_str(yaml).unwrap();
    let arr = val.as_array().unwrap();
    let confs: Vec<ComponentConfig> = arr
        .iter()
        .map(|v| ComponentConfig::from_json_value(v, "noop", None).unwrap())
        .collect();
    reg.build_processor_list(&confs, ctx).unwrap()
}

#[tokio::test]
async fn trap_throw_keeps_message_content_and_sets_detailed_error() {
    // Grounded in PART B trap [T-5288]: a `throw` in mapping keeps the message's original content
    // and sets the error flag with source label/type/path.
    let (reg, ctx) = make_ctx("org1");
    let procs = parse_procs(
        r#"
- label: validate_step
  mapping: |
    root = if this.bad { throw("bad record") } else { this }
- label: inspect_error
  mapping: |
    root = this
    root.was_errored = errored()
    root.err_msg = error()
    root.err_label = error_source_label()
    root.err_type = error_source_type()
"#,
        &reg,
        &ctx,
    );

    let batch = vec![
        Part::from_str(r#"{"id":1,"bad":true}"#),
        Part::from_str(r#"{"id":2,"bad":false}"#),
    ];
    let res = execute_all(&ctx, &procs, vec![batch]).await.unwrap();
    assert_eq!(res.len(), 1);
    let out0 = res[0][0].as_structured_ref().unwrap().to_json_value();
    // Original content ("id": 1, "bad": true) was kept when throw("bad record") fired!
    assert_eq!(out0["id"], 1);
    assert_eq!(out0["bad"], true);
    assert_eq!(out0["was_errored"], true);
    assert!(out0["err_msg"].as_str().unwrap().contains("bad record"));
    assert_eq!(out0["err_label"], "validate_step");
    assert_eq!(out0["err_type"], "mapping");
}

#[tokio::test]
async fn trap_catch_after_branch_swallows_earlier_throw_vs_try() {
    // Grounded in PART B trap:
    // "Bento runs later processors on an errored message (a `catch` after a branch swallows an earlier throw: use `try:`)"
    let (reg, ctx) = make_ctx("org1");
    let procs_without_try = parse_procs(
        r#"
- mapping: 'root = throw("step 1 failed")'
- branch:
    request_map: 'root.q = this.id'
    processors:
      - mapping: 'root.ans = this.q * 10'
    result_map: 'root.enriched = this.ans'
- catch:
    - mapping: 'root.recovered = true'
"#,
        &reg,
        &ctx,
    );

    let batch = vec![Part::from_str(r#"{"id":5}"#)];
    let res = execute_all(&ctx, &procs_without_try, vec![batch.clone()])
        .await
        .unwrap();
    // Observed in Bento 1.22 (the pinned image): the branch ran on the errored message, `catch`
    // cleared the error, and its `mapping` started from an empty document.
    assert!(!res[0][0].is_errored());
    let doc = res[0][0].as_structured_ref().unwrap().to_json_value();
    assert_eq!(doc, serde_json::json!({"recovered": true}));

    // Same, with a `mutation` in the catch: Bento 1.22 prints
    // {"enriched":50,"err_after_catch":null,"id":5,"was_err":true}
    let procs_mutation = parse_procs(
        r#"
- mapping: 'root = throw("x")'
- branch:
    request_map: 'root.q = this.id'
    processors:
      - mapping: 'root.ans = this.q * 10'
    result_map: 'root.enriched = this.ans'
- catch:
    - mutation: 'root.was_err = true'
- mutation: 'root.err_after_catch = error()'
"#,
        &reg,
        &ctx,
    );
    let res_m = execute_all(&ctx, &procs_mutation, vec![batch.clone()])
        .await
        .unwrap();
    let doc_m = res_m[0][0].as_structured_ref().unwrap().to_json_value();
    assert_eq!(
        doc_m,
        serde_json::json!({"enriched": 50, "err_after_catch": null, "id": 5, "was_err": true})
    );

    // With `try`, the branch is skipped when step 1 throws!
    let procs_with_try = parse_procs(
        r#"
- try:
    - mapping: 'root = throw("step 1 failed")'
    - branch:
        request_map: 'root.q = this.id'
        processors:
          - mapping: 'root.ans = this.q * 10'
        result_map: 'root.enriched = this.ans'
"#,
        &reg,
        &ctx,
    );
    let res2 = execute_all(&ctx, &procs_with_try, vec![batch])
        .await
        .unwrap();
    assert!(res2[0][0].is_errored());
    let doc2 = res2[0][0].as_structured_ref().unwrap().to_json_value();
    assert!(doc2.get("enriched").is_none());
}

#[tokio::test]
async fn branch_conditional_deleted_and_alignment() {
    // Grounded in internal/impl/pure/processor_branch.go
    let (reg, ctx) = make_ctx("org1");
    let procs = parse_procs(
        r#"
- branch:
    request_map: |
      root = if this.skip { deleted() } else { {"v": this.val} }
    processors:
      - mapping: 'root.doubled = this.v * 2'
    result_map: 'root.out = this.doubled'
"#,
        &reg,
        &ctx,
    );

    let batch = vec![
        Part::from_str(r#"{"id":1,"skip":false,"val":10}"#),
        Part::from_str(r#"{"id":2,"skip":true,"val":20}"#),
        Part::from_str(r#"{"id":3,"skip":false,"val":30}"#),
    ];
    let res = execute_all(&ctx, &procs, vec![batch]).await.unwrap();
    let d0 = res[0][0].as_structured_ref().unwrap().to_json_value();
    let d1 = res[0][1].as_structured_ref().unwrap().to_json_value();
    let d2 = res[0][2].as_structured_ref().unwrap().to_json_value();
    assert_eq!(d0["out"], 20);
    assert!(d1.get("out").is_none());
    assert_eq!(d2["out"], 60);
}

#[tokio::test]
async fn workflow_automatic_dag_and_cycle_detection() {
    // Grounded in internal/impl/pure/processor_workflow.go & processor_workflow_branch_map.go
    let (reg, ctx) = make_ctx("org1");
    let procs = parse_procs(
        r#"
- workflow:
    meta_path: meta.workflow
    branches:
      fetch_a:
        request_map: 'root = this.input_id'
        processors:
          - mapping: 'root = {"a_val": content().string() + "_A"}'
        result_map: 'root.a = this.a_val'
      fetch_b:
        request_map: 'root = if this.skip_b.or(false) { deleted() } else { this.input_id }'
        processors:
          - mapping: 'root = {"b_val": content().string() + "_B"}'
        result_map: 'root.b = this.b_val'
      combine_c:
        request_map: |
          root.from_a = this.a
          root.from_b = this.b.or("none")
        processors:
          - mapping: 'root.combined = this.from_a + "+" + this.from_b'
        result_map: 'root.c = this.combined'
"#,
        &reg,
        &ctx,
    );

    let batch = vec![Part::from_str(r#"{"input_id":"item1","skip_b":true}"#)];
    let res = execute_all(&ctx, &procs, vec![batch]).await.unwrap();
    let doc = res[0][0].as_structured_ref().unwrap().to_json_value();
    assert_eq!(doc["a"], "item1_A");
    assert_eq!(doc["c"], "item1_A+none");
    assert_eq!(
        doc["meta"]["workflow"]["succeeded"],
        serde_json::json!(["combine_c", "fetch_a"])
    );
    assert_eq!(
        doc["meta"]["workflow"]["skipped"],
        serde_json::json!(["fetch_b"])
    );

    // Circular dependency between two branches must be refused at construction time!
    let cycle_val: serde_json::Value = serde_norway::from_str(
        r#"
workflow:
  branches:
    x:
      request_map: 'root = this.y_out'
      processors:
        - noop: {}
      result_map: 'root.x_out = this'
    y:
      request_map: 'root = this.x_out'
      processors:
        - noop: {}
      result_map: 'root.y_out = this'
"#,
    )
    .unwrap();
    let cycle_conf = ComponentConfig::from_json_value(&cycle_val, "noop", None).unwrap();
    let err = reg
        .build_processor(&cycle_conf, &ctx, "0")
        .err()
        .expect("circular workflow must fail");
    assert!(err.to_string().contains("circular dependencies"));
}

#[tokio::test]
async fn switch_fallthrough_and_order_preservation_and_while_first_msg() {
    // Grounded in internal/impl/pure/processor_switch.go and processor_while.go
    let (reg, ctx) = make_ctx("org1");
    let procs = parse_procs(
        r#"
- switch:
    - check: 'this.n == 2'
      fallthrough: true
      processors:
        - mutation: 'root.tags = ["two"]'
    - check: 'this.n >= 2'
      processors:
        - mutation: 'root.tags = this.tags.or([]).append("gte_two")'
    - processors:
        - mutation: 'root.tags = ["other"]'
"#,
        &reg,
        &ctx,
    );

    let batch = vec![
        Part::from_str(r#"{"n":1}"#),
        Part::from_str(r#"{"n":2}"#),
        Part::from_str(r#"{"n":3}"#),
    ];
    let res = execute_all(&ctx, &procs, vec![batch]).await.unwrap();
    assert_eq!(res[0].len(), 3);
    assert_eq!(
        res[0][0].as_structured_ref().unwrap().to_json_value(),
        serde_json::json!({"n": 1, "tags": ["other"]})
    );
    assert_eq!(
        res[0][1].as_structured_ref().unwrap().to_json_value(),
        serde_json::json!({"n": 2, "tags": ["two", "gte_two"]})
    );
    assert_eq!(
        res[0][2].as_structured_ref().unwrap().to_json_value(),
        serde_json::json!({"n": 3, "tags": ["gte_two"]})
    );

    // `while` judges the batch by its first message (`batch[0]`)!
    let while_procs = parse_procs(
        r#"
- while:
    check: 'this.count < 3'
    processors:
      - mutation: 'root.count = this.count + 1'
"#,
        &reg,
        &ctx,
    );
    let w_batch = vec![
        Part::from_str(r#"{"count":0}"#),
        Part::from_str(r#"{"count":100}"#),
    ];
    let w_res = execute_all(&ctx, &while_procs, vec![w_batch])
        .await
        .unwrap();
    // Loop ran 3 times because part[0].count started at 0, even though part[1].count was 100!
    assert_eq!(
        w_res[0][0].as_structured_ref().unwrap().to_json_value()["count"],
        3
    );
    assert_eq!(
        w_res[0][1].as_structured_ref().unwrap().to_json_value()["count"],
        103
    );
}

#[tokio::test]
async fn archive_unarchive_all_formats_and_split() {
    // Grounded in internal/impl/pure/processor_archive.go, processor_unarchive.go, processor_split.go
    let (reg, ctx) = make_ctx("org1");

    for fmt in ["json_array", "lines", "binary", "tar", "zip"] {
        let yaml = format!(
            r#"
- archive:
    format: {fmt}
    path: 'item_${{! batch_index() }}.json'
- unarchive:
    format: {fmt}
"#
        );
        let procs = parse_procs(&yaml, &reg, &ctx);
        let batch = vec![Part::from_str(r#"{"a":1}"#), Part::from_str(r#"{"a":2}"#)];
        let res = execute_all(&ctx, &procs, vec![batch]).await.unwrap();
        assert_eq!(res[0].len(), 2, "format {fmt} roundtrip count");
        assert_eq!(
            res[0][0].as_structured_ref().unwrap().to_json_value(),
            serde_json::json!({"a": 1})
        );
        assert_eq!(
            res[0][1].as_structured_ref().unwrap().to_json_value(),
            serde_json::json!({"a": 2})
        );
        if fmt == "tar" || fmt == "zip" {
            assert_eq!(res[0][0].meta_get_str("archive_filename"), "item_0.json");
            assert_eq!(res[0][1].meta_get_str("archive_filename"), "item_1.json");
        }
    }

    // CSV unarchive with custom delimiter `csv:;` and split
    let csv_procs = parse_procs(
        r#"
- unarchive:
    format: "csv:;"
- split:
    size: 2
"#,
        &reg,
        &ctx,
    );
    let csv_part = Part::from_str("id;name\n1;alfa\n2;beta\n3;gamma");
    let csv_res = execute_all(&ctx, &csv_procs, vec![vec![csv_part]])
        .await
        .unwrap();
    assert_eq!(csv_res.len(), 2); // batch of 2 + batch of 1
    assert_eq!(csv_res[0].len(), 2);
    assert_eq!(csv_res[1].len(), 1);
    assert_eq!(
        csv_res[0][0].as_structured_ref().unwrap().to_json_value(),
        serde_json::json!({"id": "1", "name": "alfa"})
    );
}

#[tokio::test]
async fn xml_xxe_refused_and_cache_rate_limit_org_isolation() {
    let (reg, ctx_a) = make_ctx("org-a");
    let (_, ctx_b) = make_ctx("org-b");

    // 1. XML processor refuses XXE / DOCTYPE
    let xml_procs = parse_procs(
        r#"
- xml:
    operator: to_json
    cast: true
"#,
        &reg,
        &ctx_a,
    );
    let xxe = Part::from_str(
        r#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol">]><root>&lol;</root>"#,
    );
    let xxe_res = execute_all(&ctx_a, &xml_procs, vec![vec![xxe]])
        .await
        .unwrap();
    assert!(xxe_res[0][0].is_errored(), "XXE DOCTYPE must be refused");

    // 2. Org A registers a cache and rate_limit; Org B cannot access them!
    let mem_cache = Arc::new(ekiben_processors::resources::MemoryCache::new(None, None));
    mem_cache
        .set("secret_key", Bytes::from_static(b"org_a_secret"), None)
        .await
        .unwrap();
    ctx_a
        .resources
        .insert_cache(&OrgId::new("org-a"), "shared_name", mem_cache);

    let local_rl = Arc::new(
        ekiben_processors::resources::LocalRateLimit::new(100, 0, Duration::from_secs(1)).unwrap(),
    );
    ctx_a
        .resources
        .insert_rate_limit(&OrgId::new("org-a"), "rl_name", local_rl);

    // Org A can read its cache
    assert!(ctx_a
        .resources
        .get_cache(&OrgId::new("org-a"), "shared_name")
        .is_ok());
    // Org B cannot read Org A's cache or rate limit even using the exact same label!
    assert!(ctx_a
        .resources
        .get_cache(&OrgId::new("org-b"), "shared_name")
        .is_err());
    assert!(ctx_a
        .resources
        .get_rate_limit(&OrgId::new("org-b"), "rl_name")
        .is_err());
    assert!(ctx_b
        .resources
        .get_cache(ctx_b.org_id(), "shared_name")
        .is_err());
}

#[tokio::test]
async fn golden_part_b_seed_pipelines() {
    // Runs the sample seed pipelines from PART B against representative inputs!
    let env_vars: HashMap<String, String> = [
        ("JC_ORG_DOMAIN".to_string(), "praha.eu".to_string()),
        ("JC_SPACE".to_string(), "mesto".to_string()),
    ]
    .into_iter()
    .collect();

    let (reg, mut ctx) = make_ctx("praha");
    ctx.blobl_env = Environment::new().with_env_vars(env_vars);

    // 1. Golden test: praha-pipeline-geo-accommodation-bento.yaml (PART B)
    let praha_yaml = r#"
- try:
    - mapping: |
        root = if content().length() > 16777216 { throw("IPR answered %v bytes; more than 16 MiB is not a place layer".format(content().length())) } else { this }
    - mapping: |
        let set = "accommodation"
        let category = "accommodation"
        let item = "81c543a1199e46da87e18e12125e29a3"
        map words {
          root = if this.type() == "string" { this.trim().re_replace_all("\\s+", " ") } else { "" }
        }
        map point {
          root = if this.type() == "array" && this.length() >= 2 { this.slice(0, 2) } else { null }
        }
        let features = this.features.catch(null)
        root = if $features.type() != "array" {
          throw("IPR layer %v did not answer a feature list".format($item))
        } else if $features.length() == 0 {
          throw("IPR layer %v listed no record; the layer is never empty, so the download failed".format($item))
        } else {
          { "set": $set, "category": $category, "item": $item, "rows": $features.map_each(f -> {
            "c": f.geometry.coordinates.catch(null).apply("point"),
            "p": f.properties.catch({}),
            "key": f.properties.globalid.catch("").string().re_replace_all("[{}]", "").lowercase(),
            "name": [f.properties.nazev_zar.catch(null), f.properties.nazev.catch(null), f.properties.nazev_zast.catch(null)].map_each(n -> n.apply("words")).filter(n -> n != "").append("").index(0)
          }) }
        }
    - mapping: |
        let rows = this.rows
        let usable = $rows.filter(r -> r.key.re_match("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$") && r.name != "")
        let located = $usable.filter(r -> r.c.type() == "array" && r.c.length() == 2 && r.c.index(0).type() == "number" && r.c.index(1).type() == "number")
        let inside = $located.filter(r -> r.c.index(0) >= 14.2 && r.c.index(0) <= 14.75 && r.c.index(1) >= 49.9 && r.c.index(1) <= 50.2)
        let unique = $inside.unique(r -> r.key).unique(r -> "%v|%v|%v".format(r.name.lowercase(), (r.c.index(0) / 0.0002).floor(), (r.c.index(1) / 0.0002).floor()))
        meta read = $rows.length()
        meta unusable = $rows.length() - $usable.length()
        meta outside = $usable.length() - $inside.length()
        meta duplicate = $inside.length() - $unique.length()
        meta set = this.set
        root = if $unique.length() * 2 < $rows.length() {
          throw("IPR layer %v: only %v of %v records have a GUID, a name and a point in Prague".format(this.item, $unique.length(), $rows.length()))
        } else { this.assign({ "rows": $unique }) }
    - log:
        level: INFO
        message: 'geo-${! meta("set") }: read ${! meta("read") }, dropped ${! meta("unusable") } without GUID or name, ${! meta("outside") } without a point in Prague, ${! meta("duplicate") } duplicates'
    - mapping: |
        map words {
          root = if this.type() == "string" { this.trim().re_replace_all("\\s+", " ") } else { "" }
        }
        let set = this.set
        let category = this.category
        let source = "https://geoportalpraha.cz/data-a-sluzby/" + this.item
        root = this.rows.map_each(r -> {
          "r": r,
          "web": [r.p.web.catch(null)].filter(w -> w.type() == "string" && w.trim() != "").map_each(w -> if w.trim().re_match("^https?://") { w.trim() } else { "https://" + w.trim() }).filter(w -> w.re_match("^https?://[^\\s/]+\\.[\\x21-\\x7e]+$")),
          "kind": [r.p.typ_kkc_txt.catch(null), r.p.retezec.catch(null), r.p.typ.catch(null)].map_each(t -> t.apply("words")).filter(t -> t != "").append("").index(0),
          "street": [
            r.p.adresa_zar.catch(null).apply("words"),
            r.p.adresa.catch(null).apply("words").split(",").index(0).trim(),
            if r.p.ulice.apply("words") != "" && r.p.cpop.type() == "number" { "%v %v%v".format(r.p.ulice.apply("words"), r.p.cpop.int64(), if r.p.cor.type() == "number" { "/%v".format(r.p.cor.int64()) } else { "" }) } else { "" }
          ].filter(a -> a != "").append("").index(0),
          "season": r.p.provoz_spec.catch(null).apply("words")
        }).map_each(v -> {
          "id": "urn:ngsi-ld:%v:%v:%v:geo-%v-%v".format("PointOfInterest", env("JC_ORG_DOMAIN"), env("JC_SPACE"), $set, v.r.key),
          "type": "PointOfInterest",
          "name": { "type": "LanguageProperty", "languageMap": { "cs": v.r.name } },
          "serviceCategory": { "type": "Property", "value": $category },
          "facilityType": if v.kind != "" { { "type": "Property", "value": v.kind } } else { null },
          "address": if v.street != "" { { "type": "Property", "value": { "streetAddress": v.street, "addressLocality": "Praha", "addressCountry": "CZ" } } } else { null },
          "url": if v.web.length() > 0 { { "type": "Property", "value": v.web.index(0) } } else { null },
          "openingHours": if v.season != "" { { "type": "LanguageProperty", "languageMap": { "cs": v.season } } } else { null },
          "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": v.r.c } },
          "dataProvider": { "type": "Property", "value": "datový podklad © IPR Praha (CC BY 4.0)" },
          "source": { "type": "Property", "value": $source }
        }.filter(kv -> kv.value != null))
"#;

    let praha_procs = parse_procs(praha_yaml, &reg, &ctx);
    let praha_input = serde_json::json!({
        "type": "FeatureCollection",
        "features": [
            {
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [14.42076, 50.08804, 200.0] },
                "properties": {
                    "globalid": "{A1B2C3D4-E5F6-4789-ABCD-0123456789AB}",
                    "nazev_zar": "  Hotel   Paříž  ",
                    "ulice": "U Obecního domu",
                    "cpop": 1080,
                    "cor": 1,
                    "typ_kkc_txt": "Hotel",
                    "web": "www.hotel-pariz.cz",
                    "provoz_spec": "celoročně"
                }
            }
        ]
    });

    let praha_res = execute_all(&ctx, &praha_procs, vec![vec![Part::from_json(praha_input)]])
        .await
        .unwrap();
    assert!(!praha_res[0][0].is_errored());
    let entities = praha_res[0][0].as_structured_ref().unwrap().to_json_value();
    let entity = &entities[0];
    assert_eq!(
        entity["id"],
        "urn:ngsi-ld:PointOfInterest:praha.eu:mesto:geo-accommodation-a1b2c3d4-e5f6-4789-abcd-0123456789ab"
    );
    assert_eq!(entity["name"]["languageMap"]["cs"], "Hotel Paříž");
    assert_eq!(
        entity["address"]["value"]["streetAddress"],
        "U Obecního domu 1080/1"
    );
    assert_eq!(entity["url"]["value"], "https://www.hotel-pariz.cz");
    assert_eq!(
        entity["location"]["value"]["coordinates"],
        serde_json::json!([14.42076, 50.08804])
    );

    // 2. Golden test: helsinki-pipeline-vision-counts-bento.yaml (PART B)
    // Uses dynamic object keys `(d): ...`, `fold`, `unique`, `ts_parse`, `ts_format`!
    let helsinki_yaml = r#"
- try:
    - mapping: |
        let box = { "w": 24.76, "s": 59.91, "e": 25.29, "n": 60.31 }
        let classes = { "car": "car", "bus": "bus", "tram": "tram", "motorcycle": "motorcycle", "bicycle": "bicycle", "truck": "lorry" }
        let lanes = this.map_each(l -> l.assign({ "reason": if l.id.type() != "number" { "noId" } else if l.longitude.type() != "number" || l.latitude.type() != "number" { "noLocation" } else if !(l.longitude >= $box.w && l.longitude <= $box.e && l.latitude >= $box.s && l.latitude <= $box.n) { "outsideHelsinki" } else { "" } }))
        let kept = $lanes.filter(l -> l.reason == "")
        let streams = $kept.map_each(l -> l.counts.or({}).key_values().map_each(c -> { "lane": l.without("counts", "reason"), "class": c.key, "rows": c.value.or([]) })).flatten()
        let vehicles = $streams.filter(s -> $classes.exists(s.class))
        let hours = $vehicles.map_each(s -> s.assign({ "hours": s.rows.map_each(r -> { "at": r.time_window_start.or("").string(), "value": r.value }).map_each(r -> r.assign({ "reason": if !r.at.re_match("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:00:00\\+03:00$") { "badTime" } else if r.value.type() != "number" || r.value < 0 || r.value > 20000 || r.value.floor() != r.value { "badCount" } else { "" } })) }))
        let days = $hours.map_each(s -> s.assign({ "days": s.hours.filter(h -> h.reason == "").fold({}, t -> t.value.at.slice(0, 10).(d -> t.tally.assign({ (d): t.tally.get(d).or({}).assign({ (t.value.at.slice(11, 13)): t.value.value }) }))) }))
        let whole = $days.map_each(s -> s.days.key_values().filter(d -> d.value.keys().length() == 24).map_each(d -> { "lane": s.lane, "class": $classes.get(s.class), "day": d.key, "count": d.value.values().sum().int64() })).flatten()
        root = { "days": $whole }
    - mapping: |
        root = this.days.map_each(d -> d.day.ts_parse("2006-01-02").ts_unix().(midnight -> {
          "id": "urn:ngsi-ld:%v:%v:%v:vision-%v-%v-%v".format("TrafficFlowObserved", env("JC_ORG_DOMAIN"), env("JC_SPACE"), d.lane.id.int64(), d.class, d.day.replace_all("-", "")),
          "type": "TrafficFlowObserved",
          "intensity": { "type": "Property", "value": d.count, "observedAt": (midnight + 75600).ts_format("2006-01-02T15:04:05Z", "UTC") },
          "vehicleType": { "type": "Property", "value": d.class },
          "trafficDirection": if d.lane.name.string().lowercase().has_suffix("etelään") { { "type": "Property", "value": "south" } } else if d.lane.name.string().lowercase().has_suffix("pohjoiseen") { { "type": "Property", "value": "north" } } else { deleted() }
        }))
    - unarchive: { format: json_array }
"#;

    let helsinki_procs = parse_procs(helsinki_yaml, &reg, &ctx);
    let mut hourly_rows = Vec::new();
    for h in 0..24 {
        hourly_rows.push(serde_json::json!({
            "time_window_start": format!("2023-04-05T{h:02}:00:00+03:00"),
            "value": 10
        }));
    }
    let helsinki_input = serde_json::json!([
        {
            "id": 1,
            "name": "Kaista 1 etelään",
            "longitude": 24.93,
            "latitude": 60.19,
            "counts": {
                "truck": hourly_rows
            }
        }
    ]);
    let h_res = execute_all(
        &ctx,
        &helsinki_procs,
        vec![vec![Part::from_json(helsinki_input)]],
    )
    .await
    .unwrap();
    assert_eq!(h_res[0].len(), 1);
    let h_doc = h_res[0][0].as_structured_ref().unwrap().to_json_value();
    assert_eq!(
        h_doc["id"],
        "urn:ngsi-ld:TrafficFlowObserved:praha.eu:mesto:vision-1-lorry-20230405"
    );
    assert_eq!(h_doc["intensity"]["value"], 240);
    assert_eq!(h_doc["vehicleType"]["value"], "lorry");
    assert_eq!(h_doc["trafficDirection"]["value"], "south");
}

#[tokio::test]
async fn dedupe_lifo_keeps_the_last_message_per_key_in_batch_order() {
    // Bento 1.22 config/test/deduplicate_lifo.yaml: `strategy: LIFO` keeps the last message per key.
    let (reg, ctx) = make_ctx("org1");
    ctx.resources.insert_cache(
        &OrgId::new("org1"),
        "local",
        Arc::new(ekiben_processors::resources::MemoryCache::new(None, None)),
    );
    let procs = parse_procs(
        r#"
- dedupe:
    cache: local
    key: ${! json("k") }
    strategy: LIFO
"#,
        &reg,
        &ctx,
    );
    let batch = ["a1", "b1", "a2", "c1", "b2"]
        .iter()
        .map(|s| Part::new(format!(r#"{{"k":"{}","v":"{s}"}}"#, &s[..1])))
        .collect();
    let out = execute_all(&ctx, &procs, vec![batch]).await.unwrap();
    let got: Vec<String> = out[0]
        .iter()
        .map(|p| {
            p.as_structured_ref()
                .unwrap()
                .get_path(&["v".to_string()])
                .to_string_coerced()
        })
        .collect();
    assert_eq!(got, ["a2", "c1", "b2"]);

    let (reg, ctx) = make_ctx("org1");
    let conf = ComponentConfig::from_json_value(
        &serde_json::json!({"dedupe": {"cache": "local", "key": "x", "strategy": "random"}}),
        "noop",
        None,
    )
    .unwrap();
    assert!(reg.build_processor_list(&[conf], &ctx).is_err());
}

#[tokio::test]
async fn switch_check_reads_the_metadata_a_mapping_set() {
    // Parity (T-6623): every seed pipeline that drops by `meta drop` + `switch: check: @drop`
    // kept the message in ekiben because the check ran over empty metadata.
    let (reg, ctx) = make_ctx("org1");
    let procs = parse_procs(
        r#"
- mapping: 'meta drop = if this.n == 2 { "two" } else { "" }'
- switch:
    - check: '@drop.or("") != ""'
      processors:
        - mapping: 'root = deleted()'
"#,
        &reg,
        &ctx,
    );
    let batch = vec![Part::from_str(r#"{"n":1}"#), Part::from_str(r#"{"n":2}"#)];
    let res = execute_all(&ctx, &procs, vec![batch]).await.unwrap();
    let kept: Vec<_> = res
        .iter()
        .flatten()
        .map(|p| p.as_structured_ref().unwrap().to_json_value())
        .collect();
    assert_eq!(kept, vec![serde_json::json!({"n": 1})]);
}

#[tokio::test]
async fn unarchive_splits_as_many_members_as_the_org_may_hold() {
    let (reg, ctx) = make_ctx("org-many");
    let yaml = "- unarchive:\n    format: json_array\n";
    let doc = |n: usize| Part::from_str(&format!("[{}1]", "1,".repeat(n - 1)));
    let procs = parse_procs(yaml, &reg, &ctx);
    let err = execute_all(&ctx, &procs, vec![vec![doc(10_001)]]).await;
    assert!(
        err.map(|b| b[0][0].is_errored()).unwrap_or(true),
        "the default quota keeps the 10 000 member limit"
    );
    ctx.org.quotas.write().unwrap().max_in_flight_messages = 20_000;
    let procs = parse_procs(yaml, &reg, &ctx);
    let res = execute_all(&ctx, &procs, vec![vec![doc(15_000)]])
        .await
        .unwrap();
    assert_eq!(res[0].len(), 15_000);
}

#[test]
fn a_step_naming_a_missing_cache_refuses_the_stream_as_bento_does() {
    let (reg, ctx) = make_ctx("org-nocache");
    for yaml in [
        "- cache:\n    resource: gtfs\n    operator: set\n    key: k\n    value: v\n",
        "- dedupe:\n    cache: gtfs\n    key: k\n",
    ] {
        let val: serde_json::Value = serde_norway::from_str(yaml).unwrap();
        let confs: Vec<ComponentConfig> = val
            .as_array()
            .unwrap()
            .iter()
            .map(|v| ComponentConfig::from_json_value(v, "noop", None).unwrap())
            .collect();
        let err = reg
            .build_processor_list(&confs, &ctx)
            .err()
            .expect("refused");
        assert!(
            err.to_string()
                .contains("cache resource 'gtfs' was not found"),
            "{err}"
        );
    }
}
