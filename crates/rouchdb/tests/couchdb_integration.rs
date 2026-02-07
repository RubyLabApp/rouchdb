//! Integration tests against a real CouchDB instance.
//!
//! These tests require a running CouchDB:
//!   docker compose up -d
//!
//! Run with:
//!   cargo test -p rouchdb --test couchdb_integration -- --ignored
//!
//! All tests are marked `#[ignore]` so they don't run in `cargo test`.

use rouchdb::{
    AllDocsOptions, ChangesOptions, Database, FindOptions, ReplicationOptions,
};

/// CouchDB URL. Override with COUCHDB_URL env var.
/// Default matches the docker-compose.yml credentials.
fn couchdb_url() -> String {
    std::env::var("COUCHDB_URL")
        .unwrap_or_else(|_| "http://admin:password@localhost:5984".to_string())
}

/// Create a fresh CouchDB database with a unique name, returning its URL.
async fn fresh_remote_db(prefix: &str) -> String {
    let db_name = format!("{}_{}", prefix, uuid::Uuid::new_v4().to_string().replace('-', ""));
    let url = format!("{}/{}", couchdb_url(), db_name);

    // Create the database
    let client = reqwest::Client::new();
    let resp = client.put(&url).send().await.unwrap();
    assert!(
        resp.status().is_success(),
        "Failed to create DB {}: {}",
        db_name,
        resp.status()
    );

    url
}

/// Delete a CouchDB database.
async fn delete_remote_db(url: &str) {
    let client = reqwest::Client::new();
    let _ = client.delete(url).send().await;
}

// ==========================================================================
// Test matrix
// ==========================================================================

// --- 1. Basic CRUD via HTTP adapter ---

#[tokio::test]
#[ignore]
async fn http_put_and_get() {
    let url = fresh_remote_db("http_crud").await;
    let db = Database::http(&url);

    let result = db.put("doc1", serde_json::json!({"name": "Alice"})).await.unwrap();
    assert!(result.ok);

    let doc = db.get("doc1").await.unwrap();
    assert_eq!(doc.data["name"], "Alice");

    delete_remote_db(&url).await;
}

#[tokio::test]
#[ignore]
async fn http_update_document() {
    let url = fresh_remote_db("http_update").await;
    let db = Database::http(&url);

    let r1 = db.put("doc1", serde_json::json!({"v": 1})).await.unwrap();
    let rev = r1.rev.unwrap();

    let r2 = db.update("doc1", &rev, serde_json::json!({"v": 2})).await.unwrap();
    assert!(r2.ok);

    let doc = db.get("doc1").await.unwrap();
    assert_eq!(doc.data["v"], 2);

    delete_remote_db(&url).await;
}

#[tokio::test]
#[ignore]
async fn http_delete_document() {
    let url = fresh_remote_db("http_delete").await;
    let db = Database::http(&url);

    let r1 = db.put("doc1", serde_json::json!({"v": 1})).await.unwrap();
    let rev = r1.rev.unwrap();

    let r2 = db.remove("doc1", &rev).await.unwrap();
    assert!(r2.ok);

    let err = db.get("doc1").await;
    assert!(err.is_err());

    delete_remote_db(&url).await;
}

#[tokio::test]
#[ignore]
async fn http_all_docs() {
    let url = fresh_remote_db("http_alldocs").await;
    let db = Database::http(&url);

    db.put("alice", serde_json::json!({"name": "Alice"})).await.unwrap();
    db.put("bob", serde_json::json!({"name": "Bob"})).await.unwrap();
    db.put("charlie", serde_json::json!({"name": "Charlie"})).await.unwrap();

    let result = db.all_docs(AllDocsOptions::new()).await.unwrap();
    assert_eq!(result.total_rows, 3);

    delete_remote_db(&url).await;
}

#[tokio::test]
#[ignore]
async fn http_changes_feed() {
    let url = fresh_remote_db("http_changes").await;
    let db = Database::http(&url);

    db.put("doc1", serde_json::json!({"v": 1})).await.unwrap();
    db.put("doc2", serde_json::json!({"v": 2})).await.unwrap();

    let changes = db
        .changes(ChangesOptions::default())
        .await
        .unwrap();
    assert_eq!(changes.results.len(), 2);

    delete_remote_db(&url).await;
}

// --- 2. Local → Remote replication ---

