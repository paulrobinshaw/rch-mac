//! RCH Xcode Lane - Remote Xcode build/test gate
//!
//! This crate implements the RCH Xcode Lane, a remote build/test gate for
//! Apple-platform projects that routes safe, allowlisted Xcode commands
//! to macOS workers.

pub mod artifact;
pub mod bundle;
pub mod cancel;
pub mod classifier;
pub mod config;
pub mod destination;
pub mod host;
pub mod inventory;
pub mod job;
pub mod mock;
pub mod pipeline;
pub mod protocol;
pub mod run;
pub mod selection;
pub mod signal;
pub mod state;
pub mod summary;
pub mod timeout;
pub mod toolchain;
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
    SchemaError, SchemaId, LoadError as SchemaLoadError, validate_schema_compatibility,
    is_forward_compatible, load_artifact, load_artifact_from_file,
};
pub use inventory::{InventoryError, WorkerEntry, WorkerInventory};
pub use summary::{
    ExitCode, FailureKind, FailureSubkind, JobSummary, RunSummary, Status,
};
pub use toolchain::{
    resolve_toolchain, ToolchainError, ToolchainIdentity, ToolchainResolution, XcodeConstraint,
};
pub use destination::{
    resolve_destination, DestinationConstraint, DestinationError, Provisioning, ResolvedDestination,
};
pub use selection::{
    is_snapshot_valid, select_worker, ProbeFailure, ProtocolRange, SelectionConstraints,
    SelectionError, SelectionMode, SelectionResult, SnapshotSource, WorkerCandidate,
    WorkerSelection,
};
pub use job::{
    generate_job_id, generate_run_id, validate_identifier, Action, ArtifactProfile,
    JobKeyDestination, JobKeyInputs, JobKeyToolchain, JobSpec, JobSpecBuilder, JobSpecError,
};
pub use run::{
    ExecutionState, PlanStep, RunError, RunExecution, RunPlan, RunPlanBuilder, StepResult,
};
pub use signal::{
    CancellationCoordinator, SignalAction, SignalHandler, SignalState, DEFAULT_GRACE_PERIOD_SECONDS,
    EXIT_CODE_CANCELLED,
};
pub use cancel::{
    CancellationManager, JobCancellation, RunCancellation, update_job_state_cancelled,
    update_run_state_cancelled,
};
pub use timeout::{
    TimeoutConfig, TimeoutEnforcer, TimeoutStatus, TimeoutValidationError,
};
pub use pipeline::{Pipeline, PipelineConfig, PipelineError, PipelineResult, execute_tail};
