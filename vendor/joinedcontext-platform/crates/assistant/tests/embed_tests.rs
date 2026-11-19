//! The embedding model and the passages it embeds (T-3053, ADR-N-040 §3.1), with the pinned
//! model files and a real PostgreSQL (see `common`).

#[path = "common/corpus.rs"]
mod corpus;
#[path = "common/db.rs"]
mod db;
#[path = "common/model.rs"]
mod model;

use assistant::embed::{embed_missing, embed_until, Embedder, MODEL_FILES};
use assistant::{Error, DIMENSIONS};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (norm(a) * norm(b))
}

fn passages_text(i: usize) -> String {
    [
        "Knižnica je otvorená v pondelok až piatok od 9:00 do 18:00.",
        "Zmesový odpad vyvážame z rodinných domov každý utorok.",
        "Kaupunkibussit liikennöivät neljällä linjalla arkisin.",
    ][i]
        .to_owned()
}

#[tokio::test]
async fn a_question_lands_nearest_the_passage_that_answers_it_across_languages() {
    let embedder = Embedder::load(&model::model_dir(), 2).expect("the pinned model loads");
    let passages = embedder
        .passages(&(0..3).map(passages_text).collect::<Vec<_>>())
        .await
        .expect("passages");
    assert_eq!(passages.len(), 3);
    assert!(passages.iter().all(|v| v.len() == DIMENSIONS));
    for vector in &passages {
        let length = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((length - 1.0).abs() < 1e-4, "normalized, not {length}");
    }
    // One text at a time: the same passage gets the same vector whatever it is embedded beside.
    let alone = embedder.passages(&[passages_text(1)]).await.expect("alone");
    assert_eq!(alone[0], passages[1]);
    for (question, answer) in [
        ("Kedy môžem ísť do knižnice?", 0),
        ("Kdy se odváží odpad od rodinných domů?", 1),
        ("When do the town buses run?", 2),
        // The eval's languages over Slovak text (T-4688): each reaches the Slovak passage.
        ("When is the library open?", 0),
        ("Wann ist die Bibliothek geöffnet?", 0),
        ("Mikor viszik el a vegyes hulladékot a családi házaktól?", 1),
        ("Коли вивозять змішані відходи від приватних будинків?", 1),
    ] {
        let q = embedder.query(question).await.expect("query");
        let nearest = (0..3)
            .max_by(|&a, &b| cosine(&q, &passages[a]).total_cmp(&cosine(&q, &passages[b])))
            .expect("three passages");
        assert_eq!(nearest, answer, "{question}");
    }
    assert!(embedder.passages(&[]).await.expect("nothing").is_empty());
}

#[test]
fn a_model_file_that_is_not_the_pinned_one_is_refused_before_it_is_loaded() {
    let source = model::model_dir();
    let copy = std::env::temp_dir().join(format!("e5-tampered-{}", std::process::id()));
    std::fs::create_dir_all(&copy).expect("dir");
    for (file, _) in MODEL_FILES {
        std::fs::copy(source.join(file), copy.join(file)).expect("copy");
    }
    std::fs::write(copy.join("tokenizer.json"), b"{}").expect("tamper");
    let refused = Embedder::load(&copy, 1).err().expect("refused");
    assert!(matches!(refused, Error::Model(_)));
    let message = refused.to_string();
    assert!(
        message.contains("tokenizer.json") && message.contains("e5-small.sh"),
        "{message}"
    );

    std::fs::copy(source.join("tokenizer.json"), copy.join("tokenizer.json")).expect("restore");
    // The model, hashed as a stream (T-4357): one byte more is still not the pinned file.
    let onnx = copy.join("model_qint8_avx512_vnni.onnx");
    let mut grown = std::fs::OpenOptions::new()
        .append(true)
        .open(&onnx)
        .expect("open");
    std::io::Write::write_all(&mut grown, b"\0").expect("tamper");
    drop(grown);
    let refused = Embedder::load(&copy, 1).err().expect("refused");
    assert!(
        refused
            .to_string()
            .contains("model_qint8_avx512_vnni.onnx is not the pinned file"),
        "{refused}"
    );

    std::fs::remove_file(copy.join("model_qint8_avx512_vnni.onnx")).expect("remove");
    let missing = Embedder::load(&copy, 1).err().expect("refused");
    assert!(
        missing
            .to_string()
            .contains("model_qint8_avx512_vnni.onnx could not be read"),
        "{missing}"
    );
    std::fs::remove_dir_all(&copy).expect("clean up");
}

