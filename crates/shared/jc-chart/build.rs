//! Holds the chart library the chart view inlines (T-6887) to its line in
//! vendor/viz/vendor/SHA256SUMS: a changed or swapped file fails the build.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../vendor/viz/vendor");
    let sums = vendor.join("SHA256SUMS");
    println!("cargo:rerun-if-changed={}", sums.display());
    let sums = std::fs::read_to_string(&sums).expect("vendor/viz/vendor/SHA256SUMS");
    for file in [
        "vega-6.4.0.min.js",
        "vega-lite-6.5.0.min.js",
        "vega-interpreter-2.3.2.js",
    ] {
        let path = vendor.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = std::fs::read(&path).expect(file);
        let hex = Sha256::digest(&bytes)
            .iter()
            .fold(String::new(), |mut out, b| {
                let _ = write!(out, "{b:02x}");
                out
            });
        assert!(
            sums.lines().any(|line| line == format!("{hex}  {file}")),
            "vendor/viz/vendor/{file} is not the file SHA256SUMS names (sha256 {hex})"
        );
    }
}
