//! Cache management for RCH worker
//!
//! Implements caching per PLAN.md §Caching:
//! - DerivedData modes: off | per_job | shared
//! - Toolchain-keyed directories to prevent cross-toolchain corruption
//! - Locking for shared caches
//! - Directory layout: caches/<namespace>/derived_data/<mode>/<key>/...
//!
//! ## Cache keying
//!
//! All cache directories are additionally keyed by toolchain identity:
//! - Xcode build number (e.g., "16C5032a")
//! - macOS major version (e.g., "15")
//! - Architecture (e.g., "arm64")
//!
//! This prevents cross-toolchain corruption when using shared caches.
//!
//! ## Locking
//!
//! Shared caches use advisory file locks with configurable timeout.
//! Lock contention is logged as a warning.

mod derived_data;
mod lock;
mod toolchain_key;

pub use derived_data::{DerivedDataCache, DerivedDataMode, CacheConfig, CacheError, CacheResult, CacheStats};
pub use lock::{CacheLock, LockError, LockResult};
pub use toolchain_key::ToolchainKey;
