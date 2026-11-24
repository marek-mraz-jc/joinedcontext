//! Method and function edge cases (T-7666): bytes and string variants, wrong argument and target
//! types, size caps, every algorithm and encoding name, refusals and their messages. Each case is
//! a mapping, its input and the Go JSON result or a piece of the error a person reads.

use ekiben_bloblang::executor::SimplePartView;
use ekiben_bloblang::Environment;

/// The mapping's result on `input` as JSON text (no HTML escaping, unlike Go's), or the compile or
/// run error.
fn run(mapping: &str, input: &str) -> Result<String, String> {
    let exec = Environment::new()
        .compile_mapping(mapping)
        .map_err(|e| e.to_string())?;
    match exec.execute_bytes(input.as_bytes()) {
        Ok(Some(v)) => serde_json::to_string(&v.to_json_value()).map_err(|e| e.to_string()),
        Ok(None) => Ok("<deleted>".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Runs every case and reports all mismatches at once: `Ok` is the exact result, `Err` a piece
/// of the error.
fn check(cases: &[(&str, &str, Result<&str, &str>)]) {
    let mut bad = Vec::new();
    for (mapping, input, want) in cases {
        let got = run(mapping, input);
        let ok = match (want, &got) {
            (Ok(w), Ok(g)) => w == g,
            (Err(w), Err(g)) => g.contains(w),
            _ => false,
        };
        if !ok {
            bad.push(format!(
                "{mapping}\n    input {input}\n    want {want:?}\n    got  {got:?}"
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "{} case(s) differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

const NOT_STRING: &str = "expected string value, got number";

#[test]
fn string_methods_take_bytes_and_refuse_other_types() {
    check(&[
        (
            r#"root = "aBc".bytes().uppercase().string()"#,
            "{}",
            Ok(r#""ABC""#),
        ),
        (
            r#"root = "aBc".bytes().lowercase().string()"#,
            "{}",
            Ok(r#""abc""#),
        ),
        ("root = 5.lowercase()", "{}", Err(NOT_STRING)),
        (
            r#"root = "ab cd".bytes().capitalize().string()"#,
            "{}",
            Ok(r#""Ab Cd""#),
        ),
        ("root = 5.capitalize()", "{}", Err(NOT_STRING)),
        (
            r#"root = " a ".bytes().trim().string()"#,
            "{}",
            Ok(r#""a""#),
        ),
        (
            r#"root = "xxaxx".bytes().trim("x").string()"#,
            "{}",
            Ok(r#""a""#),
        ),
        ("root = 5.trim()", "{}", Err(NOT_STRING)),
        (r#"root = "ab".trim_prefix(5)"#, "{}", Err(NOT_STRING)),
        (
            r#"root = "ab".bytes().trim_prefix("a").string()"#,
            "{}",
            Ok(r#""b""#),
        ),
        (r#"root = 5.trim_prefix("a")"#, "{}", Err(NOT_STRING)),
        (r#"root = "ab".trim_suffix(5)"#, "{}", Err(NOT_STRING)),
        (
            r#"root = "ab".bytes().trim_suffix("b").string()"#,
            "{}",
            Ok(r#""a""#),
        ),
        (r#"root = 5.trim_suffix("a")"#, "{}", Err(NOT_STRING)),
        (r#"root = "ab".bytes().has_prefix("a")"#, "{}", Ok("true")),
        (r#"root = 5.has_prefix("a")"#, "{}", Err(NOT_STRING)),
        (r#"root = "ab".bytes().has_suffix("c")"#, "{}", Ok("false")),
        (r#"root = 5.has_suffix("a")"#, "{}", Err(NOT_STRING)),
        (r#"root = "abc".bytes().index_of("")"#, "{}", Ok("0")),
        (r#"root = "abc".bytes().index_of("c")"#, "{}", Ok("2")),
        (r#"root = 5.index_of("a")"#, "{}", Err(NOT_STRING)),
        (
            r#"root = "ab".repeat(40000000)"#,
            "{}",
            Err("repeat output exceeds maximum size cap (64 MiB)"),
        ),
        (
            r#"root = "ab".bytes().repeat(40000000)"#,
            "{}",
            Err("repeat output exceeds maximum size cap (64 MiB)"),
        ),
        (
            r#"root = "ab".bytes().repeat(2).string()"#,
            "{}",
            Ok(r#""abab""#),
        ),
        ("root = 5.repeat(2)", "{}", Err(NOT_STRING)),
        ("root = 5.reverse()", "{}", Err(NOT_STRING)),
        (
            r#"root = "aXa".bytes().replace_all("a", "b").string()"#,
            "{}",
            Ok(r#""bXb""#),
        ),
        (r#"root = 5.replace_all("a", "b")"#, "{}", Err(NOT_STRING)),
        (
            r#"root = "a".replace_all_many(["a"])"#,
            "{}",
            Err("replacements should be in pairs and must therefore be even"),
        ),
        (
            r#"root = "ab".bytes().replace_all_many(["a", "c"]).string()"#,
            "{}",
            Ok(r#""cb""#),
        ),
        (
            r#"root = "<b>x</b>".bytes().strip_html().string()"#,
            "{}",
            Ok(r#""x""#),
        ),
        (
            r#"root = "<b>x</b><i>y</i>".strip_html(["i"])"#,
            "{}",
            Ok(r#""x<i>y</i>""#),
        ),
        (
            r#"root = "<b>x</b>".strip_html([5])"#,
            "{}",
            Err(NOT_STRING),
        ),
        (
            r#"root = "\"a\\qb\"".unquote()"#,
            "{}",
            Err("invalid quoted string"),
        ),
    ]);
}

#[test]
fn regex_methods_refuse_a_non_text_pattern_and_fill_unmatched_groups() {
    check(&[
        ("root = \"a\".re_match(5)", "{}", Err(NOT_STRING)),
        ("root = \"a\".re_find_all(5)", "{}", Err(NOT_STRING)),
        (
            "root = \"a\".re_find_all_submatch(5)",
            "{}",
            Err(NOT_STRING),
        ),
        ("root = \"a\".re_find_object(5)", "{}", Err(NOT_STRING)),
        ("root = \"a\".re_find_all_object(5)", "{}", Err(NOT_STRING)),
        (
            r#"root = "b".re_find_object("(a)?(?P<n>b)")"#,
            "{}",
            Ok(r#"{"0":"b","1":"","n":"b"}"#),
        ),
        (
            r#"root = "a1 b".re_replace_all("(\\w)(\\d)?", "<$1|$2|$$|${1}|$x_1|$>")"#,
            "{}",
            Ok(r#""<a|1|$|a||$> <b||$|b||$>""#),
        ),
        (r#"root = "x".re_replace_all("x", "$")"#, "{}", Ok(r#""$""#)),
        (
            r#"root = "a1 b_".re_find_all("\\D\\S\\W?")"#,
            "{}",
            Ok(r#"["a1 ","b_"]"#),
        ),
        (
            r#"root = "a-b".re_find_all("[\\d\\w]\\b")"#,
            "{}",
            Ok(r#"["a","b"]"#),
        ),
        (r#"root = "ab".re_find_all("a\\B")"#, "{}", Ok(r#"["a"]"#)),
        (r#"root = "]a".re_find_all("[]a]+")"#, "{}", Ok(r#"["]a"]"#)),
        (r#"root = "a.b".re_find_all("\\.")"#, "{}", Ok(r#"["."]"#)),
        (r#"root = "a".re_find_all("a\\")"#, "{}", Err("")),
        (
            r#"root = "a b".re_find_all("[\\s\\S]")"#,
            "{}",
            Ok(r#"["a"," ","b"]"#),
        ),
    ]);
}

#[test]
fn encodings_hashes_and_compression_cover_every_name_and_refusal() {
    check(&[
        (r#"root = "a".encode(5)"#, "{}", Err(NOT_STRING)),
        (r#"root = 5.encode("hex")"#, "{}", Err(NOT_STRING)),
        (r#"root = "ÿ?".encode("base64url")"#, "{}", Ok(r#""w78_""#)),
        (
            r#"root = "ÿ?".encode("base64rawurl")"#,
            "{}",
            Ok(r#""w78_""#),
        ),
        (r#"root = "a".encode("ascii85")"#, "{}", Ok(r#""@/""#)),
        (
            r#"root = "\u0000\u0000\u0000\u0000".encode("ascii85")"#,
            "{}",
            Ok(r#""z""#),
        ),
        (r#"root = "a".decode(5)"#, "{}", Err(NOT_STRING)),
        (r#"root = "!!".decode("base64")"#, "{}", Err("Invalid")),
        (
            r#"root = "w78_".decode("base64url").encode("hex")"#,
            "{}",
            Ok(r#""c3bf3f""#),
        ),
        (r#"root = "!".decode("base64url")"#, "{}", Err("Invalid")),
        (
            r#"root = "w78_".decode("base64rawurl").encode("hex")"#,
            "{}",
            Ok(r#""c3bf3f""#),
        ),
        (r#"root = "!".decode("base64rawurl")"#, "{}", Err("Invalid")),
        (
            r#"root = "z @/".decode("ascii85").encode("hex")"#,
            "{}",
            Ok(r#""0000000061""#),
        ),
        (
            r#"root = "@".decode("ascii85")"#,
            "{}",
            Err("corrupt ascii85 input"),
        ),
        (
            r#"root = "a~".decode("ascii85")"#,
            "{}",
            Err("illegal ascii85 byte: 126"),
        ),
        (
            r#"root = "uuuuu".decode("ascii85")"#,
            "{}",
            Err("ascii85 overflow"),
        ),
        (
            r#"root = "uuu".decode("ascii85")"#,
            "{}",
            Err("ascii85 overflow"),
        ),
        (r#"root = "a".hash(5)"#, "{}", Err(NOT_STRING)),
        (r#"root = "a".hash("md5", 5)"#, "{}", Err(NOT_STRING)),
        (r#"root = "a".hash("crc32", "", 5)"#, "{}", Err(NOT_STRING)),
        (r#"root = 5.hash("md5")"#, "{}", Err(NOT_STRING)),
        (
            r#"root = "a".bytes().hash("md5").encode("hex")"#,
            "{}",
            Ok(r#""0cc175b9c0f1b6a831c399e269772661""#),
        ),
        (
            r#"root = "a".hash("sha256").encode("hex")"#,
            "{}",
            Ok(r#""ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb""#),
        ),
        (
            r#"root = "a".hash("sha512").encode("hex").slice(0, 16)"#,
            "{}",
            Ok(r#""1f40fc92da241694""#),
        ),
        (
            r#"root = "a".hash("hmac_sha1")"#,
            "{}",
            Err("hash algorithm hmac_sha1 requires a key argument"),
        ),
        (
            r#"root = "a".hash("hmac_sha256", "")"#,
            "{}",
            Err("hash algorithm hmac_sha256 requires a key argument"),
        ),
        (
            r#"root = "a".hash("hmac-sha512")"#,
            "{}",
            Err("hash algorithm hmac-sha512 requires a key argument"),
        ),
        (
            r#"root = "a".hash("hmac_sha512", "k").encode("hex").slice(0, 16)"#,
            "{}",
            Ok(r#""82e0c15e0d62ab17""#),
        ),
        (
            r#"root = "a".hash("crc32", "", "IEEE").encode("hex")"#,
            "{}",
            Ok(r#""e8b7be43""#),
        ),
        (
            r#"root = "a".hash("crc32", "", "Other")"#,
            "{}",
            Err("unsupported crc32 polynomial Other"),
        ),
        (r#"root = "a".compress(5)"#, "{}", Err(NOT_STRING)),
        (
            r#"root = "abc".compress("gzip").decompress("gzip").string()"#,
            "{}",
            Ok(r#""abc""#),
        ),
        (
            r#"root = "abc".compress("zlib").decompress("zlib").string()"#,
            "{}",
            Ok(r#""abc""#),
        ),
        (
            r#"root = "abc".compress("flate").decompress("flate").string()"#,
            "{}",
            Ok(r#""abc""#),
        ),
        (r#"root = "a".decompress(5)"#, "{}", Err(NOT_STRING)),
        (r#"root = "abc".decompress("gzip")"#, "{}", Err("")),
        (r#"root = "abc".decompress("zlib")"#, "{}", Err("")),
        (r#"root = "ÿÿ".bytes().decompress("flate")"#, "{}", Err("")),
        (r#"root = "abc".decompress("lz4")"#, "{}", Err("")),
    ]);
}

#[test]
fn a_decompression_past_64_mib_is_refused() {
    // 65 MiB of zeros compress to a few hundred KiB; the reader stops one byte past the cap.
    let exec = Environment::new()
        .compile_mapping(r#"root = content().decompress("lz4")"#)
        .expect("the mapping compiles");
    let zeros = vec![0u8; 65 * 1024 * 1024];
    let packed = Environment::new()
        .compile_mapping(r#"root = content().compress("lz4")"#)
        .expect("the mapping compiles")
        .exec_on_value(ekiben_bloblang::Value::Bytes(zeros))
        .expect("compresses");
    let ekiben_bloblang::Value::Bytes(packed) = packed else {
        panic!("compress returns bytes");
    };
    let e = exec
        .exec_on_value(ekiben_bloblang::Value::Bytes(packed))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("decompressed data exceeds safety limit (64 MiB)"),
        "{e}"
    );
}

#[test]
fn structured_text_formats_refuse_what_they_cannot_read_or_write() {
    check(&[
        (
            "root = this.format_json().string()",
            r#"{"a":1}"#,
            Ok(r#""{\n    \"a\": 1\n}""#),
        ),
        ("root = (0.0 / 0.0).format_json()", "{}", Err("")),
        (
            r#"root = "a: [".parse_yaml()"#,
            "{}",
            Err("failed to parse value as YAML"),
        ),
        (
            r#"root = "a,b".parse_csv(delimiter: "")"#,
            "{}",
            Err("delimiter value must be exactly one character"),
        ),
        (
            r#"root = "a,b".parse_csv(delimiter: "é")"#,
            "{}",
            Err("delimiter value must be a single ASCII character"),
        ),
        (
            r#"root = "".parse_csv()"#,
            "{}",
            Err("zero records were parsed"),
        ),
        (
            "root = \"a,b\\n1\".parse_csv(lazy_quotes: true)",
            "{}",
            Err("record on line 0: record mismatch with headers"),
        ),
        (r#"root = "mailto:x@y".parse_url().host"#, "{}", Ok(r#""""#)),
        (
            r#"root = "http://u@h/".parse_url().user"#,
            "{}",
            Ok(r#"{"name":"u"}"#),
        ),
        (
            r#"root = 5.filepath_join()"#,
            "{}",
            Err("expected array value, got number"),
        ),
        (
            r#"root = ["a", 5].filepath_join()"#,
            "{}",
            Err("path element 1:"),
        ),
        (
            r#"root = "a%zz".unescape_url_query()"#,
            "{}",
            Err("invalid URL escape"),
        ),
    ]);
}

#[test]
fn xml_parsing_and_formatting_cover_entities_cdata_attributes_and_arrays() {
    check(&[
        (
            r#"root = "<!DOCTYPE x><a/>".parse_xml()"#,
            "{}",
            Err("DTD and external entities are forbidden"),
        ),
        (
            r#"root = "<a>".bytes().parse_xml()"#,
            "{}",
            Err("failed to parse value as XML"),
        ),
        (
            r#"root = "<a>x &amp; &nope; y</a>".parse_xml()"#,
            "{}",
            Ok(r#"{"a":"x & &nope; y"}"#),
        ),
        (
            r#"root = "<a><![CDATA[<z>]]></a>".parse_xml()"#,
            "{}",
            Ok(r#"{"a":"<z>"}"#),
        ),
        (
            r##"root = "<a k=\"&bad;\" n=1>t</a>".parse_xml()"##,
            "{}",
            Ok(r##"{"a":{"#text":"t","-k":"&bad;","-n":"1"}}"##),
        ),
        (
            r##"root = {"r":{"#text":"<&>\"'"}}.format_xml().string()"##,
            "{}",
            Ok(r##""<r>&lt;&amp;&gt;&#34;&#39;</r>""##),
        ),
        (
            r#"root = {"r":{"-a":"1"}}.format_xml().string()"#,
            "{}",
            Ok(r#""<r></r>""#),
        ),
        (
            r#"root = {"r":{"i":[1,{"j":2}]}}.format_xml("  ").string()"#,
            "{}",
            Ok(r#""<r>\n  <i>1</i>\n  <i>\n    <j>2</j>\n  </i>\n</r>""#),
        ),
        (
            r#"root = {"a":1,"b":{"c":2}}.format_xml().string()"#,
            "{}",
            Ok(r#""<a>1</a>\n<b>\n    <c>2</c>\n</b>""#),
        ),
    ]);
}

#[test]
fn title_case_html_entities_and_slugs_cover_the_special_letters() {
    check(&[
        (
            r#"root = "ΣΑΣ σας".lowercase().capitalize()"#,
            "{}",
            Ok(r#""Σας Σας""#),
        ),
        (
            r#"root = "ǆ ǉ ǌ ǳ ß ﬀ ﬁ ﬂ".capitalize()"#,
            "{}",
            Ok(r#""ǅ ǈ ǋ ǲ Ss Ff Fi Fl""#),
        ),
        (
            "root = \"a\u{feff}b \u{20000}c\".capitalize()",
            "{}",
            Ok("\"A\u{feff}b \u{20000}C\""),
        ),
        (
            r#"root = "&#x41;&#X42;&#67;&#xZZ;&amp &".unescape_html()"#,
            "{}",
            Ok(r#""ABC&#xZZ;& &""#),
        ),
        (r#"root = "a<b c".strip_html()"#, "{}", Ok(r#""a&lt;b c""#)),
        (
            r#"root = "x & y @ z".slug("cs")"#,
            "{}",
            Ok(r#""x-a-y-zavinac-z""#),
        ),
        (
            r#"root = "x & y @ z".slug("de")"#,
            "{}",
            Ok(r#""x-und-y-an-z""#),
        ),
        (
            r#"root = "x & y @ z".slug("es")"#,
            "{}",
            Ok(r#""x-y-y-en-z""#),
        ),
        (
            r#"root = "x & y @ z".slug("fi")"#,
            "{}",
            Ok(r#""x-ja-y-at-z""#),
        ),
        (
            r#"root = "x & y @ z".slug("el")"#,
            "{}",
            Ok(r#""x-kai-y-z""#),
        ),
        (r#"root = "x & y".slug("id")"#, "{}", Ok(r#""x-dan-y""#)),
        (
            r#"root = "x & y @ z".slug("it")"#,
            "{}",
            Ok(r#""x-e-y-chiocciola-z""#),
        ),
        (r#"root = "x & y".slug("kk")"#, "{}", Ok(r#""x-jane-y""#)),
        (
            r#"root = "x & y @ z".slug("nb")"#,
            "{}",
            Ok(r#""x-og-y-at-z""#),
        ),
        (
            r#"root = "x & y @ z".slug("nl")"#,
            "{}",
            Ok(r#""x-en-y-at-z""#),
        ),
        (
            r#"root = "x & y @ z".slug("pl")"#,
            "{}",
            Ok(r#""x-i-y-na-z""#),
        ),
        (r#"root = "x & y".slug("ro")"#, "{}", Ok(r#""x-si-y""#)),
        (r#"root = "x & y".slug("sl")"#, "{}", Ok(r#""x-in-y""#)),
        (
            r#"root = "x & y @ z".slug("sv")"#,
            "{}",
            Ok(r#""x-och-y-snabel-a-z""#),
        ),
        (
            r#"root = "x & y @ z".slug("tr")"#,
            "{}",
            Ok(r#""x-ve-y-et-z""#),
        ),
        (r#"root = "Ä–Ü".slug("de")"#, "{}", Ok(r#""ae-ue""#)),
        (
            r#"root = "ĄĆĤĲĴĸŊŘßŢŴŸ€".slug()"#,
            "{}",
            Ok(r#""achijjkngrsstwy""#),
        ),
    ]);
}

#[test]
fn number_methods_cover_each_numeric_variant_and_refusal() {
    check(&[
        ("root = 1e300.ceil()", "{}", Ok("1e+300")),
        (
            r#"root = "18446744073709551615".uint64().ceil()"#,
            "{}",
            Ok("18446744073709551615"),
        ),
        ("root = 5.ceil()", "{}", Ok("5")),
        (
            r#"root = "a".ceil()"#,
            "{}",
            Err("expected number value, got string"),
        ),
        ("root = -1e300.floor()", "{}", Ok("-1e+300")),
        (
            r#"root = "18446744073709551615".uint64().floor()"#,
            "{}",
            Ok("18446744073709551615"),
        ),
        ("root = 5.floor()", "{}", Ok("5")),
        (
            r#"root = "a".floor()"#,
            "{}",
            Err("expected number value, got string"),
        ),
        ("root = 1e300.round()", "{}", Ok("1e+300")),
        (
            r#"root = "18446744073709551615".uint64().round()"#,
            "{}",
            Ok("18446744073709551615"),
        ),
        ("root = 5.round()", "{}", Ok("5")),
        (
            r#"root = "a".round()"#,
            "{}",
            Err("expected number value, got string"),
        ),
        (
            "root = this.i.abs()",
            r#"{"i":-9223372036854775808}"#,
            Ok("9223372036854775807"),
        ),
        (
            r#"root = "18446744073709551615".uint64().abs()"#,
            "{}",
            Ok("18446744073709551615"),
        ),
        (
            r#"root = "a".abs()"#,
            "{}",
            Err("expected number value, got string"),
        ),
        (
            "root = 5.max()",
            "{}",
            Err("expected array value, got number"),
        ),
        ("root = [].max()", "{}", Err("the array was empty")),
        (r#"root = [1, "a"].max()"#, "{}", Err("index 1 of array:")),
        (
            "root = 5.min()",
            "{}",
            Err("expected array value, got number"),
        ),
        ("root = [].min()", "{}", Err("the array was empty")),
        (r#"root = [1, "a"].min()"#, "{}", Err("index 1 of array:")),
        ("root = 2.5.sum()", "{}", Ok("2.5")),
        (
            r#"root = "a".sum()"#,
            "{}",
            Err("expected array value, got string"),
        ),
        (
            r#"root = "\u0001".parse_big_decimal(-1)"#,
            "{}",
            Err("scale must be between 0 and 16383, got -1"),
        ),
        (
            "root = 5.parse_big_decimal(0)",
            "{}",
            Err("expected bytes value, got number"),
        ),
        (
            r#"root = "".parse_big_decimal(0)"#,
            "{}",
            Err("cannot parse empty bytes as big decimal"),
        ),
        (
            r#"root = "12345678901234567".parse_big_decimal(0)"#,
            "{}",
            Err("big decimal exceeds 128-bit precision"),
        ),
        (
            r#"root = "\u0001".parse_big_decimal(0)"#,
            "{}",
            Ok(r#""1""#),
        ),
        (
            r#"root = "\u0001".bytes().parse_big_decimal(40)"#,
            "{}",
            Ok(r#""0.0000000000000000000000000000000000000001""#),
        ),
        (
            r#"root = "kb".parse_bytes()"#,
            "{}",
            Err("failed to parse bytes 'kb'"),
        ),
        (
            r#"root = "1.2.3kb".parse_bytes()"#,
            "{}",
            Err("failed to parse bytes '1.2.3kb'"),
        ),
        (
            r#"root = ["2k", "2KiB", "1 GiB", "1t", "1Ti"].map_each(s -> s.parse_bytes())"#,
            "{}",
            Ok("[2000,2048,1073741824,1000000000000,1099511627776]"),
        ),
    ]);
}

#[test]
fn general_methods_cover_batches_defaults_and_refusals() {
    check(&[
        ("root = [1].array()", "{}", Ok("[1]")),
        (
            "root = 5.not()",
            "{}",
            Err("expected bool value, got number"),
        ),
        (
            "root = 5.not_empty()",
            "{}",
            Err("expected string, array or object value, got number"),
        ),
        (r#"root = "x".bool(true)"#, "{}", Ok("true")),
        (
            r#"root = "x".bool()"#,
            "{}",
            Err("string literal: failed to parse 'x' as bool"),
        ),
        (r#"root = throw("e").bool(false)"#, "{}", Ok("false")),
        (r#"root = throw("e").bool()"#, "{}", Err("e")),
        (r#"root = throw("e").number(2)"#, "{}", Ok("2")),
        (r#"root = throw("e").number()"#, "{}", Err("e")),
        (r#"root = "x".number(3)"#, "{}", Ok("3")),
        (r#"root = this.a.or("d")"#, r#"{"a":null}"#, Ok(r#""d""#)),
        (
            r#"root = env("NO_SUCH_VAR_T7666").or("d")"#,
            "{}",
            Ok(r#""d""#),
        ),
        (
            r#"root = "x".bloblang("root = this.uppercase()")"#,
            "{}",
            Ok(r#""X""#),
        ),
        (r#"root = "x".bloblang("root = ")"#, "{}", Err("")),
        (
            r#"root = "x".bloblang("root = throw(\"no\")")"#,
            "{}",
            Err("no"),
        ),
    ]);
    // At the deepest map the stack error passes catch and or untouched; one level up it is a
    // failed assignment like any other, which they handle.
    for method in ["catch", "or"] {
        let m = format!(
            "map inf {{ root = this.apply(\"inf\").{method}(\"c\") }}\nroot = this.apply(\"inf\")"
        );
        assert_eq!(run(&m, "{}"), Ok(r#""c""#.into()), "{method}");
    }
}

#[test]
fn from_and_from_all_read_other_parts_of_the_batch() -> Result<(), Box<dyn std::error::Error>> {
    let exec = Environment::new().compile_mapping(
        "root.one = this.n.from(1)\nroot.all = this.n.from_all()\nroot.me = this.n",
    )?;
    let parts = [
        SimplePartView::from_bytes(r#"{"n":1}"#),
        SimplePartView::from_bytes(r#"{"n":2}"#),
    ];
    let mut meta = std::collections::BTreeMap::new();
    let out = exec
        .map_part(0, &parts, &mut meta, None)?
        .ok_or("a result")?;
    assert_eq!(out.to_go_json(), r#"{"all":[1,2],"me":1,"one":2}"#);
    let bad = Environment::new().compile_mapping("root = this.n.not_null().from_all()")?;
    let parts = [
        SimplePartView::from_bytes(r#"{"n":1}"#),
        SimplePartView::from_bytes("{}"),
    ];
    assert!(bad.map_part(0, &parts, &mut meta, None).is_err());
    Ok(())
}

#[test]
fn structured_methods_refuse_wrong_types_and_cover_bytes_and_bounds() {
    check(&[
        (r#"root = "ab".bytes().length()"#, "{}", Ok("2")),
        (
            "root = 5.length()",
            "{}",
            Err("expected string, array or object value, got number"),
        ),
        (
            "root = [1,2].index(5)",
            "{}",
            Err("index '5' was out of bounds for array size: 2"),
        ),
        (
            "root = [1,2].index(-3)",
            "{}",
            Err("index '-1' was out of bounds for array size: 2"),
        ),
        (
            "root = 5.index(0)",
            "{}",
            Err("expected array value, got number"),
        ),
        (
            "root = [1,2,3].slice(2, 1)",
            "{}",
            Err("lower slice bound 2 must be lower than upper (1)"),
        ),
        (
            "root = [1,2,3].slice(this.l, this.h)",
            r#"{"l":5,"h":-1}"#,
            Err("must be lower than or equal to upper bound"),
        ),
        (
            r#"root = "abc".bytes().slice(1).string()"#,
            "{}",
            Ok(r#""bc""#),
        ),
        (
            "root = 5.slice(1)",
            "{}",
            Err("expected array or string value, got number"),
        ),
        (r#"root = "abc".bytes().contains("")"#, "{}", Ok("true")),
        (r#"root = "abc".bytes().contains("bc")"#, "{}", Ok("true")),
        (r#"root = {"a":1}.contains(1)"#, "{}", Ok("true")),
        (
            "root = 5.contains(1)",
            "{}",
            Err("expected string, array or object value, got number"),
        ),
        (
            "root = 5.values()",
            "{}",
            Err("expected object value, got number"),
        ),
        (
            "root = 5.key_values()",
            "{}",
            Err("expected object value, got number"),
        ),
        (
            "root = 5.enumerated()",
            "{}",
            Err("expected array value, got number"),
        ),
        (
            "root = 5.append(1)",
            "{}",
            Err("expected array value, got number"),
        ),
        (
            "root = 5.concat([1])",
            "{}",
            Err("expected array value, got number"),
        ),
        (
            "root = [1].concat(5)",
            "{}",
            Err("expected array value, got number"),
        ),
        (
            "root = 5.flatten()",
            "{}",
            Err("expected array value, got number"),
        ),
        (
            r#"root = "a,b".bytes().split(",").map_each(p -> p.string())"#,
            "{}",
            Ok(r#"["a","b"]"#),
        ),
        (r#"root = [].split(1)"#, "{}", Ok("[[]]")),
        (
            r#"root = 5.split(",")"#,
            "{}",
            Err("expected string or array value, got number"),
        ),
        (r#"root = "".split_by(c -> c == ",")"#, "{}", Ok(r#"[""]"#)),
        (r#"root = [].split_by(x -> x == 1)"#, "{}", Ok("[[]]")),
        (
            "root = 5.split_by(x -> true)",
            "{}",
            Err("expected string or array value, got number"),
        ),
        (
            r#"root = [1, "a"].map_each(x -> x + 1)"#,
            "{}",
            Err("failed to process element 1"),
        ),
        ("root = [1, 2].map_each(x -> nothing())", "{}", Ok("[1,2]")),
        (
            r#"root = {"a":1,"b":"x"}.map_each(kv -> kv.value + 1)"#,
            "{}",
            Err("failed to process element b"),
        ),
        (
            r#"root = {"a":1,"b":2}.map_each(kv -> if kv.key == "a" { deleted() } else { kv.value })"#,
            "{}",
            Ok(r#"{"b":2}"#),
        ),
        (
            r#"root = {"a":1}.map_each_key(k -> 5)"#,
            "{}",
            Err("unexpected result from key mapping: expected string value, got number"),
        ),
        ("root = [].all(x -> x > 0)", "{}", Ok("false")),
        (r#"root = [1].all(x -> x.uppercase())"#, "{}", Err("")),
        (r#"root = [1, "a"].all(x -> x > 0)"#, "{}", Err("element 1")),
        ("root = [].any(x -> x > 0)", "{}", Ok("false")),
        (r#"root = ["a"].any(x -> x > 0)"#, "{}", Err("element 0")),
        (
            "root = 5.squash()",
            "{}",
            Err("expected array value, got number"),
        ),
        (
            r#"root = 5.without("a")"#,
            "{}",
            Err("expected object value, got number"),
        ),
        (r#"root = {"a":1}.without(5)"#, "{}", Err("argument 0")),
        (
            r#"root = 5.with("a")"#,
            "{}",
            Err("expected object value, got number"),
        ),
        (r#"root = {"a":1}.with(5)"#, "{}", Err("argument 0")),
        (
            r#"root = {"a":{"b":1,"c":2}}.with("a.b")"#,
            "{}",
            Ok(r#"{"a":{"b":1}}"#),
        ),
        (
            r#"root = {"a":{"b":1,"c":2}}.without("a.b")"#,
            "{}",
            Ok(r#"{"a":{"c":2}}"#),
        ),
        (
            r#"root = [{"k":1},{"k":1},{"k":2}].unique(x -> x.k)"#,
            "{}",
            Ok(r#"[{"k":1},{"k":2}]"#),
        ),
        (
            r#"root = [1, "a"].unique(x -> x.uppercase())"#,
            "{}",
            Err("index 0"),
        ),
        (
            r#"root = ["a".bytes(), "a".bytes(), 1.5, 1.5].unique().length()"#,
            "{}",
            Ok("2"),
        ),
        (
            r#"root = [true].unique()"#,
            "{}",
            Err("index 0: expected string or number value, got bool"),
        ),
        (r#"root = [2, 1].sort(x -> x.uppercase())"#, "{}", Err("")),
        (
            r#"root = ["b".bytes(), "a".bytes()].sort().map_each(x -> x.string())"#,
            "{}",
            Ok(r#"["a","b"]"#),
        ),
        (r#"root = ["b", 1].sort()"#, "{}", Err("")),
        (r#"root = [true, false].sort()"#, "{}", Err("")),
        (
            r#"root = [{"v":2},{"v":1}].sort_by(x -> x.uppercase())"#,
            "{}",
            Err(""),
        ),
        (
            r#"root = [1, 2].find_by(x -> x.uppercase())"#,
            "{}",
            Err("query returned an error for index 0"),
        ),
        (
            r#"root = [1, 2].find_by(x -> 5)"#,
            "{}",
            Err("query returned a non-boolean value for index 0"),
        ),
        (r#"root = [1, 2].find_by(x -> x > 5)"#, "{}", Ok("-1")),
        (
            r#"root = [1, 2].find_all_by(x -> x.uppercase())"#,
            "{}",
            Err("query returned an error for index 0"),
        ),
        (
            r#"root = [1, 2].find_all_by(x -> 5)"#,
            "{}",
            Err("query returned a non-boolean value for index 0"),
        ),
        (
            r#"root = {"a":5}.explode("a")"#,
            "{}",
            Err("expected array or object value at path 'a', found: number"),
        ),
        (
            "root = 5.zip([1])",
            "{}",
            Err("expected array value, got number"),
        ),
        (
            "root = [1].zip(5)",
            "{}",
            Err("expected array value, got number"),
        ),
        (
            "root = [1].zip([1, 2])",
            "{}",
            Err("can't zip different length array values"),
        ),
        (r#"root = [1].merge([2])"#, "{}", Ok("[1,2]")),
        (r#"root = [1].merge(2)"#, "{}", Ok("[1,2]")),
        (
            r#"root = {"a":{"x":1},"b":[1],"c":[1],"d":1,"e":1}.merge({"a":{"y":2},"b":[2],"c":2,"d":[2],"e":2})"#,
            "{}",
            Ok(r#"{"a":{"x":1,"y":2},"b":[1,2],"c":[1,2],"d":[1,2],"e":[1,2]}"#),
        ),
        (r#"root = {"a":1}.merge(5)"#, "{}", Ok(r#"{"a":1}"#)),
        (
            r#"root = 5.merge({})"#,
            "{}",
            Err("expected object or array value, got number"),
        ),
        (r#"root = [1].assign([2])"#, "{}", Ok("[1,2]")),
        (r#"root = [1].assign(2)"#, "{}", Ok("[1,2]")),
        (
            r#"root = {"a":{"x":1},"b":1}.assign({"a":{"y":2},"b":2})"#,
            "{}",
            Ok(r#"{"a":{"x":1,"y":2},"b":2}"#),
        ),
        (r#"root = {"a":1}.assign(5)"#, "{}", Ok(r#"{"a":1}"#)),
        (
            r#"root = 5.assign({})"#,
            "{}",
            Err("expected object or array value, got number"),
        ),
        (
            r#"root = [[1, [2]], {"a": []}].collapse()"#,
            "{}",
            Ok(r#"{"0.0":1,"0.1.0":2}"#),
        ),
    ]);
}

#[test]
fn functions_cover_clock_ids_ranges_counters_and_their_refusals() {
    check(&[
        ("root = nothing()", r#"{"a":1}"#, Ok("null")),
        ("root = [batch_index(), batch_size()]", "{}", Ok("[0,1]")),
        ("root = [errored(), error(), error_source_label(), error_source_path(), error_source_type()]", "{}", Ok("[false,null,null,null,null]")),
        ("root = now().length() > 0", "{}", Ok("true")),
        ("root = [timestamp_unix(), timestamp_unix_milli(), timestamp_unix_micro(), timestamp_unix_nano()].all(t -> t > 0)", "{}", Ok("true")),
        ("root = uuid_v4().length()", "{}", Ok("36")),
        ("root = hostname().length() > 0", "{}", Ok("true")),
        ("root = flow_id()", "{}", Ok(r#""""#)),
        (r#"meta a = "x"
meta e = ""
root = [meta(), meta("a"), meta("e"), meta("z"), root_meta(), root_meta("a"), root_meta("e"), root_meta("z")]"#, "{}", Ok(r#"[{},null,null,null,{"a":"x","e":""},"x",null,null]"#)),
        (r#"root = range(start: 0, stop: 3)"#, "{}", Ok("[0,1,2]")),
        (r#"root = range(stop: 3)"#, "{}", Err("missing parameter: start")),
        (r#"root = range(start: 3)"#, "{}", Err("missing parameter: stop")),
        ("root = range(0, 3, 0)", "{}", Err("step must be greater than or less than 0")),
        ("root = range(0, 3, -1)", "{}", Err("with negative step arg stop (3) must be <= start (0)")),
        ("root = range(3, 0)", "{}", Err("with positive step arg start (3) must be < stop (0)")),
        ("root = range(0, 2000000)", "{}", Err("range size 2000000 exceeds maximum allowed (1000000)")),
        ("root = [counter(5, 6), counter(5, 6), counter(5, 6)]", "{}", Ok("[5,5,5]")),
        ("root = counter(min: 2, max: 3)", "{}", Ok("2")),
        ("root = counter(min: 2, set: deleted())", "{}", Ok("2")),
        ("root = counter(1, 9, 4)", "{}", Ok("4")),
        ("root = random_int(min: 5, max: 1)", "{}", Err("min (5) must be smaller or equal than max (1)")),
        ("root = random_int(min: -1)", "{}", Err("min (-1) must be a positive number")),
        ("root = random_int(max: 9223372036854775807)", "{}", Err("max must be smaller than the max allowed for an int64")),
        (r#"root = nanoid(4, "a").length()"#, "{}", Ok("4")),
        (r#"root = nanoid(length: 3, alphabet: "b")"#, "{}", Ok(r#""bbb""#)),
        (r#"root = nanoid(4, "")"#, "{}", Err("nanoid alphabet cannot be empty")),
        ("root = cosine_similarity(a: [1, 0], b: [1, 0])", "{}", Ok("1")),
        ("root = cosine_similarity(b: [1])", "{}", Err("missing parameter: a")),
        ("root = cosine_similarity(a: [1])", "{}", Err("missing parameter: b")),
        ("root = cosine_similarity([1], [1, 2])", "{}", Err("vectors must be equal length")),
        ("root = cosine_similarity([], [])", "{}", Err("vectors cannot be empty")),
        (r#"root = cosine_similarity(["a"], [1])"#, "{}", Err("vector must be an array of numeric types")),
        (r#"root = cosine_similarity([1], ["a"])"#, "{}", Err("vector must be an array of numeric types")),
        ("root = cosine_similarity([0], [0])", "{}", Err("vectors must not be null (all zeros)")),
        (r#"root = ulid("crockford", "other")"#, "{}", Err("invalid ulid random source: other, expected secure_random or fast_random")),
        ("root = [snowflake_id(), snowflake_id()].unique().length()", "{}", Ok("2")),
    ]);
}

#[test]
fn error_functions_read_the_part_error_of_the_batch() -> Result<(), Box<dyn std::error::Error>> {
    let exec = Environment::new().compile_mapping(
        "root = [errored(), error(), error_source_label(), error_source_path(), error_source_type(), meta(), meta(\"a\"), meta(\"e\"), meta(\"z\")]",
    )?;
    let mut part = SimplePartView::from_bytes("{}");
    part.meta
        .insert("a".into(), ekiben_bloblang::Value::String("x".into()));
    part.meta
        .insert("e".into(), ekiben_bloblang::Value::String(String::new()));
    part.error = Some(ekiben_bloblang::executor::DetailedErrorInfo {
        message: "m".into(),
        label: "l".into(),
        component_type: "t".into(),
        path: "p".into(),
    });
    let mut meta = std::collections::BTreeMap::new();
    let out = exec
        .map_part(0, &[part], &mut meta, None)?
        .ok_or("a result")?;
    assert_eq!(
        out.to_go_json(),
        r#"[true,"m","l","p","t",{"a":"x","e":""},"x",null,null]"#
    );
    Ok(())
}

#[test]
fn a_frozen_clock_stops_now_and_the_unix_functions_once() -> Result<(), Box<dyn std::error::Error>>
{
    let at = jiff::Timestamp::from_second(1_700_000_000)?;
    assert!(ekiben_bloblang::functions::freeze_clock(at));
    assert!(
        !ekiben_bloblang::functions::freeze_clock(at),
        "the clock freezes once"
    );
    assert_eq!(
        run("root = timestamp_unix()", "{}"),
        Ok("1700000000".into())
    );
    assert_eq!(
        run("root = now()", "{}"),
        Ok(r#""2023-11-14T22:13:20Z""#.into())
    );
    Ok(())
}

#[test]
fn the_remaining_branches_of_methods_and_functions() {
    check(&[
        (
            r#"root = env("NO_SUCH_VAR_T7666").uppercase()"#,
            "{}",
            Err("expected string value, got null"),
        ),
        ("root = this.l.max()", r#"{"l":[1,2.5]}"#, Ok("2.5")),
        (r#"root = "true".bool()"#, "{}", Ok("true")),
        ("root = true.not()", "{}", Ok("false")),
        (r#"root = [1, "a"].sum()"#, "{}", Err("index 1:")),
        (
            r#"root = "ab".bytes().index(5)"#,
            "{}",
            Err("index '5' was out of bounds for array size: 2"),
        ),
        (
            "root = [1,2,3].slice(this.l, this.h)",
            r#"{"l":2,"h":1}"#,
            Err("lower slice bound 2 must be lower than upper (1)"),
        ),
        ("root = [1,2].slice(5)", "{}", Ok("[]")),
        (
            r#"root = {"a":1}.map_each(kv -> nothing())"#,
            "{}",
            Ok(r#"{"a":1}"#),
        ),
        (
            "root = [1].all(x -> 5)",
            "{}",
            Err("element 0: expected bool value"),
        ),
        (
            "root = [1].any(x -> 5)",
            "{}",
            Err("element 0: expected bool value"),
        ),
        (
            "root = [1].all(x -> x.uppercase())",
            "{}",
            Err("element 0:"),
        ),
        (
            "root = [1].any(x -> x.uppercase())",
            "{}",
            Err("element 0:"),
        ),
        (r#"root = {"a":1}.without("a.b")"#, "{}", Ok(r#"{"a":1}"#)),
        (r#"root = {"a":1}.with("a.b")"#, "{}", Ok(r#"{"a":1}"#)),
        (
            "root = [1].zip()",
            "{}",
            Err("zip requires at least one argument"),
        ),
        (r#"root = [1, "b"].sort()"#, "{}", Err("")),
        (
            r#"root = "NaN".number().format_json()"#,
            "{}",
            Err("json: unsupported value: NaN or Inf"),
        ),
        (r#"root = "ΑΣΑ ΑΣ".capitalize()"#, "{}", Ok(r#""Ασα Ας""#)),
        (
            r#"root = "&#1 &#x4a;&#X4A; &qqq x".unescape_html()"#,
            "{}",
            Ok(r#""&#1 JJ &qqq x""#),
        ),
        (r#"root = "x< y".strip_html()"#, "{}", Ok(r#""x&lt; y""#)),
        (
            r#"root = "a%z".unescape_url_query()"#,
            "{}",
            Err("invalid URL escape"),
        ),
        (
            r#"root = "<a></b>".parse_xml()"#,
            "{}",
            Err("failed to parse value as XML"),
        ),
        (
            r#"root = "<![CDATA[x]]><a/>".parse_xml()"#,
            "{}",
            Ok(r#"{"a":""}"#),
        ),
        (
            r#"root = "<!doctype x><a/>".parse_xml()"#,
            "{}",
            Err("DOCTYPE is forbidden"),
        ),
        (
            r#"root = "<a>t<b>1</b></a>".parse_xml()"#,
            "{}",
            Ok(r##"{"a":{"#text":"t","b":"1"}}"##),
        ),
        (
            r#"root = "<a>tru</a>".parse_xml(true)"#,
            "{}",
            Ok(r#"{"a":"tru"}"#),
        ),
        (
            r#"root = {"a":1,"b":2}.format_xml("").string()"#,
            "{}",
            Ok(r#""<a>1</a>\n<b>2</b>""#),
        ),
        (
            r#"root = "zz".decode("ascii85").encode("hex")"#,
            "{}",
            Ok(r#""0000000000000000""#),
        ),
        (r#"root = "a--b".slug()"#, "{}", Ok(r#""a-b""#)),
        (
            r#"root = [0, 1].sort_by(x -> if x == 0 { throw("bad") } else { x })"#,
            "{}",
            Err("bad"),
        ),
        (r#"root = "a&#1".unescape_html()"#, "{}", Ok(r#""a&#1""#)),
        (
            r#"root = "a & b &;".unescape_html()"#,
            "{}",
            Ok(r#""a & b &;""#),
        ),
        (
            r#"root = "a <!> b".strip_html()"#,
            "{}",
            Ok(r#""a &lt;!> b""#),
        ),
        (
            r#"root = "[1.50, 2.25]".parse_json().map_each(x -> x.round())"#,
            "{}",
            Ok("[2,2]"),
        ),
        // Message text 1.50 stays a json.Number; the number methods read it as 1.5.
        ("root = this.l.max()", r#"{"l":[1.50,1E0]}"#, Ok("1.5")),
        (
            r#"root = "a".encrypt_aes("ctr", 5, "0123456789abcdef")"#,
            "{}",
            Err(NOT_STRING),
        ),
        (
            r#"root = 5.encrypt_aes("ctr", "0123456789abcdef", "0123456789abcdef")"#,
            "{}",
            Err(NOT_STRING),
        ),
        (
            r#"root = "a".encrypt_aes("ctr", "0123456789abcdef", "short")"#,
            "{}",
            Err("the key must match the initialisation vector size"),
        ),
        (
            r#"root = "abc".encrypt_aes("ofb", "0123456789abcdef", "0123456789abcdef").decrypt_aes("ofb", "0123456789abcdef", "0123456789abcdef").string()"#,
            "{}",
            Ok(r#""abc""#),
        ),
        (
            r#"root = "0123456789abcdef".encrypt_aes("cbc", "0123456789abcdef", "0123456789abcdef").decrypt_aes("cbc", "0123456789abcdef", "0123456789abcdef").string()"#,
            "{}",
            Ok(r#""0123456789abcdef""#),
        ),
        (
            r#"root = "a".encrypt_aes("xts", "0123456789abcdef", "0123456789abcdef")"#,
            "{}",
            Err("unrecognized encryption type: xts"),
        ),
        (
            r#"root = "a".decrypt_aes("xts", "0123456789abcdef", "0123456789abcdef")"#,
            "{}",
            Err("unrecognized decryption type: xts"),
        ),
        (
            "root = random_int(seed: \"x\")",
            "{}",
            Err("failed to seed random number generator"),
        ),
        ("root = random_int(1) >= 0", "{}", Ok("true")),
        ("root = random_int(1, 2, 3)", "{}", Ok("2")),
        ("root = nanoid(4).length()", "{}", Ok("4")),
        ("root = cosine_similarity([1])", "{}", Err("")),
        ("root = range(1)", "{}", Err("")),
        (r#"root = meta("a", "b")"#, "{}", Err("")),
        ("root = counter(foo: 1)", "{}", Err("")),
        ("root = random_int(foo: 1)", "{}", Err("")),
        ("root = nanoid(foo: 1)", "{}", Err("")),
        (
            "map m { root = root_meta(\"a\") }\nroot = this.apply(\"m\")",
            "{}",
            Err("root metadata cannot be queried in this context"),
        ),
    ]);
}

#[test]
fn a_counter_wraps_at_its_maximum_across_executions() -> Result<(), Box<dyn std::error::Error>> {
    for m in [
        "root = counter(1, 2)",
        "root = counter(min: 1, max: 2, set: nothing())",
    ] {
        let exec = Environment::new().compile_mapping(m)?;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(exec.execute_bytes(b"{}")?.ok_or("a value")?.to_go_json());
        }
        assert_eq!(seen, ["1", "2", "1"], "{m}");
    }
    Ok(())
}

#[test]
fn file_rel_json_refuses_a_file_that_is_not_json() {
    let files = [("bad.json".to_string(), b"{nope".to_vec())]
        .into_iter()
        .collect();
    let env = Environment::new()
        .with_host_file(true)
        .with_importer(std::sync::Arc::new(ekiben_bloblang::env::MemoryImporter {
            files,
        }));
    let e = env
        .compile_mapping(r#"root = file_rel_json("bad.json")"#)
        .map_err(|e| e.to_string())
        .and_then(|x| {
            x.execute_bytes(b"{}")
                .map_err(|e| e.to_string())
                .map(|_| String::new())
        })
        .unwrap_err();
    assert!(
        e.contains("failed to parse JSON from bad.json") || e.contains("refused"),
        "{e}"
    );
}