#[tokio::test]
async fn missing_embeddings_are_filled_for_the_project_alone_and_a_second_pass_finds_none() {
    let embedder = Embedder::load(&model::model_dir(), 2).expect("the pinned model loads");
    let (admin, pool, name) = db::database("embed").await;
    let texts: Vec<String> = (0..20)
        .map(|i| format!("Odstavec číslo {i} o meste."))
        .collect();
    let pages: Vec<(String, &str, &str)> = texts
        .iter()
        .enumerate()
        .map(|(i, text)| (format!("https://hronov.example/{i}"), "sk", text.as_str()))
        .collect();
    let rows: Vec<(&str, &str, &str)> =
        pages.iter().map(|(u, l, t)| (u.as_str(), *l, *t)).collect();
    corpus::pages(&pool, "hronov", "web", &rows).await;
    corpus::pages(
        &pool,
        "lipno",
        "web",
        &[("https://lipno.example/", "cs", "Knihovna je otevřena.")],
    )
    .await;

    // Capped: 7 now, the other 13 on the next call, more than one batch each time.
    assert_eq!(
        embed_missing(&pool, &embedder, "hronov", 7)
            .await
            .expect("first"),
        7
    );
    assert_eq!(
        embed_missing(&pool, &embedder, "hronov", 100)
            .await
            .expect("rest"),
        13
    );
    assert_eq!(
        embed_missing(&pool, &embedder, "hronov", 100)
            .await
            .expect("none"),
        0
    );

    let count = |project: &'static str| {
        let pool = pool.clone();
        async move {
            let mut tx = assistant::project_scope(&pool, project)
                .await
                .expect("scope");
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM chunks WHERE embedding IS NULL")
                .fetch_one(&mut *tx)
                .await
                .expect("count");
            n
        }
    };
    assert_eq!(count("hronov").await, 0);
    assert_eq!(
        count("lipno").await,
        1,
        "another project's chunk is not this call's"
    );
    assert!(matches!(
        embed_missing(&pool, &embedder, "Not A Project", 10).await,
        Err(Error::Project(_))
    ));
    db::drop_database(admin, pool, &name).await;
}

/// T-4749: a passage whose text the project already has a vector for takes that vector, and a
/// text the batch holds twice is embedded once; both chunks of it get the vector.
#[tokio::test]
async fn a_text_seen_before_takes_its_vector_without_the_model() {
    let embedder = Embedder::load(&model::model_dir(), 2).expect("the pinned model loads");
    let (admin, pool, name) = db::database("embedonce").await;
    let footer = "Mestský úrad, Námestie 1, telefón 048 000 000.";
    let ids = corpus::pages(
        &pool,
        "hronov",
        "web",
        &[
            ("https://hronov.example/a", "sk", footer),
            ("https://hronov.example/b", "sk", footer),
            (
                "https://hronov.example/c",
                "sk",
                "Úradné hodiny sú v pondelok.",
            ),
            (
                "https://hronov.example/d",
                "sk",
                "Úradné hodiny sú v pondelok.",
            ),
        ],
    )
    .await;
    // The footer's vector is already known, and it is not what the model would say: a chunk
    // that takes it was never read by the model.
    let known = format!("[{}]", vec!["0.0625"; assistant::DIMENSIONS].join(","));
    let mut tx = assistant::project_scope(&pool, "hronov")
        .await
        .expect("scope");
    sqlx::query("UPDATE chunks SET embedding = $2::vector WHERE id = $1")
        .bind(ids[0])
        .bind(&known)
        .execute(&mut *tx)
        .await
        .expect("known vector");
    tx.commit().await.expect("commit");

    assert_eq!(
        embed_missing(&pool, &embedder, "hronov", 100)
            .await
            .expect("embedded"),
        3
    );
    let mut tx = assistant::project_scope(&pool, "hronov")
        .await
        .expect("scope");
    let vectors: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, embedding::text FROM chunks ORDER BY id")
            .fetch_all(&mut *tx)
            .await
            .expect("vectors");
    tx.rollback().await.expect("rollback");
    let of = |id: i64| &vectors.iter().find(|(i, _)| *i == id).expect("chunk").1;
    assert_eq!(of(ids[1]), of(ids[0]), "the footer's known vector");
    assert!(of(ids[1]).starts_with("[0.0625,"), "{}", of(ids[1]));
    assert_eq!(of(ids[2]), of(ids[3]), "one vector for the text read twice");
    assert_ne!(of(ids[2]), of(ids[0]));
    db::drop_database(admin, pool, &name).await;
}

