// rust-embed needs ui/dist to exist at compile time; `cargo test` runs without a UI build.
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/dist");
    std::fs::create_dir_all(&dist).expect("create ui/dist");
    println!("cargo:rerun-if-changed=ui/dist");
}
