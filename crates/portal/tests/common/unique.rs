//! A name no other test makes, for the tests that share one database or one temp directory.

use std::sync::atomic::{AtomicU64, Ordering};

/// 26 lowercase hex digits: the clock tells runs sharing one database apart, the process id two
/// test binaries, and a per-process counter two tests of one binary that read the same clock on
/// parallel threads (two connectors tests once got one person and moved each other's list).
/// Hex digits only, so it fits a DNS label, a Postgres identifier and a subject alike.
pub fn unique() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!(
        "{nanos:016x}{:06x}{:04x}",
        std::process::id() & 0xff_ffff,
        n & 0xffff
    )
}
