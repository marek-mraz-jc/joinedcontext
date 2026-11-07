//! OData relative URLs (`odata_url::parse`, ADR-0024 decision 3): never panics, and what it
//! accepts writes itself back to a URL that parses to the same tree (T-3503). Seeded from the
//! OASIS ABNF test cases by `dev/odata-url-abnf.py --seeds fuzz/corpus/odata_url_parse`.
#![no_main]
use libfuzzer_sys::fuzz_target;
use odata_url::{parse, Limits, Member, Model, UrlErrorKind};

/// Names of the ABNF test cases' service, every member on every type.
struct Service;

impl Model for Service {
    fn entity_set(&self, name: &str) -> Option<&str> {
        ["Categories", "Customers", "Orders", "Products", "People"]
            .contains(&name)
            .then_some("Any")
    }

    fn member(&self, _ty: &str, name: &str) -> Option<Member<'_>> {
        Some(match name {
            "ID" | "Name" | "Price" | "Age" | "BirthDate" | "Location" => Member::Primitive { collection: false },
            "Names" | "EmailAddresses" => Member::Primitive { collection: true },
            "Address" => Member::Complex { ty: "Any", collection: false },
            "Addresses" => Member::Complex { ty: "Any", collection: true },
            "Customer" | "Product" | "Supplier" => Member::Navigation { ty: "Any", collection: false },
            "Items" | "Orders" | "Products" => Member::Navigation { ty: "Any", collection: true },
            _ => return None,
        })
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let limits = Limits::strict();
    let Ok(uri) = parse(text, &Service, &limits) else {
        return;
    };
    let canonical = uri.to_string();
    match parse(&canonical, &Service, &limits) {
        Ok(again) => assert_eq!(again, uri, "{text:?} wrote {canonical:?}"),
        // The canonical form spells spaces `%20` and may outgrow a length limit.
        Err(e) => assert!(matches!(e.kind, UrlErrorKind::LimitExceeded { .. }), "{text:?} wrote {canonical:?}: {e}"),
    }
});
