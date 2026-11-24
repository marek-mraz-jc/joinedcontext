//! Any manifest repository file: splitting it into documents, parsing each as a manifest,
//! assembling the in-memory repository and compiling its roles never panic, and the documents keep
//! every line of the file (T-7794).
#![no_main]

use jc_manifest::loader::{parse_yaml_documents, RawManifest, Repository};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let chunks = parse_yaml_documents(text);
    let lines: usize = chunks.iter().map(|c| c.content.lines().count()).sum();
    assert!(lines <= text.lines().count());
    let manifests: Vec<RawManifest> = chunks
        .iter()
        .filter(|chunk| !jc_manifest::loader::is_empty_doc(&chunk.content))
        .filter_map(|chunk| serde_norway::from_str(&chunk.content).ok())
        .collect();
    for manifest in &manifests {
        let _ = jc_manifest::loader::extract_space(&manifest.spec);
    }
    let Ok(repo) = Repository::from_manifests(manifests, Some("example.org")) else {
        return;
    };
    let _ = repo.literal_spaces();
    let _ = repo.misplaced();
    let _ = repo.policy_ref_domains();
    let _ = jc_manifest::roles::compile(&repo);
    let _ = jc_manifest::pipeline_identity::derived(&repo);
    let _ = jc_manifest::validate::unmade_writes(&repo);
    let _ = jc_manifest::validate::open_data_endpoints(&repo);
    let _ = jc_manifest::foreign_models::plan_flags(&repo);
});
