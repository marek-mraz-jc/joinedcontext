//! Differences the public pipeline corpus (corpus/RESULTS.md) found between ekiben and Bento 1.22.0, one case
//! each, with Bento's `blobl` output recorded beside the input by `scripts/corpus/record.py`.

use std::io::Cursor;

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    mapping: String,
    input: Vec<String>,
    stdout: String,
    stderr: String,
    code: i32,
}

#[test]
fn ekiben_blobl_prints_what_bento_printed_for_each_corpus_regression() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("parity/corpus_regressions.json")).expect("cases parse");
    assert!(!cases.is_empty());
    for c in cases {
        let input: String = c.input.iter().map(|l| format!("{l}\n")).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code =
            ekiben_server::blobl::run(&c.mapping, false, Cursor::new(input), &mut out, &mut err);
        assert_eq!(
            String::from_utf8_lossy(&out),
            c.stdout,
            "stdout of '{}'",
            c.name
        );
        assert_eq!(
            String::from_utf8_lossy(&err),
            c.stderr,
            "stderr of '{}'",
            c.name
        );
        assert_eq!(code, c.code, "exit code of '{}'", c.name);
    }
}

#[test]
fn stdin_input_streams_lines_like_bento() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("out.txt");
    let cfg = dir.path().join("stdin.yaml");
    std::fs::write(
        &cfg,
        format!(
            "input: {{stdin: {{}}}}\npipeline: {{processors: [{{mapping: 'root = this.merge({{\"g\": 1000000.0 * 1000000000000000.0}})'}}]}}\noutput: {{file: {{path: '{}', codec: lines}}}}\n",
            out.display()
        ),
    )
    .expect("write config");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ekiben"))
        .arg("run")
        .arg(&cfg)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn ekiben");
    use std::io::Write;
    // Bento 1.22.0 writes this for the input below: lines that are not JSON pass unchanged (CR stripped),
    // the object comes back with sorted keys and Go's float text.
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"a\n\nb\r\n{\"z\":1,\"a\":2}")
        .expect("write stdin");
    assert!(child.wait().expect("wait").success());
    assert_eq!(
        std::fs::read_to_string(out).expect("output"),
        "a\n\nb\n{\"a\":2,\"g\":1e+21,\"z\":1}\n"
    );
}

fn run_config(yaml: &str) -> (std::process::Output, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("out.txt");
    let cfg = dir.path().join("c.yaml");
    std::fs::write(&cfg, yaml.replace("OUT", &out.display().to_string())).expect("write config");
    let res = std::process::Command::new(env!("CARGO_BIN_EXE_ekiben"))
        .arg("run")
        .arg(&cfg)
        .output()
        .expect("run ekiben");
    (res, std::fs::read_to_string(out).unwrap_or_default())
}

#[test]
fn cache_get_miss_flags_bentos_error_text() {
    // Source: jem-davies/bento-testbench exists_cache.yaml switches on this text.
    let (res, out) = run_config(
        r#"input: {generate: {count: 1, interval: "", mapping: 'root = {"hello":"world"}'}}
pipeline:
  processors:
    - cache: {resource: acache, operator: get, key: somekey}
    - mapping: 'root = {"err": error()}'
output: {file: {path: 'OUT', codec: lines}}
cache_resources:
  - label: acache
    memory: {compaction_interval: ""}
"#,
    );
    assert!(res.status.success());
    // Bento 1.22.0 output for the same config.
    assert_eq!(
        out,
        "{\"err\":\"operator failed for key 'somekey': key does not exist\"}\n"
    );
}

#[test]
fn undefined_processor_resource_refuses_the_stream() {
    // Source: four corpus configs whose resources live in another file; Bento refuses to start them.
    let (res, out) = run_config(
        r#"input: {generate: {count: 1, interval: "", mapping: 'root = {}'}}
pipeline: {processors: [{resource: shared_processor}]}
output: {file: {path: 'OUT', codec: lines}}
"#,
    );
    assert!(!res.status.success());
    assert!(out.is_empty());
    let text =
        String::from_utf8_lossy(&res.stdout).to_string() + &String::from_utf8_lossy(&res.stderr);
    assert!(
        text.contains("processor resource 'shared_processor' was not found"),
        "{text}"
    );
}

#[test]
fn mutation_reads_metadata_written_earlier_in_the_same_mapping() {
    let (res, out) = run_config(
        r#"input:
  generate:
    count: 1
    interval: ""
    mapping: |
      root = {"a":1}
      meta pre = "p"
pipeline:
  processors:
    - mutation: |
        meta y = "v"
        meta pre = deleted()
        root.m = metadata()
    - mapping: |
        meta z = "w"
        root = this
        root.m2 = metadata()
output: {file: {path: 'OUT', codec: lines}}
"#,
    );
    assert!(res.status.success());
    // Bento 1.22.0: `mutation` works in place, `mapping` reads the input message's metadata.
    assert_eq!(out, "{\"a\":1,\"m\":{\"y\":\"v\"},\"m2\":{\"y\":\"v\"}}\n");
}

#[test]
fn json_schema_refuses_a_message_its_schema_does_not_admit() {
    // The method validates (T-6622): a pipeline relying on it must not pass invalid data on.
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = ekiben_server::blobl::run(
        r#"root = this.json_schema("{\"type\":\"object\",\"required\":[\"a\"]}")"#,
        false,
        Cursor::new("{}\n{\"a\":1}\n"),
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8_lossy(&out), "{\"a\":1}\n");
    assert!(String::from_utf8_lossy(&err).starts_with("failed to execute map: "));
}
