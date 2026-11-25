//! The working and recovery recipients an administrator pastes into a data target (T-7784): any
//! text is a recipient or a refusal naming its field, and every text has a fingerprint of four
//! groups of four hex digits.
#![no_main]

use joinedcontext_portal::org_backup::data::{fingerprint, parse_recipients};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let pasted: Vec<String> = text.splitn(3, '\n').map(str::to_owned).collect();
    match parse_recipients(&pasted) {
        Ok(recipients) => {
            assert_eq!(recipients.len(), pasted.len().min(2));
            for (recipient, text) in recipients.iter().zip(&pasted) {
                assert_eq!(recipient.to_string(), text.trim());
            }
        }
        Err(why) => assert!(
            why.starts_with("workingRecipient: ") || why.starts_with("recoveryRecipient: "),
            "{why}"
        ),
    }
    for line in &pasted {
        let print = fingerprint(line);
        assert_eq!(print.len(), 19);
        assert!(print
            .split(':')
            .all(|g| g.len() == 4 && g.bytes().all(|b| b.is_ascii_hexdigit())));
        assert_eq!(print, fingerprint(line.trim()));
    }
});
