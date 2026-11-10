//! Page boundaries (jc-ekiben-paging): where a run may cut a batch into pages, and that the
//! messages written stay those of one batch run breadth first.

use std::sync::Arc;

use ekiben_bloblang::Environment;
use ekiben_core::{
    execute_all, execute_paged, Batch, ComponentConfig, CoreError, OrgId, Paging, Part,
    ProcContext, Processor, Registry, ResourceManager, StreamMetrics,
};
use tokio::sync::watch;

fn make_ctx() -> (Arc<Registry>, ProcContext) {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("org"));
    let (_tx, rx) = watch::channel(false);
    let ctx = ProcContext {
        stream_id: "test-stream".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("test-stream", "org"),
        label: "root".to_string(),
        component_type: "root".to_string(),
        path: "pipeline.processors".to_string(),
        file_root: None,
        shutdown_rx: rx,
    };
    (reg, ctx)
}

fn confs(yaml: &str) -> Vec<ComponentConfig> {
    let val: serde_json::Value = serde_norway::from_str(yaml).unwrap();
    val.as_array()
        .unwrap()
        .iter()
        .map(|v| ComponentConfig::from_json_value(v, "noop", None).unwrap())
        .collect()
}

fn build(yaml: &str) -> (ProcContext, Vec<Arc<dyn Processor>>, Vec<ComponentConfig>) {
    let (reg, ctx) = make_ctx();
    let confs = confs(yaml);
    let procs = reg.build_processor_list(&confs, &ctx).unwrap();
    (ctx, procs, confs)
}

/// Each batch as its messages' text, errored ones marked.
fn texts(batches: &[Batch]) -> Vec<Vec<String>> {
    batches
        .iter()
        .map(|b| {
            b.iter()
                .map(|p| {
                    let text = String::from_utf8_lossy(&p.as_bytes_ref()).into_owned();
                    if p.is_errored() {
                        format!("ERR {text}")
                    } else {
                        text
                    }
                })
                .collect()
        })
        .collect()
}

async fn paged(yaml: &str, input: Batch, rows: usize) -> Vec<Batch> {
    let (ctx, procs, confs) = build(yaml);
    let paging = Paging::new(&confs, rows);
    let mut out = Vec::new();
    let out_ref = &mut out;
    execute_paged(&ctx, &procs, input, &paging, |b| {
        out_ref.push(b);
        async { Ok::<(), CoreError>(()) }
    })
    .await
    .unwrap();
    out
}

async fn whole(yaml: &str, input: Batch) -> Vec<Batch> {
    let (ctx, procs, _) = build(yaml);
    execute_all(&ctx, &procs, vec![input]).await.unwrap()
}

fn messages(batches: &[Batch]) -> Vec<String> {
    texts(batches).into_iter().flatten().collect()
}

fn array_of(n: usize) -> Part {
    let items: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"n":{i},"k":{}}}"#, i % 7))
        .collect();
    Part::from_str(&format!("[{}]", items.join(",")))
}

/// The od-* pipelines' shape: page list, fetch per page, rows, then split and archive.
const OPEN_DATA: &str = r#"
- mapping: 'root = this'
- unarchive: { format: json_array }
- switch:
    - check: 'this.n % 2 == 0'
      processors:
        - mapping: 'root = this.merge({"even": true})'
- mapping: 'root = this.merge({"row": this.n})'
- split: { size: 300, byte_size: 2097152 }
- archive: { format: json_array }
- mapping: 'root = if this.type() == "array" { this } else { [this] }'
- unarchive: { format: json_array }
- try:
    - mapping: 'root = this'
- split: { size: 1000, byte_size: 1048576 }
- archive: { format: json_array }
"#;

/// The events pipelines' shape: the whole listing joined again for one set-wide step.
const EVENTS: &str = r#"
- unarchive: { format: json_array }
- branch:
    processors:
      - mapping: 'root = this.n'
    result_map: 'root.seen = this'
- archive: { format: json_array }
- mutation: 'root = {"count": this.length()}'
"#;

