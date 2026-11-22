//! A profile or specification file added or changed rebuilds the crate that embeds them.

fn main() {
    println!("cargo:rerun-if-changed=../profiles");
    println!("cargo:rerun-if-changed=../specs");
}
