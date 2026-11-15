#![forbid(unsafe_code)]
//! Password hashing runs off the async workers (P-4-01): a burst of anonymous login POSTs, each
//! one Argon2id hash, leaves the runtime free to answer every other request.

use kaisatsu_oidc::dummy_verify_off_runtime;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_of_password_hashes_leaves_the_runtime_answering() {
    let one = Instant::now();
    kaisatsu_crypto::password::dummy_verify_password("calibrate");
    let hash = one.elapsed();

    let burst: Vec<_> = (0..4)
        .map(|_| tokio::spawn(dummy_verify_off_runtime("wrong password")))
        .collect();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let ping = Instant::now();
    tokio::spawn(async {}).await.expect("ping");
    let waited = ping.elapsed();
    for h in burst {
        h.await.expect("join").expect("hash");
    }
    assert!(
        waited < hash / 2,
        "a spawned task waited {waited:?} behind password hashes of {hash:?} each"
    );
}
