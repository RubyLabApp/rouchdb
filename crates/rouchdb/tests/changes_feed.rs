//! Changes feed advanced options: since, limit, include_docs, updates/deletes.

mod common;

use common::{delete_remote_db, fresh_remote_db};
use rouchdb::{ChangesOptions, Database};

#[tokio::test]
#[ignore]
async fn changes_since_sequence() {
    let url = fresh_remote_db("ch_since").await;
    let db = Database::http(&url);

    db.put("doc1", serde_json::json!({"v": 1})).await.unwrap();
    db.put("doc2", serde_json::json!({"v": 2})).await.unwrap();
    db.put("doc3", serde_json::json!({"v": 3})).await.unwrap();

    let all = db.changes(ChangesOptions::default()).await.unwrap();
    assert_eq!(all.results.len(), 3);

    let since_seq = all.results[1].seq.clone();
    let partial = db
        .changes(ChangesOptions {
            since: since_seq,
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(partial.results.len() < 3);

    delete_remote_db(&url).await;
}

#[tokio::test]
#[ignore]
async fn changes_with_limit() {
    let url = fresh_remote_db("ch_limit").await;
    let db = Database::http(&url);

    for i in 0..10 {
        db.put(&format!("doc{}", i), serde_json::json!({"i": i})).await.unwrap();
    }

    let changes = db
        .changes(ChangesOptions {
            limit: Some(3),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(changes.results.len(), 3);

    delete_remote_db(&url).await;
}

#[tokio::test]
#[ignore]
async fn changes_include_docs() {
    let url = fresh_remote_db("ch_docs").await;
    let db = Database::http(&url);

    db.put("doc1", serde_json::json!({"name": "Alice"})).await.unwrap();

    let changes = db
        .changes(ChangesOptions {
            include_docs: true,
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(changes.results.len(), 1);
    let doc = changes.results[0].doc.as_ref().unwrap();
    assert_eq!(doc["name"], "Alice");

    delete_remote_db(&url).await;
}

#[tokio::test]
#[ignore]
async fn changes_after_updates_and_deletes() {
    let url = fresh_remote_db("ch_upddel").await;
    let db = Database::http(&url);

    let r1 = db.put("doc1", serde_json::json!({"v": 1})).await.unwrap();
    db.put("doc2", serde_json::json!({"v": 1})).await.unwrap();
    let r3 = db.put("doc3", serde_json::json!({"v": 1})).await.unwrap();

    db.update("doc1", &r1.rev.unwrap(), serde_json::json!({"v": 2})).await.unwrap();
    db.remove("doc3", &r3.rev.unwrap()).await.unwrap();

    let changes = db.changes(ChangesOptions::default()).await.unwrap();

    let ids: Vec<&str> = changes.results.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"doc1"));
    assert!(ids.contains(&"doc2"));
    assert!(ids.contains(&"doc3"));

    let doc3_change = changes.results.iter().find(|r| r.id == "doc3").unwrap();
    assert!(doc3_change.deleted);

    delete_remote_db(&url).await;
}
