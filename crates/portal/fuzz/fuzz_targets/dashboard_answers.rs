//! What the dashboard publisher reads from outside: the catalogue's `package_show` and
//! `resource_view_list` answers. Any JSON yields an outcome or a reason, never a panic, and a
//! package without the platform's mark is never written over.
#![no_main]

use jcctl::publish::ckan::{CkanApi, CkanError, GENERATOR};
use joinedcontext_portal::reconciler::ckan_dashboards::{publish, Desired};
use libfuzzer_sys::fuzz_target;
use serde_json::{json, Value};

struct Answers {
    package: Value,
    views: Value,
    wrote: bool,
}

impl CkanApi for Answers {
    fn show(&self, action: &str, _: &str) -> Result<Option<Value>, CkanError> {
        let answer = match action {
            "package_show" => &self.package,
            _ => &self.views,
        };
        Ok((!answer.is_null()).then(|| answer.clone()))
    }

    fn action(&mut self, _: &str, _: &Value) -> Result<Value, CkanError> {
        self.wrote = true;
        Ok(json!({}))
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(answer) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let desired = Desired {
        package: json!({
            "name": "bbsk-ukazovatele", "title": "Ukazovatele", "notes": "n", "type": "dashboard",
            "owner_org": "bbsk",
            "extras": [{ "key": "generated_by", "value": GENERATOR }],
            "resources": [{ "name": "bikes", "url": "https://api.example.sk/api/endpoint/s" }],
        }),
        views: vec![(
            "s".to_owned(),
            json!({ "view_type": "jc_visualization", "jc_name": "p0w0", "title": "t" }),
        )],
    };
    let mut api = Answers {
        package: answer["package"].clone(),
        views: answer["views"].clone(),
        wrote: false,
    };
    let foreign = api.package.is_object()
        && !api.package["extras"]
            .as_array()
            .is_some_and(|extras| extras.iter().any(|e| e["key"] == "generated_by" && e["value"] == GENERATOR));
    let outcome = publish(&mut api, &desired);
    if foreign {
        assert!(outcome.is_err() && !api.wrote, "a package somebody else made is never written");
    }
});
