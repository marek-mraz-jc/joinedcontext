//! `ekiben blobl`: a mapping over line-delimited input, printed as Bento's `blobl` prints it
//! (internal/cli/blobl/cli.go and core.go).

use std::io::{BufRead, Write};

use ekiben_bloblang::{Environment, Executor, Value};

/// Runs `mapping` over every line of `input`. A parse error prints `failed to parse mapping: ...` with the
/// source line and returns 1; a line that fails prints `failed to execute map: ...` to `err` and the run
/// goes on; the result is 0 then.
pub fn run(
    mapping: &str,
    raw: bool,
    input: impl BufRead,
    out: &mut impl Write,
    err: &mut impl Write,
) -> i32 {
    let exec = match Environment::new()
        .with_host_env(true)
        .compile_mapping(mapping)
    {
        Ok(exec) => exec,
        Err(e) => {
            let _ = writeln!(
                err,
                "failed to parse mapping: {}",
                e.at_position_structured(mapping)
            );
            return 1;
        }
    };
    for line in input.lines() {
        let Ok(line) = line else {
            let _ = writeln!(err, "failed to read input");
            return 1;
        };
        match line_output(&exec, &line, raw) {
            Ok(bytes) => {
                let _ = out.write_all(&bytes);
                let _ = writeln!(out);
            }
            Err(e) => {
                let _ = writeln!(err, "failed to execute map: {e}");
            }
        }
    }
    let _ = out.flush();
    0
}

/// One line of `ekiben blobl`, printed as Bento's `blobl` prints it (internal/cli/blobl/core.go): strings
/// and bytes raw, a deleted message as an empty line, an untouched message re-encoded when it is JSON
/// (unless `raw`), anything else as `json.Marshal` output.
pub fn line_output(exec: &Executor, line: &str, raw: bool) -> Result<Vec<u8>, String> {
    let res = exec.execute_bytes_in_place(line.as_bytes()).map_err(|e| {
        let e = e.to_string();
        // Bento replaces the whole error when the input is not JSON and the mapping read `this`.
        match e.find("unable to reference message as structured") {
            Some(i) => e[i..].to_string(),
            None => e,
        }
    })?;
    Ok(match res {
        None => Vec::new(),
        Some(Value::String(s)) => s.into_bytes(),
        Some(Value::Bytes(b)) => b,
        Some(Value::Nothing) => {
            match ekiben_bloblang::go_json::decode_with_numbers(line.as_bytes()) {
                Some(v) if !raw => v.to_go_json().into_bytes(),
                _ => line.as_bytes().to_vec(),
            }
        }
        Some(v) => v.to_go_json().into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Broken;

    impl std::io::Read for Broken {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("pipe closed"))
        }
    }

    fn exec(mapping: &str) -> Executor {
        Environment::new()
            .compile_mapping(mapping)
            .expect("mapping")
    }

    #[test]
    fn unreadable_input_stops_with_one() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let input = std::io::BufReader::new(Broken);
        assert_eq!(run("root = this", false, input, &mut out, &mut err), 1);
        assert_eq!(String::from_utf8_lossy(&err), "failed to read input\n");
        assert!(out.is_empty());
    }

    #[test]
    fn each_result_kind_prints_as_bento_prints_it() {
        assert_eq!(
            line_output(&exec("root = deleted()"), "{}", false).unwrap(),
            b""
        );
        assert_eq!(
            line_output(&exec("root = \"s\""), "{}", false).unwrap(),
            b"s"
        );
        assert_eq!(
            line_output(&exec("root = \"b\".bytes()"), "{}", false).unwrap(),
            b"b"
        );
        assert_eq!(
            line_output(&exec("root = {\"a\": 1}"), "x", false).unwrap(),
            br#"{"a":1}"#
        );
        // Untouched: JSON input is re-encoded unless raw, anything else is printed as read.
        let keep = exec("meta x = 1");
        assert_eq!(
            line_output(&keep, r#"{ "a" : 1 }"#, false).unwrap(),
            br#"{"a":1}"#
        );
        assert_eq!(
            line_output(&keep, r#"{ "a" : 1 }"#, true).unwrap(),
            br#"{ "a" : 1 }"#
        );
        assert_eq!(line_output(&keep, "not json", false).unwrap(), b"not json");
        let err = line_output(&exec("root = this.a"), "not json", false).unwrap_err();
        assert!(
            err.starts_with("unable to reference message as structured"),
            "{err}"
        );
        let err = line_output(&exec("root = throw(\"no\")"), "{}", false).unwrap_err();
        assert!(err.contains("no"), "{err}");
    }
}
