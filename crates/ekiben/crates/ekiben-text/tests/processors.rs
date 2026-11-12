//! Rust twins of Bento 1.22.0's processor_jq_test.go, processor_jmespath_test.go,
//! processor_grok_test.go and msgpack/processor_test.go: same case names, same inputs.

use std::sync::Arc;

use bytes::Bytes;
use ekiben_bloblang::{Environment, Value};
use ekiben_core::{
    Batch, ComponentConfig, CoreError, OrgId, Part, ProcContext, Processor, Registry,
    ResourceManager, StreamMetrics,
};
use tokio::sync::watch;

fn ctx(file_root: Option<std::path::PathBuf>) -> (Arc<Registry>, ProcContext) {
    let reg = Arc::new(Registry::new());
    ekiben_text::register(&reg);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("t"));
    let (_tx, rx) = watch::channel(false);
    let c = ProcContext {
        stream_id: "test-stream".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("test-stream", "t"),
        label: "root".to_string(),
        component_type: "root".to_string(),
        path: "pipeline.processors".to_string(),
        file_root,
        shutdown_rx: rx,
    };
    (reg, c)
}

fn build_yaml(yaml: &str) -> Result<(Arc<dyn Processor>, ProcContext), CoreError> {
    let conf: serde_json::Value = serde_norway::from_str(yaml).expect("test yaml");
    let (reg, c) = ctx(None);
    let comp = ComponentConfig::from_json_value(&conf, "noop", None)?;
    let p = reg.build_processor_list(&[comp], &c)?.remove(0);
    Ok((p, c))
}

async fn run(yaml: &str, inputs: &[&str]) -> Vec<Batch> {
    let (p, c) = build_yaml(yaml).expect("config builds");
    let batch: Batch = inputs.iter().map(|s| Part::new(Bytes::from(s.to_string()))).collect();
    p.process_batch(&c, batch).await.expect("no batch error")
}

fn bytes(p: &Part) -> String {
    String::from_utf8(p.as_bytes_ref().to_vec()).expect("utf-8")
}

// processor_jq_test.go

