//! Parser edge cases (T-7666): Go string escapes, comments and line endings, imports, maps,
//! `let`/`meta` statements, literals, lambdas, `if`/`match` expressions, argument lists and
//! field interpolations. Each malformed mapping is refused with the message a person can act on.

use std::collections::HashMap;
use std::sync::Arc;

use ekiben_bloblang::env::MemoryImporter;
use ekiben_bloblang::Environment;

type R = Result<(), Box<dyn std::error::Error>>;

/// The mapping's result on `input` as Go JSON text, or the compile or run error.
fn run_with(env: &Environment, mapping: &str, input: &str) -> Result<String, String> {
    let exec = env.compile_mapping(mapping).map_err(|e| e.to_string())?;
    match exec.execute_bytes(input.as_bytes()) {
        Ok(Some(v)) => Ok(v.to_go_json()),
        Ok(None) => Ok("<deleted>".into()),
        Err(e) => Err(e.to_string()),
    }
}

fn run(mapping: &str, input: &str) -> Result<String, String> {
    run_with(&Environment::new(), mapping, input)
}

fn importer(files: &[(&str, &[u8])]) -> Environment {
    let files = files
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_vec()))
        .collect::<HashMap<_, _>>();
    Environment::new().with_importer(Arc::new(MemoryImporter { files }))
}

