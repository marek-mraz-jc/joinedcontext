//! Grounded Bloblang test suite covering:
//! 1. Bento doc examples (`about.md`, `walkthrough.md`, `arithmetic.md`, `advanced.md`, `methods_*.go`, `functions.go`).
//! 2. Defect 2: Self-recursive named `map inf { root = this.apply("inf") }` fails with
//!    `exceeded maximum allowed stacks` on the default 2 MiB debug test thread and never aborts,
//!    including when `query_targets()` is called before execution!
//! 3. PART B worker traps (`merge` vs `assign`, `number()` vs `int64()` base-0 octal trap,
//!    `ts_strptime` rollover, `range(0, n, step)` dropping last partial chunk, `.get()` on dotted keys,
//!    `match` rebinding `this`, dynamic object keys, `strip_html().unescape_html()`).

use std::collections::HashMap;
use std::sync::Arc;

use ekiben_bloblang::env::MemoryImporter;
use ekiben_bloblang::{EmptyBatch, Environment, SimplePartView, Value};

fn run_map(mapping: &str, input_json: &str) -> Result<Option<String>, String> {
    let env = Environment::new();
    let exec = env.compile_mapping(mapping).map_err(|e| e.to_string())?;
    let _targets = exec.query_targets();
    let _assigns = exec.assignment_targets();
    let res = exec
        .execute_bytes(input_json.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(res.map(|v| match v {
        Value::Nothing => input_json.to_string(),
        Value::String(s) => s,
        Value::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        other => serde_json::to_string(&other.to_json_value()).unwrap(),
    }))
}

#[test]
fn defect_recursive_map_exceeds_max_stacks_without_stack_overflow() {
    // Grounded in internal/bloblang/mapping/executor.go (defaultMaxMapStacks = 5000, errStacks)
    // and Section 4 Defect 2: must pass on a default 2 MiB thread in debug mode AND survive query_targets().
    let env = Environment::new();
    let exec = env
        .compile_mapping(
            r#"
map inf {
  root = this.apply("inf")
}
root = this.apply("inf")
"#,
        )
        .expect("compile recursive map");

    // Static pre-pass over AST must not loop forever on recursive map!
    let targets = exec.query_targets();
    assert!(!targets.is_empty());

    let err = exec
        .execute_bytes(b"{}")
        .expect_err("recursive map must fail with max stacks error");
    let msg = err.to_string();
    assert!(
        msg.contains("exceeded maximum allowed stacks of 5000"),
        "unexpected error: {msg}"
    );
}

#[test]
fn defect_unknown_method_and_function_refused_at_parse_time() {
    let env = Environment::new();
    let err = env
        .compile_mapping("root = this.foo.nonexistent_method_xyz()")
        .err()
        .expect("unknown method must fail at parse time");
    assert!(
        err.to_string()
            .contains("unrecognised method 'nonexistent_method_xyz'"),
        "got: {err}"
    );

    let fn_err = env
        .compile_mapping("root = nonexistent_func_xyz()")
        .err()
        .expect("unknown function must fail at parse time");
    assert!(
        fn_err
            .to_string()
            .contains("unrecognised function 'nonexistent_func_xyz'"),
        "got: {fn_err}"
    );
}

#[test]
fn bento_about_md_assignment_coalesce_slicing_conditionals_match_maps() {
    // Grounded in website/docs/guides/bloblang/about.md
    let out = run_map(
        r#"
root.id = this.thing.id
root.type = "yo"
content = thing.doc.message
"#,
        r#"{"thing":{"id":"wat1","doc":{"title":"wut","message":"hello world"}}}"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(out, r#"{"content":"hello world","id":"wat1","type":"yo"}"#);

    // Special characters in paths
    let out = run_map(
        r#"root."foo.bar".baz = this."buz bev".fub"#,
        r#"{"buz bev":{"fub":"hello world"}}"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(out, r#"{"foo.bar":{"baz":"hello world"}}"#);

    // Coalesce
    for (inp, exp) in [
        (
            r#"{"thing":{"article":{"type":"foo"}}}"#,
            r#"{"new_doc":{"type":"foo"}}"#,
        ),
        (
            r#"{"thing":{"comment":{"type":"bar"}}}"#,
            r#"{"new_doc":{"type":"bar"}}"#,
        ),
        (
            r#"{"thing":{"type":"baz"}}"#,
            r#"{"new_doc":{"type":"baz"}}"#,
        ),
    ] {
        let out = run_map(
            r#"root.new_doc.type = this.thing.(article | comment | this).type"#,
            inp,
        )
        .unwrap()
        .unwrap();
        assert_eq!(out, exp);
    }

    // Slicing [start:end:step]
    let out = run_map(
        r#"
root.substring = this.message[1:4]
root.subarray = this.items[1:3]
root.from_start = this.message[:3]
root.to_end = this.message[2:]
root.every_second = this.message[::2]
root.reverse = this.message[::-1]
root.complex_slice = this.items[1:5:2]
"#,
        r#"{"message":"hello world","items":["a","b","c","d","e","f"]}"#,
    )
    .unwrap()
    .unwrap();
    let jv: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(jv["substring"], "ell");
    assert_eq!(jv["subarray"], serde_json::json!(["b", "c"]));
    assert_eq!(jv["from_start"], "hel");
    assert_eq!(jv["to_end"], "llo world");
    assert_eq!(jv["every_second"], "hlowrd");
    assert_eq!(jv["reverse"], "dlrow olleh");
    assert_eq!(jv["complex_slice"], serde_json::json!(["b", "d"]));
}

#[test]
fn bento_arithmetic_md_degradation_and_comparisons() {
    // Grounded in website/docs/guides/bloblang/arithmetic.md and internal/bloblang/query/arithmetic.go
    let out = run_map(
        r#"
root.int_add = 5 + 3
root.int_mul = 5 * 3
root.int_div = 6 / 3
root.float_add = 5 + 3.5
root.mod_val = 10 % 3
root.eq_num = 10 == 10.0
root.str_add = "hello " + "world"
"#,
        r#"{}"#,
    )
    .unwrap()
    .unwrap();
    let jv: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(jv["int_add"], 8);
    assert_eq!(jv["int_mul"], 15);
    assert_eq!(jv["int_div"], 2.0);
    assert_eq!(jv["float_add"], 8.5);
    assert_eq!(jv["mod_val"], 1);
    assert_eq!(jv["eq_num"], true);
    assert_eq!(jv["str_add"], "hello world");

    // Division by zero caught by .catch
    let out = run_map(r#"root.val = (10 / 0).catch(-1)"#, r#"{}"#)
        .unwrap()
        .unwrap();
    assert_eq!(out, r#"{"val":-1}"#);
}

#[test]
fn trap_merge_vs_assign() {
    // Grounded in internal/bloblang/query/methods_structured.go and PART B traps
    let inp = r#"{"foo":{"first_name":"fooer","likes":"bars"},"bar":{"second_name":"barer","likes":"foos"}}"#;
    let merged = run_map(r#"root = this.foo.merge(this.bar)"#, inp)
        .unwrap()
        .unwrap();
    assert_eq!(
        merged,
        r#"{"first_name":"fooer","likes":["bars","foos"],"second_name":"barer"}"#
    );

    let assigned = run_map(r#"root = this.foo.assign(this.bar)"#, inp)
        .unwrap()
        .unwrap();
    assert_eq!(
        assigned,
        r#"{"first_name":"fooer","likes":"foos","second_name":"barer"}"#
    );
}

#[test]
fn trap_number_vs_int64_base0_octal() {
    // Grounded in internal/impl/pure/bloblang_numbers.go and PART B bbsk-pipeline-podujatia-mostenica-bento.yaml
    // `int64()` uses base 0: "0xDEAD" -> 57005, "08" fails as invalid octal, whereas `"08".number().int64()` -> 8!
    let hex_out = run_map(r#"root.v = "0xDEAD".int64()"#, r#"{}"#)
        .unwrap()
        .unwrap();
    assert_eq!(hex_out, r#"{"v":57005}"#);

    let octal_err = run_map(r#"root.v = "08".int64()"#, r#"{}"#);
    assert!(
        octal_err.is_err(),
        "'08'.int64() must fail as invalid octal"
    );

    let via_number = run_map(r#"root.v = "08".number().int64()"#, r#"{}"#)
        .unwrap()
        .unwrap();
    assert_eq!(via_number, r#"{"v":8}"#);

    // Float with decimals must fail int64() unless rounded first
    assert!(run_map(r#"root.v = (12.34).int64()"#, r#"{}"#).is_err());
    assert_eq!(
        run_map(r#"root.v = (12.34).round().int64()"#, r#"{}"#)
            .unwrap()
            .unwrap(),
        r#"{"v":12}"#
    );
}

#[test]
fn trap_ts_strptime_rollover_and_range_step() {
    // Grounded in PART B KNOWLEDGE.md traps [T-5677, T-5720]
    // 1. ts_strptime("%d.%m.%Y") takes "31.02.2026" as 3 March 2026
    let rolled = run_map(
        r#"root.d = "31.02.2026".ts_strptime("%d.%m.%Y").ts_format("2006-01-02", "UTC")"#,
        r#"{}"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(rolled, r#"{"d":"2026-03-03"}"#);

    // 2. range(0, n, step) uses (stop - start) / step, dropping the last partial chunk
    let ranged = run_map(
        r#"
root.even = range(0, 10, 2)
root.odd = range(0, 9, 2)
"#,
        r#"{}"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(ranged, r#"{"even":[0,2,4,6,8],"odd":[0,2,4,6]}"#);
}

#[test]
fn trap_get_dotted_keys_and_match_rebind_this() {
    // Grounded in PART B traps [T-5274, T-5426]
    // .get("a.b") splits on '.', whereas this."a.b" reads literal key "a.b"
    let out = run_map(
        r#"
root.via_get = this.get("a.b")
root.via_quoted = this."a.b"
"#,
        r#"{"a":{"b":"nested"},"a.b":"literal_dotted"}"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(out, r#"{"via_get":"nested","via_quoted":"literal_dotted"}"#);

    // `match this.doc { ... }` rebinds `this` to `this.doc`, while `match { ... }` keeps outer `this`
    let match_out = run_map(
        r#"
root.rebound = match this.doc {
  this.kind == "a" => this.val,
  _ => "none"
}
root.bare = match {
  this.doc.kind == "a" => this.outer_val,
  _ => "none"
}
"#,
        r#"{"outer_val":"from_root","doc":{"kind":"a","val":"from_doc"}}"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(match_out, r#"{"bare":"from_root","rebound":"from_doc"}"#);
}

#[test]
fn trap_strip_html_and_unescape_html() {
    // Grounded in PART B trap [T-6215] and internal/bloblang/query/methods_strings.go
    let out = run_map(
        r#"
root.stripped = this.html.strip_html()
root.preserve = this.html.strip_html(["article"])
root.text = "<p>1 &lt; 2 &amp; 3</p>".strip_html().unescape_html()
"#,
        r#"{"html":"<article><p>the plain <strong>old text</strong></p></article>"}"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        out,
        r#"{"preserve":"<article>the plain old text</article>","stripped":"the plain old text","text":"1 < 2 & 3"}"#
    );
}

#[test]
fn string_regex_hash_encode_xml_yaml_csv_methods() {
    // Grounded in internal/bloblang/query/methods_strings.go and internal/impl/xml/bloblang.go
    let out = run_map(
        r#"
root.cap = "the foo bar".capitalize()
root.enc = "hello world".encode("hex").decode("hex").string()
root.sha1 = "hello world".hash("sha1").encode("hex")
root.hmac = "hello world".hash("hmac_sha1", "static-key").encode("hex")
root.xxh = "hello world".hash("xxhash64").string()
root.re_obj = "-axxb-ab-".re_find_object("a(?P<foo>x*)b")
root.re_all_obj = "-axxb-ab-".re_find_all_object("a(?P<foo>x*)b")
root.re_rep = "foo ADD 70".re_replace_all("ADD ([0-9]+)", "+($1)")
root.csv = "foo,bar\nfoo 1,bar 1\nfoo 2,bar 2".parse_csv()
root.xml = "<root><title>Hello</title><num id=\"99\">123</num></root>".parse_xml(cast: true)
root.slug = "Gopher & Bento".slug()
"#,
        r#"{}"#,
    )
    .unwrap()
    .unwrap();
    let jv: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(jv["cap"], "The Foo Bar");
    assert_eq!(jv["enc"], "hello world");
    assert_eq!(jv["sha1"], "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed");
    assert_eq!(jv["hmac"], "d87e5f068fa08fe90bb95bc7c8344cb809179d76");
    assert_eq!(jv["re_obj"], serde_json::json!({"0": "axxb", "foo": "xx"}));
    assert_eq!(
        jv["re_all_obj"],
        serde_json::json!([{"0": "axxb", "foo": "xx"}, {"0": "ab", "foo": ""}])
    );
    assert_eq!(jv["re_rep"], "foo +(70)");
    assert_eq!(
        jv["csv"],
        serde_json::json!([{"bar": "bar 1", "foo": "foo 1"}, {"bar": "bar 2", "foo": "foo 2"}])
    );
    assert_eq!(
        jv["xml"],
        serde_json::json!({"root": {"title": "Hello", "num": {"#text": 123, "-id": 99}}})
    );
    assert_eq!(jv["slug"], "gopher-and-bento");
}

#[test]
fn structured_methods_fold_squash_with_without_collapse_explode_zip() {
    // Grounded in internal/bloblang/query/methods_structured.go and internal/impl/pure/bloblang_objects.go
    let out = run_map(
        r#"
root.sum = [3, 8, 11].fold(0, item -> item.tally + item.value)
root.squashed = [{"a": [1]}, {"b": [2]}, {"a": [3]}].squash()
root.with_res = {"inner":{"a":"first","b":"second","c":"third"},"d":"fourth","e":"fifth"}.with("inner.a","inner.c","d")
root.without_res = {"inner":{"a":"first","b":"second","c":"third"},"d":"fourth","e":"fifth"}.without("inner.a","inner.c","d")
root.collapsed = {"foo":[{"bar":"1"},{"bar":{}},{"bar":"2"}]}.collapse()
root.exploded = {"id":1,"value":["a","b"]}.explode("value")
root.zipped = ["a","b"].zip([1,2], [3,4])
"#,
        r#"{}"#,
    )
    .unwrap()
    .unwrap();
    let jv: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(jv["sum"], 22);
    assert_eq!(jv["squashed"], serde_json::json!({"a": [1, 3], "b": [2]}));
    assert_eq!(
        jv["with_res"],
        serde_json::json!({"d": "fourth", "inner": {"a": "first", "c": "third"}})
    );
    assert_eq!(
        jv["without_res"],
        serde_json::json!({"e": "fifth", "inner": {"b": "second"}})
    );
    assert_eq!(
        jv["collapsed"],
        serde_json::json!({"foo.0.bar": "1", "foo.2.bar": "2"})
    );
    assert_eq!(
        jv["exploded"],
        serde_json::json!([{"id": 1, "value": "a"}, {"id": 1, "value": "b"}])
    );
    assert_eq!(jv["zipped"], serde_json::json!([["a", 1, 3], ["b", 2, 4]]));
}

#[test]
fn time_and_iso8601_methods() {
    // Grounded in internal/impl/pure/bloblang_time.go
    let out = run_map(
        r#"
root.fmt1 = (1597405526).ts_format(format: "2006-Jan-02 15:04:05", tz: "UTC")
root.fmt2 = "2020-08-14T11:50:26.371Z".ts_format("2006-Jan-02 15:04:05.999999", "UTC")
root.strftime = "2020-08-14T11:50:26.371Z".ts_strftime("%Y-%b-%d %H:%M:%S.%f", "UTC")
root.tz_offset = "2026-10-18T15:00:00Z".ts_format("-07:00", "Europe/Bratislava")
root.dur = "2h".parse_duration() / 1000000000
root.iso_dur = "PT2.5S".parse_duration_iso8601() / 1000000000
root.sub_iso = "2026-10-20T12:00:00Z".ts_sub_iso8601("P1D").ts_format("2006-01-02T15:04:05Z", "UTC")
"#,
        r#"{}"#,
    )
    .unwrap()
    .unwrap();
    let jv: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(jv["fmt1"], "2020-Aug-14 11:45:26");
    assert_eq!(jv["fmt2"], "2020-Aug-14 11:50:26.371");
    assert_eq!(jv["strftime"], "2020-Aug-14 11:50:26.371000");
    assert_eq!(jv["tz_offset"], "+02:00");
    assert_eq!(jv["dur"], 7200.0);
    assert_eq!(jv["iso_dur"], 2.5);
    assert_eq!(jv["sub_iso"], "2026-10-19T12:00:00Z");
}

#[test]
fn field_interpolation_and_imports_and_security_defaults() {
    // 1. Field interpolation with `${! ... }` and escaped `${{! ... }}`
    let env = Environment::new();
    let field = env
        .compile_field("prefix-${! this.topic }-${{!escaped}}")
        .unwrap();
    let parts = [SimplePartView::from_bytes(
        br#"{"topic":"events"}"#.to_vec(),
    )];
    let resolved = field.resolve_string(0, &parts).unwrap();
    assert_eq!(resolved, "prefix-events-${!escaped}");

    // 2. Host `file()` is OFF by default and fails at runtime unless enabled
    let file_exec = env
        .compile_mapping(r#"root = file("/etc/passwd")"#)
        .unwrap();
    assert!(
        file_exec.execute_bytes(b"{}").is_err(),
        "file() must be off by default"
    );

    // 3. `import` is OFF by default, works when MemoryImporter is provided
    assert!(env
        .compile_mapping(
            r#"
import "./shared.blobl"
root = this.apply("double")
"#
        )
        .is_err());

    let mut files = HashMap::new();
    files.insert(
        "shared.blobl".to_string(),
        b"map double {\n  root = this * 2\n}\n".to_vec(),
    );
    let env_with_import = Environment::new().with_importer(Arc::new(MemoryImporter { files }));
    let exec = env_with_import
        .compile_mapping(
            r#"
import "./shared.blobl"
root.val = this.n.apply("double")
"#,
        )
        .unwrap();
    let res = exec.execute_bytes(br#"{"n":21}"#).unwrap().unwrap();
    assert_eq!(res.to_json_value(), serde_json::json!({"val": 42}));

    // 4. EmptyBatch coverage
    let static_field = env.compile_field("static-only").unwrap();
    assert_eq!(
        static_field.resolve_string(0, &EmptyBatch).unwrap(),
        "static-only"
    );
}

#[test]
fn whole_floats_marshal_like_go_and_type_errors_name_their_source_last() {
    // Bento 1.22 config/test/bloblang/csv.yaml sums number() values into `[6,41,114]`.
    assert_eq!(
        run_map(
            r#"root = [["1","2","3"]].map_each(this.map_each(this.number(0)).sum())"#,
            "{}"
        ),
        Ok(Some("[6]".to_string()))
    );
    assert_eq!(
        serde_json::to_string(&Value::Float(-3.0).to_json_value()).unwrap(),
        "-3"
    );
    assert_eq!(
        serde_json::to_string(&Value::Float(2.5).to_json_value()).unwrap(),
        "2.5"
    );
    // Bento 1.22 config/test/bloblang/csv_formatter_test.yaml asserts this error text.
    assert_eq!(
        run_map("root = this.a.keys()", r#"{"a":null}"#),
        Err(
            "failed assignment (line 1): expected object value, got null from field `this.a`"
                .to_string()
        )
    );
}

#[test]
fn an_integral_float_serialises_without_a_fraction_as_go_does() {
    use ekiben_bloblang::Value;
    let text = |f: f64| serde_json::to_string(&Value::Float(f).to_json_value()).unwrap();
    assert_eq!(text(0.0), "0");
    assert_eq!(text(5.0), "5");
    assert_eq!(text(-12.0), "-12");
    assert_eq!(text(1.5), "1.5");
}

#[test]
fn values_serialize_as_go_encoding_json_does() {
    // Bento 1.22 output, recorded with .bento/bento (T-6623).
    let v = Environment::new()
        .compile_mapping(
            r#"root = {"f": 4.0, "g": 10000000000.0 * 100000000000.0, "h": 0.0000001, "i": 3.0 / 2.0, "b": 3 / 1, "s": "<&>\u2028", "t": "2020-01-01T00:00:00.120Z".ts_parse("2006-01-02T15:04:05Z07:00")}"#,
        )
        .unwrap()
        .execute_bytes(b"{}")
        .unwrap()
        .unwrap();
    assert_eq!(
        v.to_go_json_unescaped(),
        r#"{"b":3,"f":4,"g":1e+21,"h":1e-7,"i":1.5,"s":"<&>\u2028","t":"2020-01-01T00:00:00.12Z"}"#
    );
    assert_eq!(
        v.to_go_json(),
        r#"{"b":3,"f":4,"g":1e+21,"h":1e-7,"i":1.5,"s":"\u003c\u0026\u003e\u2028","t":"2020-01-01T00:00:00.12Z"}"#
    );
}

#[test]
fn a_json_number_parses_to_the_nearest_float() {
    // serde_json's default float parser is off by an ulp here; Bento prints ...051.
    assert_eq!(
        run_map("root = this.x", r#"{"x":14.395302776000051}"#).unwrap(),
        Some("14.395302776000051".to_string())
    );
}

#[test]
fn a_failed_assignment_names_the_line_bento_names() {
    // Bento counts newlines over the first <character offset> bytes, so a multibyte
    // character before a statement moves its line back.
    for (mapping, line) in [
        ("let a = \"é\"\nroot.zz = throw(\"x\")", 1),
        ("let a = \"é\"\n\nroot.zz = throw(\"x\")", 2),
        ("let a = \"€\"\nroot.b = 1\nroot.zz = throw(\"x\")", 2),
        ("root.a = 1\n# c\nroot.b = throw(\"x\")", 3),
    ] {
        let err = run_map(mapping, "{}").unwrap_err();
        assert!(
            err.contains(&format!("(line {line})")),
            "{mapping:?}: {err}"
        );
    }
}

#[test]
fn ts_format_and_ts_parse_take_the_hour_offset_layouts() {
    assert_eq!(
        run_map(
            r#"root = [1771628400.ts_format("-07", "Europe/Madrid"), 1771628400.ts_format("Z07", "UTC"), "2026-02-21 +01".ts_parse("2006-01-02 -07").ts_unix()]"#,
            "{}"
        )
        .unwrap(),
        Some(r#"["+01","Z",1771628400]"#.to_string())
    );
}

/// Runs `mapping` and prints its result as Bento writes a message: Go's JSON encoder.
fn go_map(mapping: &str, input_json: &str) -> Result<String, String> {
    let exec = Environment::new()
        .compile_mapping(mapping)
        .map_err(|e| e.to_string())?;
    let v = exec
        .execute_bytes(input_json.as_bytes())
        .map_err(|e| e.to_string())?
        .ok_or("no result")?;
    Ok(v.to_go_json())
}

// Every expected text below is what `bento blobl` 1.22.0 printed for the same mapping and input.

#[test]
fn message_numbers_keep_the_text_go_read() {
    let input = r#"{"a":5.0,"b":1E2,"c":12345678901234567890,"d":-0.0}"#;
    assert_eq!(go_map("root = this", input).unwrap(), input);
    assert_eq!(
        go_map("root = this.a.int64()", input)
            .unwrap_err()
            .rsplit(": ")
            .take(3)
            .collect::<Vec<_>>(),
        ["invalid syntax", "parsing \"5.0\"", "strconv.ParseInt"]
    );
}

#[test]
fn url_query_escaping_follows_go() {
    assert_eq!(
        go_map(r#"root = "a*b~c d/é".escape_url_query()"#, "{}").unwrap(),
        r#""a%2Ab~c+d%2F%C3%A9""#
    );
}

#[test]
fn regex_classes_are_ascii_as_in_re2() {
    assert_eq!(
        go_map(
            r#"root = [this.s.re_replace_all("\\s", "_"), this.s.re_match("^\\w+$")]"#,
            "{\"s\":\"a\u{a0}b čx\"}"
        )
        .unwrap(),
        "[\"a\u{a0}b_čx\",false]"
    );
}

#[test]
fn split_on_an_empty_delimiter_gives_characters() {
    assert_eq!(
        go_map(r#"root = "abč".split("")"#, "{}").unwrap(),
        r#"["a","b","č"]"#
    );
}

#[test]
fn parse_xml_follows_mxj() {
    let input =
        r#"{"x":"<r xmlns:x=\"u\" a=\"1\"><v> 2 </v><e/><b>true</b><n>NaN</n><t>a\r\nb</t></r>"}"#;
    assert_eq!(
        go_map("root = this.x.parse_xml(true)", input).unwrap(),
        r#"{"r":{"-a":1,"-x":"u","b":true,"e":"","n":"NaN","t":"a\nb","v":2}}"#
    );
}

#[test]
fn floats_print_as_go_formats_them() {
    assert_eq!(
        go_map("root = [0.1 + 0.2, 1e21, 123456789.125, 5e-324]", "{}").unwrap(),
        "[0.30000000000000004,1e+21,123456789.125,5e-324]"
    );
}

#[test]
fn sort_orders_equal_keys_as_go_pdqsort() {
    let m = r#"root = range(0,20).map_each(i -> {"k": [3,1,2,1,3,2,1,3,2,1,3,2,1,3,2,1,3,2,1,3].index(i), "i": i}).sort_by(e -> e.k).map_each(e -> e.i)"#;
    assert_eq!(
        go_map(m, "{}").unwrap(),
        "[9,1,18,3,15,12,6,5,8,11,14,17,2,7,10,0,13,4,16,19]"
    );
    let m = r#"root = range(0,70).map_each(i -> {"k": (i*13)%4, "i": i}).sort_by(e -> e.k.string()).map_each(e -> e.i)"#;
    assert_eq!(
        go_map(m, "{}").unwrap(),
        "[8,44,32,68,4,40,28,64,0,52,36,60,12,24,48,56,16,20,9,53,17,21,49,57,13,25,45,61,65,29,41,5,69,33,1,37,34,18,38,2,66,30,42,6,62,26,46,10,58,22,50,14,54,35,19,55,15,51,23,59,11,47,27,63,7,43,31,67,3,39]"
    );
}

#[test]
fn sort_errors_read_as_bento_writes_them() {
    for (m, want) in [
        (
            r#"root = [{"k":1},{"k":"x"}].sort_by(e -> e.k)"#,
            "sort_by element 0: expected string value, got number from field `e.k` (1)",
        ),
        (
            r#"root = [3,"a",1].sort()"#,
            "sort element 0: expected string value, got number (3)",
        ),
        (
            "root = [3,1,2].sort(p -> p.left)",
            "expected bool value, got number from sort argument (1)",
        ),
        (
            r#"root = [1, 2, "x"].join(",")"#,
            "array literal: failed to join element 0: expected string value, got number (1)",
        ),
    ] {
        let err = go_map(m, "{}").unwrap_err();
        assert!(err.ends_with(&format!("(line 1): {want}")), "{m}: {err}");
    }
}

#[test]
fn number_reads_a_string_as_go_parse_float() {
    for (m, want) in [
        (
            r#"root = " 5".number()"#,
            r#"string literal: strconv.ParseFloat: parsing " 5": invalid syntax"#,
        ),
        (
            r#"root = "1e400".number()"#,
            r#"string literal: strconv.ParseFloat: parsing "1e400": value out of range"#,
        ),
        (
            "root = this.b.number()",
            "expected number value, got bool from field `this.b` (true)",
        ),
        (
            "root = this.x.number()",
            "expected number value, got null from field `this.x`",
        ),
    ] {
        let err = go_map(m, r#"{"b":true}"#).unwrap_err();
        assert!(err.ends_with(&format!("(line 1): {want}")), "{m}: {err}");
    }
    assert_eq!(go_map(r#"root = "Inf".number()"#, "{}").unwrap(), "null");
}

#[test]
fn an_empty_path_argument_is_the_key_empty_never_the_root() {
    // A lookup table `.get(name)` with a name folded to "" read the whole table (bratislava/statistics).
    for (m, want) in [
        (r#"root = {"":5}.get("")"#, "5"),
        (r#"root = [1,2].get("")"#, "null"),
        (r#"root = {"a":1}.set("", 5)"#, r#"{"":5,"a":1}"#),
        (r#"root = {"a":1,"":2}.without("")"#, r#"{"a":1}"#),
        (r#"root = {"a":1}.exists("")"#, "false"),
        (r#"root = {"a":1,"":2}.with("")"#, r#"{"":2}"#),
    ] {
        assert_eq!(go_map(m, "{}").unwrap(), want, "{m}");
    }
}

#[test]
fn numbers_inside_arrays_and_objects_compare_as_numbers() {
    // praha/geo-delivery: a ring closes when its first and last positions are equal, 14.10 == 14.1.
    let input = r#"{"a":[[14.10,50.0],[1,2],[14.1,50]],"d":{"x":5.0}}"#;
    assert_eq!(
        go_map(
            r#"root = [this.a.index(0) == this.a.index(-1), this.d == {"x":5}, this.a.contains([14.1,50])]"#,
            input
        )
        .unwrap(),
        "[true,true,true]"
    );
}

#[test]
fn capitalize_breaks_words_as_x_text_title() {
    assert_eq!(
        go_map(
            r#"root = "poppeli/haapa mustikkamaa-korkeasaari o'neil x.y 1abc éva àb_cd mcDONALD a..b a:b straße ǆungla hello,world «q» a’b".capitalize()"#,
            "{}"
        )
        .unwrap(),
        r#""Poppeli/Haapa Mustikkamaa-Korkeasaari O'neil X.y 1Abc Éva Àb_cd Mcdonald A..B A:b Straße ǅungla Hello,World «Q» A’b""#
    );
}

#[test]
fn format_pads_as_go_fmt_and_joined_bytes_stay_raw() {
    // The GTFS pipelines build a zip in a mapping: "%04x" must pad and `+` must keep raw bytes.
    assert_eq!(
        go_map(
            r#"root = ["%04x".format(5), "%08x".format(255.0.int64()), "%5.1f|".format(2.25)]"#,
            "{}"
        )
        .unwrap(),
        r#"["0005","000000ff","  2.2|"]"#
    );
    assert_eq!(
        go_map(
            r#"root = ("504b".decode("hex") + "ff00".decode("hex")).encode("hex")"#,
            "{}"
        )
        .unwrap(),
        r#""504bff00""#
    );
}

#[test]
fn unescape_html_reads_every_reference_go_reads() {
    // Bento 1.22.0 output (Go html.UnescapeString): helsinki/nature writes `&deg;` as `°`.
    let m = r#"root = "a &deg; b &amp c &ampx &notit; &notin; &#x80; &#0; &#xZ; &#65 &; &zz; &AElig &lt;&gt; &nvlt; &#x110000; &".unescape_html()"#;
    assert_eq!(
        go_map(m, "{}").unwrap(),
        r#""a ° b \u0026 c \u0026x ¬it; ∉ € � \u0026#xZ; A \u0026; \u0026zz; Æ \u003c\u003e \u003c⃒ � \u0026""#
    );
}
