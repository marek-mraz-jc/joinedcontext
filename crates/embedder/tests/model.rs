//! The real model (`JC_ASSISTANT_TEST_MODEL_DIR`, `ORT_DYLIB_PATH`): vectors through the service
//! equal the ones the same model gives in process, and how many texts a second it serves here.

mod common;

use jc_door::testing::Issuer;
use jc_embedder::client::{Remote, RemoteConfig};
use jc_embedder::server::{Config, DeploymentConfig, Limits, Service, router};
use jc_embedder::{Embedder, Kind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn model_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("JC_ASSISTANT_TEST_MODEL_DIR").unwrap_or_else(|_| {
            panic!(
                "set JC_ASSISTANT_TEST_MODEL_DIR to a directory filled by scripts/ci/e5-small.sh"
            )
        }),
    )
}

/// The largest difference allowed between a number in process and through the service: the same
/// code on the same thread count gives the same floats, so this only admits JSON's round trip
/// of an f32 (serde_json writes the shortest form that reads back exactly).
const TOLERANCE: f32 = 1e-6;

fn texts() -> Vec<String> {
    [
        "Knižnica je otvorená v pondelok až piatok od 9:00 do 18:00.",
        "Zmesový odpad vyvážame z rodinných domov každý utorok.",
        "Kaupunkibussit liikennöivät neljällä linjalla arkisin.",
        "When is the library open?",
        "Коли вивозять змішані відходи від приватних будинків?",
        "",
    ]
    .map(str::to_owned)
    .to_vec()
}

async fn through_the_service(threads: usize) -> (Remote, Embedder) {
    let issuer = Arc::new(Issuer::start("https://id.a.example/realms/a", "a1").await);
    let cfg = Config {
        listen_addr: "127.0.0.1:0".parse().expect("addr"),
        metrics_addr: None,
        model_path: model_dir(),
        threads,
        limits: Limits::default(),
        deployments: vec![DeploymentConfig {
            name: "a".into(),
            issuer: issuer.issuer.clone(),
            jwks_url: issuer.jwks_url.clone(),
            clients: vec!["assistant-a".into()],
            max_texts_per_minute: 100_000,
        }],
    };
    let served = Embedder::load(&cfg.model_path, threads).expect("model");
    let url = common::serve(router(Arc::new(
        Service::new(served, &cfg).expect("service"),
    )))
    .await;
    let token_url = common::token_endpoint(issuer, "assistant-a", "embedder").await;
    let remote = Remote::new(RemoteConfig {
        url,
        token_url,
        client_id: "assistant-a".into(),
        client_secret: "s".into(),
    })
    .expect("client");
    (
        remote,
        Embedder::load(&model_dir(), threads).expect("model"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vectors_through_the_service_equal_the_in_process_ones() {
    let (remote, local) = through_the_service(2).await;
    for kind in [Kind::Query, Kind::Passage] {
        let served = remote.embed(kind, &texts()).await.expect("served");
        let mut worst = 0f32;
        for (text, got) in texts().iter().zip(&served) {
            // One at a time in process, as the assistant embeds a question.
            let alone = &local
                .embed(kind, std::slice::from_ref(text))
                .await
                .expect("local")[0];
            for (a, b) in alone.iter().zip(got) {
                worst = worst.max((a - b).abs());
            }
        }
        println!(
            "{kind:?}: largest difference {worst:e} over {} texts",
            texts().len()
        );
        assert!(worst <= TOLERANCE, "{kind:?}: {worst}");
    }
    // A question and the same words as a passage are different vectors (the prefix reached the
    // model).
    let q = remote.embed(Kind::Query, &texts()[..1]).await.expect("q");
    let p = remote.embed(Kind::Passage, &texts()[..1]).await.expect("p");
    assert_ne!(q, p);
}

/// The pinned model's own vector for one question: unit length, and pointing where this model and
/// tokenizer pointed when they were pinned (mean pooling over every token, then normalized).
/// Service and in-process run the same code, so only a reference catches a changed pooling or norm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pinned_model_gives_its_reference_vector() {
    // The int8 model's kernels differ by CPU (VNNI or not: 0.0628 against 0.0729 on one CI runner, run
    // 45135233711), so the check is the direction of the first 32 numbers, not each value. A pooling
    // that subtracts or a norm that does not divide turns the vector around or off unit length.
    const REFERENCE: [f32; 32] = [
        0.072_945_38,
        -0.013_466_948,
        -0.008_591_671,
        -0.008_915_837,
        0.090_851_33,
        -0.055_048_26,
        0.029_731_08,
        0.086_602_02,
        0.047_306_303,
        0.016_226_903,
        0.053_703_14,
        -0.005_479_561,
        0.025_197_761,
        -0.092_675_85,
        -0.016_596_88,
        0.050_257_336,
        0.054_185_066,
        -0.029_046_122,
        -0.038_041_33,
        -0.036_105_86,
        0.073_562_08,
        -0.014_732_069,
        -0.058_972_82,
        0.039_645_232,
        0.052_605_033,
        0.036_623_698,
        0.046_371_39,
        0.002_135_763_6,
        0.114_167_66,
        -0.070_666_13,
        -0.033_836_01,
        -0.049_947_347,
    ];
    let length = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    for threads in [1, 2] {
        let local = Embedder::load(&model_dir(), threads).expect("model");
        let vector = &local
            .embed(Kind::Query, &["When is the library open?".into()])
            .await
            .expect("vector")[0];
        assert_eq!(vector.len(), jc_embedder::DIMENSIONS);
        let norm = length(vector);
        assert!((norm - 1.0).abs() < 1e-5, "unit length: {norm}");
        let head = &vector[..REFERENCE.len()];
        let cosine = head.iter().zip(REFERENCE).map(|(g, w)| g * w).sum::<f32>()
            / (length(head) * length(&REFERENCE));
        assert!(
            cosine > 0.9,
            "{threads} threads: cosine {cosine} to the reference, got {head:?}"
        );
    }
}

/// The load figure: passages of a chunk's length through the service on this host.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn texts_per_second_through_the_service() {
    let threads = std::env::var("EMBEDDER_LOAD_THREADS")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(2);
    let (remote, local) = through_the_service(threads).await;
    let passage = "Mestský úrad je otvorený v pondelok, stredu a piatok od 8:00 do 15:30; \
                   v utorok a vo štvrtok od 8:00 do 12:00. "
        .repeat(8);
    let batch: Vec<String> = (0..64).map(|i| format!("{i} {passage}")).collect();
    let began = Instant::now();
    let served = remote.embed(Kind::Passage, &batch).await.expect("served");
    let remote_rate = served.len() as f64 / began.elapsed().as_secs_f64();
    let began = Instant::now();
    let alone = local.embed(Kind::Passage, &batch).await.expect("local");
    let local_rate = alone.len() as f64 / began.elapsed().as_secs_f64();
    println!(
        "{} passages of {} bytes, {threads} ONNX threads: service {remote_rate:.1} texts/s, in process {local_rate:.1} texts/s",
        batch.len(),
        batch[0].len()
    );
    assert_eq!(served.len(), batch.len());
}
