// SPDX-License-Identifier: EUPL-1.2
//! `multipart/mixed` batches (Protocol 4.01 section 11.7.7, RFC 2046 section 5.1) over a byte
//! slice already bounded by `max_body_bytes`. Strict where a lenient reader would let two parsers
//! disagree about where a member ends (threat model O10): no preamble, no epilogue, no header
//! folding, a delimiter followed by anything but `--` or a line end refused, a lone CR refused, a
//! `Content-Length` that must match, and change sets (the second multipart level) refused whole.
//! A line end of the framing is CRLF or a bare LF (clients that write the body as text send LF);
//! a part's body keeps its bytes, and a CR before the delimiter's LF belongs to the delimiter.

use memchr::memmem;

use crate::{
    check_member, header, Batch, BatchError, BatchLimit, Feature, Limits, Method, Request,
};

const CRLF: &[u8] = b"\r\n";

/// The length of the line end (CRLF or LF) `bytes` starts with, if it starts with one.
fn line_end(bytes: &[u8]) -> Option<usize> {
    if bytes.starts_with(CRLF) {
        Some(2)
    } else if bytes.starts_with(b"\n") {
        Some(1)
    } else {
        None
    }
}

/// `bytes` without one trailing CR.
fn chomp(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

/// The first empty line: where the lines before it end (their last line end excluded) and where
/// the bytes after it start. `bytes` starting with a line end is an empty first line.
fn blank_line(bytes: &[u8]) -> Option<(usize, usize)> {
    if let Some(n) = line_end(bytes) {
        return Some((0, n));
    }
    memchr::memchr_iter(b'\n', bytes).find_map(|i| {
        let after = i.saturating_add(1);
        let n = line_end(at(bytes, after))?;
        let head = chomp(bytes.get(..i).unwrap_or_default()).len();
        Some((head, after.saturating_add(n)))
    })
}

/// The characters RFC 2046 section 5.1.1 allows in a boundary (`bchars`).
fn is_bchar(c: char) -> bool {
    c.is_ascii_alphanumeric() || "'()+_,-./:=? ".contains(c)
}

fn valid_boundary(boundary: &str) -> bool {
    (1..=70).contains(&boundary.len()) && boundary.chars().all(is_bchar) && !boundary.ends_with(' ')
}

/// The boundary of a `multipart/mixed` `Content-Type` (RFC 2046 section 5.1.1): 1 to 70
/// `bchars`, not ending in a space, quoted or not.
///
/// ```
/// use odata_batch::{boundary, BatchError};
/// assert_eq!(boundary("multipart/mixed; boundary=batch_36522ad7")?, "batch_36522ad7");
/// assert_eq!(boundary("Multipart/Mixed;Boundary=\"b 1\"")?, "b 1");
/// assert_eq!(boundary("application/json"), Err(BatchError::ContentType));
/// # Ok::<(), BatchError>(())
/// ```
///
/// # Errors
///
/// [`BatchError::ContentType`] for another media type, or a missing or invalid boundary.
pub fn boundary(content_type: &str) -> Result<String, BatchError> {
    let mut parts = content_type.split(';');
    let essence = parts.next().unwrap_or_default().trim();
    if !essence.eq_ignore_ascii_case("multipart/mixed") {
        return Err(BatchError::ContentType);
    }
    let value = parts
        .filter_map(|p| p.split_once('='))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("boundary"))
        .map(|(_, value)| value.trim())
        .ok_or(BatchError::ContentType)?;
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    if valid_boundary(value) {
        Ok(value.to_owned())
    } else {
        Err(BatchError::ContentType)
    }
}

/// `count` transport-padding bytes (spaces and tabs) at the start of `bytes`.
fn padding(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|b| matches!(b, b' ' | b'\t'))
        .count()
}

fn at(bytes: &[u8], from: usize) -> &[u8] {
    bytes.get(from..).unwrap_or_default()
}

/// Header lines `name: value`, separated by CRLF or LF, under the per-block bounds. `offset` is where
/// `block` starts in the body.
fn headers(
    block: &[u8],
    offset: usize,
    limits: &Limits,
) -> Result<Vec<(String, String)>, BatchError> {
    if block.len() > limits.max_part_header_bytes {
        return Err(BatchError::limit(
            BatchLimit::HeaderBytes,
            limits.max_part_header_bytes,
        ));
    }
    let framing = BatchError::Framing { offset };
    if block.is_empty() {
        return Ok(Vec::new());
    }
    let text = std::str::from_utf8(block).map_err(|_| framing.clone())?;
    let mut out = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if out.len() >= limits.max_headers_per_part {
            return Err(BatchError::limit(
                BatchLimit::Headers,
                limits.max_headers_per_part,
            ));
        }
        let (name, value) = line.split_once(':').ok_or_else(|| framing.clone())?;
        let token = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
        if !token || value.contains(['\r', '\n']) {
            return Err(framing);
        }
        out.push((name.to_owned(), value.trim_matches([' ', '\t']).to_owned()));
    }
    Ok(out)
}

