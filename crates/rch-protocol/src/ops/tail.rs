//! Tail operation types.
//!
//! Cursor-based log/event streaming.

use serde::{Deserialize, Serialize};

/// Tail request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TailRequest {
    /// The job ID to tail.
    pub job_id: String,
    /// Cursor from previous tail response (null for start).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Maximum bytes to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// Maximum events to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_events: Option<u32>,
}

/// Tail response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TailResponse {
    /// The job ID.
    pub job_id: String,
    /// Cursor for the next tail request (null if complete).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Log chunk (UTF-8 text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_chunk: Option<String>,
    /// Structured events.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<serde_json::Value>,
}
