//! jc-manifest's assembly, Git checkout and roles tests as one binary (moved from jcctl's, T-7794):
//! `cargo test -p jc-manifest --test assembly roles_tests::`.

mod assemble_tests;
mod common;
mod git_checkouts_tests;
mod preview_render_tests;
mod roles_tests;
mod validate_layout_tests;
