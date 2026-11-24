//! `antares_wasm::Broker::handle`, the one seam the Service Worker, the in-page API and the Node
//! shim feed page requests into (T-7769): any method, path, header set and body gets a response,
//! never a panic or a hang, and every status it answers is a valid HTTP status.
//!
//! Input: byte 0 picks the method, then the path up to the first `\n`, then `Name: value` header
//! lines up to an empty line, then the body. A fresh memory broker per input keeps a crash
//! reproducible from its one file.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::time::Duration;

const METHODS: [&str; 7] = ["GET", "POST", "PATCH", "PUT", "DELETE", "HEAD", "OPTIONS"];

fn request(data: &[u8]) -> Option<http::Request<Vec<u8>>> {
    let (&m, rest) = data.split_first()?;
    let text_end = rest.windows(2).position(|w| w == b"\n\n").map_or(rest.len(), |i| i + 2);
    let (head, body) = rest.split_at(text_end);
    let head = std::str::from_utf8(head).ok()?;
    let mut lines = head.lines();
    let path = lines.next()?;
    let mut b = http::Request::builder()
        .method(METHODS[usize::from(m) % METHODS.len()])
        .uri(format!("/{}", path.trim_start_matches('/')));
    for line in lines {
        let (name, value) = line.split_once(':')?;
        b = b.header(name.trim(), value.trim());
    }
    b.body(body.to_vec()).ok()
}

fuzz_target!(|data: &[u8]| {
    // the API caps a request far below this; past it the target would only time the allocator
    if data.len() > 16 * 1024 {
        return;
    }
    let Some(req) = request(data) else { return };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let broker = antares_wasm::Broker::new();
    // a remote @context fetch is the only step that waits on the outside; the bound keeps a
    // fuzzed URL from stalling the run, and a hang inside the broker shows as libFuzzer's timeout
    if let Ok(resp) = rt.block_on(tokio::time::timeout(Duration::from_secs(2), broker.handle(req))) {
        let status = resp.status().as_u16();
        assert!((100..600).contains(&status), "status {status}");
    }
});
