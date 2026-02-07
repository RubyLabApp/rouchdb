use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Result, RouchError};
use crate::rev_tree::RevTree;

// ---------------------------------------------------------------------------
// Revision
// ---------------------------------------------------------------------------

/// A CouchDB revision identifier: `{pos}-{hash}`.
///
/// - `pos` is the generation number (starts at 1, increments each edit).
/// - `hash` is a 32-character hex MD5 digest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Revision {
    pub pos: u64,
    pub hash: String,
}

impl Revision {
    pub fn new(pos: u64, hash: String) -> Self {
        Self { pos, hash }
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.pos, self.hash)
    }
}

impl FromStr for Revision {
    type Err = RouchError;

    fn from_str(s: &str) -> Result<Self> {
        let (pos_str, hash) = s
            .split_once('-')
            .ok_or_else(|| RouchError::InvalidRev(s.to_string()))?;
        let pos: u64 = pos_str
            .parse()
            .map_err(|_| RouchError::InvalidRev(s.to_string()))?;
        Ok(Revision {
            pos,
            hash: hash.to_string(),
        })
    }
}

impl Ord for Revision {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.pos
            .cmp(&other.pos)
            .then_with(|| self.hash.cmp(&other.hash))
    }
}

impl PartialOrd for Revision {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// ---------------------------------------------------------------------------
// AttachmentMeta
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub content_type: String,
    pub digest: String,
    pub length: u64,
    #[serde(default)]
    pub stub: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// A CouchDB-compatible document.
#[derive(Debug, Clone)]
pub struct Document {
    pub id: String,
    pub rev: Option<Revision>,
    pub deleted: bool,
    pub data: serde_json::Value,
    pub attachments: HashMap<String, AttachmentMeta>,
}

impl Document {
    /// Create a new document from a JSON value.
    ///
    /// Extracts `_id`, `_rev`, `_deleted`, and `_attachments` from the value
    /// and puts the remaining fields in `data`.
    pub fn from_json(mut value: serde_json::Value) -> Result<Self> {
        let obj = value
            .as_object_mut()
            .ok_or_else(|| RouchError::BadRequest("document must be a JSON object".into()))?;

        let id = obj
            .remove("_id")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();

        let rev = obj
            .remove("_rev")
            .and_then(|v| v.as_str().map(String::from))
            .map(|s| s.parse::<Revision>())
            .transpose()?;

        let deleted = obj
            .remove("_deleted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let attachments: HashMap<String, AttachmentMeta> = obj
            .remove("_attachments")
            .map(|v| serde_json::from_value(v).unwrap_or_default())
            .unwrap_or_default();

        Ok(Document {
            id,
            rev,
            deleted,
            data: value,
            attachments,
        })
    }

    /// Convert back to a JSON value with CouchDB underscore fields.
    pub fn to_json(&self) -> serde_json::Value {
        let mut obj = match &self.data {
            serde_json::Value::Object(m) => m.clone(),
            _ => serde_json::Map::new(),
        };

        obj.insert("_id".into(), serde_json::Value::String(self.id.clone()));

        if let Some(rev) = &self.rev {
            obj.insert("_rev".into(), serde_json::Value::String(rev.to_string()));
        }

        if self.deleted {
            obj.insert("_deleted".into(), serde_json::Value::Bool(true));
        }

        if !self.attachments.is_empty() {
            obj.insert(
                "_attachments".into(),
                serde_json::to_value(&self.attachments).unwrap(),
            );
        }

        serde_json::Value::Object(obj)
    }
}

// ---------------------------------------------------------------------------
// DocumentMetadata — stored in the database alongside the rev tree
// ---------------------------------------------------------------------------

/// Internal metadata stored per document in the adapter.
#[derive(Debug, Clone)]
pub struct DocMetadata {
    pub id: String,
    pub rev_tree: RevTree,
    pub seq: u64,
}

// ---------------------------------------------------------------------------
// Option / response types shared across the crate
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct GetOptions {
    /// Retrieve a specific revision.
    pub rev: Option<String>,
    /// Include conflicting revisions in `_conflicts`.
    pub conflicts: bool,
    /// Return all open (leaf) revisions.
    pub open_revs: Option<OpenRevs>,
    /// Include full revision history.
    pub revs: bool,
}

#[derive(Debug, Clone)]
pub enum OpenRevs {
    All,
    Specific(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PutResponse {
    pub ok: bool,
    pub id: String,
    pub rev: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocResult {
    pub ok: bool,
    pub id: String,
    pub rev: Option<String>,
    pub error: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct BulkDocsOptions {
    /// When false (replication), accept revisions as-is.
    /// When true (default), generate new revisions and check conflicts.
    pub new_edits: bool,
}

impl BulkDocsOptions {
    pub fn new() -> Self {
        Self { new_edits: true }
    }

    pub fn replication() -> Self {
        Self { new_edits: false }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AllDocsOptions {
    pub start_key: Option<String>,
    pub end_key: Option<String>,
    pub key: Option<String>,
    pub keys: Option<Vec<String>>,
    pub include_docs: bool,
    pub descending: bool,
    pub skip: u64,
    pub limit: Option<u64>,
    pub inclusive_end: bool,
}

impl AllDocsOptions {
    pub fn new() -> Self {
        Self {
            inclusive_end: true,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllDocsRow {
    pub id: String,
    pub key: String,
    pub value: AllDocsRowValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllDocsRowValue {
    pub rev: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllDocsResponse {
    pub total_rows: u64,
    pub offset: u64,
    pub rows: Vec<AllDocsRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbInfo {
    pub db_name: String,
    pub doc_count: u64,
    pub update_seq: Seq,
}

// ---------------------------------------------------------------------------
// Changes types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ChangesOptions {
    pub since: Seq,
    pub limit: Option<u64>,
    pub descending: bool,
    pub include_docs: bool,
    pub live: bool,
    pub doc_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEvent {
    pub seq: Seq,
    pub id: String,
    pub changes: Vec<ChangeRev>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeRev {
    pub rev: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesResponse {
    pub results: Vec<ChangeEvent>,
    pub last_seq: Seq,
}

// ---------------------------------------------------------------------------
// Replication-related types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkGetItem {
    pub id: String,
    pub rev: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkGetResponse {
    pub results: Vec<BulkGetResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkGetResult {
    pub id: String,
    pub docs: Vec<BulkGetDoc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkGetDoc {
    pub ok: Option<serde_json::Value>,
    pub error: Option<BulkGetError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkGetError {
    pub id: String,
    pub rev: String,
    pub error: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevsDiffResponse {
    #[serde(flatten)]
    pub results: HashMap<String, RevsDiffResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevsDiffResult {
    pub missing: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub possible_ancestors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Sequence type — supports both numeric (local) and opaque string (CouchDB)
// ---------------------------------------------------------------------------

/// A database sequence identifier.
///
/// Local adapters use numeric sequences (0, 1, 2, ...).
/// CouchDB 3.x uses opaque string sequences that must be passed back as-is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Seq {
    Num(u64),
    Str(String),
}

impl Seq {
    /// The zero sequence (start from the beginning).
    pub fn zero() -> Self {
        Seq::Num(0)
    }

    /// Extract the numeric value. For opaque strings, parses the numeric
    /// prefix (e.g., `"13-abc..."` → `13`). Returns 0 if unparseable.
    pub fn as_num(&self) -> u64 {
        match self {
            Seq::Num(n) => *n,
            Seq::Str(s) => s
                .split('-')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0),
        }
    }

    /// Format for use in HTTP query parameters.
    pub fn to_query_string(&self) -> String {
        match self {
            Seq::Num(n) => n.to_string(),
            Seq::Str(s) => s.clone(),
        }
    }
}

impl Default for Seq {
    fn default() -> Self {
        Seq::Num(0)
    }
}

impl From<u64> for Seq {
    fn from(n: u64) -> Self {
        Seq::Num(n)
    }
}

impl std::fmt::Display for Seq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Seq::Num(n) => write!(f, "{}", n),
            Seq::Str(s) => write!(f, "{}", s),
        }
    }
}

// ---------------------------------------------------------------------------
// Attachment options
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct GetAttachmentOptions {
    pub rev: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_display_and_parse() {
        let rev = Revision::new(3, "abc123".into());
        assert_eq!(rev.to_string(), "3-abc123");

        let parsed: Revision = "3-abc123".parse().unwrap();
        assert_eq!(parsed, rev);
    }

    #[test]
    fn revision_ordering() {
        let r1 = Revision::new(1, "aaa".into());
        let r2 = Revision::new(2, "aaa".into());
        let r3 = Revision::new(2, "bbb".into());
        assert!(r1 < r2);
        assert!(r2 < r3);
    }

    #[test]
    fn invalid_revision() {
        assert!("nope".parse::<Revision>().is_err());
        assert!("abc-123".parse::<Revision>().is_err());
    }

    #[test]
    fn document_from_json_roundtrip() {
        let json = serde_json::json!({
            "_id": "doc1",
            "_rev": "1-abc",
            "name": "Alice",
            "age": 30
        });

        let doc = Document::from_json(json).unwrap();
        assert_eq!(doc.id, "doc1");
        assert_eq!(doc.rev.as_ref().unwrap().to_string(), "1-abc");
        assert_eq!(doc.data["name"], "Alice");
        assert!(!doc.data.as_object().unwrap().contains_key("_id"));

        let back = doc.to_json();
        assert_eq!(back["_id"], "doc1");
        assert_eq!(back["_rev"], "1-abc");
        assert_eq!(back["name"], "Alice");
    }

    #[test]
    fn document_from_json_minimal() {
        let json = serde_json::json!({"hello": "world"});
        let doc = Document::from_json(json).unwrap();
        assert!(doc.id.is_empty());
        assert!(doc.rev.is_none());
        assert!(!doc.deleted);
    }

    #[test]
    fn bulk_docs_options_defaults() {
        let opts = BulkDocsOptions::new();
        assert!(opts.new_edits);

        let repl = BulkDocsOptions::replication();
        assert!(!repl.new_edits);
    }
}
