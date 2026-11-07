// SPDX-License-Identifier: EUPL-1.2
//! The batch response writers (Protocol 4.01 section 11.7.7.3, JSON Format 4.01 section 19.2).
//! Neither lets a response change the framing: a header with a line break, or a body that
//! holds the multipart delimiter, is refused before a byte is written.

use std::io::{self, Write};

use memchr::memmem;
use serde_json::{Map, Value};

use crate::{header, is_json, Response};

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what)
}

/// The reason phrase of the statuses a batch member answers with; empty for any other (RFC
/// 9112 section 4 allows an empty reason).
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        424 => "Failed Dependency",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "",
    }
}

/// Headers whose names are tokens and whose values hold no line break.
fn check_headers(response: &Response) -> io::Result<()> {
    let clean = response.headers.iter().all(|(name, value)| {
        !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_graphic() && b != b':')
            && !value.contains(['\r', '\n'])
    });
    if clean {
        Ok(())
    } else {
        Err(invalid("a response header would break the framing"))
    }
}

/// A `multipart/mixed` batch response under `boundary` (the caller's fresh one, sent in the
/// response `Content-Type`): one `application/http` part per response, its `Content-ID` echoed.
///
/// ```
/// use odata_batch::{write_multipart, Response};
/// let ok = Response { id: Some("1".into()), status: 204, headers: vec![], body: vec![] };
/// let mut out = Vec::new();
/// write_multipart(&[ok], "batchresponse_1", &mut out)?;
/// assert_eq!(
///     out,
///     b"--batchresponse_1\r\nContent-Type: application/http\r\nContent-ID: 1\r\n\r\nHTTP/1.1 204 No Content\r\n\r\n\r\n--batchresponse_1--\r\n"
/// );
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for an invalid boundary, a header with a line break or a
/// body that holds the delimiter (nothing is written then); otherwise what `out` answers.
pub fn write_multipart<W: Write>(
    responses: &[Response],
    boundary: &str,
    mut out: W,
) -> io::Result<()> {
    if crate::boundary(&format!("multipart/mixed; boundary=\"{boundary}\"")).is_err() {
        return Err(invalid("not a multipart boundary"));
    }
    let dash = format!("--{boundary}");
    let delimiter = format!("\n{dash}");
    let finder = memmem::Finder::new(delimiter.as_bytes());
    for response in responses {
        check_headers(response)?;
        let id_breaks = response
            .id
            .as_ref()
            .is_some_and(|id| id.contains(['\r', '\n']));
        if id_breaks
            || finder.find(&response.body).is_some()
            || response.body.starts_with(dash.as_bytes())
        {
            return Err(invalid("a response would break the framing"));
        }
    }
    for response in responses {
        write!(out, "--{boundary}\r\nContent-Type: application/http\r\n")?;
        if let Some(id) = &response.id {
            write!(out, "Content-ID: {id}\r\n")?;
        }
        write!(
            out,
            "\r\nHTTP/1.1 {} {}\r\n",
            response.status,
            reason(response.status)
        )?;
        for (name, value) in &response.headers {
            write!(out, "{name}: {value}\r\n")?;
        }
        out.write_all(b"\r\n")?;
        out.write_all(&response.body)?;
        out.write_all(b"\r\n")?;
    }
    write!(out, "--{boundary}--\r\n")
}

/// A JSON batch response (JSON Format 4.01 section 19.2): `{"responses": [...]}`, each with its
/// `id`, `status`, `headers` when any, and `body` when not empty: as JSON when the
/// `Content-Type` is JSON and the bytes parse, otherwise as a string.
///
/// ```
/// use odata_batch::{write_json, Response};
/// let failed = Response { id: Some("2".into()), status: 424, headers: vec![], body: vec![] };
/// let mut out = Vec::new();
/// write_json(&[failed], &mut out)?;
/// assert_eq!(out, br#"{"responses":[{"id":"2","status":424}]}"#);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for a header with a line break or a non-JSON body that is
/// not UTF-8; otherwise what `out` answers.
pub fn write_json<W: Write>(responses: &[Response], mut out: W) -> io::Result<()> {
    // Every member is checked before the first byte; a JSON body is written as its own bytes,
    // never parsed into a tree beside them (T-6049).
    let mut bodies = Vec::with_capacity(responses.len());
    for response in responses {
        check_headers(response)?;
        let json = header(&response.headers, "Content-Type").is_none_or(is_json)
            && serde_json::from_slice::<serde::de::IgnoredAny>(&response.body).is_ok();
        let body = if json {
            None
        } else {
            let text = std::str::from_utf8(&response.body);
            Some(text.map_err(|_| invalid("a binary body in a JSON batch"))?)
        };
        bodies.push(body);
    }
    out.write_all(b"{\"responses\":[")?;
    for (at, (response, text)) in responses.iter().zip(bodies).enumerate() {
        out.write_all(if at == 0 { b"{" } else { b",{" })?;
        // Members in the order section 19.2 lists them.
        if let Some(id) = &response.id {
            out.write_all(b"\"id\":")?;
            serde_json::to_writer(&mut out, id).map_err(io::Error::from)?;
            out.write_all(b",")?;
        }
        write!(out, "\"status\":{}", response.status)?;
        if !response.headers.is_empty() {
            let headers: Map<String, Value> = response
                .headers
                .iter()
                .map(|(name, value)| (name.clone(), value.clone().into()))
                .collect();
            out.write_all(b",\"headers\":")?;
            serde_json::to_writer(&mut out, &headers).map_err(io::Error::from)?;
        }
        if !response.body.is_empty() {
            out.write_all(b",\"body\":")?;
            match text {
                Some(text) => serde_json::to_writer(&mut out, text).map_err(io::Error::from)?,
                None => out.write_all(&response.body)?,
            }
        }
        out.write_all(b"}")?;
    }
    out.write_all(b"]}")
}
