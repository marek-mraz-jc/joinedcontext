//! A bucket's ListObjectVersions answer (T-7784): any text parses to versions without a panic,
//! newest first, each named by what its own block holds.
#![no_main]

use joinedcontext_portal::org_backup::s3::parse_versions;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(xml) = std::str::from_utf8(data) else {
        return;
    };
    let versions = parse_versions(xml);
    let blocks = xml.matches("<Version>").count() + xml.matches("<DeleteMarker>").count();
    assert!(versions.len() <= blocks);
    assert!(versions
        .windows(2)
        .all(|w| w[0].last_modified >= w[1].last_modified));
    for version in &versions {
        assert!(version.key.len() <= xml.len());
        assert!(version.version_id.len() <= xml.len());
    }
});
