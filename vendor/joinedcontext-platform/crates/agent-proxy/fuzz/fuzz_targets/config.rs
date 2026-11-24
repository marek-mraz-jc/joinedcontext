//! The configuration a deployment sets and the model provider's key answers, any bytes: the
//! settings parse or refuse without a panic, an Organization realm set that is refused changes
//! nothing, and a key answer is read to a state (T-7795).
#![no_main]

use agent_proxy::{config, model_key};
use libfuzzer_sys::fuzz_target;

const VARS: [&str; 12] = [
    "JC_PROXY_BIND",
    "JC_PORTAL_BASE",
    "JC_GATEWAY_BASE",
    "JC_MODEL_BASE",
    "JC_MODEL_PROVIDER",
    "JC_OIDC_ISSUER",
    "JC_OIDC_ORGANIZATION_REALMS",
    "JC_DAILY_TOKENS_PER_PERSON",
    "JC_DAILY_TOKENS_ASSISTANT",
    "JC_MODEL_KEY_PROBE_SECONDS",
    "JC_FORGE_REPO",
    "JC_OIDC_BACKCHANNEL_ISSUER",
];

fuzz_target!(|data: &[u8]| {
    let [which, s0, s1, rest @ ..] = data else {
        return;
    };
    let status = u16::from_le_bytes([*s0, *s1]);
    let _ = model_key::read_key_answer(status, rest);
    let _ = model_key::read_credits_answer(status, rest);
    let _ = model_key::of_refusal(status);
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let var = VARS[usize::from(*which) % VARS.len()];
    let lookup = |name: &str| (name == var).then(|| text.to_owned());
    let _ = config::Config::from_lookup(lookup);
    config::fuzzing::realms(text);
});
