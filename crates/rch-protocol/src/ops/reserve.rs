//! Reserve/release operation types.
//!
//! Lease management for worker capacity reservation.

use serde::{Deserialize, Serialize};

/// Reserve request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReserveRequest {
    /// Optional requested TTL in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u32>,
}

impl Default for ReserveRequest {
    fn default() -> Self {
        Self { ttl_seconds: None }
    }
}

/// Reserve response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReserveResponse {
    /// The assigned lease ID.
    pub lease_id: String,
    /// TTL in seconds until the lease expires.
    pub ttl_seconds: u32,
}

/// Release request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseRequest {
    /// The lease ID to release.
    pub lease_id: String,
}

/// Release response payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleaseResponse {
    /// Always true on success (release is idempotent).
    pub released: bool,
}
