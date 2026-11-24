//! Input: the format, a NUL byte, then a JSON array of arguments. Without a NUL (or with JSON
//! that does not decode) the arguments are one of each Value kind.
#![no_main]

use ekiben_bloblang::go_fmt::sprintf;
use ekiben_bloblang::go_json::decode_with_numbers;
use ekiben_bloblang::Value;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (format, args) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], decode_with_numbers(&data[at + 1..])),
        None => (data, None),
    };
    let args = match args {
        Some(Value::Array(items)) => items.to_vec(),
        Some(other) => vec![other],
        None => vec![
            Value::Null,
            Value::Bool(true),
            Value::Int(-7),
            Value::Uint(u64::MAX),
            Value::Float(1.5),
            Value::String("s".into()),
            Value::Bytes(vec![0xff, 0]),
        ],
    };
    let _ = sprintf(&String::from_utf8_lossy(format), &args);
});
