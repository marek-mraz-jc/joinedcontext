// SPDX-License-Identifier: EUPL-1.2
//! `multipart/mixed` batches: the Protocol 4.01 section 11.7.7 examples, and every framing a
//! second parser could read differently refused (threat model O10).

#[cfg(test)]
mod o10 {
    use odata_batch::{boundary, parse_multipart, BatchError, Feature, Limits, Method};

    const B: &str = "batch_36522ad7-fc75-4b56-8c71-56071383e77b";

    /// Protocol 4.01 section 11.7.7, example 63 (the change set in the middle removed or kept).
    fn example(with_change_set: bool) -> String {
        let change_set = "--batch_36522ad7-fc75-4b56-8c71-56071383e77b\r\n\
            Content-Type: multipart/mixed; boundary=changeset_77162fcd-b8da-41ac-a9f8-9357efbbd\r\n\
            \r\n\
            --changeset_77162fcd-b8da-41ac-a9f8-9357efbbd\r\n\
            Content-Type: application/http\r\n\
            Content-ID: 1\r\n\
            \r\n\
            POST /service/Customers HTTP/1.1\r\n\
            Host: host\r\n\
            Content-Type: application/json\r\n\
            \r\n\
            {}\r\n\
            --changeset_77162fcd-b8da-41ac-a9f8-9357efbbd--\r\n";
        format!(
            "--{B}\r\n\
            Content-Type: application/http\r\n\
            Content-Transfer-Encoding: binary\r\n\
            \r\n\
            GET /service/Customers('ALFKI') HTTP/1.1\r\n\
            Host: host\r\n\
            \r\n\
            {}\
            --{B}\r\n\
            Content-Type: application/http\r\n\
            \r\n\
            GET /service/Products HTTP/1.1\r\n\
            Host: host\r\n\
            \r\n\
            --{B}--\r\n",
            if with_change_set { change_set } else { "" }
        )
    }

    fn parse(body: &str) -> Result<odata_batch::Batch, BatchError> {
        parse_multipart(body.as_bytes(), "b", &Limits::strict())
    }

    fn one(part_headers: &str, request: &str) -> String {
        format!("--b\r\n{part_headers}\r\n{request}\r\n--b--")
    }

    const HTTP: &str = "Content-Type: application/http\r\n";

    #[test]
    fn the_protocol_example_reads_and_its_change_set_is_not_implemented() {
        let ct = format!("multipart/mixed; boundary={B}");
        let batch = parse_multipart(
            example(false).as_bytes(),
            &boundary(&ct).unwrap(),
            &Limits::strict(),
        )
        .unwrap();
        let urls: Vec<_> = batch
            .requests
            .iter()
            .map(|r| (r.method, r.url.as_str()))
            .collect();
        assert_eq!(
            urls,
            [
                (Method::Get, "/service/Customers('ALFKI')"),
                (Method::Get, "/service/Products")
            ]
        );
        assert_eq!(
            batch.requests[0].headers,
            [("Host".to_owned(), "host".to_owned())]
        );
        assert_eq!(
            parse_multipart(example(true).as_bytes(), B, &Limits::strict()),
            Err(BatchError::NotImplemented(Feature::ChangeSet))
        );
    }