/// One `application/http` part starting at body offset `offset`.
fn part(content: &[u8], offset: usize, limits: &Limits) -> Result<Request, BatchError> {
    let Some((end, inner_at)) = blank_line(content).filter(|&(end, _)| end > 0) else {
        return Err(BatchError::Framing { offset });
    };
    let mime = headers(content.get(..end).unwrap_or_default(), offset, limits)?;
    let essence = header(&mime, "Content-Type")
        .and_then(|t| t.split(';').next())
        .map(|t| t.trim().to_ascii_lowercase());
    match essence.as_deref() {
        Some("multipart/mixed") => return Err(BatchError::NotImplemented(Feature::ChangeSet)),
        Some("application/http") => {}
        _ => return Err(BatchError::PartType { offset }),
    }
    if header(&mime, "Content-Transfer-Encoding").is_some_and(|e| !e.eq_ignore_ascii_case("binary"))
    {
        return Err(BatchError::PartType { offset });
    }
    let inner = at(content, inner_at);
    let request_offset = offset.saturating_add(inner_at);
    let (line, rest) = match memchr::memchr(b'\n', inner) {
        Some(i) => (
            chomp(inner.get(..i).unwrap_or_default()),
            at(inner, i.saturating_add(1)),
        ),
        None => (inner, &b""[..]),
    };
    let (method, url) = request_line(line, request_offset)?;
    // Headers, then an empty line and the body; or, without an empty line, headers up to the end
    // of the part and no body (Protocol 11.7.7 examples).
    let (head, body) = blank_line(rest).map_or_else(
        || (rest.strip_suffix(b"\n").map_or(rest, chomp), &b""[..]),
        |(end, start)| (rest.get(..end).unwrap_or_default(), at(rest, start)),
    );
    let request_headers = headers(head, request_offset, limits)?;
    if let Some(length) = header(&request_headers, "Content-Length") {
        if length.parse::<usize>().ok() != Some(body.len()) {
            return Err(BatchError::ContentLength { offset });
        }
    }
    Ok(Request {
        id: header(&mime, "Content-ID").map(str::to_owned),
        method,
        url,
        headers: request_headers,
        body: body.to_vec(),
        depends_on: Vec::new(),
    })
}

/// `METHOD SP url [SP HTTP/1.1]`.
fn request_line(line: &[u8], offset: usize) -> Result<(Method, String), BatchError> {
    let invalid = BatchError::RequestLine { offset };
    let text = std::str::from_utf8(line).map_err(|_| invalid.clone())?;
    let words: Vec<&str> = text.split(' ').collect();
    let (token, url) = match words.as_slice() {
        [token, url] | [token, url, "HTTP/1.1"] if !url.is_empty() => (*token, *url),
        _ => return Err(invalid),
    };
    // HTTP methods are case-sensitive (RFC 9110 section 9.1); only JSON batches write them in
    // lower case.
    match Method::parse(token).filter(|m| m.as_str() == token) {
        Some(method) => Ok((method, url.to_owned())),
        None if !token.is_empty() && token.bytes().all(|b| b.is_ascii_uppercase()) => {
            Err(BatchError::NotImplemented(Feature::Method))
        }
        None => Err(invalid),
    }
}

/// A `multipart/mixed` batch body with the boundary of its `Content-Type` ([`boundary`]).
///
/// ```
/// use odata_batch::{parse_multipart, Limits, Method};
/// let body = b"--b\r\nContent-Type: application/http\r\nContent-ID: 1\r\n\r\nGET Buildings('b1') HTTP/1.1\r\nAccept: application/json\r\n\r\n--b--\r\n";
/// let batch = parse_multipart(body, "b", &Limits::strict())?;
/// assert_eq!(batch.requests[0].method, Method::Get);
/// assert_eq!(batch.requests[0].id.as_deref(), Some("1"));
/// assert_eq!(batch.requests[0].headers, [("Accept".to_owned(), "application/json".to_owned())]);
/// # Ok::<(), odata_batch::BatchError>(())
/// ```
///
/// # Errors
///
/// [`BatchError`], by the first problem found.
pub fn parse_multipart(body: &[u8], boundary: &str, limits: &Limits) -> Result<Batch, BatchError> {
    if body.len() > limits.max_body_bytes {
        return Err(BatchError::limit(
            BatchLimit::BodyBytes,
            limits.max_body_bytes,
        ));
    }
    if !valid_boundary(boundary) {
        return Err(BatchError::ContentType);
    }
    let dash = format!("--{boundary}");
    let delimiter = format!("\n--{boundary}");
    let finder = memmem::Finder::new(delimiter.as_bytes());
    if !body.starts_with(dash.as_bytes()) {
        return Err(BatchError::Framing { offset: 0 });
    }
    let mut pos = dash.len();
    let mut batch = Batch::default();
    loop {
        pos = pos.saturating_add(padding(at(body, pos)));
        let rest = at(body, pos);
        if let Some(after) = rest.strip_prefix(b"--") {
            if after.is_empty() || line_end(after) == Some(after.len()) {
                return Ok(batch);
            }
            return Err(BatchError::Framing { offset: pos });
        }
        let Some(n) = line_end(rest) else {
            return Err(BatchError::Framing { offset: pos });
        };
        pos = pos.saturating_add(n);
        let content = at(body, pos);
        let end = finder
            .find(content)
            .ok_or(BatchError::Framing { offset: pos })?;
        if batch.requests.len() >= limits.max_requests {
            return Err(BatchError::limit(BatchLimit::Requests, limits.max_requests));
        }
        // A CR before the delimiter's LF belongs to the delimiter (RFC 2046 section 5.1.1).
        let request = part(chomp(content.get(..end).unwrap_or_default()), pos, limits)?;
        check_member(&request, &batch.requests, limits)?;
        batch.requests.push(request);
        pos = pos.saturating_add(end).saturating_add(delimiter.len());
    }
}
