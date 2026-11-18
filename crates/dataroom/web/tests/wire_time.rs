//! Dates the portal pages print, pinned from chrono 0.4.45 before the move to jiff (T-7238):
//! the about rows, the hourly next update, JSON-LD and the sitemap keep these bytes.

mod common;

use common::{branding, catalogue, dataset};
use dataroom_web::helpers::{jc_about, jc_schema_org_dict, jc_sitemap_xml};
use serde_json::json;

#[test]
fn page_dates_print_as_chrono_did() {
    // The two 2026 clock changes in Bratislava and Helsinki: the pages print UTC either way.
    let ds = dataset(
        "t",
        json!({
            "issued": "2026-03-29T00:30:00.120Z",
            "modified": "2026-10-25T00:45:30.5+02:00",
            "frequency": "http://publications.europa.eu/resource/authority/frequency/HOURLY",
        }),
    );
    let rows = jc_about(&ds, None, "en", "sk");
    let texts: Vec<&str> = rows.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "city",
            "Every hour",
            "March 29, 2026",
            "October 24, 2026",
            "October 24, 2026, 23:45 (UTC)"
        ]
    );
    let cat = catalogue(json!({}));
    let dict = jc_schema_org_dict(&cat, &ds, None, &branding(), "en");
    assert_eq!(dict["datePublished"], "2026-03-29T00:30:00.120+00:00");
    assert_eq!(dict["dateModified"], "2026-10-24T22:45:30.500+00:00");
    assert_eq!(dict["temporalCoverage"], "2026-03-29/..");
    let xml = jc_sitemap_xml(&cat, std::slice::from_ref(&ds), &[]);
    assert!(xml.contains("<lastmod>2026-10-24</lastmod>"), "{xml}");
}
