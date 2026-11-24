//! Input: a JSON scanner config (`{"scanner": {...}}` or `codec`), as an author writes it in a
//! stream. Building it must answer a scanner or a config error; the built scanner then reads a
//! fixed sample.
#![no_main]

use ekiben_io_file::inputs::Scanner;
use libfuzzer_sys::fuzz_target;

const SAMPLE: &[u8] = b"\xEF\xBB\xBFid,name\n1,a\n2,b\n{\"k\":1}\n<a>1</a>\n";

fuzz_target!(|data: &[u8]| {
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_slice::<serde_json::Value>(data)
    else {
        return;
    };
    for built in [Scanner::from_config(&obj), Scanner::lines_by_default(&obj)] {
        if let Ok(scanner) = built {
            let _ = scanner.is_lines();
            let _ = scanner.scan(SAMPLE, "upload/a.csv");
        }
    }
});
