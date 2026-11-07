//! One input line of `bento blobl`, rendered byte for byte as Bento 1.22 prints it
//! (internal/cli/blobl/cli.go executeBloblangMapping).

use crate::executor::Executor;
use crate::value::Value;

/// Maps one stdin line. `Ok` is the stdout line without its newline (empty for a deleted
/// message); `Err` is the stderr line Bento prints, `failed to execute map: ...`.
pub fn run_line(exec: &Executor, input: &[u8], raw: bool) -> Result<String, String> {
    let res = if raw {
        exec.exec_on_value(Value::Bytes(input.to_vec()))
            .map(|v| (!matches!(v, Value::Delete)).then_some(v))
    } else {
        exec.execute_bytes(input)
    };
    // Bento replaces the whole error when the input was not JSON (errors.As ErrNoContext),
    // dropping the "failed assignment" frame around it.
    let res = res.map_err(|e| {
        let e = e.to_string();
        const NO_CONTEXT: &str = "unable to reference message as structured";
        let e = e.find(NO_CONTEXT).map_or(e.as_str(), |i| &e[i..]);
        format!("failed to execute map: {e}")
    });
    match res? {
        None | Some(Value::Delete) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s),
        Some(Value::Bytes(b)) => Ok(String::from_utf8_lossy(&b).into_owned()),
        Some(Value::Nothing) => Ok(match crate::go_json::decode_with_numbers(input) {
            Some(v) if !raw => v.to_go_json(),
            _ => String::from_utf8_lossy(input).into_owned(),
        }),
        Some(v) => Ok(v.to_go_json()),
    }
}
