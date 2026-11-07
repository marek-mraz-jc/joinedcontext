// SPDX-License-Identifier: EUPL-1.2
//! JSON batches: the JSON Format 4.01 section 19 example, member shapes and the refused forms.

#[cfg(test)]
mod json {
    use odata_batch::{parse_json, Batch, BatchError, Feature, Limits, Method};

    fn parse(body: &str) -> Result<Batch, BatchError> {
        parse_json(body.as_bytes(), &Limits::strict())
    }

    /// JSON Format 4.01 section 19.1, the request example (its atomicity group taken out or kept).
    fn example(group: bool) -> String {
        let group = if group {
            r#""atomicityGroup":"g1","#
        } else {
            ""
        };
        format!(
            r#"{{"requests":[
              {{"id":"0","method":"get","url":"/service/Customers('ALFKI')"}},
              {{"id":"1",{group}"dependsOn":["0"],"method":"patch","url":"/service/Customers('ALFKI')",
                "headers":{{"Prefer":"return=minimal"}},"body":{{"Name":"x"}}}},
              {{"id":"2","method":"post","url":"/service/Notes","headers":{{"content-type":"text/plain"}},"body":"hi"}},
              {{"id":"3","@note":true,"method":"DELETE","url":"/service/Products(0)"}}
            ],"@trace":1}}"#
        )
    }

    #[test]
    fn the_format_example_reads_and_its_atomicity_group_is_not_implemented() {
        let batch = parse(&example(false)).unwrap();
        let shape: Vec<_> = batch
            .requests
            .iter()
            .map(|r| {
                (
                    r.id.as_deref(),
                    r.method,
                    r.headers.len(),
                    r.body.as_slice(),
                    r.depends_on.as_slice(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (Some("0"), Method::Get, 0, &b""[..], &[][..]),
                (Some("1"), Method::Patch, 1, br#"{"Name":"x"}"#, &[0][..]),
                (Some("2"), Method::Post, 1, b"hi", &[]),
                (Some("3"), Method::Delete, 0, b"", &[]),
            ]
        );
        assert_eq!(
            parse(&example(true)),
            Err(BatchError::NotImplemented(Feature::AtomicityGroup))
        );
        assert_eq!(parse(r#"{"requests":[]}"#), Ok(Batch::default()));
    }

    #[test]
    fn json_bodies_keep_their_json() {
        let body = r#"{"requests":[{"id":"1","method":"post","url":"x","headers":{"Content-Type":"application/geo+json; charset=utf-8"},"body":[1,"a"]}]}"#;
        assert_eq!(parse(body).unwrap().requests[0].body, br#"[1,"a"]"#);
    }

    #[test]
    fn numbers_in_bodies_keep_their_bits() {
        let numbers = "[5e-324,1.7976931348623157e308,0.30000000000000004,2.2250738585072014e-308,9007199254740993,-2.6255000472898025e-302]";
        for number in numbers.trim_matches(['[', ']']).split(',') {
            let body = format!(
                r#"{{"requests":[{{"id":"1","method":"post","url":"x","body":{number}}}]}}"#
            );
            let read = parse(&body).unwrap().requests[0].body.clone();
            let bits = |text: &[u8]| {
                String::from_utf8_lossy(text)
                    .parse::<f64>()
                    .map(f64::to_bits)
            };
            assert_eq!(bits(&read), bits(number.as_bytes()), "{number}");
        }
    }

    #[test]
    fn malformed_members_are_refused() {
        let member = |m: &str| format!(r#"{{"requests":[{m}]}}"#);
        for (body, refused) in [
            ("[]".to_owned(), BatchError::Malformed),
            (r#"{"requests":{}}"#.to_owned(), BatchError::Malformed),
            (r#"{"responses":[]}"#.to_owned(), BatchError::Malformed),
            (
                r#"{"requests":[], "x":1}"#.to_owned(),
                BatchError::Malformed,
            ),
            (member("1"), BatchError::Malformed),
            (
                member(r#"{"id":"1","method":"get","url":"x","extra":1}"#),
                BatchError::Malformed,
            ),
            (
                member(r#"{"method":"get","url":"x"}"#),
                BatchError::Malformed,
            ),
            (
                member(r#"{"id":1,"method":"get","url":"x"}"#),
                BatchError::Malformed,
            ),
            (
                member(r#"{"id":"1","method":"get"}"#),
                BatchError::Malformed,
            ),
            (member(r#"{"id":"1","url":"x"}"#), BatchError::Malformed),
            (
                member(r#"{"id":"1","method":"g-t","url":"x"}"#),
                BatchError::Malformed,
            ),
            (
                member(r#"{"id":"1","method":"","url":"x"}"#),
                BatchError::Malformed,
            ),
            (
                member(r#"{"id":"1","method":"head","url":"x"}"#),
                BatchError::NotImplemented(Feature::Method),
            ),
            (
                member(r#"{"id":"1","method":"get","url":"x","headers":[]}"#),
                BatchError::Malformed,
            ),
            (
                member(r#"{"id":"1","method":"get","url":"x","headers":{"a":1}}"#),
                BatchError::Malformed,
            ),
            (
                member(r#"{"id":"1","method":"get","url":"x","dependsOn":"0"}"#),
                BatchError::Malformed,
            ),
            (
                member(r#"{"id":"1","method":"get","url":"x","dependsOn":[0]}"#),
                BatchError::Malformed,
            ),
            (
                member(r#"{"id":"1","method":"get","url":"x","dependsOn":["1"]}"#),
                BatchError::UnknownDependency,
            ),
            (
                member(
                    r#"{"id":"1","method":"post","url":"x","headers":{"Content-Type":"image/png"},"body":"iVBO"}"#,
                ),
                BatchError::NotImplemented(Feature::MediaBody),
            ),
            (
                member(
                    r#"{"id":"1","method":"post","url":"x","headers":{"Content-Type":"text/plain"},"body":{}}"#,
                ),
                BatchError::NotImplemented(Feature::MediaBody),
            ),
            (
                member(r#"{"id":"1","method":"post","url":"$batch"}"#),
                BatchError::NestedBatch,
            ),
            (
                member(
                    r#"{"id":"1","method":"get","url":"x"},{"id":"2","method":"get","url":"$1?$select=a"}"#,
                ),
                BatchError::NotImplemented(Feature::ContentIdReference),
            ),
            (
                member(
                    r#"{"id":"1","method":"get","url":"x"},{"id":"1","method":"get","url":"y"}"#,
                ),
                BatchError::DuplicateId,
            ),
        ] {
            assert_eq!(parse(&body), Err(refused), "{body}");
        }
        assert_eq!(
            parse("{\n \"requests\": ["),
            Err(BatchError::Json {
                line: 2,
                column: 14
            })
        );
    }

    #[test]
    fn errors_carry_a_status_and_fixed_text() {
        assert_eq!(BatchError::Malformed.status(), 400);
        assert_eq!(
            BatchError::limit(odata_batch::BatchLimit::BodyBytes, 1).status(),
            413
        );
        assert_eq!(BatchError::NotImplemented(Feature::Method).status(), 501);
        assert_eq!(
            BatchError::limit(odata_batch::BatchLimit::Requests, 1).status(),
            400
        );
        let texts: Vec<String> = [
            BatchError::limit(odata_batch::BatchLimit::UrlLength, 7),
            BatchError::ContentType,
            BatchError::PartType { offset: 1 },
            BatchError::RequestLine { offset: 2 },
            BatchError::ContentLength { offset: 3 },
            BatchError::Json { line: 4, column: 5 },
            BatchError::Malformed,
            BatchError::DuplicateId,
            BatchError::UnknownDependency,
            BatchError::NestedBatch,
            BatchError::NotImplemented(Feature::MediaBody),
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            texts,
            [
                "the batch exceeds the URL length limit of 7",
                "the batch is not multipart/mixed with a valid boundary",
                "a part is not a binary application/http request (byte 1)",
                "a member's request line is not METHOD url HTTP/1.1 (byte 2)",
                "a member's Content-Length disagrees with its body (byte 3)",
                "the batch is not JSON (line 4, column 5)",
                "the batch is not shaped as JSON Format section 19 asks",
                "two members have the same id",
                "dependsOn names no earlier member",
                "a batch member is itself a batch",
                "not implemented: binary member bodies",
            ]
        );
        let names: Vec<_> = [
            Feature::AtomicityGroup,
            Feature::ContentIdReference,
            Feature::Method,
        ]
        .map(Feature::as_str)
        .into();
        assert_eq!(
            names,
            [
                "atomicity groups",
                "references to another member's result",
                "this method in a batch"
            ]
        );
        let limits: Vec<_> = [
            odata_batch::BatchLimit::BodyBytes,
            odata_batch::BatchLimit::Requests,
            odata_batch::BatchLimit::HeaderBytes,
            odata_batch::BatchLimit::Headers,
            odata_batch::BatchLimit::DependsOn,
            odata_batch::BatchLimit::IdLength,
            odata_batch::BatchLimit::JsonDepth,
        ]
        .map(odata_batch::BatchLimit::as_str)
        .into();
        assert_eq!(
            limits,
            [
                "body size",
                "members",
                "header block size",
                "headers per member",
                "dependsOn entries",
                "id length",
                "JSON nesting depth"
            ]
        );
        assert_eq!(Feature::ChangeSet.as_str(), "change sets");
        assert_eq!(
            BatchError::Framing { offset: 4 }.to_string(),
            "the multipart body is not framed as RFC 2046 asks (byte 4)"
        );
        let error: &dyn std::error::Error = &BatchError::NestedBatch;
        assert!(error.source().is_none());
    }
}
