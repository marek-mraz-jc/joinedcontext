//! Input: a mapping's text. It compiles or is refused with a parse error; a compiled mapping runs
//! once over a small fixed document and ends in a value or an error.
#![no_main]

use ekiben_bloblang::Environment;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(exec) = Environment::new().compile_mapping(text) else {
        return;
    };
    let _ = exec.query_targets();
    let _ = exec.assignment_targets();
    let _ = exec.execute_bytes(br#"{"a":1,"b":"x","c":[1,2,{"d":null}],"e":{"f":true}}"#);
});
