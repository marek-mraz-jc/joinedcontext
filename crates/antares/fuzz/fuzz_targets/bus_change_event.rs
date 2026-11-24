//! A change event read off the bus (`antares_bus::nats::decode`): any payload bytes and subject
//! never panic, and an event is accepted only on a `changes.<tenant>.…` subject naming its own
//! tenant (T-7803).
#![no_main]
use antares_bus::nats::subject_tenant_agrees;
use antares_bus::ChangeEvent;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    // first line the subject, the rest the payload
    let (subject, payload) = match data.iter().position(|b| *b == b'\n') {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &data[..0]),
    };
    let Ok(subject) = std::str::from_utf8(subject) else {
        return;
    };
    let Ok(ev) = serde_json::from_slice::<ChangeEvent>(payload) else {
        return;
    };
    let agrees = subject_tenant_agrees(subject, &ev);
    let mut parts = subject.split('.');
    if parts.next() == Some("changes") && agrees {
        assert_eq!(
            parts.next(),
            Some(ev.tenant.as_str()),
            "{subject} vs {}",
            ev.tenant.as_str()
        );
    }
});
