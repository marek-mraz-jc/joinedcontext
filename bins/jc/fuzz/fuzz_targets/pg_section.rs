//! Any `[<module>.database]` URLs: the check never panics, and a URL it admits carries a role
//! and no password (T-7634).
#![no_main]

use jc::pg::DatabaseSection;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut parts = text.splitn(3, '\n');
    let url = parts.next().unwrap_or_default().to_owned();
    let migrate = parts.next().map(str::to_owned);
    let maintenance = parts.next().map(str::to_owned);
    let section = DatabaseSection {
        url: url.clone(),
        password_file: "/nonexistent/pw".into(),
        max_connections: Some(1),
        migrate_password_file: migrate.as_ref().map(|_| "/nonexistent/mpw".into()),
        migrate_url: migrate,
        maintenance_password_file: maintenance.as_ref().map(|_| "/nonexistent/xpw".into()),
        maintenance_url: maintenance,
    };
    if section.validate().is_ok() {
        let parsed = url::Url::parse(&url).expect("validated");
        assert!(parsed.password().is_none(), "{url}");
        assert!(section.app_role().is_ok_and(|r| !r.is_empty()), "{url}");
        let _ = section.require_owner_split(true);
        assert!(section.app_url_with_password().is_err(), "no password file");
    }
});
