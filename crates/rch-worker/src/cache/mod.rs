//! Cache management for RCH worker
//!
//! Implements caching per PLAN.md §Caching:
//! - DerivedData modes: off | per_job | shared
//! - SPM cache modes: off | shared
//! - Toolchain-keyed directories to prevent cross-toolchain corruption
//! - Locking for shared caches
//!
//! ## Directory layout
//!
//! - DerivedData: `caches/<namespace>/derived_data/<mode>/<key>/...`
//! - SPM: `caches/<namespace>/spm/<toolchain_key>/<resolved_hash>/...`
//!
//! ## Cache keying
//!
//! All cache directories are additionally keyed by toolchain identity:
//! - Xcode build number (e.g., "16C5032a")
//! - macOS major version (e.g., "15")
//! - Architecture (e.g., "arm64")
//!
//! SPM caches are further keyed by Package.resolved content hash.
//!
//! This prevents cross-toolchain corruption when using shared caches.
//!
//! ## Locking
//!
//! Shared caches use advisory file locks with configurable timeout.
//! Lock contention is logged as a warning.

mod derived_data;
mod gc;
mod lock;
mod result;
mod spm;
mod toolchain_key;

pub use derived_data::{DerivedDataCache, DerivedDataMode, CacheConfig, CacheError, CacheResult, CacheStats};
pub use gc::{CacheGc, EvictionPolicy, GcResult};
pub use lock::{CacheLock, LockError, LockResult};
pub use result::{ResultCache, ResultCacheEntry, ResultCacheStats};
pub use spm::{SpmCache, SpmCacheMode, SpmCacheKey, SpmCacheStats};
pub use toolchain_key::ToolchainKey;
