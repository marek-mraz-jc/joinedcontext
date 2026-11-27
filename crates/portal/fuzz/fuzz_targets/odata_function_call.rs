//! An OData function call from the URL (`name(a=1,b=@x)` plus the query's aliases): it never
//! panics, a refusal is a 400 with a message, and every parameter it takes is named once by an
//! identifier (T-7894).
#![no_main]

use joinedcontext_portal::api::odata_ops::function_call;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let (call, query) = text.split_once('?').unwrap_or((text, ""));
    match function_call(call, query) {
        Ok((name, params)) => {
            assert!(!name.contains('('), "{name:?} from {call:?}");
            for key in params.keys() {
                assert!(
                    !key.is_empty() && !key.contains(['=', ',', '(', ')']),
                    "{key:?} from {call:?}"
                );
            }
        }
        Err((status, _, message)) => {
            assert_eq!(status.as_u16(), 400, "{message}");
            assert!(!message.is_empty());
        }
    }
});
