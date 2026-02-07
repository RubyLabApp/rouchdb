# Conflict Resolution

Conflicts are a natural part of distributed databases. When the same document is edited on two replicas before they sync, both replicas create new revisions that branch from the same parent. CouchDB (and RouchDB) handle this gracefully: rather than rejecting one edit, both revisions are preserved. The system deterministically picks a **winner** so every replica agrees on what `db.get()` returns, while the losing revisions remain accessible for manual resolution.

## Why Conflicts Happen

Consider two replicas, A and B, that have both synced document `todo:1` at revision `1-abc`:

```text
Replica A: todo:1 @ 1-abc  -->  update  -->  2-def
Replica B: todo:1 @ 1-abc  -->  update  -->  2-ghi
```

When A and B sync, the revision tree for `todo:1` becomes:

```text
1-abc --> 2-def  (branch from replica A)
      --> 2-ghi  (branch from replica B)
```

Both `2-def` and `2-ghi` are valid. The system picks one as the winner; the other becomes a **conflict**.

## The Deterministic Winner Algorithm

CouchDB uses a deterministic algorithm so that every replica independently arrives at the same winner without any coordination:

1. **Non-deleted leaves beat deleted leaves.** A live document always wins over a tombstone at the same generation.
2. **Higher position (generation) wins.** If one branch has more edits, it wins.
3. **Lexicographically greater hash breaks ties.** When two leaves have the same generation and deletion status, the one with the larger hash string wins.

This means if `2-ghi` and `2-def` are both non-deleted at generation 2, then `2-ghi` wins because `"ghi" > "def"` lexicographically.

RouchDB exposes this algorithm through the `winning_rev()` function:

```rust
use rouchdb::winning_rev;

// Given a RevTree (the full revision tree for a document)
let winner = winning_rev(&rev_tree);
// Returns Option<Revision> -- the winning leaf revision
```

## Detecting Conflicts

### Reading Conflicts with get_with_opts

To see whether a document has conflicts, use `get_with_opts` with `conflicts: true`:

```rust
use rouchdb::{Database, GetOptions};

let db = Database::memory("mydb");

// ... after replication creates a conflict ...

let doc = db.get_with_opts("todo:1", GetOptions {
    conflicts: true,
    ..Default::default()
}).await?;

// The document body is the winning revision
println!("Winner: {} rev={}", doc.id, doc.rev.as_ref().unwrap());

// Check the _conflicts field in the returned JSON
let json = doc.to_json();
if let Some(conflicts) = json.get("_conflicts") {
    println!("Conflicting revisions: {}", conflicts);
}
```

### Using collect_conflicts

If you have access to the document's revision tree (from the adapter's internal metadata), you can use the `collect_conflicts` utility:

```rust
use rouchdb_core::merge::collect_conflicts;

// rev_tree: RevTree -- the document's full revision tree
let conflicts = collect_conflicts(&rev_tree);

for conflict_rev in &conflicts {
    println!("Conflict: {}", conflict_rev);
    // conflict_rev is a Revision { pos, hash }
}
```

`collect_conflicts` returns all non-winning, non-deleted leaf revisions. Deleted leaves are excluded because a delete inherently resolves that branch.

### Using is_deleted

Check whether the winning revision of a document is a deletion:

```rust
use rouchdb_core::merge::is_deleted;

if is_deleted(&rev_tree) {
    println!("The document's winning revision is deleted");
}
```

## Resolving Conflicts

The standard resolution strategy is:

1. Read the winning revision and all conflicting revisions.
2. Merge the data as your application sees fit.
3. Update the winner with the merged data.
4. Delete each losing revision.

After this, the document has a single non-deleted leaf and no more conflicts.

### Complete Example

