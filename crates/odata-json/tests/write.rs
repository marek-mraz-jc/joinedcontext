// SPDX-License-Identifier: EUPL-1.2
//! The writers, pinned byte for byte in `tests/golden/` per metadata level and version
//! (`ODATA_JSON_BLESS=1` rewrites the files; a change is a reviewed diff), and an I/O error
//! wherever the sink fails.

#[cfg(test)]
mod service;

#[cfg(test)]
mod writers {

    use std::io::{self, Write};
    use std::path::Path;

    use crate::service::{ctx, minimal, model, resource};
    use odata_json::{
        write_collection, write_entity, write_entity_reference, write_entity_references,
        write_service_document, Ctx, EntityValue, MetadataLevel, ODataVersion, Page,
    };
    use serde_json::json;

    const LEVELS: [(MetadataLevel, &str); 3] = [
        (MetadataLevel::None, "none"),
        (MetadataLevel::Minimal, "minimal"),
        (MetadataLevel::Full, "full"),
    ];
    const VERSIONS: [(ODataVersion, &str); 2] =
        [(ODataVersion::V4_0, "4.0"), (ODataVersion::V4_01, "4.01")];

    fn golden(name: &str, written: &[u8]) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden")
            .join(name);
        if std::env::var_os("ODATA_JSON_BLESS").is_some() {
            std::fs::write(&path, written).unwrap();
        }
        let pinned = std::fs::read(&path).unwrap();
        assert_eq!(
            String::from_utf8_lossy(written),
            String::from_utf8_lossy(&pinned),
            "{name}"
        );
        let parsed: serde_json::Value = serde_json::from_slice(written).unwrap();
        assert!(parsed.is_object(), "{name}");
    }

    fn b1() -> EntityValue {
        EntityValue {
            id: "Buildings('b1')".into(),
            properties: vec![
                ("id".into(), json!("b1")),
                ("name".into(), json!(null)),
                ("height".into(), json!(42)),
                ("area".into(), json!(1.5)),
                ("price".into(), json!(10)),
                ("active".into(), json!(true)),
                ("extra".into(), json!({"free": [1]})),
                ("anything".into(), json!([1])),
                ("tags".into(), json!(["a"])),
                ("odd".into(), json!("?")),
                (
                    "address".into(),
                    json!({"street": "Main", "floors": [1, 2], "dynamic": 1}),
                ),
                ("homes".into(), json!([{"street": "Side"}, null])),
                ("dynamic".into(), json!("not declared")),
            ],
        }
    }

    fn written(write: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> Vec<u8> {
        let mut out = Vec::new();
        write(&mut out).unwrap();
        out
    }

    fn every_ctx() -> impl Iterator<Item = (Ctx<'static>, String)> {
        LEVELS.into_iter().flat_map(|(level, l)| {
            VERSIONS
                .into_iter()
                .map(move |(version, v)| (ctx(level, version), format!("{l}-{v}")))
        })
    }

    #[test]
    fn entities_and_collections_per_level_and_version() {
        let model = model();
        let buildings = resource(&model, "Buildings");
        let names = ["id", "height"];
        let projected = resource(&model, "Buildings").select(&names);
        let page = Page {
            count: Some(2),
            next_link: Some("https://example.org/odata/Buildings?$skiptoken=b2"),
        };
        let b2 = EntityValue {
            id: "Buildings('b2')".into(),
            properties: vec![("id".into(), json!("b2"))],
        };
        for (ctx, suffix) in every_ctx() {
            golden(
                &format!("entity-{suffix}.json"),
                &written(|out| write_entity(&ctx, &buildings, &b1(), out)),
            );
            golden(
                &format!("collection-{suffix}.json"),
                &written(|out| write_collection(&ctx, &projected, &[b1(), b2.clone()], &page, out)),
            );
        }
    }

    #[test]
    fn references_and_the_service_document_per_level_and_version() {
        let model = model();
        let ids = ["People('p1')", "People('p2')"];
        let page = Page {
            count: Some(2),
            next_link: None,
        };
        // The default context is one of them.
        golden(
            "reference-minimal-4.0.json",
            &written(|out| write_entity_reference(&minimal(), "People('p1')", out)),
        );
        for (ctx, suffix) in every_ctx() {
            golden(
                &format!("reference-{suffix}.json"),
                &written(|out| write_entity_reference(&ctx, "People('p1')", out)),
            );
            golden(
                &format!("references-{suffix}.json"),
                &written(|out| write_entity_references(&ctx, &ids, &page, out)),
            );
            golden(
                &format!("service-{suffix}.json"),
                &written(|out| write_service_document(&ctx, &model, out)),
            );
        }
    }

    fn prefix(version: ODataVersion) -> &'static str {
        match version {
            ODataVersion::V4_0 => "@odata.",
            ODataVersion::V4_01 => "@",
        }
    }

    #[test]
    fn the_metadata_etag_follows_the_context_url() {
        let tag = "\"5bbd\"";
        for (ctx, suffix) in every_ctx() {
            let tagged = Ctx {
                metadata_etag: Some(tag),
                ..ctx
            };
            let body = String::from_utf8(written(|out| {
                write_entity_reference(&tagged, "People('p1')", out)
            }))
            .unwrap();
            let want = match tagged.level {
                MetadataLevel::None => None,
                _ => Some(format!(
                    r#""{}context":"https://example.org/odata/$metadata#$ref","{}metadataEtag":"\"5bbd\"","#,
                    prefix(tagged.version),
                    prefix(tagged.version)
                )),
            };
            match want {
                Some(head) => assert!(body.starts_with(&format!("{{{head}")), "{suffix}: {body}"),
                None => assert!(!body.contains("metadataEtag"), "{suffix}: {body}"),
            }
        }
    }

    /// One writer on the failing sink.
    type Writer<'w> = &'w dyn Fn(&mut Failing) -> io::Result<()>;

    /// A sink that takes `left` bytes, then fails.
    struct Failing {
        left: usize,
    }

    impl Write for Failing {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.left == 0 {
                return Err(io::Error::other("full"));
            }
            let taken = buf.len().min(self.left);
            self.left = self.left.saturating_sub(taken);
            Ok(taken)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Bytes `write` takes from a sink that never fails.
    fn length(write: Writer<'_>) -> usize {
        let mut counting = Failing { left: usize::MAX };
        write(&mut counting).unwrap();
        usize::MAX.saturating_sub(counting.left)
    }

    /// Every writer on one sink type, under every context and page shape: the same instantiation
    /// writes every branch and fails at every byte.
    #[test]
    fn a_failing_sink_is_an_io_error_wherever_it_fails() {
        let model = model();
        let buildings = resource(&model, "Buildings");
        let pages = [
            Page::default(),
            Page {
                count: Some(1),
                next_link: Some("next"),
            },
        ];
        for (ctx, _) in every_ctx() {
            for page in &pages {
                let writers: [Writer<'_>; 5] = [
                    &|out| write_entity(&ctx, &buildings, &b1(), out),
                    &|out| write_collection(&ctx, &buildings, &[b1(), b1()], page, out),
                    &|out| write_entity_reference(&ctx, "People('p1')", out),
                    &|out| {
                        write_entity_references(&ctx, &["People('p1')", "People('p2')"], page, out)
                    },
                    &|out| write_service_document(&ctx, &model, out),
                ];
                for write in writers {
                    let length = length(write);
                    // Under Miri a sample of the bytes: the interpreter is slow, the paths the same.
                    for left in (0..length).step_by(if cfg!(miri) { 61 } else { 1 }) {
                        assert!(write(&mut Failing { left }).is_err(), "{left} of {length}");
                    }
                }
            }
        }
    }
}