#[test]
fn go_escapes_unescape_and_bad_ones_are_refused() {
    assert_eq!(
        run(r#"root = "\a\b\f\v\'\x41é\U0001F600\q\n\r\t\\\"""#, "{}"),
        Ok(r#""\u0007\b\f\u000b'Aé😀\\q\n\r\t\\\"""#.into())
    );
    assert_eq!(run(r#"root = "\a\u00e9""#, "{}"), Ok(r#""\u0007é""#.into()));
    for (lit, why) in [
        (r#""\x4""#, "short \\x escape"),
        (r#""\xZZ""#, "invalid digit found in string"),
        (r#""\u12""#, "short \\u escape"),
        (r#""\uZZZZ""#, "invalid digit found in string"),
        (r#""\a\uD800""#, "invalid unicode codepoint"),
        (r#""\U12""#, "short \\U escape"),
        (r#""\U0000ZZZZ""#, "invalid digit found in string"),
        (r#""\U0011FFFF""#, "invalid unicode codepoint"),
    ] {
        let e = run(&format!("root = {lit}"), "{}").unwrap_err();
        assert!(
            e.contains(&format!("failed to unescape quoted string contents: {why}")),
            "{lit}: {e}"
        );
    }
}

#[test]
fn unterminated_strings_are_refused_where_they_stop() {
    for (m, why) in [
        (r#"root = """abc"#, "required: expected end triple-quote"),
        ("root = \"ab\ncd\"", "required: expected end quote"),
        (r#"root = "abc"#, "required: expected end quote"),
        ("import foo", "expected quoted string"),
    ] {
        let e = run(m, "{}").unwrap_err();
        assert!(e.contains(why), "{m:?}: {e}");
    }
    assert_eq!(run(r#"root = """a"b""""#, "{}"), Ok(r#""a\"b""#.into()));
}

#[test]
fn comments_at_the_end_and_crlf_line_breaks_end_a_statement() {
    assert_eq!(
        run("root.a = 1\n# trailing comment", "{}"),
        Ok(r#"{"a":1}"#.into())
    );
    assert_eq!(
        run("root.a = 1 # trailing comment", "{}"),
        Ok(r#"{"a":1}"#.into())
    );
    assert_eq!(
        run("root.a = 1\r\nroot.b = 2", "{}"),
        Ok(r#"{"a":1,"b":2}"#.into())
    );
    // An empty mapping assigns nothing: the result is the untouched `nothing`.
    assert_eq!(run("", r#"{"a":1}"#), Ok("null".into()));
    assert_eq!(run("  # only a comment", r#"{"a":1}"#), Ok("null".into()));
    let e = run("root.a = 1 root.b = 2", "{}").unwrap_err();
    assert!(e.contains("expected line break between statements"), "{e}");
}

#[test]
fn a_whole_file_import_is_read_or_refused_with_the_importer_error() -> R {
    let env = importer(&[
        ("a.blobl", b"root = this.x + 1"),
        ("bad.blobl", b"\xff\xfe"),
    ]);
    assert_eq!(
        run_with(&env, r#"from "a.blobl""#, r#"{"x":1}"#),
        Ok("2".into())
    );
    assert_eq!(
        run_with(&env, "from \"a.blobl\"\n# done\n", r#"{"x":2}"#),
        Ok("3".into())
    );
    let missing = run_with(&env, r#"from "nope.blobl""#, "{}").unwrap_err();
    assert!(
        missing.contains("failed to read import: import file 'nope.blobl' not found"),
        "{missing}"
    );
    let utf8 = run_with(&env, r#"from "bad.blobl""#, "{}").unwrap_err();
    assert!(utf8.contains("invalid UTF-8 in import"), "{utf8}");
    let off = run(r#"from "a.blobl""#, "{}").unwrap_err();
    assert!(run("from \"a.blobl\" root = 1", "{}").is_err());
    assert!(run("from x", "{}").is_err());
    assert!(
        off.contains("imports are disabled in this context"),
        "{off}"
    );
    Ok(())
}

#[test]
fn a_statement_import_brings_maps_and_refuses_collisions_and_empty_files() {
    let env = importer(&[
        ("maps.blobl", b"map inc { root = this + 1 }"),
        ("none.blobl", b"root = 1"),
        ("bad.blobl", b"\xff"),
    ]);
    assert_eq!(
        run_with(
            &env,
            "import \"maps.blobl\"\nroot = this.x.apply(\"inc\")",
            r#"{"x":4}"#
        ),
        Ok("5".into())
    );
    for (m, why) in [
        ("import \"bad.blobl\"\nroot = 1", "invalid UTF-8 in import"),
        (
            "import \"nope.blobl\"\nroot = 1",
            "failed to read import: import file 'nope.blobl' not found",
        ),
        (
            "map m { import \"maps.blobl\" }\nroot = 1",
            "importing mappings is not allowed within this block",
        ),
    ] {
        let e = run_with(&env, m, "{}").unwrap_err();
        assert!(e.contains(why), "{m:?}: {e}");
    }
    // Bento reports these at the single-query reading, which got further than the statement.
    assert!(run_with(&env, "import \"none.blobl\"\nroot = 1", "{}").is_err());
    assert!(run_with(
        &env,
        "map inc { root = 0 }\nimport \"maps.blobl\"\nroot = 1",
        "{}"
    )
    .is_err());
}

#[test]
fn maps_are_named_bodied_closed_and_unique() {
    assert_eq!(
        run(
            "map \"my map\" { root = 1 }\nroot = this.apply(\"my map\")",
            "{}"
        ),
        Ok("1".into())
    );
    assert_eq!(
        run("map m { root = this }\nroot = 2.apply(\"m\")", "{}"),
        Ok("2".into())
    );
    assert_eq!(run("map m {}\nroot = 3", "{}"), Ok("3".into()));
    for (m, why) in [
        ("map { root = 1 }", "expected map name"),
        ("map m root = 1", "expected '{' starting map body"),
        ("map m { root = 1", "expected '}' closing map body"),
        (
            "map a { map b { root = 1 } }",
            "defining maps is not allowed within this block",
        ),
        (
            "map a { root = 1 }\nmap a { root = 2 }",
            "map name collision: a",
        ),
        (
            "map m { meta a = 1 }",
            "setting meta fields is not allowed within this block",
        ),
    ] {
        let e = run(m, "{}").unwrap_err();
        assert!(e.contains(why), "{m:?}: {e}");
    }
}

#[test]
fn let_and_meta_statements_take_quoted_names_and_refuse_a_missing_part() {
    assert_eq!(run("let \"a b\" = 1\nroot = 2", "{}"), Ok("2".into()));
    assert_eq!(run("let $x = 4\nroot = $x", "{}"), Ok("4".into()));
    assert_eq!(
        run("meta \"k k\" = 1\nroot = @\"k k\"", "{}"),
        Ok("1".into())
    );
    for (m, why) in [
        ("let = 1", "expected variable name"),
        ("let x 1", "expected '=' in let assignment"),
        ("root = $", "expected variable name after '$'"),
    ] {
        let e = run(m, "{}").unwrap_err();
        assert!(e.contains(why), "{m:?}: {e}");
    }
    assert!(run("meta foo bar", "{}").is_err());
    assert!(run("= 1", "{}").is_err());
    assert!(run("root. = 1", "{}").is_err());
}

#[test]
fn a_root_if_statement_falls_back_to_an_expression_when_it_is_not_one() {
    assert_eq!(
        run(r#"if this.a { "x" } else { "y" }"#, r#"{"a":false}"#),
        Ok(r#""y""#.into())
    );
    for m in [
        "if this.a root = 1",
        "if this.a { root = 1",
        "if this.a { root = 1 } else if this.b root = 2",
        "if this.a { root = 1 } else if this.b { root = 2",
        "if this.a { root = 1 } else root = 3",
        "if this.a { root = 1 } else { root = 3",
    ] {
        assert!(run(m, "{}").is_err(), "{m}");
    }
}

#[test]
fn numbers_brackets_and_paths_refuse_what_they_cannot_read() {
    assert_eq!(run("root = []", "{}"), Ok("[]".into()));
    assert_eq!(run(r#"root = [this.a, ]"#, r#"{"a":1}"#), Ok("[1]".into()));
    assert_eq!(
        run(r#"root = {"a": this.b, }"#, r#"{"b":1}"#),
        Ok(r#"{"a":1}"#.into())
    );
    assert_eq!(
        run(r#"root = {"a": 1, "b": [2]}"#, "{}"),
        Ok(r#"{"a":1,"b":[2]}"#.into())
    );
    assert_eq!(run("root = @\"k\"", "{}"), Ok("null".into()));
    assert_eq!(
        run("root = this.a[1:3:]", r#"{"a":"abcd"}"#),
        Ok(r#""bc""#.into())
    );
    for (m, why) in [
        (
            "root = 99999999999999999999",
            "failed to parse '99999999999999999999' as integer",
        ),
        ("root = [this.a 1]", "expected ',' or ']' in array literal"),
        (r#"root = {"a" 1}"#, "expected ':' after object key"),
        (
            "root = {1: 2}",
            "object keys must be strings, received: number",
        ),
        (
            r#"root = {"a": this.b 1}"#,
            "expected ',' or '}' in object literal",
        ),
        ("root = this.a[0", "expected closing bracket ']'"),
        ("root = this.a[1:2 x]", "expected closing bracket ']'"),
        ("root = this.a.(this.b", "expected closing bracket ')'"),
        ("root = this.a.!", "expected field path or method after '.'"),
        ("root = (this.a", "required: expected closing bracket ')'"),
        ("root = )", "expected query"),
    ] {
        let e = run(m, "{}").unwrap_err();
        assert!(e.contains(why), "{m:?}: {e}");
    }
    assert!(run("root = 1ex", "{}").is_err());
    assert!(run("root = -this", "{}").is_err());
}

#[test]
fn lambdas_refuse_reserved_shadowing_and_doubled_labels() {
    assert_eq!(
        run("root = this.a.(x -> x + 1)", r#"{"a":1}"#),
        Ok("2".into())
    );
    for (m, why) in [
        (
            "root = this.a.(root -> root)",
            "context label `root` is not allowed",
        ),
        (
            "root = this.a.(x -> this.b.(x -> x))",
            "context label `x` would shadow a parent context",
        ),
        (
            "root = this.a.(x -> y -> 1)",
            "it would be in poor taste to capture the same context under both 'x' and 'y'",
        ),
    ] {
        let e = run(m, "{}").unwrap_err();
        assert!(e.contains(why), "{m:?}: {e}");
    }
}

#[test]
fn if_and_match_expressions_refuse_a_missing_brace_or_arrow() {
    for (m, why) in [
        (
            "root = if this.a 1",
            "required: expected '{' after if condition",
        ),
        (
            "root = if this.a { 1 } else if this.b 2",
            "required: expected '{' after else if condition",
        ),
        (
            "root = if this.a { 1 } else if this.b { 2",
            "required: expected '}' closing else if branch",
        ),
        (
            "root = if this.a { 1 } else 3",
            "required: expected '{' after else",
        ),
        (
            "root = if this.a { 1 } else { 3",
            "required: expected '}' closing else branch",
        ),
        (
            "root = match this.a 1",
            "required: expected '{' starting match cases",
        ),
        ("root = match this { 1 2 }", "expected '=>' in match case"),
        (
            "root = match this { 1 => 2 3 }",
            "expected ',', newline, or '}' after match case",
        ),
    ] {
        let e = run(m, "{}").unwrap_err();
        assert!(e.contains(why), "{m:?}: {e}");
    }
    assert_eq!(
        run(
            "root = match this {\n  1 => \"one\" # first\n  _ => \"other\"\n}",
            "1"
        ),
        Ok(r#""one""#.into())
    );
}

#[test]
fn argument_lists_refuse_duplicates_mixes_and_missing_parameters() {
    assert_eq!(
        run(r#"root = "abc".contains("b", )"#, "{}"),
        Ok("true".into())
    );
    for (m, why) in [
        (
            r#"root = "a".replace_all(old: "a", old: "b")"#,
            "duplicate named arg: old",
        ),
        (
            r#"root = "a".contains("a" "b")"#,
            "required: expected comma ',' or closing bracket ')'",
        ),
        (
            r#"root = "a".replace_all(old: "a", "b")"#,
            "cannot mix named and nameless arguments",
        ),
        (r#"root = "a".contains()"#, "missing parameter: value"),
        (
            r#"root = "abc".slice(3, 1)"#,
            "lower slice bound 3 must be lower than upper (1)",
        ),
    ] {
        let e = run(m, "{}").unwrap_err();
        assert!(e.contains(why), "{m:?}: {e}");
    }
}

#[test]
fn field_interpolations_refuse_an_open_or_overlong_expression() {
    let env = Environment::new();
    for (f, why) in [
        (
            "${{! this.a",
            "required: expected end of escaped expression",
        ),
        ("${! this.a", "required: expected end of expression '}'"),
        (
            "${! this.a this.b }",
            "required: expected end of expression '}'",
        ),
        ("${! ) }", "expected query"),
    ] {
        let e = env
            .compile_field(f)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(e.contains(why), "{f:?}: {e}");
    }
    assert!(env.compile_field("plain ${! 1 + 1 } text").is_ok());
}