#[test]
fn a_cut_comes_only_where_every_later_step_up_to_a_split_reads_one_message() {
    let cuts = Paging::new(&confs(OPEN_DATA), 1000).cuts;
    // Up to each split every step reads one message; right before an archive (after the split
    // that feeds it) no cut: the archive joins its batch.
    let (t, f) = (true, false);
    assert_eq!(cuts, vec![t, t, t, t, f, t, t, t, t, f, t]);

    let events = Paging::new(&confs(EVENTS), 1000).cuts;
    assert_eq!(
        events,
        vec![false, false, true, true],
        "the archive keeps its batch whole"
    );

    for joined in [
        "- unarchive: { format: json_array }\n- mapping: 'root = batch_index()'\n",
        "- unarchive: { format: json_array }\n- mapping: 'root = batch_size()'\n",
        "- unarchive: { format: json_array }\n- mapping: 'root = from(0)'\n",
        "- unarchive: { format: json_array }\n- log: { message: '${! batch_index() }' }\n",
        "- unarchive: { format: json_array }\n- group_by_value: { value: '${! this.k }' }\n",
        "- unarchive: { format: json_array }\n- while: { check: 'false', processors: [] }\n",
        "- unarchive: { format: json_array }\n- branch: { processors: [ { archive: { format: lines } } ] }\n",
        "- unarchive: { format: json_array }\n- switch: [ { check: 'batch_index() == 0', processors: [] } ]\n",
        "- unarchive: { format: json_array }\n- http: { url: 'http://x', batch_as_multipart: true }\n",
    ] {
        assert!(!Paging::new(&confs(joined), 10).cuts[0], "{joined}");
    }
}

#[tokio::test]
async fn pages_write_the_rows_one_batch_writes_in_their_order() {
    let input = vec![array_of(2_500)];
    let one = whole(OPEN_DATA, input.clone()).await;
    let pages = paged(OPEN_DATA, input, 1000).await;
    // A split regroups each page on its own (300, 300, 300, 100 per page of 1,000 where the
    // whole batch gave 300 x 8 + 100), so the writes group the rows differently: the rows and
    // their order are the same.
    assert_eq!(rows(&pages), rows(&one));
    assert_eq!(rows(&one).len(), 2_500);
    assert_eq!(
        pages.len(),
        10,
        "pages of 1,000, 1,000 and 500, each split by 300"
    );
    assert!(pages.iter().all(|b| b.len() == 1));
}

/// Each written message's rows: an array's members, any other message itself.
fn rows(batches: &[Batch]) -> Vec<String> {
    batches
        .iter()
        .flatten()
        .flat_map(|p| match p.as_structured_ref().unwrap() {
            ekiben_bloblang::Value::Array(a) => {
                a.iter().map(|v| v.to_json_value().to_string()).collect()
            }
            v => vec![v.to_json_value().to_string()],
        })
        .collect()
}

