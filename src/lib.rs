//! RCH Xcode Lane - Remote Xcode build/test gate
//!
//! This crate implements the RCH Xcode Lane, a remote build/test gate for
//! Apple-platform projects that routes safe, allowlisted Xcode commands
//! to macOS workers.

pub mod artifact;
pub mod bundle;
pub mod classifier;
pub mod config;
pub mod host;
pub mod inventory;
pub mod mock;
pub mod protocol;
pub mod state;
pub mod summary;
pub mod worker;

pub use classifier::{
    Classifier, ClassifierConfig, ClassifierPolicy, ClassifierResult, ConfigConstraints,
    ConfigError, EffectivePolicy, ExplainOutput, Invocation, MatchedConstraintsOutput,
    PolicyAllowlist, PolicyConstraints, PolicyDenylist, RepoConfig, VerifyAction,
};
pub use config::{
    BuiltinDefaults, ConfigError as MergeConfigError, ConfigOrigin, ConfigSource, EffectiveConfig,
};
pub use mock::{MockWorker, MockState, Job, JobState as MockJobState, Lease, FailureConfig, FailureInjector};
pub use protocol::{RpcRequest, RpcResponse, Operation, ErrorCode, RpcError};
pub use state::{
    CurrentStep, JobState, JobStateData, JobStateError, RunState, RunStateData, RunStateError,
};
pub use bundle::{
    BundleError, BundleMode, BundleResult, Bundler, EntryType, ExcludeError, ExcludeRules,
    ManifestEntry, SourceManifest,
};
pub use host::{RpcClient, RpcClientConfig, RpcResult, RpcError as HostRpcError, Transport, MockTransport, SshTransport};
pub use worker::RpcHandler;
pub use artifact::{
    ArtifactEntry, ArtifactEntryType, ArtifactManifest, IntegrityError, ManifestError,
};
pub use inventory::{InventoryError, WorkerEntry, WorkerInventory};
pub use summary::{
    ExitCode, FailureKind, FailureSubkind, JobSummary, RunSummary, Status,
};
