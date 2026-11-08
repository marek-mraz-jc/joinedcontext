//! Stream definitions kept across restarts through `StreamPersistence` (what `jc` backs with
//! Postgres): a created continuous stream is saved as posted, a removal is deleted, a run-once
//! stream is never saved, a refused save refuses the create, and a restore starts the stream
//! without saving it again.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ekiben_core::{EnvAccess, OrgId, ResourceManager, StreamConfig};
use ekiben_server::{build_full_registry, StreamManager, StreamPersistence};
use futures::future::BoxFuture;
use serde_json::{json, Value};

#[derive(Default)]
struct Recorder {
    rows: Mutex<BTreeMap<(String, String), Value>>,
    saves: Mutex<usize>,
    refuse: Mutex<bool>,
}

impl StreamPersistence for Recorder {
    fn saved<'a>(
        &'a self,
        org: &'a OrgId,
        id: &'a str,
        posted: &'a Value,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if *self.refuse.lock().unwrap() {
                return Err("database down".into());
            }
            *self.saves.lock().unwrap() += 1;
            self.rows
                .lock()
                .unwrap()
                .insert((org.0.clone(), id.to_owned()), posted.clone());
            Ok(())
        })
    }
    fn deleted<'a>(&'a self, org: &'a OrgId, id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if *self.refuse.lock().unwrap() {
                return Err("database down".into());
            }
            self.rows
                .lock()
                .unwrap()
                .remove(&(org.0.clone(), id.to_owned()));
            Ok(())
        })
    }
}

fn manager(rec: &Arc<Recorder>) -> StreamManager {
    StreamManager::new(
        build_full_registry(),
        ResourceManager::new(),
        None,
        EnvAccess::AllowListed,
    )
    .with_persistence(rec.clone())
}

const STREAM: &str = r#"{"org":"city-a","input":{"generate":{"mapping":"root = \"${EKIBEN_VAR_PERSIST_TEST:x}\"","interval":"1s"}},"output":{"drop":{}}}"#;

const ONCE: &str = r#"{"org":"city-a","run":"once","input":{"generate":{"count":1,"mapping":"root = 1"}},"output":{"drop":{}}}"#;

async fn post(m: &StreamManager, id: &str, text: &str) -> Result<(), ekiben_core::CoreError> {
    let (conf, _) = m.parse_config(text, false)?;
    m.create_posted(id.into(), conf, StreamConfig::posted_json(text))
        .await
        .map(|_| ())
}

#[tokio::test]
async fn created_streams_are_saved_as_posted_and_removed_on_delete() {
    let rec = Arc::new(Recorder::default());
    let m = manager(&rec);
    post(&m, "live", STREAM).await.unwrap();
    let org = OrgId("city-a".into());
    let rows = rec.rows.lock().unwrap().clone();
    let saved = &rows[&("city-a".to_owned(), "live".to_owned())];
    assert_eq!(
        saved["input"]["generate"]["mapping"],
        json!("root = \"${EKIBEN_VAR_PERSIST_TEST:x}\""),
        "kept before interpolation"
    );

    // A run-once stream is not kept.
    post(&m, "once", ONCE).await.unwrap();
    assert!(!rec.rows.lock().unwrap().contains_key(&("city-a".into(), "once".into())));

    // A restart: a fresh manager restores the saved stream without saving it again.
    let saves = *rec.saves.lock().unwrap();
    let again = manager(&rec);
    again
        .restore(org.clone(), "live".into(), saved.clone())
        .await
        .unwrap();
    assert!(again.read("live", Some(&org)).await.is_ok(), "restored");
    assert_eq!(*rec.saves.lock().unwrap(), saves, "a restore saves nothing");

    // A refused delete keeps the stream running and its row in place.
    *rec.refuse.lock().unwrap() = true;
    assert!(again.delete("live", Some(&org)).await.is_err());
    assert!(again.read("live", Some(&org)).await.is_ok());
    *rec.refuse.lock().unwrap() = false;
    again.delete("live", Some(&org)).await.unwrap();
    assert!(rec.rows.lock().unwrap().is_empty(), "removed");
    m.stop_all(std::time::Duration::from_secs(2)).await;
    again.stop_all(std::time::Duration::from_secs(2)).await;
}

#[tokio::test]
async fn a_refused_save_refuses_the_create() {
    let rec = Arc::new(Recorder::default());
    *rec.refuse.lock().unwrap() = true;
    let m = manager(&rec);
    let err = post(&m, "live", STREAM).await.unwrap_err().to_string();
    assert!(err.contains("was not saved"), "{err}");
    assert!(m
        .read("live", Some(&OrgId("city-a".into())))
        .await
        .is_err());
}