    #[test]
    fn members_read_with_and_without_headers_and_bodies() {
        let body = format!(
            "--b \t\r\n{HTTP}Content-ID: a\r\n\r\nPOST Buildings HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 8\r\n\r\n{{\"id\":1}}\r\n\
             --b\r\n{HTTP}\r\nDELETE Buildings('b1')\r\n\
             --b\r\n{HTTP}\r\nGET Buildings\r\n\r\n\
             --b\r\n{HTTP}\r\nPUT Buildings('b2') HTTP/1.1\r\n\r\nraw\r\n\
             --b\r\n{HTTP}\r\nPATCH Buildings('b3') HTTP/1.1\r\nPrefer: return=minimal\r\n\
             --b\r\n{HTTP}\r\nGET People HTTP/1.1\r\nAccept:application/json\r\n--b--"
        );
        let batch = parse(&body).unwrap();
        let shape: Vec<_> = batch
            .requests
            .iter()
            .map(|r| {
                (
                    r.id.as_deref(),
                    r.method,
                    r.url.as_str(),
                    r.headers.len(),
                    r.body.as_slice(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (Some("a"), Method::Post, "Buildings", 2, &b"{\"id\":1}"[..]),
                (None, Method::Delete, "Buildings('b1')", 0, b""),
                (None, Method::Get, "Buildings", 0, b""),
                (None, Method::Put, "Buildings('b2')", 0, b"raw"),
                (None, Method::Patch, "Buildings('b3')", 1, b""),
                (None, Method::Get, "People", 1, b""),
            ]
        );
        assert_eq!(parse("--b--"), Ok(odata_batch::Batch::default()));
        assert_eq!(parse("--b--\r\n"), Ok(odata_batch::Batch::default()));
    }

    #[test]
    fn bare_lf_framing_reads_and_bodies_keep_their_bytes() {
        // The NLstn compliance suite's body: Go text with LF line ends.
        let suite = "--b\nContent-Type: application/http\nContent-Transfer-Encoding: binary\n\n\
                     GET Products HTTP/1.1\nAccept: application/json\n\n\n--b--";
        let batch = parse(suite).unwrap();
        assert_eq!(batch.requests[0].url, "Products");
        assert_eq!(
            batch.requests[0].headers,
            [("Accept".to_owned(), "application/json".to_owned())]
        );
        assert_eq!(batch.requests[0].body, b"");
        let bodies = |body: &str| -> Vec<Vec<u8>> {
            parse(body)
                .unwrap()
                .requests
                .into_iter()
                .map(|r| r.body)
                .collect()
        };
        // LF framing, CRLF framing and both in one batch read the same parts; a body keeps its
        // CRLF and LF, and only the line end before the delimiter belongs to the framing.
        assert_eq!(
            bodies("--b\n{HTTP}\n\nPOST x\n\na\r\nb\nc\n--b\nContent-Type: application/http\r\n\r\nPOST y\r\nContent-Length: 2\n\nd\n\r\n--b--\n"
                .replace("{HTTP}", "Content-Type: application/http")
                .as_str()),
            [b"a\r\nb\nc".to_vec(), b"d\n".to_vec()]
        );
        assert_eq!(parse("--b--\n"), Ok(odata_batch::Batch::default()));
        // A bare LF before the boundary in a body is a delimiter now: the part is cut there.
        assert_eq!(
            parse(&format!(
                "--b\r\n{HTTP}\r\nPOST x\r\n\r\nab\n--b\r\ncd\r\n--b--"
            )),
            Err(BatchError::Framing { offset: 57 })
        );
    }

    #[test]
    fn ambiguous_framing_is_refused() {
        let framing = |offset| Err(BatchError::Framing { offset });
        for (body, refused) in [
            // A lone CR, in the framing and in a header block.
            (
                "--b\rContent-Type: application/http\r\n\r\nGET x\r\n--b--".to_owned(),
                framing(3),
            ),
            (
                one("Content-Type: application/http\rContent-ID: 1\r\n", "GET x"),
                framing(5),
            ),
            (one(HTTP, "GET x HTTP/1.1\r\nA: 1\rB: 2\r\n"), framing(39)),
            // Text outside the parts: a preamble, an epilogue, an unterminated body.
            ("preamble\r\n--b--".to_owned(), framing(0)),
            ("--b--\r\nepilogue".to_owned(), framing(3)),
            ("--b--x".to_owned(), framing(3)),
            (format!("--b\r\n{HTTP}\r\nGET x"), framing(5)),
            // A delimiter followed by anything but `--` or CRLF.
            (
                format!("--b\r\n{HTTP}\r\nGET x\r\n--bX\r\n--b--"),
                framing(49),
            ),
            // A delimiter inside a body cuts the part; the rest is not a part.
            (
                format!("--b\r\n{HTTP}\r\nPOST x\r\n\r\nab\r\n--b\r\ncd\r\n--b--"),
                framing(58),
            ),
            // No header block end, folding, a name that is not a token, a non-UTF-8 header.
            (
                "--b\r\nContent-Type: application/http\r\n--b--".to_owned(),
                framing(5),
            ),
            (
                one("Content-Type: application/http\r\n x\r\n", "GET x"),
                framing(5),
            ),
            (
                one("Content-Type: application/http\r\n x: y\r\n", "GET x"),
                framing(5),
            ),
            (
                one("Content-Type: application/http\r\nA\u{e9}: y\r\n", "GET x"),
                framing(5),
            ),
            (one(HTTP, "GET x HTTP/1.1\r\nno colon\r\n"), framing(39)),
            (one(HTTP, "GET x HTTP/1.1\r\n: empty name\r\n"), framing(39)),
        ] {
            assert_eq!(parse(&body), refused, "{body:?}");
        }
        let latin1 = b"--b\r\nContent-Type: application/http\r\nA: \xe9\r\n\r\nGET x\r\n--b--";
        assert_eq!(parse_multipart(latin1, "b", &Limits::strict()), framing(5));
    }

    #[test]
    fn parts_must_be_binary_http_requests() {
        for (body, refused) in [
            (one("Content-Type: text/plain\r\n", "GET x"), BatchError::PartType { offset: 5 }),
            (one("Content-ID: 1\r\n", "GET x"), BatchError::PartType { offset: 5 }),
            (
                one("Content-Type: application/http\r\nContent-Transfer-Encoding: base64\r\n", "GET x"),
                BatchError::PartType { offset: 5 },
            ),
            (one(HTTP, "GET"), BatchError::RequestLine { offset: 39 }),
            (one(HTTP, "GET "), BatchError::RequestLine { offset: 39 }),
            (one(HTTP, "GET  x"), BatchError::RequestLine { offset: 39 }),
            (one(HTTP, "GET x HTTP/1.0"), BatchError::RequestLine { offset: 39 }),
            (one(HTTP, "GET x HTTP/1.1 extra"), BatchError::RequestLine { offset: 39 }),
            (one(HTTP, "get x"), BatchError::RequestLine { offset: 39 }),
            (one(HTTP, " x"), BatchError::RequestLine { offset: 39 }),
            (one(HTTP, "OPTIONS x"), BatchError::NotImplemented(Feature::Method)),
            (one(HTTP, "POST x\r\nContent-Length: 3\r\n\r\nab"), BatchError::ContentLength { offset: 5 }),
            (one(HTTP, "POST x\r\nContent-Length: two\r\n\r\nab"), BatchError::ContentLength { offset: 5 }),
            (one(HTTP, "GET Buildings/$batch"), BatchError::NestedBatch),
            (one(HTTP, "POST $batch?x=1"), BatchError::NestedBatch),
            (
                format!("--b\r\n{HTTP}Content-ID: 1\r\n\r\nGET x\r\n--b\r\n{HTTP}\r\nGET $1/owner\r\n--b--"),
                BatchError::NotImplemented(Feature::ContentIdReference),
            ),
            (
                format!("--b\r\n{HTTP}Content-ID: 1\r\n\r\nGET x\r\n--b\r\n{HTTP}Content-ID: 1\r\n\r\nGET y\r\n--b--"),
                BatchError::DuplicateId,
            ),
        ] {
            assert_eq!(parse(&body), Err(refused), "{body:?}");
        }
        let latin1 = b"--b\r\nContent-Type: application/http\r\n\r\nGET \xe9\r\n--b--";
        assert_eq!(
            parse_multipart(latin1, "b", &Limits::strict()),
            Err(BatchError::RequestLine { offset: 39 })
        );
        assert!(parse(&one(HTTP, "GET $metadata")).is_ok());
        assert!(parse(&one(HTTP, "GET $2/x")).is_ok());
    }

    #[test]
    fn boundaries_follow_rfc_2046() {
        for (content_type, read) in [
            ("multipart/mixed;boundary=b", Ok("b")),
            (
                "MULTIPART/MIXED ; charset=x; BOUNDARY = \"a b'()+_,-./:=?\"",
                Ok("a b'()+_,-./:=?"),
            ),
            (
                "multipart/mixed; boundary=\"\"",
                Err(BatchError::ContentType),
            ),
            (
                "multipart/mixed; boundary=\"b \"",
                Err(BatchError::ContentType),
            ),
            ("multipart/mixed; boundary=b;", Ok("b")),
            (
                "multipart/mixed; boundary=a{b",
                Err(BatchError::ContentType),
            ),
            ("multipart/mixed; charset=x", Err(BatchError::ContentType)),
            (
                "multipart/related; boundary=b",
                Err(BatchError::ContentType),
            ),
        ] {
            assert_eq!(
                boundary(content_type).as_deref().map_err(Clone::clone),
                read,
                "{content_type}"
            );
        }
        assert_eq!(
            boundary(&format!("multipart/mixed; boundary={}", "a".repeat(71))),
            Err(BatchError::ContentType)
        );
        assert_eq!(
            parse_multipart(b"--\r\n", "", &Limits::strict()),
            Err(BatchError::ContentType)
        );
    }
}
