// SPDX-License-Identifier: EUPL-1.2
//! The response writers: both formats, every refusal that keeps a response from changing the
//! framing, and a sink that fails at every byte.

#[cfg(test)]
mod write {
    use std::io::{self, Write};

    use odata_batch::{write_json, write_multipart, Response};

    /// A sink that takes `left` bytes, then fails; `usize::MAX` never fails.
    struct Sink {
        left: usize,
        out: Vec<u8>,
    }

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.left == 0 {
                return Err(io::Error::other("full"));
            }
            let n = buf.len().min(self.left);
            self.left = self.left.saturating_sub(n);
            self.out.extend_from_slice(buf.get(..n).unwrap_or_default());
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn sink() -> Sink {
        Sink {
            left: usize::MAX,
            out: Vec::new(),
        }
    }

    fn response(id: Option<&str>, status: u16, headers: &[(&str, &str)], body: &[u8]) -> Response {
        Response {
            id: id.map(str::to_owned),
            status,
            headers: headers
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect(),
            body: body.to_vec(),
        }
    }

    fn responses() -> Vec<Response> {
        vec![
            response(
                Some("1"),
                201,
                &[
                    ("Content-Type", "application/json"),
                    ("Location", "Buildings('b1')"),
                ],
                br#"{"id":"b1"}"#,
            ),
            response(None, 204, &[], b""),
            response(
                Some("3"),
                404,
                &[("Content-Type", "text/plain")],
                b"no such building",
            ),
            response(Some("4"), 299, &[], b"not json"),
        ]
    }

    fn multipart(responses: &[Response]) -> io::Result<Vec<u8>> {
        let mut out = sink();
        write_multipart(responses, "r", &mut out)?;
        Ok(out.out)
    }

    fn json(responses: &[Response]) -> io::Result<Vec<u8>> {
        let mut out = sink();
        write_json(responses, &mut out)?;
        Ok(out.out)
    }

    #[test]
    fn multipart_responses_are_framed_and_echo_the_content_id() {
        let text = String::from_utf8(multipart(&responses()).unwrap()).unwrap();
        assert_eq!(
            text,
            "--r\r\nContent-Type: application/http\r\nContent-ID: 1\r\n\r\nHTTP/1.1 201 Created\r\n\
             Content-Type: application/json\r\nLocation: Buildings('b1')\r\n\r\n{\"id\":\"b1\"}\r\n\
             --r\r\nContent-Type: application/http\r\n\r\nHTTP/1.1 204 No Content\r\n\r\n\r\n\
             --r\r\nContent-Type: application/http\r\nContent-ID: 3\r\n\r\nHTTP/1.1 404 Not Found\r\n\
             Content-Type: text/plain\r\n\r\nno such building\r\n\
             --r\r\nContent-Type: application/http\r\nContent-ID: 4\r\n\r\nHTTP/1.1 299 \r\n\r\nnot json\r\n\
             --r--\r\n"
        );
        assert_eq!(multipart(&[]).unwrap(), b"--r--\r\n");
    }

    #[test]
    fn every_reason_phrase_is_the_rfc_one() {
        let phrases: Vec<String> = [
            200, 201, 204, 400, 401, 403, 404, 405, 409, 412, 413, 424, 500, 501, 503,
        ]
        .into_iter()
        .map(|status| {
            let text =
                String::from_utf8(multipart(&[response(None, status, &[], b"")]).unwrap()).unwrap();
            text.lines().nth(3).unwrap_or_default().to_owned()
        })
        .collect();
        assert_eq!(
            phrases,
            [
                "HTTP/1.1 200 OK",
                "HTTP/1.1 201 Created",
                "HTTP/1.1 204 No Content",
                "HTTP/1.1 400 Bad Request",
                "HTTP/1.1 401 Unauthorized",
                "HTTP/1.1 403 Forbidden",
                "HTTP/1.1 404 Not Found",
                "HTTP/1.1 405 Method Not Allowed",
                "HTTP/1.1 409 Conflict",
                "HTTP/1.1 412 Precondition Failed",
                "HTTP/1.1 413 Content Too Large",
                "HTTP/1.1 424 Failed Dependency",
                "HTTP/1.1 500 Internal Server Error",
                "HTTP/1.1 501 Not Implemented",
                "HTTP/1.1 503 Service Unavailable",
            ]
        );
    }