#[tokio::test]
async fn test_jq_all_parts() {
    let out = run(
        "jq:\n  query: .foo.bar\n",
        &[r#"{"foo":{"bar":0}}"#, r#"{"foo":{"bar":1}}"#, r#"{"foo":{"bar":2}}"#],
    )
    .await;
    assert_eq!(out.len(), 1);
    for (i, p) in out[0].iter().enumerate() {
        assert_eq!(bytes(p), i.to_string());
    }
}

#[tokio::test]
async fn test_jq_validation() {
    let out = run("jq:\n  query: .foo.bar\n", &["this is bad json"]).await;
    assert_eq!(out.len(), 1);
    assert_eq!(bytes(&out[0][0]), "this is bad json");
    assert!(out[0][0].is_errored());
}

#[tokio::test]
async fn test_jq_mutation() {
    let (p, c) = build_yaml("jq:\n  query: '{foo: .foo} | .foo.bar = \"baz\"'\n").unwrap();
    let og = serde_json::json!({"foo": {"original": {"content": "is this"}}, "bar": "remove this"});
    let part = Part::from_json(og.clone());
    let out = p.process_batch(&c, vec![part]).await.unwrap();
    assert_eq!(
        bytes(&out[0][0]),
        r#"{"foo":{"bar":"baz","original":{"content":"is this"}}}"#
    );
}

#[tokio::test]
async fn test_jq() {
    let cases: &[(&str, &str, &str, &str)] = &[
        ("select obj", ".foo.bar", r#"{"foo":{"bar":{"baz":1}}}"#, r#"{"baz":1}"#),
        ("select array", ".foo.bar", r#"{"foo":{"bar":["baz","qux"]}}"#, r#"["baz","qux"]"#),
        ("select obj as str", ".foo.bar", r#"{"foo":{"bar":"{\"baz\":1}"}}"#, r#""{\"baz\":1}""#),
        ("select str", ".foo.bar", r#"{"foo":{"bar":"hello world"}}"#, r#""hello world""#),
        ("select float", ".foo.bar", r#"{"foo":{"bar":0.123}}"#, "0.123"),
        ("select int", ".foo.bar", r#"{"foo":{"bar":123}}"#, "123"),
        ("select bool", ".foo.bar", r#"{"foo":{"bar":true}}"#, "true"),
        ("null result", ".baz.qux", r#"{"foo":{"bar":true}}"#, "null"),
        ("empty string", ".foo.bar", r#"{"foo":{"bar":""}}"#, r#""""#),
        (
            "convert to csv",
            "[.ts,.id,.msg] | @csv",
            r#"{"id":"1054fe28","msg":"sample \"log\"","ts":1641393111}"#,
            r#""1641393111,\"1054fe28\",\"sample \"\"log\"\"\"""#,
        ),
    ];
    for (name, path, input, want) in cases {
        let out = run(&format!("jq:\n  query: '{path}'\n"), &[input]).await;
        assert_eq!(out.len(), 1, "{name}");
        assert_eq!(bytes(&out[0][0]), *want, "{name}");
    }
    // "invalid type": a time.Time in the structured message.
    let (p, c) = build_yaml("jq:\n  query: '.ts | length'\n").unwrap();
    let mut m = std::collections::BTreeMap::new();
    m.insert(
        "ts".to_string(),
        Value::Timestamp(chrono::DateTime::from_timestamp(1000, 0).unwrap().fixed_offset()),
    );
    let out = p
        .process_batch(&c, vec![Part::from_structured(Value::Object(m.into()))])
        .await
        .unwrap();
    let err = out[0][0].error_get().expect("errored").to_string();
    assert!(err.contains("invalid type: time.Time"), "{err}");
}

#[tokio::test]
async fn test_jq_output_raw() {
    let cases: &[(&str, &str, &str, &str)] = &[
        ("select obj", ".foo.bar", r#"{"foo":{"bar":{"baz":1}}}"#, r#"{"baz":1}"#),
        ("select array", ".foo.bar", r#"{"foo":{"bar":["baz","qux"]}}"#, r#"["baz","qux"]"#),
        ("select obj as str", ".foo.bar", r#"{"foo":{"bar":"{\"baz\":1}"}}"#, r#"{"baz":1}"#),
        ("select str", ".foo.bar", r#"{"foo":{"bar":"hello world"}}"#, "hello world"),
        ("select float", ".foo.bar", r#"{"foo":{"bar":0.123}}"#, "0.123"),
        ("select int", ".foo.bar", r#"{"foo":{"bar":123}}"#, "123"),
        ("select bool", ".foo.bar", r#"{"foo":{"bar":true}}"#, "true"),
        ("null result", ".baz.qux", r#"{"foo":{"bar":true}}"#, "null"),
        ("empty string", ".foo.bar", r#"{"foo":{"bar":""}}"#, ""),
        (
            "convert to csv",
            "[.ts,.id,.msg] | @csv",
            r#"{"id":"1054fe28","msg":"sample \"log\"","ts":1641393111}"#,
            r#"1641393111,"1054fe28","sample ""log""""#,
        ),
    ];
    for (name, path, input, want) in cases {
        let out = run(&format!("jq:\n  query: '{path}'\n  output_raw: true\n"), &[input]).await;
        assert_eq!(out.len(), 1, "{name}");
        assert_eq!(bytes(&out[0][0]), *want, "{name}");
    }
}

#[tokio::test]
async fn jq_metadata_variable_and_empty_env() {
    let (p, c) = build_yaml("jq:\n  query: '[$metadata.source, $ENV, env]'\n").unwrap();
    let mut part = Part::new(Bytes::from_static(b"{}"));
    part.meta_set_str("source", "eea");
    let out = p.process_batch(&c, vec![part]).await.unwrap();
    assert_eq!(bytes(&out[0][0]), r#"["eea",{},{}]"#);
}

#[tokio::test]
async fn jq_no_output_drops_the_message_and_limits_hold() {
    let out = run("jq:\n  query: empty\n", &["{}"]).await;
    assert!(out.is_empty());
    let out = run("jq:\n  query: repeat(1)\n", &["{}"]).await;
    let err = out[0][0].error_get().expect("errored").to_string();
    assert!(err.contains("emitted more than"), "{err}");
    let long = format!("jq:\n  query: '{}'\n", ".".repeat(ekiben_text::processor_jq::MAX_QUERY_BYTES + 1));
    let err = build_yaml(&long).err().expect("refused").to_string();
    assert!(err.contains("more than"), "{err}");
    assert!(build_yaml("jq:\n  query: '.foo |'\n").is_err());
}

// processor_jmespath_test.go

#[tokio::test]
async fn test_jmes_path_all_parts() {
    let out = run(
        "jmespath:\n  query: foo.bar\n",
        &[r#"{"foo":{"bar":0}}"#, r#"{"foo":{"bar":1}}"#, r#"{"foo":{"bar":2}}"#],
    )
    .await;
    for (i, p) in out[0].iter().enumerate() {
        assert_eq!(bytes(p), i.to_string());
    }
}

#[tokio::test]
async fn test_jmes_path_validation() {
    let out = run("jmespath:\n  query: foo.bar\n", &["this is bad json"]).await;
    assert_eq!(bytes(&out[0][0]), "this is bad json");
    assert!(out[0][0].is_errored());
}

#[tokio::test]
async fn test_jmes_path_mutation() {
    let (p, c) = build_yaml("jmespath:\n  query: \"{foo: merge(foo, {bar:'baz'})}\"\n").unwrap();
    let part = Part::from_json(serde_json::json!({"foo": {"original": {"content": "is this"}}}));
    let out = p.process_batch(&c, vec![part]).await.unwrap();
    assert_eq!(
        bytes(&out[0][0]),
        r#"{"foo":{"bar":"baz","original":{"content":"is this"}}}"#
    );
}

#[tokio::test]
async fn test_jmes_path() {
    let cases: &[(&str, &str, &str, &str)] = &[
        ("select obj", "foo.bar", r#"{"foo":{"bar":{"baz":1}}}"#, r#"{"baz":1}"#),
        ("select array", "foo.bar", r#"{"foo":{"bar":["baz","qux"]}}"#, r#"["baz","qux"]"#),
        ("select obj as str", "foo.bar", r#"{"foo":{"bar":"{\"baz\":1}"}}"#, r#""{\"baz\":1}""#),
        ("select str", "foo.bar", r#"{"foo":{"bar":"hello world"}}"#, r#""hello world""#),
        ("select float", "foo.bar", r#"{"foo":{"bar":0.123}}"#, "0.123"),
        ("select int", "foo.bar", r#"{"foo":{"bar":123}}"#, "123"),
        ("select bool", "foo.bar", r#"{"foo":{"bar":true}}"#, "true"),
        ("addition int", "sum([foo.bar, `6`])", r#"{"foo":{"bar":123}}"#, "129"),
    ];
    for (name, path, input, want) in cases {
        let out = run(&format!("jmespath:\n  query: \"{path}\"\n"), &[input]).await;
        assert_eq!(bytes(&out[0][0]), *want, "{name}");
    }
}

// processor_grok_test.go

#[tokio::test]
async fn test_grok_all_parts() {
    let out = run(
        "grok:\n  expressions:\n    - \"%{WORD:first},%{INT:second:int}\"\n",
        &["foo,0", "foo,1", "foo,2"],
    )
    .await;
    let got: Vec<String> = out[0].iter().map(bytes).collect();
    assert_eq!(
        got,
        [
            r#"{"first":"foo","second":0}"#,
            r#"{"first":"foo","second":1}"#,
            r#"{"first":"foo","second":2}"#
        ]
    );
}

#[tokio::test]
async fn test_grok() {
    let cases: &[(&str, &str, &str, &str, &str)] = &[
        (
            "Common apache parsing",
            "%{COMMONAPACHELOG}",
            r#"127.0.0.1 - - [23/Apr/2014:22:58:32 +0200] "GET /index.php HTTP/1.1" 404 207"#,
            r#"{"auth":"-","bytes":"207","clientip":"127.0.0.1","httpversion":"1.1","ident":"-","request":"/index.php","response":"404","timestamp":"23/Apr/2014:22:58:32 +0200","verb":"GET"}"#,
            "{}",
        ),
        (
            "Test pattern definitions",
            "%{ACTION:action} connection from %{IPV4:ipv4}",
            "pass connection from 127.0.0.1",
            r#"{"action":"pass","ipv4":"127.0.0.1"}"#,
            r#"{"ACTION":"(pass|deny)"}"#,
        ),
        (
            "Test dot path in name definition",
            "%{WORD:nested.name} %{INT:nested.value:int} bazes from %{IPV4:nested.ipv4}",
            "foo 5 bazes from 192.0.1.11",
            r#"{"nested":{"ipv4":"192.0.1.11","name":"foo","value":5}}"#,
            "{}",
        ),
    ];
    for (name, pattern, input, want, defs) in cases {
        let yaml = format!("grok:\n  expressions:\n    - '{pattern}'\n  pattern_definitions: {defs}\n");
        let out = run(&yaml, &[input]).await;
        assert_eq!(out.len(), 1, "{name}");
        assert_eq!(bytes(&out[0][0]), *want, "{name}");
    }
}

#[tokio::test]
async fn test_grok_file_imports() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("foos"),
        "\nFOOFLAT %{WORD:first} %{WORD:second} %{WORD:third}\nFOONESTED %{INT:nested.first:int} %{WORD:nested.second} %{WORD:nested.third}\n",
    )
    .unwrap();
    let yaml = format!(
        "grok:\n  expressions:\n    - \"%{{FOONESTED}}\"\n    - \"%{{FOOFLAT}}\"\n  pattern_paths: [ {} ]\n",
        dir.path().display()
    );
    let (p, c) = build_yaml(&yaml).unwrap();
    let out = p
        .process_batch(&c, vec![Part::new(Bytes::from_static(b"hello foo bar"))])
        .await
        .unwrap();
    assert_eq!(bytes(&out[0][0]), r#"{"first":"hello","second":"foo","third":"bar"}"#);
    let out = p
        .process_batch(&c, vec![Part::new(Bytes::from_static(b"10 foo bar"))])
        .await
        .unwrap();
    assert_eq!(
        bytes(&out[0][0]),
        r#"{"nested":{"first":10,"second":"foo","third":"bar"}}"#
    );
}

#[tokio::test]
async fn grok_refuses_cycles_traversal_and_reports_no_match() {
    let err = build_yaml("grok:\n  expressions: ['%{A}']\n  pattern_definitions: {A: '%{B}', B: '%{A}'}\n")
        .err()
        .expect("cycle refused")
        .to_string();
    assert!(err.contains("failed to create grok compiler"), "{err}");
    let dir = tempfile::tempdir().unwrap();
    let (reg, c) = ctx(Some(dir.path().to_path_buf()));
    let conf: serde_json::Value = serde_norway::from_str(
        "grok:\n  expressions: ['%{WORD}']\n  pattern_paths: ['../etc']\n",
    )
    .unwrap();
    let comp = ComponentConfig::from_json_value(&conf, "noop", None).unwrap();
    assert!(reg.build_processor_list(&[comp], &c).is_err());
    let out = run("grok:\n  expressions: ['%{INT:n}']\n", &["no digits"]).await;
    assert_eq!(
        out[0][0].error_get().expect("errored").to_string(),
        "no pattern matches found"
    );
}

// msgpack/processor_test.go

#[test]
fn test_msg_pack_to_json() {
    use base64::Engine;
    let input = base64::engine::general_purpose::STANDARD
        .decode("iKNrZXmjZm9vp3RydWVLZXnDqGZhbHNlS2V5wqdudWxsS2V5wKZpbnRLZXnQe6hmbG9hdEtlectARszMzMzMzaVhcnJheZGjYmFypm5lc3RlZIGja2V5o2Jheg==")
        .unwrap();
    let p = ekiben_text::msgpack_processor::MsgpackProcessor::new("to_json").unwrap();
    let mut part = Part::new(Bytes::from(input));
    p.process(&mut part).unwrap();
    let v = part.as_structured().unwrap();
    let Value::Object(m) = &v else { panic!("object") };
    // Go reads int8(123); this workspace's Value carries no integer width.
    assert_eq!(m["intKey"], Value::Int(123));
    assert_eq!(
        bytes(&part),
        r#"{"array":["bar"],"falseKey":false,"floatKey":45.6,"intKey":123,"key":"foo","nested":{"key":"baz"},"nullKey":null,"trueKey":true}"#
    );
}

#[test]
fn test_msg_pack_from_json() {
    let cases: &[(&str, &str, &[(&str, Value)])] = &[
        (
            "basic",
            r#"{"key":"foo","trueKey":true,"falseKey":false,"nullKey":null,"intKey":123,"floatKey":45.6,"array":["bar"],"nested":{"key":"baz"}}"#,
            &[("intKey", Value::Int(123)), ("floatKey", Value::Float(45.6))],
        ),
        (
            "various ints",
            r#"{"int8": 13, "uint8": 254, "int16": -257, "uint16" : 65534, "int32" : -70123, "uint32" : 2147483648, "int64" : -9223372036854775808, "uint64": 18446744073709551615}"#,
            &[
                ("int8", Value::Int(13)),
                ("uint8", Value::Int(254)),
                ("int16", Value::Int(-257)),
                ("uint16", Value::Int(65534)),
                ("int32", Value::Int(-70123)),
                ("uint32", Value::Int(2147483648)),
                ("int64", Value::Int(i64::MIN)),
                ("uint64", Value::Uint(u64::MAX)),
            ],
        ),
    ];
    let p = ekiben_text::msgpack_processor::MsgpackProcessor::new("from_json").unwrap();
    for (name, input, want) in cases {
        let mut part = Part::new(Bytes::from(input.to_string()));
        p.process(&mut part).unwrap();
        let back = ekiben_bloblang::msgpack::decode(&part.as_bytes()).unwrap();
        let Value::Object(m) = back else { panic!("{name}: object") };
        for (k, v) in *want {
            assert_eq!(&m[*k], v, "{name}: {k}");
        }
    }
    assert!(ekiben_text::msgpack_processor::MsgpackProcessor::new("nope")
        .err()
        .unwrap()
        .contains("operator not recognised: nope"));
}
