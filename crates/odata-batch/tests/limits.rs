// SPDX-License-Identifier: EUPL-1.2
//! Every bound of `Limits` holds at its value and answers `LimitExceeded` one past it, in both
//! formats (threat model O11).

#[cfg(test)]
mod limits {
    use std::fmt::Write;

    use odata_batch::{parse_json, parse_multipart, BatchError, BatchLimit, Limits};

    const TIGHT: Limits = Limits {
        max_body_bytes: 4096,
        max_requests: 2,
        max_part_header_bytes: 60,
        max_headers_per_part: 2,
        max_depends_on: 2,
        max_id_len: 3,
        max_url_len: 5,
        max_json_depth: 2,
    };

    fn exceeded(limit: BatchLimit, max: usize) -> Result<odata_batch::Batch, BatchError> {
        Err(BatchError::limit(limit, max))
    }

    fn multipart(parts: &[(&str, &str)]) -> String {
        let mut body = String::new();
        for (part_headers, request) in parts {
            let _ = write!(
                body,
                "--b\r\nContent-Type: application/http\r\n{part_headers}\r\n{request}\r\n"
            );
        }
        body + "--b--"
    }

    fn json(members: &[&str]) -> String {
        format!(r#"{{"requests":[{}]}}"#, members.join(","))
    }

    fn get(id: &str, url: &str) -> String {
        format!(r#"{{"id":"{id}","method":"get","url":"{url}"}}"#)
    }

    #[test]
    fn multipart_bounds_hold_at_the_limit_and_refuse_one_past_it() {
        let parse = |body: &str| parse_multipart(body.as_bytes(), "b", &TIGHT);
        let body = |n: usize| format!("POST x\r\n\r\n{}", "[".repeat(n));
        let headers = |n: usize| format!("GET x\r\n{}", vec!["A: 1"; n].join("\r\n"));
        for (ok, over, limit, max) in [
            (
                multipart(&[("", "GET x"); 2]),
                multipart(&[("", "GET x"); 3]),
                BatchLimit::Requests,
                2,
            ),
            (
                multipart(&[("", &headers(2))]),
                multipart(&[("", &headers(3))]),
                BatchLimit::Headers,
                2,
            ),
            (
                multipart(&[("A: 1\r\n", "GET x")]),
                multipart(&[("A: 1\r\nB: 2\r\n", "GET x")]),
                BatchLimit::Headers,
                2,
            ),
            (
                multipart(&[(&format!("Content-ID: {}\r\n", "a".repeat(3)), "GET x")]),
                multipart(&[(&format!("Content-ID: {}\r\n", "a".repeat(4)), "GET x")]),
                BatchLimit::IdLength,
                3,
            ),
            (
                multipart(&[("", "GET abcde")]),
                multipart(&[("", "GET abcdef")]),
                BatchLimit::UrlLength,
                5,
            ),
            (
                multipart(&[("", &body(2))]),
                multipart(&[("", &body(3))]),
                BatchLimit::JsonDepth,
                2,
            ),
            (
                multipart(&[(&format!("A: {}\r\n", "a".repeat(25)), "GET x")]),
                multipart(&[(&format!("A: {}\r\n", "a".repeat(26)), "GET x")]),
                BatchLimit::HeaderBytes,
                60,
            ),
        ] {
            assert!(parse(&ok).is_ok(), "{ok:?}");
            assert_eq!(parse(&over), exceeded(limit, max), "{over:?}");
        }
        let text = multipart(&[("", "POST x\r\nContent-Type: text/plain\r\n\r\n[[[")]);
        assert!(parse(&text).is_ok());
        // Transport padding after the first delimiter fills the body to the bound.
        let size = |n: usize| {
            multipart(&[("", "GET x")]).replacen("--b", &format!("--b{}", " ".repeat(n)), 1)
        };
        let at = TIGHT.max_body_bytes.saturating_sub(size(0).len());
        assert!(parse(&size(at)).is_ok());
        assert_eq!(parse(&size(at + 1)), exceeded(BatchLimit::BodyBytes, 4096));
    }

    #[test]
    fn json_bounds_hold_at_the_limit_and_refuse_one_past_it() {
        let parse = |body: &str| parse_json(body.as_bytes(), &TIGHT);
        let depends = |n: usize| {
            let ids: Vec<String> = (0..n).map(|i| format!("\"{i}\"")).collect();
            let mut members: Vec<String> = (0..n).map(|i| get(&i.to_string(), "x")).collect();
            members.push(format!(
                r#"{{"id":"z","method":"get","url":"x","dependsOn":[{}]}}"#,
                ids.join(",")
            ));
            json(&members.iter().map(String::as_str).collect::<Vec<_>>())
        };
        let nested = |n: usize| {
            json(&[&format!(
                r#"{{"id":"1","method":"post","url":"x","body":{}{}}}"#,
                "[".repeat(n),
                "]".repeat(n)
            )])
        };
        for (ok, over, limit, max) in [
            (
                json(&[&get("1", "x"), &get("2", "x")]),
                json(&[&get("1", "x"), &get("2", "x"), &get("3", "x")]),
                BatchLimit::Requests,
                2,
            ),
            (
                json(&[&get("abc", "x")]),
                json(&[&get("abcd", "x")]),
                BatchLimit::IdLength,
                3,
            ),
            (
                json(&[&get("1", "abcde")]),
                json(&[&get("1", "abcdef")]),
                BatchLimit::UrlLength,
                5,
            ),
            (nested(2), nested(3), BatchLimit::JsonDepth, 2),
        ] {
            assert!(parse(&ok).is_ok(), "{ok}");
            assert_eq!(parse(&over), exceeded(limit, max), "{over}");
        }
        let wide = Limits {
            max_requests: 9,
            ..TIGHT
        };
        assert!(parse_json(depends(2).as_bytes(), &wide).is_ok());
        assert_eq!(
            parse_json(depends(3).as_bytes(), &wide),
            exceeded(BatchLimit::DependsOn, 2)
        );
        let size = |n: usize| format!("{}{}", json(&[]), " ".repeat(n));
        let at = TIGHT.max_body_bytes.saturating_sub(size(0).len());
        assert!(parse(&size(at)).is_ok());
        assert_eq!(parse(&size(at + 1)), exceeded(BatchLimit::BodyBytes, 4096));
    }

    #[test]
    fn strings_hide_brackets_from_the_depth_scan() {
        let body = json(&[r#"{"id":"1","method":"post","url":"x","body":"[[[[\"[[[[\\"}"#]);
        assert!(parse_json(body.as_bytes(), &TIGHT).is_ok(), "{body}");
    }
}
