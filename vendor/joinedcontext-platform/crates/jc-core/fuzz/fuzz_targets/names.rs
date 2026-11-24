//! Any text as a name, a domain, a URN or a remote host: every check answers and never panics,
//! and a URN that parses reads back from what it prints (T-7766).
#![no_main]

use jc_core::names::{self, OrganizationId};
use jc_core::urn::{apply_render_prefix, type_of_id, Urn};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = names::validate_dns1123_label(text);
    let _ = names::validate_not_reserved(text);
    let _ = names::validate_namespace(text);
    let _ = names::validate_space_name(text);
    let _ = names::validate_org_domain(text);
    let _ = names::validate_local_id(text);
    let _ = names::validate_entity_type(text);
    let _ = names::validate_locale(text);
    let _ = names::looks_like_a_credential(text);
    let _ = names::org_id(text);
    if let Ok(org) = OrganizationId::parse(text) {
        assert_eq!(org.as_str(), text);
    }
    let _ = OrganizationId::from_domain(text);
    if let Ok(urn) = text.parse::<Urn>() {
        assert_eq!(urn.to_string().parse::<Urn>().ok().as_ref(), Some(&urn));
        assert_eq!(type_of_id(text), Some(urn.entity_type()));
    }
    let _ = type_of_id(text);
    let mid = (0..=text.len() / 2)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0);
    let (a, b) = text.split_at(mid);
    let _ = apply_render_prefix(a, b);
    let _ = jc_core::address::check_remote_host(text);
});
