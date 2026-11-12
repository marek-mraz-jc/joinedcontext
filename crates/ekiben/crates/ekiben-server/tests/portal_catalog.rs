//! The Portal offers and accepts only the inputs and processors this runner registers (T-6825).
//! Its two copies, the jc-core list the save path checks and the JSON the Portal editor filters
//! its pickers by, are written from `build_full_registry()` here and never by hand:
//! `EKIBEN_BLESS=1 cargo test -p ekiben-server --test portal_catalog` rewrites both, and the
//! plain run fails while either differs from the registry.

use std::path::{Path, PathBuf};

use ekiben_server::build_full_registry;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

const RUST: &str = "vendor/joinedcontext-platform/crates/jc-core/src/kinds/ekiben_components.rs";
const JSON: &str = "crates/portal/ui/src/schemas/ekiben-components.json";

fn rust_list(out: &mut String, doc: &str, name: &str, items: &[String]) {
    out.push_str(&format!("\n/// {doc}\npub const {name}: &[&str] = &[\n"));
    for item in items {
        out.push_str(&format!("    \"{item}\",\n"));
    }
    out.push_str("];\n");
}

fn expected() -> (String, String) {
    let names = build_full_registry().component_names();
    let (inputs, processors) = (&names["inputs"], &names["processors"]);
    let mut rust = String::from(
        "//! The inputs and processors the pipeline runner (ekiben) registers, beside the Bento catalog\n\
         //! the forms are drawn from (T-6825). Written by crates/ekiben/crates/ekiben-server/tests/\n\
         //! portal_catalog.rs from `build_full_registry()`; rerun it with EKIBEN_BLESS=1, never edit by hand.\n",
    );
    rust_list(
        &mut rust,
        "Every input name the runner builds.",
        "INPUTS",
        inputs,
    );
    rust_list(
        &mut rust,
        "Every processor name the runner builds.",
        "PROCESSORS",
        processors,
    );
    let json = serde_json::json!({ "inputs": inputs, "processors": processors });
    let json = format!("{}\n", serde_json::to_string_pretty(&json).expect("a list"));
    (rust, json)
}

#[test]
fn portal_lists_match_the_registry() {
    let root = repo_root();
    let (rust, json) = expected();
    if std::env::var_os("EKIBEN_BLESS").is_some() {
        std::fs::write(root.join(RUST), &rust).expect("write the jc-core list");
        std::fs::write(root.join(JSON), &json).expect("write the editor list");
    }
    for (path, want) in [(RUST, rust), (JSON, json)] {
        let got = std::fs::read_to_string(root.join(path)).unwrap_or_default();
        assert_eq!(
            got, want,
            "{path} differs from the runner's registry: rerun this test with EKIBEN_BLESS=1"
        );
    }
}
