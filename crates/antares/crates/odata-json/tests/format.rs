// SPDX-License-Identifier: EUPL-1.2
//! Format parameters, context URLs, version negotiation and `Prefer` (JSON Format 4.01 sections
//! 3 and 10; Protocol 4.01 sections 8.2.6 to 8.2.8).

#[cfg(test)]
mod headers {
    use odata_json::{
        context_url, negotiate_version, parse_prefer, ContextTarget, Ctx, HeaderError,
        MetadataLevel, ODataVersion, Prefer,
    };

    const ROOT: &str = "https://example.org/odata/";

    fn minimal() -> Ctx<'static> {
        Ctx {
            service_root: ROOT,
            level: MetadataLevel::Minimal,
            version: ODataVersion::V4_0,
            metadata_etag: None,
        }
    }

    #[test]
    fn metadata_levels_and_versions_read_and_write_themselves() {
        for (text, level) in [
            ("none", Some(MetadataLevel::None)),
            ("Minimal", Some(MetadataLevel::Minimal)),
            ("FULL", Some(MetadataLevel::Full)),
            ("", None),
            ("fully", None),
        ] {
            assert_eq!(MetadataLevel::parse(text), level, "{text}");
        }
        assert_eq!(ODataVersion::V4_0.to_string(), "4.0");
        assert_eq!(ODataVersion::V4_01.to_string(), "4.01");
    }

    #[test]
    fn context_urls_name_their_target() {
        let names = ["id", "height"];
        for (target, url) in [
            (ContextTarget::ServiceDocument, "$metadata"),
            (
                ContextTarget::Collection {
                    set: "",
                    select: None,
                },
                "$metadata#",
            ),
            (
                ContextTarget::Collection {
                    set: "Buildings",
                    select: None,
                },
                "$metadata#Buildings",
            ),
            (
                ContextTarget::Collection {
                    set: "Buildings",
                    select: Some(&names),
                },
                "$metadata#Buildings(id,height)",
            ),
            (
                ContextTarget::Entity {
                    set: "Buildings",
                    select: None,
                },
                "$metadata#Buildings/$entity",
            ),
            (
                ContextTarget::Entity {
                    set: "Buildings",
                    select: Some(&["*"]),
                },
                "$metadata#Buildings(*)/$entity",
            ),
            (ContextTarget::Reference, "$metadata#$ref"),
            (ContextTarget::References, "$metadata#Collection($ref)"),
        ] {
            assert_eq!(context_url(&minimal(), &target), format!("{ROOT}{url}"));
        }
        assert_eq!(ContextTarget::ServiceDocument.to_string(), "");
    }

    #[test]
    fn the_response_version_is_the_highest_both_sides_speak() {
        use HeaderError::{Malformed, UnsupportedVersion};
        use ODataVersion::{V4_0, V4_01};
        for (version, max, negotiated) in [
            (None, None, Ok(V4_0)),
            (Some("4.0"), None, Ok(V4_0)),
            (Some(" 4.01 "), None, Ok(V4_01)),
            (None, Some("4.0"), Ok(V4_0)),
            (Some("4.01"), Some("4.0"), Ok(V4_0)),
            (None, Some("4.01"), Ok(V4_01)),
            (None, Some("4.1"), Ok(V4_01)),
            (None, Some("5.0"), Ok(V4_01)),
            (None, Some("3.0"), Err(UnsupportedVersion)),
            (None, Some("3.99"), Err(UnsupportedVersion)),
            (Some("4.02"), None, Err(UnsupportedVersion)),
            (Some("3.0"), None, Err(UnsupportedVersion)),
            (Some("4"), None, Err(Malformed)),
            (Some("4."), None, Err(Malformed)),
            (Some(".1"), None, Err(Malformed)),
            (Some("4.001"), None, Err(Malformed)),
            (Some("400.0"), None, Err(Malformed)),
            (Some("4.x"), None, Err(Malformed)),
            (None, Some("99.99"), Ok(V4_01)),
            (None, Some("4.0.1"), Err(Malformed)),
        ] {
            assert_eq!(
                negotiate_version(version, max),
                negotiated,
                "{version:?} {max:?}"
            );
        }
        for (error, text) in [
            (
                HeaderError::TooLong,
                "the header is longer than this service reads",
            ),
            (Malformed, "the header is not a version number"),
            (
                UnsupportedVersion,
                "the requested OData version is not supported",
            ),
        ] {
            assert_eq!(error.to_string(), text);
        }
    }

    #[test]
    fn prefer_keeps_the_first_readable_instance_of_each_mapped_preference() {
        let prefer = |maxpagesize, continue_on_error| Prefer {
            maxpagesize,
            continue_on_error,
        };
        for (header, expected) in [
            ("", prefer(None, false)),
            ("odata.maxpagesize=25", prefer(Some(25), false)),
            (
                "MAXPAGESIZE = \"7\" ; odata.track-changes",
                prefer(Some(7), false),
            ),
            (
                "odata.maxpagesize=0, odata.maxpagesize=5",
                prefer(None, false),
            ),
            ("odata.maxpagesize=x", prefer(None, false)),
            ("odata.maxpagesize", prefer(None, false)),
            (
                "respond-async, wait=\"10,20\", maxpagesize=3",
                prefer(Some(3), false),
            ),
            ("x=\"a\\\"b,c\", maxpagesize=4", prefer(Some(4), false)),
            ("maxpagesize=\"\\9\"", prefer(Some(9), false)),
            ("a\\b, x=\"\\\\\", maxpagesize=2", prefer(Some(2), false)),
            ("maxpagesize=\"", prefer(None, false)),
            ("odata.continue-on-error", prefer(None, true)),
            ("continue-on-error=TRUE", prefer(None, true)),
            (
                "continue-on-error=false, continue-on-error",
                prefer(None, false),
            ),
        ] {
            assert_eq!(parse_prefer(header, 64), Ok(expected), "{header}");
        }
        assert_eq!(parse_prefer(&"a".repeat(65), 64), Err(HeaderError::TooLong));
    }
}
