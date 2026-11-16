// viz serves the open-data explorer, the Portal UI's `vite.explorer.config.ts` build that
// `pnpm build` in crates/portal/ui writes to dist-explorer/. Nothing of it is committed (T-7102):
// this embeds what that build left, with the SHA-256 of each file (viz's ETag and its start log).
// Without a UI build the files are empty and viz serves no /explorer/; `JC_REQUIRE_EXPLORER=1`
// (the image builds) makes that a build error instead.
use sha2::{Digest, Sha256};
use std::path::Path;

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/portal/ui/dist-explorer");
    // A missing directory would rerun this script, and relink jc, on every build.
    std::fs::create_dir_all(&dist).expect("create crates/portal/ui/dist-explorer");
    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rerun-if-env-changed=JC_REQUIRE_EXPLORER");
    let required = std::env::var("JC_REQUIRE_EXPLORER").is_ok_and(|v| v == "1");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    for (name, var) in [("explorer.js", "JS"), ("explorer.css", "CSS")] {
        let built = dist.join(name);
        let (path, sum) = match std::fs::read(&built) {
            Ok(bytes) if !bytes.is_empty() => {
                let sum: String = Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                (built, sum)
            }
            _ if required => panic!(
                "JC_REQUIRE_EXPLORER=1 and {} is missing or empty: run `pnpm build` in \
                 crates/portal/sdk, then in crates/portal/ui, before building jc",
                built.display()
            ),
            _ => {
                let empty = out.join(name);
                std::fs::write(&empty, b"").expect("write the empty explorer placeholder");
                (empty, String::new())
            }
        };
        println!("cargo:rustc-env=JC_EXPLORER_{var}={}", path.display());
        println!("cargo:rustc-env=JC_EXPLORER_{var}_SHA256={sum}");
    }
}
