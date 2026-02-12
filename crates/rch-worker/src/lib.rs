//! RCH Xcode Lane Worker
//!
//! The worker is a binary that runs on macOS workers (Mac mini).
//! It implements the stdin/stdout JSON RPC protocol for executing
//! Xcode build/test jobs.
//!
//! This crate can be used in two modes:
//! - **Standalone binary**: invoked via SSH for production use
//! - **In-process library**: for unit and integration testing with mock state

pub mod artifact_commit;
pub mod cache;
pub mod config;
pub mod executor;
pub mod handlers;
pub mod mock_state;
pub mod rpc;
pub mod simulator;
pub mod source_store;

pub use artifact_commit::{
    ArtifactCommitConfig, ArtifactCommitError, ArtifactCommitter, BackendIdentity, WorkerIdentity,
    cleanup_orphaned_jobs, find_orphaned_jobs, is_job_complete,
};
pub use config::WorkerConfig;
pub use executor::{
    Executor, ExecutorConfig, ExecutorError, ExecutorResult, ExecutionResult, ExecutionStatus,
    JobInput, JobKeyInputs, ToolchainInput, DestinationInput, ArtifactProfile,
    TOOLCHAIN_SCHEMA_ID, TOOLCHAIN_SCHEMA_VERSION, DESTINATION_SCHEMA_ID, DESTINATION_SCHEMA_VERSION,
};
pub use executor::mcp::{McpExecutor, McpEvent, McpEventType, McpExecutionSummary};
pub use mock_state::MockState;
pub use rpc::RpcHandler;
pub use source_store::{SourceStore, SourceMetadata, StoreError};
pub use cache::{
    DerivedDataCache, DerivedDataMode, CacheConfig, CacheError, CacheResult, CacheStats,
    CacheLock, LockError, LockResult, ToolchainKey,
    SpmCache, SpmCacheMode, SpmCacheKey, SpmCacheStats,
    ResultCache, ResultCacheEntry, ResultCacheStats,
    CacheGc, EvictionPolicy, GcResult,
};
pub use simulator::{
    cleanup_orphaned as cleanup_orphaned_simulators, delete_simulator, delete_simulator_by_name,
    find_device_type, find_orphaned_ephemeral, find_runtime, provision_ephemeral,
    simulator_exists, EphemeralSimulator, SimulatorError, SimulatorResult, EPHEMERAL_PREFIX,
};
