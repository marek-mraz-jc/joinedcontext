// rust-embed needs ui/dist to exist at compile time; a plain `cargo test` runs without a UI build.
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/dist");
    std::fs::create_dir_all(&dist).expect("create ui/dist");
    // The kit of AP-56 is embedded the same way (src/agents/kit.rs).
    let kit = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/dist");
    std::fs::create_dir_all(kit.join("runtime")).expect("create sdk/dist/runtime");
    // The App templates' live demos (T-3306) are embedded from the same build.
    std::fs::create_dir_all(kit.join("demos")).expect("create sdk/dist/demos");
    println!("cargo:rerun-if-changed=ui/dist");
    println!("cargo:rerun-if-changed=sdk/dist");
}
