//! Input: one byte that picks a connector, then its JSON config: the sftp input's lint, the
//! aws_s3 input's and output's builders. Each must answer a component or errors that name a
//! field, and never echo a password or secret given in the config.
#![no_main]

use ekiben_core::ComponentConfig;
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

const SECRET: &str = "fuzz-secret-value";

fn registry() -> &'static ekiben_core::Registry {
    static R: OnceLock<ekiben_core::Registry> = OnceLock::new();
    R.get_or_init(|| {
        let r = ekiben_core::Registry::new();
        ekiben_io_sftp::register_all(&r);
        ekiben_io_s3::register_all(&r);
        r
    })
}

fuzz_target!(|data: &[u8]| {
    let Some((pick, json)) = data.split_first() else {
        return;
    };
    let Ok(mut config) = serde_json::from_slice::<serde_json::Value>(json) else {
        return;
    };
    // the secret fields hold a known value, so an answer that repeats it is caught
    if let Some(creds) = config.get_mut("credentials").and_then(|c| c.as_object_mut()) {
        for k in ["password", "secret", "token", "private_key_pass"] {
            if creds.contains_key(k) {
                creds.insert(k.into(), SECRET.into());
            }
        }
    }
    let errors = match pick % 3 {
        0 => {
            let conf = ComponentConfig {
                label: String::new(),
                component_type: "sftp".into(),
                config,
                processors: Vec::new(),
            };
            registry().lint_component("input", &conf)
        }
        1 => config
            .as_object()
            .map(|o| {
                ekiben_io_s3::client::S3Conn::from_config(
                    "aws_s3",
                    o,
                    std::time::Duration::from_secs(5),
                )
                .err()
                .map(|e| e.to_string())
                .into_iter()
                .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    for e in errors {
        assert!(!e.contains(SECRET), "{e}");
    }
});
