// SPDX-License-Identifier: EUPL-1.2
//! Threat model O22: every bound of `BodyLimits` refuses the first body over it, through every
//! reader, and takes the body at it.

#[cfg(test)]
mod service;

#[cfg(test)]
mod o22 {

    use crate::service::{minimal, model, resource};
    use odata_json::{
        read_entity_body, read_property_body, read_reference_body, BodyError, BodyLimit,
        BodyLimits, WriteKind,
    };

    fn tiny() -> BodyLimits {
        BodyLimits {
            max_bytes: 120,
            max_depth: 3,
            max_properties: 4,
            max_bind_links: 2,
            max_collection_len: 3,
            bind_url: odata_url::Limits::strict(),
        }
    }

    /// What each reader answers for `body`: entity update, property `tags`, reference `parts`.
    fn answers(body: &str) -> [Option<BodyError>; 3] {
        let model = model();
        let buildings = resource(&model, "Buildings");
        let bytes = body.as_bytes();
        [
            read_entity_body(&minimal(), &buildings, WriteKind::Update, bytes, &tiny()).err(),
            read_property_body(&buildings, "tags", bytes, &tiny()).err(),
            read_reference_body(&minimal(), &buildings, "parts", bytes, &tiny()).err(),
        ]
    }

    #[test]
    fn every_body_limit_is_refused() {
        let limit = |limit, max| Some(BodyError::LimitExceeded { limit, max });
        for (body, refused) in [
            (
                format!(r#"{{"value":["{}"]}}"#, "a".repeat(107)),
                limit(BodyLimit::Bytes, 120),
            ),
            (
                r#"{"value":[[[1]]]}"#.to_owned(),
                limit(BodyLimit::Depth, 3),
            ),
            (
                r#"{"value":[{"a":1,"b":2,"c":3,"d":4}]}"#.to_owned(),
                limit(BodyLimit::Properties, 4),
            ),
            (
                r#"{"value":[1,2,3,4]}"#.to_owned(),
                limit(BodyLimit::CollectionLength, 3),
            ),
        ] {
            assert_eq!(
                answers(&body),
                [refused.clone(), refused.clone(), refused],
                "{body}"
            );
        }
        let three = r#"{"value":[{"@id":"Buildings('a')"},{"@id":"Buildings('b')"},{"@id":"Buildings('c')"}]}"#;
        assert_eq!(answers(three)[2], limit(BodyLimit::BindLinks, 2));
    }

    #[test]
    fn a_body_at_every_bound_is_taken() {
        let model = model();
        let buildings = resource(&model, "Buildings");
        let at_bytes = format!(r#"{{"value":["{}"]}}"#, "a".repeat(106));
        assert_eq!(at_bytes.len(), 120);
        assert!(read_property_body(&buildings, "tags", at_bytes.as_bytes(), &tiny()).is_ok());
        let at_depth = br#"{"address":{"floors":[1,2,3]},"tags":["a"],"name":"n"}"#;
        assert!(
            read_entity_body(&minimal(), &buildings, WriteKind::Update, at_depth, &tiny()).is_ok()
        );
        let two = br#"{"value":[{"@id":"Buildings('a')"},{"@id":"Buildings('b')"}]}"#;
        assert!(read_reference_body(&minimal(), &buildings, "parts", two, &tiny()).is_ok());
    }
}
