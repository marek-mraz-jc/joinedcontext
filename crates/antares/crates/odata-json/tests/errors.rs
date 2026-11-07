// SPDX-License-Identifier: EUPL-1.2
//! The error body (JSON Format 4.01 section 21.1) and the reader's errors as the surface renders
//! them: fixed text, a declared target at most, and never a value from the request (threat
//! model O20, property over random inputs).

#[cfg(test)]
mod service;

#[cfg(test)]
mod o20 {

    use crate::service::{minimal, model, resource};
    use odata_json::{
        read_entity_body, read_property_body, read_reference_body, write_error, BodyError,
        BodyLimit, BodyLimits, ErrorDetail, Feature, ODataError, WriteKind,
    };
    use proptest::prelude::*;

    fn body(error: &ODataError) -> String {
        let mut out = Vec::new();
        write_error(error, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    type Case = (
        BodyError,
        u16,
        &'static str,
        Option<&'static str>,
        &'static str,
    );

    fn rendered(cases: Vec<Case>) {
        for (error, status, code, target, message) in cases {
            let odata = ODataError::from(&error);
            assert_eq!(error.status(), status, "{error:?}");
            assert_eq!(
                (odata.code.as_str(), odata.target.as_deref()),
                (code, target),
                "{error:?}"
            );
            assert_eq!(odata.message, message);
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn body_errors_without_a_target_have_a_code_a_status_and_fixed_text() {
        rendered(vec![
            (
                BodyError::LimitExceeded {
                    limit: BodyLimit::Bytes,
                    max: 9,
                },
                413,
                "LimitExceeded",
                None,
                "the request body exceeds the body size limit of 9",
            ),
            (
                BodyError::LimitExceeded {
                    limit: BodyLimit::Depth,
                    max: 2,
                },
                400,
                "LimitExceeded",
                None,
                "the request body exceeds the nesting depth limit of 2",
            ),
            (
                BodyError::Syntax { line: 1, column: 2 },
                400,
                "MalformedBody",
                None,
                "the request body is not JSON (line 1, column 2)",
            ),
            (
                BodyError::DuplicateMember,
                400,
                "MalformedBody",
                None,
                "the request body names a member twice",
            ),
            (
                BodyError::NotAnObject,
                400,
                "MalformedBody",
                None,
                "the request body is not a JSON object",
            ),
            (
                BodyError::TypeMismatch,
                400,
                "TypeMismatch",
                None,
                "the request body names another type",
            ),
            (
                BodyError::UnknownProperty,
                400,
                "UnknownProperty",
                None,
                "the request body names an unknown property",
            ),
        ]);
    }

    #[test]
    fn body_errors_with_a_target_name_a_declared_property() {
        let named = |property: &str| property.to_owned();
        rendered(vec![
            (
                BodyError::InvalidValue {
                    property: named("a/b"),
                },
                400,
                "InvalidValue",
                Some("a/b"),
                "a value does not have the declared type",
            ),
            (
                BodyError::NullNotAllowed {
                    property: named("a"),
                },
                400,
                "InvalidValue",
                Some("a"),
                "a property that is not nullable is null",
            ),
            (
                BodyError::MissingKey {
                    property: named("id"),
                },
                400,
                "MissingKey",
                Some("id"),
                "the key property is missing",
            ),
            (
                BodyError::MissingProperty {
                    property: named("name"),
                },
                400,
                "MissingProperty",
                Some("name"),
                "a required property is missing",
            ),
            (
                BodyError::ReadOnly {
                    property: named("c"),
                },
                400,
                "ReadOnly",
                Some("c"),
                "the property cannot be written",
            ),
            (
                BodyError::InvalidBind {
                    navigation: named("owner"),
                },
                400,
                "InvalidBind",
                Some("owner"),
                "the bind does not address an entity this navigation property reaches",
            ),
            (
                BodyError::NotImplemented(Feature::DeepInsert),
                501,
                "NotImplemented",
                None,
                "deep insert is not implemented",
            ),
            (
                BodyError::NotImplemented(Feature::DeepUpdate),
                501,
                "NotImplemented",
                None,
                "deep update is not implemented",
            ),
            (
                BodyError::NotImplemented(Feature::CollectionBind),
                501,
                "NotImplemented",
                None,
                "binding a collection-valued navigation property is not implemented",
            ),
            (
                BodyError::NotImplemented(Feature::Unbind),
                501,
                "NotImplemented",
                None,
                "binding a navigation property to null is not implemented",
            ),
        ]);
        for (limit, name) in [
            (BodyLimit::Properties, "properties"),
            (BodyLimit::BindLinks, "bind links"),
            (BodyLimit::CollectionLength, "collection length"),
        ] {
            assert_eq!(limit.as_str(), name);
        }
    }

    #[test]
    fn the_error_body_has_no_innererror_and_its_details() {
        let mut error = ODataError::new("BadRequest", "the request body is not valid");
        error.target = Some("height".into());
        error.details = vec![
            ErrorDetail {
                code: "InvalidValue".into(),
                message: "wrong type".into(),
                target: Some("height".into()),
            },
            ErrorDetail {
                code: "Other".into(),
                message: "more".into(),
                target: None,
            },
        ];
        assert_eq!(
            body(&error),
            r#"{"error":{"code":"BadRequest","details":[{"code":"InvalidValue","message":"wrong type","target":"height"},{"code":"Other","message":"more"}],"message":"the request body is not valid","target":"height"}}"#
        );
        assert!(!body(&ODataError::new("a", "b")).contains("details"));
    }

    /// Every failing body built around `secret`, read the way the surface reads it.
    fn refusals(secret: &str) -> Vec<BodyError> {
        let model = model();
        let buildings = resource(&model, "Buildings");
        let ctx = minimal();
        let entity = [
            format!(r#"{{"{secret}":1}}"#),
            format!(r#"{{"height":"{secret}"}}"#),
            format!(r#"{{"address":{{"{secret}":1}}}}"#),
            format!(r##"{{"@odata.type":"#City.{secret}"}}"##),
            format!(r#"{{"owner@odata.bind":"Buildings('{secret}')"}}"#),
            format!(r#"{{"owner@odata.bind":"{secret}"}}"#),
            format!(r#"{{"{secret}@odata.bind":"People('p')"}}"#),
            format!(r#"{{"owner":{{"id":"{secret}"}}}}"#),
            format!(r#"{{"id":"{secret}"#),
            format!(r#"{{"id":"{secret}","id":"{secret}"}}"#),
            format!(r#"["{secret}"]"#),
        ];
        let mut errors: Vec<BodyError> = entity
            .iter()
            .filter_map(|text| {
                read_entity_body(
                    &ctx,
                    &buildings,
                    WriteKind::Update,
                    text.as_bytes(),
                    &BodyLimits::strict(),
                )
                .err()
            })
            .collect();
        errors.extend(
            read_property_body(
                &buildings,
                "height",
                format!(r#"{{"value":"{secret}"}}"#).as_bytes(),
                &BodyLimits::strict(),
            )
            .err(),
        );
        errors.extend(
            read_property_body(&buildings, secret, br#"{"value":1}"#, &BodyLimits::strict()).err(),
        );
        let reference = format!(r#"{{"@id":"People('{secret}')/x"}}"#);
        errors.extend(
            read_reference_body(
                &ctx,
                &buildings,
                "owner",
                reference.as_bytes(),
                &BodyLimits::strict(),
            )
            .err(),
        );
        errors.extend(
            read_reference_body(&ctx, &buildings, secret, b"{}", &BodyLimits::strict()).err(),
        );
        errors
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(if cfg!(miri) { 2 } else { 256 }))]

        #[test]
        fn the_error_body_never_contains_a_request_value(tail in "[0-9a-z]{4,12}") {
            let secret = format!("zq{tail}");
            let errors = refusals(&secret);
            prop_assert_eq!(errors.len(), 15);
            for error in errors {
                let written = body(&ODataError::from(&error));
                prop_assert!(!written.contains(&secret), "{}", written);
                prop_assert!(!error.to_string().contains(&secret), "{}", error);
            }
        }
    }
}
