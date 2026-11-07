//! LinkML JSON at load (LK4): any bytes either load or answer an error,
//! never a panic, and a model that loads renders with every writer.
#![no_main]
use libfuzzer_sys::fuzz_target;
use linkml_model::{Limits, Model};

const LIMITS: Limits = Limits { max_bytes: 64 * 1024, max_depth: 32, max_elements: 4096 };

fuzz_target!(|data: &[u8]| {
    if let Ok(model) = Model::read(data, &LIMITS) {
        for write in [
            linkml_model::yaml,
            linkml_model::json_schema,
            linkml_model::context,
            linkml_model::shacl,
            linkml_model::owl,
            linkml_model::rdfs,
            linkml_model::markdown,
        ] {
            let _ = write(&model);
        }
    }
});
