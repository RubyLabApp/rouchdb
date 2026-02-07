# Quickstart

This guide walks you through the core features of RouchDB in 5 minutes.

## Create a Database

```rust
use rouchdb::Database;

#[tokio::main]
async fn main() -> rouchdb::Result<()> {
    // In-memory (data lost when dropped — great for testing)
    let db = Database::memory("mydb");

    // Persistent (stored on disk via redb)
    // let db = Database::open("mydb.redb", "mydb")?;

    // Remote CouchDB
    // let db = Database::http("http://admin:password@localhost:5984/mydb");

    Ok(())
}
```

## Put and Get Documents

```rust
use rouchdb::Database;

#[tokio::main]
async fn main() -> rouchdb::Result<()> {
    let db = Database::memory("mydb");

    // Create a document
    let result = db.put("user:alice", serde_json::json!({
        "name": "Alice",
        "email": "alice@example.com",
        "age": 30
    })).await?;

    println!("Created with rev: {}", result.rev.unwrap());

    // Read it back
    let doc = db.get("user:alice").await?;
    println!("Name: {}", doc.data["name"]); // "Alice"

    Ok(())
}
```

## Update and Delete

Every update requires the current revision to prevent conflicts:

```rust
use rouchdb::Database;

#[tokio::main]
async fn main() -> rouchdb::Result<()> {
    let db = Database::memory("mydb");

    // Create
    let r1 = db.put("user:alice", serde_json::json!({"name": "Alice", "age": 30})).await?;
    let rev = r1.rev.unwrap();

    // Update (must provide current rev)
    let r2 = db.update("user:alice", &rev, serde_json::json!({
        "name": "Alice",
        "age": 31
    })).await?;

    // Delete (must provide current rev)
    let rev2 = r2.rev.unwrap();
    db.remove("user:alice", &rev2).await?;

    Ok(())
}
```

## Query with Mango

Find documents matching a selector:

```rust
use rouchdb::{Database, FindOptions};

#[tokio::main]
async fn main() -> rouchdb::Result<()> {
    let db = Database::memory("mydb");

    db.put("alice", serde_json::json!({"name": "Alice", "age": 30})).await?;
    db.put("bob", serde_json::json!({"name": "Bob", "age": 25})).await?;
    db.put("carol", serde_json::json!({"name": "Carol", "age": 35})).await?;

    // Find users older than 28
    let result = db.find(FindOptions {
        selector: serde_json::json!({"age": {"$gte": 28}}),
        ..Default::default()
    }).await?;

    for doc in &result.docs {
        println!("{}: age {}", doc["name"], doc["age"]);
    }
    // Alice: age 30
    // Carol: age 35

    Ok(())
}
```

## Sync Two Databases

```rust
use rouchdb::Database;

#[tokio::main]
async fn main() -> rouchdb::Result<()> {
    let local = Database::memory("local");
    let remote = Database::memory("remote");

    // Add data to each side
    local.put("doc1", serde_json::json!({"from": "local"})).await?;
    remote.put("doc2", serde_json::json!({"from": "remote"})).await?;

    // Bidirectional sync
    let (push, pull) = local.sync(&remote).await?;
    println!("Push: {} docs written", push.docs_written);
    println!("Pull: {} docs written", pull.docs_written);

    // Both databases now have both documents
    let info = local.info().await?;
    println!("Local has {} docs", info.doc_count); // 2

    Ok(())
}
```

## Next Steps

- [Core Concepts](./concepts.md) — understand documents, revisions, and conflicts
- [CRUD Operations](../guides/crud.md) — complete guide to document operations
- [Querying](../guides/querying.md) — Mango selectors and map/reduce views
- [Replication](../guides/replication.md) — sync with CouchDB
