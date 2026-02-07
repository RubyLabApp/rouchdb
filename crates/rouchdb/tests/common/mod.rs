/// Shared helpers for integration tests against a real CouchDB instance.
///
/// These tests require a running CouchDB:
///   docker compose up -d
///
/// Run with:
///   cargo test -p rouchdb --test '*' -- --ignored
///
/// All tests are marked `#[ignore]` so they don't run in `cargo test`.
/// CouchDB URL. Override with COUCHDB_URL env var.
/// Default matches the docker-compose.yml credentials.
pub fn couchdb_url() -> String {
    std::env::var("COUCHDB_URL")
        .unwrap_or_else(|_| "http://admin:password@localhost:15984".to_string())
}

/// Create a fresh CouchDB database with a unique name, returning its URL.
pub async fn fresh_remote_db(prefix: &str) -> String {
    let db_name = format!(
        "{}_{}",
        prefix,
        uuid::Uuid::new_v4().to_string().replace('-', "")
    );
    let url = format!("{}/{}", couchdb_url(), db_name);

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
pub async fn delete_remote_db(url: &str) {
    let client = reqwest::Client::new();
    let _ = client.delete(url).send().await;
}
