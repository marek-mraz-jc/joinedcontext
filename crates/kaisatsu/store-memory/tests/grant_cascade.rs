#![forbid(unsafe_code)]
//! Deletes take their grants along (S3-15) and membership is a row (S3-16), in both stores.
//! The cases live in tests/cascade_cases; kaisatsu-store-pg runs them against `PgStore`.

use cascade_cases::realm;
use kaisatsu_store_memory::MemoryStore;
use std::sync::Arc;

mod cascade_cases;

#[tokio::test]
async fn deleted_realm_role() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::deleted_realm_role(store.as_ref()).await;
}

#[tokio::test]
async fn deleted_client() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::deleted_client(store.as_ref()).await;
}

#[tokio::test]
async fn deleted_group() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::deleted_group(store.as_ref()).await;
}

#[tokio::test]
async fn deleted_person() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::deleted_person(store.as_ref()).await;
}

#[tokio::test]
async fn stale_profile_write() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::stale_profile_write(store.as_ref()).await;
}

#[tokio::test]
async fn named_path_without_membership() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::named_path_without_membership(store.as_ref()).await;
}

#[tokio::test]
async fn deleted_client_role() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::deleted_client_role(store.as_ref()).await;
}

#[tokio::test]
async fn documents_lose_what_is_deleted() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::documents_lose_what_is_deleted(store.as_ref()).await;
}

#[tokio::test]
async fn renamed_client() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::renamed_client(store.as_ref()).await;
}

#[tokio::test]
async fn deleted_service_account() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::deleted_service_account(store.as_ref()).await;
}

#[tokio::test]
async fn edited_group() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::edited_group(store.as_ref()).await;
}

#[tokio::test]
async fn subgroup_of_another_organization() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::subgroup_of_another_organization(store.as_ref()).await;
}

#[tokio::test]
async fn grants_to_the_absent() {
    let store = Arc::new(MemoryStore::default());
    realm(store.as_ref()).await;
    cascade_cases::grants_to_the_absent(store.as_ref()).await;
}
