#![no_main]
//! Any bytes an IMAP server hands over as a mail: attachments queue under a bare name, or none.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let queued = ekiben_io_remote::fuzzing::mail(data);
    assert!(queued <= data.len());
});