/// T-4749: the worker's minute embeds until its time budget is spent, not up to a count, and a
/// budget already spent embeds nothing. The budget is three batches as this machine embeds them
/// (T-4807: a fixed 1.5 s let a fast runner embed all 400 passages), the corpus thirty.
#[tokio::test]
async fn passages_are_embedded_until_the_budget_is_spent() {
    let embedder = Embedder::load(&model::model_dir(), 2).expect("the pinned model loads");
    let (admin, pool, name) = db::database("embedbudget").await;
    let total = 30 * assistant::embed::BATCH;
    let texts: Vec<String> = (0..total)
        .map(|i| format!("Odstavec číslo {i} o meste a jeho úrade, ktorý je otvorený."))
        .collect();
    let pages: Vec<(String, &str, &str)> = texts
        .iter()
        .enumerate()
        .map(|(i, text)| (format!("https://hronov.example/{i}"), "sk", text.as_str()))
        .collect();
    let rows: Vec<(&str, &str, &str)> =
        pages.iter().map(|(u, l, t)| (u.as_str(), *l, *t)).collect();
    corpus::pages(&pool, "hronov", "web", &rows).await;

    let spent = std::time::Instant::now();
    assert_eq!(
        embed_until(&pool, &embedder, "hronov", spent)
            .await
            .expect("none"),
        0
    );
    let batch = assistant::embed::BATCH as i64;
    embed_missing(&pool, &embedder, "hronov", batch)
        .await
        .expect("warm-up batch");
    let one = std::time::Instant::now();
    embed_missing(&pool, &embedder, "hronov", batch)
        .await
        .expect("timed batch");
    let budget = one.elapsed() * 3;
    let began = std::time::Instant::now();
    let embedded = embed_until(&pool, &embedder, "hronov", began + budget)
        .await
        .expect("some");
    let took = began.elapsed();
    assert!(embedded >= assistant::embed::BATCH, "{embedded}");
    assert!(
        embedded < total - 2 * assistant::embed::BATCH,
        "it stopped at the budget: {embedded} in {took:?}"
    );
    assert!(
        took < budget + std::time::Duration::from_secs(5),
        "at most one batch past the budget: {took:?}"
    );
    db::drop_database(admin, pool, &name).await;
}