    #[test]
    fn a_response_cannot_change_the_framing() {
        let refused = |r: Response| {
            let m = multipart(std::slice::from_ref(&r)).map_err(|e| e.kind());
            assert_eq!(m, Err(io::ErrorKind::InvalidInput), "{r:?}");
        };
        refused(response(None, 200, &[("A", "1\r\n--r--")], b""));
        refused(response(None, 200, &[("A", "1\n")], b""));
        refused(response(None, 200, &[("A:", "1")], b""));
        refused(response(None, 200, &[("", "1")], b""));
        refused(response(None, 200, &[("A B", "1")], b""));
        refused(response(Some("1\r\nX: y"), 200, &[], b""));
        refused(response(Some("1\n"), 200, &[], b""));
        refused(response(None, 200, &[], b"a\r\n--r--\r\n"));
        refused(response(None, 200, &[], b"--r--"));
        // A reader that takes LF framing would cut here too.
        refused(response(None, 200, &[], b"a\n--r"));
        let mut out = sink();
        let good_then_bad = [
            response(None, 200, &[], b""),
            response(None, 200, &[], b"--r"),
        ];
        assert!(write_multipart(&good_then_bad, "r", &mut out).is_err());
        assert!(out.out.is_empty(), "nothing is written before the check");
        for boundary in ["", "a{", "b ", &"a".repeat(71)] {
            assert_eq!(
                write_multipart(&[], boundary, sink()).map_err(|e| e.kind()),
                Err(io::ErrorKind::InvalidInput)
            );
        }
        // The delimiter without its leading CRLF inside a body is plain text.
        assert!(multipart(&[response(None, 200, &[], b"a--r--")]).is_ok());
    }

    #[test]
    fn json_responses_keep_section_19_member_order() {
        let text = String::from_utf8(json(&responses()).unwrap()).unwrap();
        assert_eq!(
            text,
            concat!(
                r#"{"responses":[{"id":"1","status":201,"headers":{"Content-Type":"application/json","Location":"Buildings('b1')"},"body":{"id":"b1"}},"#,
                r#"{"status":204},"#,
                r#"{"id":"3","status":404,"headers":{"Content-Type":"text/plain"},"body":"no such building"},"#,
                r#"{"id":"4","status":299,"body":"not json"}]}"#
            )
        );
        assert_eq!(json(&[]).unwrap(), br#"{"responses":[]}"#);
        let invalid = |r: Response| json(&[r]).map_err(|e| e.kind());
        assert_eq!(
            invalid(response(None, 200, &[], b"\xff")),
            Err(io::ErrorKind::InvalidInput)
        );
        assert_eq!(
            invalid(response(None, 200, &[("A", "\r")], b"")),
            Err(io::ErrorKind::InvalidInput)
        );
    }

    #[test]
    fn a_json_body_is_written_as_its_own_bytes() {
        // T-6049: the body is checked and written as sent, its member order kept; a body its
        // JSON Content-Type does not hold is a string; a binary one refuses the whole batch
        // before a byte is written
        let kept = response(None, 200, &[], b" {\"z\":1,\"a\":[1, 2]}\n");
        let broken = response(
            None,
            200,
            &[("Content-Type", "application/json")],
            b"{\"a\":",
        );
        let mut out = sink();
        let binary = response(None, 200, &[], b"\xff");
        assert!(write_json(&[kept.clone(), binary], &mut out).is_err());
        assert_eq!(out.out, Vec::<u8>::new());
        let text = String::from_utf8(json(&[kept, broken]).unwrap()).unwrap();
        assert_eq!(
            text,
            concat!(
                "{\"responses\":[{\"status\":200,\"body\": {\"z\":1,\"a\":[1, 2]}\n},",
                r#"{"status":200,"headers":{"Content-Type":"application/json"},"body":"{\"a\":"}]}"#
            )
        );
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["responses"][0]["body"]["a"][1], 2);
    }

    #[test]
    fn a_failing_sink_fails_every_write() {
        let all = responses();
        for (name, full) in [
            ("multipart", multipart(&all).unwrap()),
            ("json", json(&all).unwrap()),
        ] {
            for left in 0..full.len() {
                let mut out = Sink {
                    left,
                    out: Vec::new(),
                };
                let result = if name == "json" {
                    write_json(&all, &mut out)
                } else {
                    write_multipart(&all, "r", &mut out)
                };
                assert!(result.is_err(), "{name} at {left}");
                assert_eq!(
                    out.out,
                    full.get(..left).unwrap_or_default(),
                    "{name} at {left}"
                );
            }
        }
        let mut out = sink();
        assert!(out.flush().is_ok());
    }
}
