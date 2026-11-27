#![forbid(unsafe_code)]
//! The `DaiyaStore` behaviour contract (R8) on the memory store; daiya-store-pg runs the same
//! module on Postgres.

mod contract;

#[tokio::test]
async fn store_contract_holds_on_the_memory_store() {
    contract::run_contract(&daiya_store_memory::MemoryStore::new()).await;
}

/// A folder whose parent is empty sits at the root; a dashboard whose data is no object is kept
/// as it is, without the version and uid members an object gets.
#[tokio::test]
async fn an_empty_parent_is_the_root_and_non_object_data_is_kept() {
    use daiya_model::{DaiyaStore, OrgId};
    let store = daiya_store_memory::MemoryStore::new();
    let org = OrgId::from_u128(77);
    let saved = store
        .upsert_folder(contract::folder(org, "top", Some("")))
        .await
        .expect("an empty parent is the root");
    assert_eq!(saved.uid, "top");
    let mut board = contract::dashboard(org, "arr", None);
    board.data = serde_json::json!([1, 2]);
    let saved = store
        .upsert_dashboard(board, "first", false)
        .await
        .expect("stored");
    assert_eq!(saved.data, serde_json::json!([1, 2]));
    let read = store
        .get_dashboard(org, "arr")
        .await
        .unwrap()
        .expect("found");
    assert_eq!(read.data, serde_json::json!([1, 2]));
}