```rust
use rouchdb::{Database, GetOptions, RouchError};
use serde_json::json;

async fn resolve_conflicts(db: &Database, doc_id: &str) -> rouchdb::Result<()> {
    // Step 1: Read the winner with conflicts
    let doc = db.get_with_opts(doc_id, GetOptions {
        conflicts: true,
        ..Default::default()
    }).await?;

    let winner_rev = doc.rev.as_ref().unwrap().to_string();
    let winner_data = doc.data.clone();

    // Extract conflict revisions from the JSON representation
    let doc_json = doc.to_json();
    let conflict_revs: Vec<String> = doc_json
        .get("_conflicts")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    if conflict_revs.is_empty() {
        println!("No conflicts to resolve");
        return Ok(());
    }

    // Step 2: Read each conflicting revision
    let mut all_versions = vec![winner_data.clone()];
    for rev in &conflict_revs {
        let conflict_doc = db.get_with_opts(doc_id, GetOptions {
            rev: Some(rev.clone()),
            ..Default::default()
        }).await?;
        all_versions.push(conflict_doc.data);
    }

    // Step 3: Merge the data (application-specific logic)
    // This example takes the winner and appends notes from losers
    let merged = merge_application_data(&all_versions);

    // Step 4: Update the winner with merged data
    let update_result = db.update(doc_id, &winner_rev, merged).await?;
    let new_rev = update_result.rev.unwrap();
    println!("Updated winner to rev {}", new_rev);

    // Step 5: Delete each losing revision
    for rev in &conflict_revs {
        db.remove(doc_id, rev).await?;
        println!("Deleted conflict rev {}", rev);
    }

    println!("All conflicts resolved for {}", doc_id);
    Ok(())
}

fn merge_application_data(versions: &[serde_json::Value]) -> serde_json::Value {
    // Your merge logic here. Common strategies:
    // - Last-write-wins (pick the one with the latest timestamp field)
    // - Field-level merge (combine non-overlapping fields)
    // - Domain-specific (e.g., union of tags, max of counters)

    // Simple example: take the first version's data
    // and merge "tags" arrays from all versions
    let mut result = versions[0].clone();
    let mut all_tags: Vec<serde_json::Value> = Vec::new();

    for version in versions {
        if let Some(tags) = version.get("tags").and_then(|t| t.as_array()) {
            for tag in tags {
                if !all_tags.contains(tag) {
                    all_tags.push(tag.clone());
                }
            }
        }
    }

    if !all_tags.is_empty() {
        result["tags"] = serde_json::Value::Array(all_tags);
    }

    result
}
```

## Common Merge Strategies

### Last-Write-Wins (LWW)

If your documents include a `modified_at` timestamp, pick the most recent version:

```rust
fn lww_merge(versions: &[serde_json::Value]) -> serde_json::Value {
    versions.iter()
        .max_by_key(|v| {
            v.get("modified_at")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string()
        })
        .cloned()
        .unwrap_or(json!({}))
}
```

### Field-Level Merge

Combine non-overlapping changes from different replicas:

```rust
fn field_merge(
    base: &serde_json::Value,
    a: &serde_json::Value,
    b: &serde_json::Value,
) -> serde_json::Value {
    let mut result = base.clone();
    if let Some(obj) = result.as_object_mut() {
        // For each field, if only one side changed it, take that change
        for (key, b_val) in b.as_object().unwrap_or(&serde_json::Map::new()) {
            let base_val = base.get(key);
            let a_val = a.get(key);
            if a_val == base_val && Some(b_val) != base_val {
                obj.insert(key.clone(), b_val.clone());
            }
        }
        for (key, a_val) in a.as_object().unwrap_or(&serde_json::Map::new()) {
            let base_val = base.get(key);
            if Some(a_val) != base_val {
                obj.insert(key.clone(), a_val.clone());
            }
        }
    }
    result
}
```

## Prevention: Reducing Conflicts

While conflicts are handled gracefully, you can reduce their frequency:

- **Sync frequently.** Shorter intervals between replications mean less opportunity for divergent edits.
- **Use fine-grained documents.** Instead of one big document, split data into smaller documents that are less likely to be edited concurrently.
- **Design for commutative operations.** CRDTs and append-only patterns naturally avoid conflicts.
