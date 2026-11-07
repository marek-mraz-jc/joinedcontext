# rumqttc 0.25.1, with rustls-webpki 0.103 (T-4129)

`src/` is rumqttc 0.25.1 from crates.io byte for byte (`.crate` sha256
0feff8d882bff0b2fddaf99355a10336d43dd3ed44204f85ece28cf9626ab519, the checksum in
Cargo.lock before this copy), Apache-2.0 (LICENSE). `Cargo.toml` is the published manifest
with its examples, tests and dev-dependencies removed, a cargo-machete note (the
dependency's library is named `webpki`), and one change: `rustls-webpki` 0.102.8 -> 0.103.13. rumqttc uses webpki for one error variant
(`src/tls.rs`, `TlsError::WebPki`), whose type is the same in 0.103.

Second change (T-5859): `src/tls.rs` reads PEM with `rustls::pki_types::pem::PemObject`
instead of `rustls-pemfile` (unmaintained, RUSTSEC-2025-0134), so the manifest drops that
dependency. Malformed PEM is still an `io::ErrorKind::InvalidData` error, a file without a
certificate or key still `NoValidCertInChain`/`NoValidKeyInChain`; the `tls::tests` module
pins both. The 18 `use pretty_assertions::assert_eq;` test imports are gone too (the
dev-dependency was removed), so `cargo test -p rumqttc --lib` runs from the workspace root.

Why: every rumqttc release and upstream main pin `rustls-webpki ^0.102.8`, which has
RUSTSEC-2026-0049/-0098/-0099/-0104 and GHSA-82j2-j2ch-gfr8, fixed only in 0.103
(upstream issues bytebeamio/rumqtt#1046 and #1067; PRs #1037, #1072 and #1073 unmerged).

Delete this directory and the `[patch.crates-io]` line in the root Cargo.toml once a
rumqttc release depends on rustls-webpki 0.103.