#[tokio::test]
#[ignore]
async fn replicate_memory_to_couchdb() {
    let url = fresh_remote_db("repl_to_couch").await;
    let local = Database::memory("local");
    let remote = Database::http(&url);

    local.put("doc1", serde_json::json!({"name": "Alice"})).await.unwrap();
    local.put("doc2", serde_json::json!({"name": "Bob"})).await.unwrap();
    local.put("doc3", serde_json::json!({"name": "Charlie"})).await.unwrap();

    let result = local.replicate_to(&remote).await.unwrap();
    assert!(result.ok);
    assert_eq!(result.docs_written, 3);

    // Verify docs exist on CouchDB
    let doc = remote.get("doc1").await.unwrap();
    assert_eq!(doc.data["name"], "Alice");

    let info = remote.info().await.unwrap();
    assert_eq!(info.doc_count, 3);

    delete_remote_db(&url).await;
}

// --- 3. Remote → Local replication ---

#[tokio::test]
#[ignore]
async fn replicate_couchdb_to_memory() {
    let url = fresh_remote_db("repl_from_couch").await;
    let remote = Database::http(&url);
    let local = Database::memory("local");

    remote.put("doc1", serde_json::json!({"city": "NYC"})).await.unwrap();
    remote.put("doc2", serde_json::json!({"city": "LA"})).await.unwrap();

    let result = local.replicate_from(&remote).await.unwrap();
    assert!(result.ok);
    assert_eq!(result.docs_written, 2);

    let doc = local.get("doc1").await.unwrap();
    assert_eq!(doc.data["city"], "NYC");

    delete_remote_db(&url).await;
}

// --- 4. Bidirectional sync ---

#[tokio::test]
#[ignore]
async fn bidirectional_sync_with_couchdb() {
    let url = fresh_remote_db("bidir_sync").await;
    let local = Database::memory("local");
    let remote = Database::http(&url);

    // Create docs on each side
    local.put("local_doc", serde_json::json!({"from": "local"})).await.unwrap();
    remote.put("remote_doc", serde_json::json!({"from": "remote"})).await.unwrap();

    // Sync
    let (push, pull) = local.sync(&remote).await.unwrap();
    assert!(push.ok);
    assert!(pull.ok);

    // Both should have both docs
    let _ = local.get("remote_doc").await.unwrap();
    let _ = remote.get("local_doc").await.unwrap();

    delete_remote_db(&url).await;
}

// --- 5. Incremental replication ---

#[tokio::test]
#[ignore]
async fn incremental_replication_to_couchdb() {
    let url = fresh_remote_db("incr_repl").await;
    let local = Database::memory("local");
    let remote = Database::http(&url);

    // First batch
    local.put("doc1", serde_json::json!({"v": 1})).await.unwrap();
    let r1 = local.replicate_to(&remote).await.unwrap();
    assert_eq!(r1.docs_written, 1);

    // Second batch
    local.put("doc2", serde_json::json!({"v": 2})).await.unwrap();
    local.put("doc3", serde_json::json!({"v": 3})).await.unwrap();
    let r2 = local.replicate_to(&remote).await.unwrap();
    assert_eq!(r2.docs_read, 2);
    assert_eq!(r2.docs_written, 2);

    let info = remote.info().await.unwrap();
    assert_eq!(info.doc_count, 3);

    delete_remote_db(&url).await;
}

// --- 6. Replication with deletes ---

#[tokio::test]
#[ignore]
async fn replicate_deletes_to_couchdb() {
    let url = fresh_remote_db("repl_del").await;
    let local = Database::memory("local");
    let remote = Database::http(&url);

    // Create and sync
    let r1 = local.put("doc1", serde_json::json!({"v": 1})).await.unwrap();
    local.replicate_to(&remote).await.unwrap();

    // Delete locally
    local.remove("doc1", &r1.rev.unwrap()).await.unwrap();

    // Replicate the delete
    let result = local.replicate_to(&remote).await.unwrap();
    assert!(result.ok);

    // Remote should see the deletion
    let err = remote.get("doc1").await;
    assert!(err.is_err());

    delete_remote_db(&url).await;
}

// --- 7. Replication with updates ---

#[tokio::test]
#[ignore]
async fn replicate_updates_to_couchdb() {
    let url = fresh_remote_db("repl_upd").await;
    let local = Database::memory("local");
    let remote = Database::http(&url);

    // Create and sync
    let r1 = local.put("doc1", serde_json::json!({"v": 1})).await.unwrap();
    local.replicate_to(&remote).await.unwrap();

    // Update locally
    local
        .update("doc1", &r1.rev.unwrap(), serde_json::json!({"v": 2}))
        .await
        .unwrap();

    // Replicate update
    local.replicate_to(&remote).await.unwrap();

    let doc = remote.get("doc1").await.unwrap();
    assert_eq!(doc.data["v"], 2);

    delete_remote_db(&url).await;
}

// --- 8. Batched replication ---

