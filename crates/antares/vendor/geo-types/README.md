# geo-types 0.7.20, without its rstar 0.8-0.11 and 0.13 options (T-7429)

`src/`, `LICENSE-APACHE` and `LICENSE-MIT` are geo-types 0.7.20 from crates.io byte for
byte (`.crate` sha256 777d18aa0f12f8b285331cd867133ee14422b3f023f6d388034c47d43e28786a, the
checksum in Cargo.lock before this copy), MIT OR Apache-2.0. `Cargo.toml` is the published
manifest with its dev-dependencies removed and one change: the optional `rstar_0_8`,
`rstar_0_9`, `rstar_0_10`, `rstar_0_11` and `rstar_0_13` dependencies go, with the
`rstar`, `use-rstar` and `use-rstar_0_8..0_11` features that enable them. The cfgs in
`src/` that name those features are declared under `[lints.rust.unexpected_cfgs]` and
compile as off. `rstar_0_12`, the one geo 0.33 enables, stays.

Why: Cargo.lock and the package list `cargo auditable` embeds in the image binary carry
every optional dependency of a crate, enabled or not, so geo-types 0.7.20 alone put
heapless 0.6.1, 0.7.17 and 0.8.0 (rstar 0.8-0.13) in front of `cargo audit` even though
only rstar 0.12 is compiled (RUSTSEC-2026-0574..0582, see `vendor/rstar/README.md`).

Delete this directory together with `vendor/rstar` (that README says when).