/// An embedder service like `crates/embedder`'s: `413` for a request holding a text over its
/// byte cap or one holding `ODMIETNI`, else a vector per text. It counts the texts it was sent
/// that hold `ODMIETNI`. Its base URL and its token endpoint.
async fn refusing_embedder(refused_sent: Arc<AtomicUsize>) -> (String, String) {
    use axum::{http::StatusCode, Json};
    use serde_json::{json, Value};
    let app = axum::Router::new()
        .route(
            "/token",
            axum::routing::post(|| async {
                Json(json!({"access_token": "t", "token_type": "Bearer", "expires_in": 300}))
            }),
        )
        .route(
            "/v1/embeddings",
            axum::routing::post(move |Json(body): Json<Value>| {
                let refused_sent = Arc::clone(&refused_sent);
                async move {
                    let input: Vec<String> =
                        serde_json::from_value(body["input"].clone()).unwrap_or_default();
                    let held = input.iter().filter(|t| t.contains("ODMIETNI")).count();
                    refused_sent.fetch_add(held, Ordering::SeqCst);
                    if let Some(at) = input
                        .iter()
                        .position(|t| t.len() > jc_embedder::MAX_TEXT_BYTES || t.contains("ODMIETNI"))
                    {
                        let why = format!("text {at} is longer than {} bytes", jc_embedder::MAX_TEXT_BYTES);
                        return (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({"error": why})));
                    }
                    let data: Vec<Value> = (0..input.len())
                        .map(|index| json!({"index": index, "embedding": vec![0.05_f32; DIMENSIONS]}))
                        .collect();
                    (
                        StatusCode::OK,
                        Json(json!({"model": jc_embedder::MODEL,
                                    "model_sha256": jc_embedder::MODEL_SHA256, "data": data})),
                    )
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{addr}"), format!("http://{addr}/token"))
}

/// T-7290 (jc-prod zilina: "413 Payload Too Large text 5 is longer than 8192 bytes" every
/// minute): a 20 000-byte passage of multi-byte letters stored before passages were split is
/// sent within the cap and embedded; a passage the embedder still refuses is marked with why,
/// its batch's other passages get their vectors, and the next round does not send it again.
#[tokio::test]
async fn a_passage_over_the_cap_is_embedded_and_one_refused_is_not_sent_again() {
    let (admin, pool, name) = db::database("embedcap").await;
    let refused_sent = Arc::new(AtomicUsize::new(0));
    let (url, token_url) = refusing_embedder(Arc::clone(&refused_sent)).await;
    let embedder = Embedder::remote(jc_embedder::client::RemoteConfig {
        url,
        token_url,
        client_id: "jc-assistant".into(),
        client_secret: "s".into(),
    })
    .expect("remote");
    let long = "Zimný štadión v Žiline je otvorený denne. ".repeat(500);
    assert!(long.len() > 20_000);
    let ids = corpus::pages(
        &pool,
        "zilina",
        "web",
        &[
            (
                "https://zilina.example/a",
                "sk",
                "Knižnica je otvorená v pondelok.",
            ),
            ("https://zilina.example/b", "sk", &long),
            ("https://zilina.example/c", "sk", "ODMIETNI tento odsek."),
        ],
    )
    .await;
    assert_eq!(
        embed_missing(&pool, &embedder, "zilina", 100)
            .await
            .expect("first round"),
        3
    );
    let state = |id: i64| {
        let pool = pool.clone();
        async move {
            let mut tx = assistant::project_scope(&pool, "zilina")
                .await
                .expect("scope");
            let row: (bool, Option<String>) = sqlx::query_as(
                "SELECT embedding IS NOT NULL, embed_error FROM chunks WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .expect("chunk");
            row
        }
    };
    assert_eq!(state(ids[0]).await, (true, None));
    assert_eq!(
        state(ids[1]).await,
        (true, None),
        "the long passage is embedded"
    );
    let (embedded, why) = state(ids[2]).await;
    assert!(!embedded);
    assert!(why.as_deref().is_some_and(|w| w.contains("413")), "{why:?}");
    let sent = refused_sent.load(Ordering::SeqCst);
    assert_eq!(
        embed_missing(&pool, &embedder, "zilina", 100)
            .await
            .expect("next round"),
        0
    );
    assert_eq!(refused_sent.load(Ordering::SeqCst), sent, "not sent again");
    db::drop_database(admin, pool, &name).await;
}
