// SPDX-License-Identifier: EUPL-1.2
//! Round trips: a batch written from random members reads back to exactly those members, in
//! both formats; arbitrary bytes never panic either parser.

#[cfg(test)]
mod round_trips {
    use std::collections::BTreeMap;

    use odata_batch::{parse_json, parse_multipart, Limits, Method, Request};
    use proptest::prelude::*;
    use serde_json::{json, Value};

    /// One member: with an id (multipart) or a body (JSON), method, URL, headers, body.
    type Member = (bool, Method, String, BTreeMap<String, String>, Vec<u8>);

    fn method() -> impl Strategy<Value = Method> {
        prop_oneof![
            Just(Method::Get),
            Just(Method::Post),
            Just(Method::Put),
            Just(Method::Patch),
            Just(Method::Delete),
        ]
    }

    /// Headers the parser hands back unchanged: token names other than the two it reads,
    /// values without surrounding blanks.
    fn headers() -> impl Strategy<Value = BTreeMap<String, String>> {
        proptest::collection::btree_map(
            "X-[A-Za-z0-9-]{1,10}",
            prop_oneof![Just(String::new()), "[!-~]([ -~]{0,18}[!-~])?"],
            0..4,
        )
    }

    fn members() -> impl Strategy<Value = Vec<Member>> {
        proptest::collection::vec(
            (
                any::<bool>(),
                method(),
                "[A-Za-z][A-Za-z0-9()',=]{0,20}",
                headers(),
                "[a-z \r\n-]{0,40}",
            ),
            0..6,
        )
        .prop_map(|members| {
            members
                .into_iter()
                .map(|(id, m, url, h, body)| (id, m, url, h, body.into_bytes()))
                .collect()
        })
    }

    fn multipart(members: &[Member]) -> (Vec<u8>, Vec<Request>) {
        let mut body = Vec::new();
        let mut expected = Vec::new();
        for (at, (id, method, url, headers, content)) in members.iter().enumerate() {
            let id = id.then(|| at.to_string());
            body.extend_from_slice(b"--b\r\nContent-Type: application/http\r\n");
            if let Some(id) = &id {
                body.extend_from_slice(format!("Content-ID: {id}\r\n").as_bytes());
            }
            body.extend_from_slice(
                format!("\r\n{} {url} HTTP/1.1\r\n", method.as_str()).as_bytes(),
            );
            for (name, value) in headers {
                body.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
            }
            body.extend_from_slice(b"\r\n");
            body.extend_from_slice(content);
            body.extend_from_slice(b"\r\n");
            expected.push(Request {
                id,
                method: *method,
                url: url.clone(),
                headers: headers.clone().into_iter().collect(),
                body: content.clone(),
                depends_on: Vec::new(),
            });
        }
        body.extend_from_slice(b"--b--\r\n");
        (body, expected)
    }

    proptest! {
        // Miri runs every case through its interpreter: past the weekly job's 90 min (T-3706)
        #![proptest_config(ProptestConfig::with_cases(if cfg!(miri) { 4 } else { 256 }))]

        #[test]
        fn multipart_batches_read_back(members in members()) {
            let (body, expected) = multipart(&members);
            // A body that starts with `--b` or holds `\n--b` is a delimiter; the writer refuses both.
            prop_assume!(!members.iter().any(|m| [&b"\n"[..], &m.4].concat().windows(4).any(|w| w == b"\n--b")));
            prop_assert_eq!(parse_multipart(&body, "b", &Limits::strict()).map(|b| b.requests), Ok(expected));
        }

        #[test]
        fn json_batches_read_back(
            members in members(),
            depends in proptest::collection::vec(any::<usize>(), 0..4),
        ) {
            let mut requests = Vec::new();
            let mut expected = Vec::new();
            for (at, (with_body, method, url, headers, content)) in members.iter().enumerate() {
                let text = String::from_utf8(content.clone()).unwrap_or_default();
                let depends_on: Vec<usize> = if at == 0 { Vec::new() } else { depends.iter().map(|d| d.checked_rem(at).unwrap_or_default()).collect() };
                let mut member = json!({"id": at.to_string(), "method": method.as_str().to_lowercase(), "url": url,
                    "dependsOn": depends_on.iter().map(ToString::to_string).collect::<Vec<_>>()});
                if !headers.is_empty() {
                    member["headers"] = json!(headers);
                }
                if *with_body {
                    member["body"] = json!(text);
                }
                requests.push(member);
                expected.push(Request {
                    id: Some(at.to_string()),
                    method: *method,
                    url: url.clone(),
                    headers: headers.clone().into_iter().collect(),
                    body: if *with_body { Value::from(text).to_string().into_bytes() } else { Vec::new() },
                    depends_on,
                });
            }
            let body = json!({"requests": requests}).to_string();
            prop_assert_eq!(parse_json(body.as_bytes(), &Limits::strict()).map(|b| b.requests), Ok(expected));
        }

        #[test]
        fn numbers_in_json_bodies_keep_their_bits(number in any::<f64>().prop_filter("finite", |n| n.is_finite())) {
            let body = json!({"requests": [{"id": "1", "method": "post", "url": "x", "body": number}]}).to_string();
            let read = parse_json(body.as_bytes(), &Limits::strict()).map(|b| b.requests[0].body.clone());
            let bits = read.map(|text| String::from_utf8_lossy(&text).parse::<f64>().map(f64::to_bits));
            prop_assert_eq!(bits, Ok(Ok(number.to_bits())));
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let limits = Limits::strict();
            let _ = parse_multipart(&bytes, "b", &limits);
            let _ = parse_json(&bytes, &limits);
        }
    }
}
