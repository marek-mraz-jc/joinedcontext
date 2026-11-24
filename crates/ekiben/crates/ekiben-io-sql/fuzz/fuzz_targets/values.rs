//! Input: a selector byte (mod 5), then the value: 0 a postgres parameter (type oid u16, a flag
//! byte, the text), 1 a postgres binary column (type oid u16, the bytes), 2 a mysql binary
//! datetime, 3 a mysql text datetime, 4 a sqlite time text.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&which, rest)) = data.split_first() else {
        return;
    };
    ekiben_io_sql::fuzzing::values(which, rest);
});
