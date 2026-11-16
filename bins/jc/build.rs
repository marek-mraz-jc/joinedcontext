// viz serves the open-data explorer, the Portal UI's `vite.explorer.config.ts` build that
// `pnpm build` in crates/portal/ui writes to dist-explorer/, and embeds it from there; nothing of
// it is committed (T-7102). Without a UI build this leaves empty files for include_bytes!, and viz
// then serves no /explorer/. The image builds check the bundle before compiling jc
// (bins/jc/Dockerfile, image.yml) and after (.github/scripts/image-smoke.sh).
use std::path::Path;

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/portal/ui/dist-explorer");
    std::fs::create_dir_all(&dist).expect("create crates/portal/ui/dist-explorer");
    for name in ["explorer.js", "explorer.css"] {
        let path = dist.join(name);
        if !path.exists() {
            // Dated 1970, so writing it does not count as a change and rerun this script.
            std::fs::File::create(&path)
                .and_then(|f| f.set_modified(std::time::UNIX_EPOCH))
                .expect("write the empty explorer placeholder");
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
