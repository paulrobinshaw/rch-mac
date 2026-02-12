//! Resumable upload support for source bundles
//!
//! Per PLAN.md:
//! - When worker advertises feature `upload_resumable`:
//! - `upload_source` payload includes optional `resume` object with `upload_id` and `offset`
//! - Worker responds with `next_offset` for resumable uploads
//! - Enables recovery from interrupted large bundle uploads

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Upload session state for resumable uploads
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSession {
    /// Unique upload ID
    pub upload_id: String,
    /// Source SHA256 being uploaded
    pub source_sha256: String,
    /// Total content length
    pub content_length: u64,
    /// Current offset (bytes uploaded)
    pub offset: u64,
    /// Chunk checksums for verification
    pub chunk_checksums: Vec<String>,
}

impl UploadSession {
    /// Create a new upload session
    pub fn new(upload_id: String, source_sha256: String, content_length: u64) -> Self {
        Self {
            upload_id,
            source_sha256,
            content_length,
            offset: 0,
            chunk_checksums: Vec::new(),
        }
    }

    /// Check if upload is complete
    pub fn is_complete(&self) -> bool {
        self.offset >= self.content_length
    }

    /// Get remaining bytes to upload
    pub fn remaining(&self) -> u64 {
        self.content_length.saturating_sub(self.offset)
    }

    /// Update offset after successful chunk upload
    pub fn advance(&mut self, bytes_uploaded: u64) {
        self.offset = self.offset.saturating_add(bytes_uploaded);
    }
}

/// Request payload for resumable upload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeRequest {
    /// Upload session ID from previous attempt
    pub upload_id: String,
    /// Offset to resume from
    pub offset: u64,
}

/// Response payload for resumable upload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeResponse {
    /// Upload session ID
    pub upload_id: String,
    /// Next offset to upload from
    pub next_offset: u64,
    /// Whether upload is complete
    pub complete: bool,
}

/// Upload session store for tracking in-progress uploads
#[derive(Debug, Default)]
pub struct UploadSessionStore {
    sessions: Mutex<HashMap<String, UploadSession>>,
}

impl UploadSessionStore {
    /// Create a new session store
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Create or get an existing upload session
    pub fn get_or_create(
        &self,
        upload_id: &str,
        source_sha256: &str,
        content_length: u64,
    ) -> UploadSession {
        let mut sessions = self.sessions.lock().unwrap();
        sessions
            .entry(upload_id.to_string())
            .or_insert_with(|| {
                UploadSession::new(upload_id.to_string(), source_sha256.to_string(), content_length)
            })
            .clone()
    }

    /// Get an existing session
    pub fn get(&self, upload_id: &str) -> Option<UploadSession> {
        let sessions = self.sessions.lock().unwrap();
        sessions.get(upload_id).cloned()
    }

    /// Update a session
    pub fn update(&self, session: UploadSession) {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(session.upload_id.clone(), session);
    }

    /// Remove a completed session
    pub fn remove(&self, upload_id: &str) -> Option<UploadSession> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.remove(upload_id)
    }

    /// Check if a session exists for the given source_sha256
    pub fn find_by_source(&self, source_sha256: &str) -> Option<UploadSession> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .values()
            .find(|s| s.source_sha256 == source_sha256)
            .cloned()
    }

    /// Clean up stale sessions (older than max_age_seconds)
    pub fn cleanup_stale(&self, _max_age_seconds: u64) {
        // For now, just clear all sessions
        // In production, would track creation time and only clear old ones
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, s| !s.is_complete());
    }
}

/// Generate a unique upload ID
pub fn generate_upload_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();

    let random: u64 = rand::random();
    format!("upload-{:x}-{:016x}", timestamp, random)
}

/// Thread-safe upload session store
pub type SharedUploadStore = Arc<UploadSessionStore>;

/// Create a new shared upload store
pub fn new_upload_store() -> SharedUploadStore {
    Arc::new(UploadSessionStore::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upload_session_new() {
        let session = UploadSession::new(
            "upload-001".to_string(),
            "sha256-abc".to_string(),
            1000,
        );

        assert_eq!(session.upload_id, "upload-001");
        assert_eq!(session.source_sha256, "sha256-abc");
        assert_eq!(session.content_length, 1000);
        assert_eq!(session.offset, 0);
        assert!(!session.is_complete());
        assert_eq!(session.remaining(), 1000);
    }

    #[test]
    fn test_upload_session_advance() {
        let mut session = UploadSession::new(
            "upload-001".to_string(),
            "sha256-abc".to_string(),
            1000,
        );

        session.advance(500);
        assert_eq!(session.offset, 500);
        assert!(!session.is_complete());
        assert_eq!(session.remaining(), 500);

        session.advance(500);
        assert_eq!(session.offset, 1000);
        assert!(session.is_complete());
        assert_eq!(session.remaining(), 0);
    }

    #[test]
    fn test_session_store_get_or_create() {
        let store = UploadSessionStore::new();

        let session1 = store.get_or_create("upload-001", "sha256-abc", 1000);
        assert_eq!(session1.upload_id, "upload-001");
        assert_eq!(session1.offset, 0);

        // Getting again should return same session
        let session2 = store.get_or_create("upload-001", "sha256-abc", 1000);
        assert_eq!(session2.upload_id, "upload-001");
    }

    #[test]
    fn test_session_store_update() {
        let store = UploadSessionStore::new();

        let mut session = store.get_or_create("upload-001", "sha256-abc", 1000);
        session.advance(500);
        store.update(session);

        let retrieved = store.get("upload-001").unwrap();
        assert_eq!(retrieved.offset, 500);
    }

    #[test]
    fn test_session_store_remove() {
        let store = UploadSessionStore::new();

        store.get_or_create("upload-001", "sha256-abc", 1000);
        assert!(store.get("upload-001").is_some());

        store.remove("upload-001");
        assert!(store.get("upload-001").is_none());
    }

    #[test]
    fn test_session_store_find_by_source() {
        let store = UploadSessionStore::new();

        store.get_or_create("upload-001", "sha256-abc", 1000);
        store.get_or_create("upload-002", "sha256-def", 2000);

        let found = store.find_by_source("sha256-abc").unwrap();
        assert_eq!(found.upload_id, "upload-001");

        let not_found = store.find_by_source("sha256-xyz");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_generate_upload_id() {
        let id1 = generate_upload_id();
        let id2 = generate_upload_id();

        assert!(id1.starts_with("upload-"));
        assert!(id2.starts_with("upload-"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_resume_request_serialization() {
        let req = ResumeRequest {
            upload_id: "upload-001".to_string(),
            offset: 500,
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("upload-001"));
        assert!(json.contains("500"));

        let parsed: ResumeRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.upload_id, "upload-001");
        assert_eq!(parsed.offset, 500);
    }

    #[test]
    fn test_resume_response_serialization() {
        let resp = ResumeResponse {
            upload_id: "upload-001".to_string(),
            next_offset: 500,
            complete: false,
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ResumeResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.upload_id, "upload-001");
        assert_eq!(parsed.next_offset, 500);
        assert!(!parsed.complete);
    }
}