#[tokio::test]
#[ignore]
async fn batched_replication_to_couchdb() {
    let url = fresh_remote_db("batch_repl").await;
    let local = Database::memory("local");
    let remote = Database::http(&url);

    // Create more docs than batch size
    for i in 0..25 {
        local
            .put(
                &format!("doc{:03}", i),
                serde_json::json!({"i": i}),
            )
            .await
            .unwrap();
    }

    let result = local
        .replicate_to_with_opts(
            &remote,
            ReplicationOptions {
                batch_size: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(result.ok);
    assert_eq!(result.docs_written, 25);

    let info = remote.info().await.unwrap();
    assert_eq!(info.doc_count, 25);

    delete_remote_db(&url).await;
}

// --- 9. Already-synced replication is a no-op ---

#[tokio::test]
#[ignore]
async fn already_synced_noop() {
    let url = fresh_remote_db("synced_noop").await;
    let local = Database::memory("local");
    let remote = Database::http(&url);

    local.put("doc1", serde_json::json!({"v": 1})).await.unwrap();
    local.replicate_to(&remote).await.unwrap();

    // Second replication should write nothing
    let result = local.replicate_to(&remote).await.unwrap();
    assert!(result.ok);
    assert_eq!(result.docs_written, 0);

    delete_remote_db(&url).await;
}

// --- 10. Memory ↔ Redb replication ---

#[tokio::test]
#[ignore]
async fn replicate_memory_to_redb() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.redb");
    let memory = Database::memory("source");
    let redb = Database::open(&path, "target").unwrap();

    memory.put("doc1", serde_json::json!({"v": 1})).await.unwrap();
    memory.put("doc2", serde_json::json!({"v": 2})).await.unwrap();

    let result = memory.replicate_to(&redb).await.unwrap();
    assert!(result.ok);
    assert_eq!(result.docs_written, 2);

    let doc = redb.get("doc1").await.unwrap();
    assert_eq!(doc.data["v"], 1);
}

// --- 11. Redb → CouchDB replication ---

#[tokio::test]
#[ignore]
async fn replicate_redb_to_couchdb() {
    let url = fresh_remote_db("redb_to_couch").await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.redb");
    let local = Database::open(&path, "local").unwrap();
    let remote = Database::http(&url);

    local.put("doc1", serde_json::json!({"origin": "redb"})).await.unwrap();

    let result = local.replicate_to(&remote).await.unwrap();
    assert!(result.ok);
    assert_eq!(result.docs_written, 1);

    let doc = remote.get("doc1").await.unwrap();
    assert_eq!(doc.data["origin"], "redb");

    delete_remote_db(&url).await;
}

// --- 12. Mango query against CouchDB ---

#[tokio::test]
#[ignore]
async fn mango_query_against_couchdb_data() {
    let url = fresh_remote_db("mango_couch").await;
    let remote = Database::http(&url);
    let local = Database::memory("local");

    // Create docs on CouchDB
    remote.put("alice", serde_json::json!({"name": "Alice", "age": 30})).await.unwrap();
    remote.put("bob", serde_json::json!({"name": "Bob", "age": 25})).await.unwrap();
    remote.put("charlie", serde_json::json!({"name": "Charlie", "age": 35})).await.unwrap();

    // Replicate to local
    local.replicate_from(&remote).await.unwrap();

    // Mango query on local data
    let result = local
        .find(FindOptions {
            selector: serde_json::json!({"age": {"$gte": 30}}),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(result.docs.len(), 2);

    delete_remote_db(&url).await;
}

// --- 13. Multiple rounds of sync ---

#[tokio::test]
#[ignore]
async fn multiple_sync_rounds() {
    let url = fresh_remote_db("multi_sync").await;
    let local = Database::memory("local");
    let remote = Database::http(&url);

    // Round 1: local creates, syncs
    local.put("doc1", serde_json::json!({"round": 1})).await.unwrap();
    local.sync(&remote).await.unwrap();

    // Round 2: remote creates, syncs
    remote.put("doc2", serde_json::json!({"round": 2})).await.unwrap();
    local.sync(&remote).await.unwrap();

    // Round 3: both create, sync
    local.put("doc3", serde_json::json!({"round": 3})).await.unwrap();
    remote.put("doc4", serde_json::json!({"round": 4})).await.unwrap();
    local.sync(&remote).await.unwrap();

    // Both should have all 4 docs
    let local_info = local.info().await.unwrap();
    let remote_info = remote.info().await.unwrap();
    assert_eq!(local_info.doc_count, 4);
    assert_eq!(remote_info.doc_count, 4);

    delete_remote_db(&url).await;
}