#[tokio::test]
async fn a_step_joining_the_whole_listing_still_sees_all_of_it() {
    let input = vec![array_of(2_500)];
    let one = whole(EVENTS, input.clone()).await;
    let pages = paged(EVENTS, input, 100).await;
    assert_eq!(messages(&pages), vec![r#"{"count":2500}"#.to_string()]);
    assert_eq!(messages(&pages), messages(&one));
}

#[tokio::test]
async fn batch_index_keeps_counting_across_what_would_be_pages() {
    let yaml = "- unarchive: { format: json_array }\n- mapping: 'root = batch_index()'\n";
    let pages = paged(yaml, vec![array_of(250)], 100).await;
    let got = messages(&pages);
    assert_eq!(got.len(), 250);
    assert_eq!(got[249], "249");
}

#[tokio::test]
async fn an_errored_message_stays_flagged_in_its_place_across_pages() {
    // Row 150 throws: it is the first message of the second page.
    let yaml = r#"
- unarchive: { format: json_array }
- mapping: 'root = if this.n == 150 || this.n == 151 { throw("bad row") } else { this }'
- try:
    - mapping: 'root = this.merge({"ok": true})'
- catch:
    - mapping: 'root = {"caught": this.n}'
"#;
    let one = whole(yaml, vec![array_of(300)]).await;
    let pages = paged(yaml, vec![array_of(300)], 150).await;
    assert_eq!(messages(&pages), messages(&one));
    let got = messages(&pages);
    assert_eq!(got[150], r#"{"caught":150}"#);
    assert_eq!(got[152], r#"{"k":5,"n":152,"ok":true}"#);
}

#[tokio::test]
async fn dedupe_drops_a_repeat_from_a_later_page() {
    let yaml = r#"
- unarchive: { format: json_array }
- dedupe:
    cache: seen
    key: '${! json("k") }'
"#;
    let (reg, ctx) = make_ctx();
    let cache_conf = ComponentConfig::from_json_value(
        &serde_json::json!({"label": "seen", "memory": {}}),
        "memory",
        None,
    )
    .unwrap();
    let sctx = ekiben_core::StreamContext {
        stream_id: "test-stream".to_string(),
        org: ctx.org.clone(),
        resources: ctx.resources.clone(),
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: ctx.metrics.clone(),
        label: String::new(),
        path: "stream".to_string(),
        file_root: None,
        env_vars: Arc::new(Default::default()),
        shutdown_rx: ctx.shutdown_rx.clone(),
    };
    let cache = reg.build_cache(&cache_conf, &sctx).unwrap();
    ctx.resources
        .insert_cache(&OrgId::new("org"), "seen".to_string(), cache);
    let confs = confs(yaml);
    let procs = reg.build_processor_list(&confs, &ctx).unwrap();
    let paging = Paging::new(&confs, 3);
    assert!(paging.cuts[0]);
    let mut out = Vec::new();
    let out_ref = &mut out;
    execute_paged(&ctx, &procs, vec![array_of(20)], &paging, |b| {
        out_ref.push(b);
        async { Ok::<(), CoreError>(()) }
    })
    .await
    .unwrap();
    // k = n % 7: the first seven rows are new, every later one repeats a key, on whichever page.
    let got = messages(&out);
    assert_eq!(got.len(), 7, "{got:?}");
    assert!(out.len() >= 3, "the rows ran as pages: {}", out.len());
}

#[tokio::test]
async fn unarchive_pages_hold_the_members_process_batch_returns() {
    let (reg, ctx) = make_ctx();
    let cases: [(&str, &str); 5] = [
        ("json_array", r#"[1,{"a":"b"},"x",null,[2],3]"#),
        ("lines", "a\nb\n\nc\n"),
        ("csv", "id,name\n1,a\n2,b\n3,c\n4,d\n5,e\n"),
        (
            "json_documents",
            "{\"a\":1} {\"a\":2}\n{\"a\":3} 4 \"five\"",
        ),
        ("json_array", r#"{"not":"an array"}"#),
    ];
    for (format, doc) in cases {
        let conf = ComponentConfig::from_json_value(
            &serde_json::json!({"unarchive": {"format": format}}),
            "noop",
            None,
        )
        .unwrap();
        let proc = reg.build_processor(&conf, &ctx, "0").unwrap();
        let input = vec![Part::from_str(doc), Part::from_str("[7,8]")];
        let one = proc.process_batch(&ctx, input.clone()).await.unwrap();
        let pages: Vec<Batch> = match proc.pages(&ctx, input, 2) {
            Ok(pages) => pages.collect(),
            Err(_) => panic!("unarchive pages {format}"),
        };
        assert_eq!(messages(&pages), messages(&one), "{format}");
        assert!(pages.iter().all(|p| p.len() <= 2), "{format}");
    }
}

#[tokio::test]
async fn a_bad_csv_row_flags_its_whole_message_and_lets_no_row_through() {
    let (reg, ctx) = make_ctx();
    let conf = ComponentConfig::from_json_value(
        &serde_json::json!({"unarchive": {"format": "csv"}}),
        "noop",
        None,
    )
    .unwrap();
    let proc = reg.build_processor(&conf, &ctx, "0").unwrap();
    let doc = "id,name\n1,a\n2,b\n3,c,extra\n4,d\n";
    let pages: Vec<Batch> = match proc.pages(&ctx, vec![Part::from_str(doc)], 1) {
        Ok(pages) => pages.collect(),
        Err(_) => panic!("unarchive pages"),
    };
    let got = texts(&pages);
    assert_eq!(got, vec![vec![format!("ERR {doc}")]]);
    let one = proc
        .process_batch(&ctx, vec![Part::from_str(doc)])
        .await
        .unwrap();
    assert_eq!(texts(&one), got);
}

#[tokio::test]
async fn a_json_array_past_the_member_bound_goes_through_as_pages() {
    let (reg, ctx) = make_ctx();
    let conf = ComponentConfig::from_json_value(
        &serde_json::json!({"unarchive": {"format": "json_array"}}),
        "noop",
        None,
    )
    .unwrap();
    let proc = reg.build_processor(&conf, &ctx, "0").unwrap();
    let doc = array_of(25_000);
    // One batch of 25,000 messages is still refused (the member bound, 10,000).
    let one = proc.process_batch(&ctx, vec![doc.clone()]).await.unwrap();
    assert!(one[0][0].is_errored());
    let pages: Vec<Batch> = match proc.pages(&ctx, vec![doc], 1000) {
        Ok(pages) => pages.collect(),
        Err(_) => panic!("unarchive pages"),
    };
    assert_eq!(pages.len(), 25);
    assert_eq!(pages.iter().map(Vec::len).sum::<usize>(), 25_000);
    assert!(pages.iter().flatten().all(|p| !p.is_errored()));
}
