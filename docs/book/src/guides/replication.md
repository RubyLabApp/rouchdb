# Replication

Replication is what makes RouchDB a local-first database. It implements the CouchDB replication protocol, allowing bidirectional sync between any two databases -- local to local, local to remote CouchDB, or even remote to remote.

## Quick Start

```rust
use rouchdb::Database;
use serde_json::json;

let local = Database::open("data/myapp.redb", "myapp")?;
let remote = Database::http("http://localhost:5984/myapp");

// Push local changes to CouchDB
local.replicate_to(&remote).await?;

// Pull remote changes to local
local.replicate_from(&remote).await?;

// Or do both directions at once
let (push_result, pull_result) = local.sync(&remote).await?;
```

## Setting Up CouchDB with Docker

A minimal `docker-compose.yml` for local development:

```yaml
version: "3"
services:
  couchdb:
    image: couchdb:3
    ports:
      - "5984:5984"
    environment:
      COUCHDB_USER: admin
      COUCHDB_PASSWORD: password
    volumes:
      - couchdata:/opt/couchdb/data

volumes:
  couchdata:
```

Start it and create a database:

```bash
docker compose up -d

# Create the database
curl -X PUT http://admin:password@localhost:5984/myapp
```

Then connect from RouchDB:

```rust
let remote = Database::http("http://admin:password@localhost:5984/myapp");
```

## Replication Methods

### replicate_to

Push documents from this database to a target.

```rust
let result = local.replicate_to(&remote).await?;
println!("Pushed {} docs", result.docs_written);
```

### replicate_from

Pull documents from a source into this database.

```rust
let result = local.replicate_from(&remote).await?;
println!("Pulled {} docs", result.docs_written);
```

### sync

Bidirectional sync: pushes first, then pulls. Returns both results as a tuple.

```rust
let (push, pull) = local.sync(&remote).await?;

println!("Push: {} written, Pull: {} written",
    push.docs_written, pull.docs_written);
```

### replicate_to_with_opts

Push with custom replication options.

```rust
use rouchdb::ReplicationOptions;

let result = local.replicate_to_with_opts(&remote, ReplicationOptions {
    batch_size: 50,
    batches_limit: 5,
}).await?;
```

## ReplicationOptions

```rust
use rouchdb::ReplicationOptions;

let opts = ReplicationOptions {
    batch_size: 100,   // documents per batch (default: 100)
    batches_limit: 10, // max batches to buffer (default: 10)
};
```

| Field | Default | Description |
|-------|---------|-------------|
| `batch_size` | 100 | Number of documents to process in each replication batch. Smaller values mean more frequent checkpoints. |
| `batches_limit` | 10 | Maximum number of batches to buffer. Controls memory usage for large replications. |

## How the Replication Protocol Works

RouchDB implements the standard CouchDB replication protocol. Each replication run follows these steps:

1. **Read checkpoint** -- Load the last successfully replicated sequence from the local document store. This allows replication to resume where it left off.

2. **Fetch changes** -- Query the source's changes feed starting from the checkpoint sequence, limited to `batch_size` changes per request.

3. **Compute revs_diff** -- Send the changed document IDs and their revisions to the target. The target responds with which revisions it is missing, avoiding redundant transfers.

4. **Fetch missing documents** -- Use `bulk_get` to retrieve only the documents and revisions the target does not have.

5. **Write to target** -- Write the missing documents to the target using `bulk_docs` with `new_edits: false` (replication mode), which preserves the original revision IDs and merges them into the target's revision trees.

6. **Save checkpoint** -- Persist the last replicated sequence so the next run can start from where this one ended.

Steps 2-6 repeat in a loop until no more changes remain.

## ReplicationResult

Every replication call returns a `ReplicationResult`:

```rust
use rouchdb::ReplicationResult;

let result = local.replicate_to(&remote).await?;

if result.ok {
    println!("Replication succeeded");
} else {
    println!("Replication had errors:");
    for err in &result.errors {
        println!("  - {}", err);
    }
}

println!("Documents read:    {}", result.docs_read);
println!("Documents written: {}", result.docs_written);
println!("Last sequence:     {}", result.last_seq);
```

| Field | Type | Description |
|-------|------|-------------|
| `ok` | `bool` | `true` if no errors occurred. |
| `docs_read` | `u64` | Number of change events read from the source. |
| `docs_written` | `u64` | Number of documents written to the target. |
| `errors` | `Vec<String>` | Descriptions of any errors during replication. |
| `last_seq` | `Seq` | The source sequence up to which replication completed. |

Note that `docs_read` may be greater than `docs_written` when the target already has some of the documents (incremental replication).

## Incremental Replication

Replication is incremental by default. Checkpoints are stored as local documents (prefixed with `_local/`) that are not themselves replicated. After an initial full sync, subsequent calls only transfer new changes:

```rust
// First run: syncs everything
let r1 = local.replicate_to(&remote).await?;
println!("Initial: {} docs written", r1.docs_written); // e.g. 500

// Add some new documents
local.put("new_doc", json!({"data": "hello"})).await?;

// Second run: only syncs the delta
let r2 = local.replicate_to(&remote).await?;
println!("Incremental: {} docs written", r2.docs_written); // 1
```

## Replication Events

The `ReplicationEvent` enum is available for progress tracking:

```rust
use rouchdb::ReplicationEvent;

// ReplicationEvent variants:
// ReplicationEvent::Change { docs_read }  -- progress update
// ReplicationEvent::Paused                -- waiting for more changes
// ReplicationEvent::Active                -- replication resumed
// ReplicationEvent::Complete(result)      -- replication finished
// ReplicationEvent::Error(message)        -- an error occurred
```

## Complete Example: Local-to-CouchDB Sync

```rust
use rouchdb::{Database, ReplicationOptions};
use serde_json::json;

#[tokio::main]
async fn main() -> rouchdb::Result<()> {
    // Open persistent local database
    let local = Database::open("data/todos.redb", "todos")?;

    // Connect to CouchDB
    let remote = Database::http("http://admin:password@localhost:5984/todos");

    // Create some local documents
    local.put("todo:1", json!({
        "title": "Buy groceries",
        "done": false
    })).await?;

    local.put("todo:2", json!({
        "title": "Write documentation",
        "done": true
    })).await?;

    // Push to CouchDB with custom batch size
    let push = local.replicate_to_with_opts(&remote, ReplicationOptions {
        batch_size: 50,
        batches_limit: 10,
    }).await?;

    println!("Push complete: {} docs written", push.docs_written);

    // Pull any changes others made on CouchDB
    let pull = local.replicate_from(&remote).await?;
    println!("Pull complete: {} docs written", pull.docs_written);

    // Check local state
    let info = local.info().await?;
    println!("Local database: {} docs, seq {}",
        info.doc_count, info.update_seq);

    Ok(())
}
```

## Local-to-Local Replication

Replication works between any two adapters, not just local and remote. This is useful for backup, migration, or testing:

```rust
let db_a = Database::memory("a");
let db_b = Database::memory("b");

db_a.put("doc1", json!({"from": "a"})).await?;
db_b.put("doc2", json!({"from": "b"})).await?;

// Sync both directions
let (push, pull) = db_a.sync(&db_b).await?;

// Both databases now have both documents
assert_eq!(db_a.info().await?.doc_count, 2);
assert_eq!(db_b.info().await?.doc_count, 2);
```
