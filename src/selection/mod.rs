//! Worker Selection Algorithm (rch-mac-axz.7)
//!
//! Implements deterministic worker selection per PLAN.md normative spec.
//!
//! Selection algorithm:
//! 1. Filter by required tags (macos, xcode + any repo-required)
//! 2. Probe or load cached capabilities (bounded staleness via probe_ttl_seconds)
//! 3. Filter by constraints (destination exists, required Xcode available)
//! 4. Sort deterministically: by priority (lower = higher), then by stable name
//! 5. Choose first
//!
//! Default mode is `deterministic` - dynamic metrics MUST NOT affect ordering.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::destination::{DestinationConstraint, resolve_destination};
use crate::inventory::WorkerEntry;
use crate::toolchain::{resolve_toolchain, XcodeConstraint};
use crate::worker::Capabilities;

/// Schema version for worker_selection.json
pub const SCHEMA_VERSION: u32 = 1;

/// Schema identifier
pub const SCHEMA_ID: &str = "rch-xcode/worker_selection@1";

/// Default probe TTL in seconds (5 minutes)
pub const DEFAULT_PROBE_TTL_SECONDS: u64 = 300;

/// Worker selection mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    /// Deterministic selection - dynamic metrics MUST NOT affect ordering
    #[default]
    Deterministic,
    /// Adaptive selection - dynamic metrics MAY be used as tie-breakers
    Adaptive,
}

/// Probe failure record for worker_selection.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeFailure {
    /// Worker name that failed to probe
    pub worker: String,
    /// Error message
    pub probe_error: String,
    /// Duration of the probe attempt in milliseconds
    pub probe_duration_ms: u64,
}

/// Protocol version range from worker probe
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolRange {
    /// Minimum supported protocol version
    pub min: u32,
    /// Maximum supported protocol version (inclusive)
    pub max: u32,
}

/// Worker selection artifact (worker_selection.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerSelection {
    /// Schema version
    pub schema_version: u32,

    /// Schema identifier
    pub schema_id: String,

    /// When selection was performed
    pub created_at: DateTime<Utc>,

    /// Run identifier
    pub run_id: String,

    /// Negotiated protocol version
    pub negotiated_protocol_version: u32,

    /// Worker's supported protocol range
    pub worker_protocol_range: ProtocolRange,

    /// Selected worker name
    pub selected_worker: String,

    /// Selected worker SSH host
    pub selected_worker_host: String,

    /// Selection mode used
    pub selection_mode: SelectionMode,

    /// Number of workers passing tag + constraint filters
    pub candidate_count: u32,

    /// Probe failures (workers that couldn't be probed)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probe_failures: Vec<ProbeFailure>,

    /// Age of the capabilities snapshot used (in seconds)
    pub snapshot_age_seconds: u64,

    /// Source of the snapshot
    pub snapshot_source: SnapshotSource,

    /// Metrics used for adaptive selection (only set if mode=adaptive)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adaptive_metrics: Option<AdaptiveMetrics>,
}

/// Metrics used for adaptive worker selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveMetrics {
    /// Selected worker's free disk in bytes
    pub disk_free_bytes: u64,

    /// Selected worker's available memory in bytes (if reported)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_available_bytes: Option<u64>,

    /// Whether disk_free was a deciding factor
    pub disk_was_tiebreaker: bool,

    /// Whether memory was a deciding factor
    pub memory_was_tiebreaker: bool,
}

/// Source of capabilities snapshot
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotSource {
    /// Freshly probed from worker
    Fresh,
    /// Loaded from cache
    Cached,
}

/// Worker selection errors
#[derive(Debug, thiserror::Error)]
pub enum SelectionError {
    /// No workers match required tags
    #[error("No workers have required tags: {tags:?}")]
    NoTagMatch { tags: Vec<String> },

    /// No workers have required Xcode
    #[error("No workers have required Xcode: {constraint}")]
    NoXcodeMatch { constraint: String },

    /// No workers have required destination
    #[error("No workers have required destination: {destination}")]
    NoDestinationMatch { destination: String },

    /// All probes failed
    #[error("All worker probes failed: {workers:?}")]
    AllProbesFailed { workers: Vec<String> },

    /// No workers configured
    #[error("No workers configured in inventory")]
    NoWorkersConfigured,

    /// Worker incompatible (catch-all)
    #[error("Worker incompatible: {reason}")]
    WorkerIncompatible { reason: String },
}

/// Constraints for worker selection
#[derive(Debug, Clone, Default)]
pub struct SelectionConstraints {
    /// Required tags (all must match)
    pub required_tags: Vec<String>,

    /// Xcode constraint
    pub xcode: XcodeConstraint,

    /// Destination constraint (optional for filtering)
    pub destination: Option<DestinationConstraint>,

    /// Selection mode
    pub mode: SelectionMode,

    /// Probe TTL in seconds (for cached snapshots)
    pub probe_ttl_seconds: u64,
}

impl SelectionConstraints {
    /// Create with default Xcode lane requirements (macos, xcode tags)
    pub fn xcode_lane() -> Self {
        Self {
            required_tags: vec!["macos".to_string(), "xcode".to_string()],
            probe_ttl_seconds: DEFAULT_PROBE_TTL_SECONDS,
            ..Default::default()
        }
    }

    /// Add additional required tags
    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.required_tags.extend(tags.into_iter().map(|t| t.into()));
        self
    }

    /// Set Xcode constraint
    pub fn with_xcode(mut self, xcode: XcodeConstraint) -> Self {
        self.xcode = xcode;
        self
    }

    /// Set destination constraint
    pub fn with_destination(mut self, destination: DestinationConstraint) -> Self {
        self.destination = Some(destination);
        self
    }

    /// Set selection mode
    pub fn with_mode(mut self, mode: SelectionMode) -> Self {
        self.mode = mode;
        self
    }
}

/// A candidate worker with its capabilities snapshot
#[derive(Debug, Clone)]
pub struct WorkerCandidate<'a> {
    /// Worker entry from inventory
    pub worker: &'a WorkerEntry,

    /// Capabilities snapshot
    pub capabilities: Capabilities,

    /// When the snapshot was captured
    pub snapshot_time: DateTime<Utc>,

    /// Whether the snapshot was freshly probed
    pub fresh: bool,
}

impl<'a> WorkerCandidate<'a> {
    /// Create from worker entry and capabilities
    pub fn new(worker: &'a WorkerEntry, capabilities: Capabilities, fresh: bool) -> Self {
        let snapshot_time = capabilities.created_at;
        Self {
            worker,
            capabilities,
            snapshot_time,
            fresh,
        }
    }

    /// Get snapshot age in seconds
    pub fn snapshot_age_seconds(&self) -> u64 {
        let now = Utc::now();
        (now - self.snapshot_time)
            .num_seconds()
            .max(0) as u64
    }
}

/// Result of worker selection
#[derive(Debug)]
pub struct SelectionResult<'a> {
    /// Selected worker candidate
    pub candidate: WorkerCandidate<'a>,

    /// Total candidates that passed tag filter
    pub tag_filtered_count: usize,

    /// Total candidates that passed all constraint filters
    pub constraint_filtered_count: usize,

    /// Probe failures encountered
    pub probe_failures: Vec<ProbeFailure>,

    /// Negotiated protocol version
    pub negotiated_protocol_version: u32,

    /// Workers filtered out by Xcode constraint
    pub filtered_no_xcode: Vec<String>,

    /// Workers filtered out by destination constraint
    pub filtered_no_destination: Vec<String>,

    /// Selection mode used
    pub selection_mode: SelectionMode,

    /// Adaptive metrics (if mode=adaptive)
    pub adaptive_metrics: Option<AdaptiveMetrics>,
}

impl<'a> SelectionResult<'a> {
    /// Build worker_selection.json artifact
    pub fn to_artifact(&self, run_id: &str) -> WorkerSelection {
        WorkerSelection {
            schema_version: SCHEMA_VERSION,
            schema_id: SCHEMA_ID.to_string(),
            created_at: Utc::now(),
            run_id: run_id.to_string(),
            negotiated_protocol_version: self.negotiated_protocol_version,
            worker_protocol_range: ProtocolRange {
                min: self.candidate.capabilities.protocol_min,
                max: self.candidate.capabilities.protocol_max,
            },
            selected_worker: self.candidate.worker.name.clone(),
            selected_worker_host: self.candidate.worker.host.clone(),
            selection_mode: self.selection_mode,
            candidate_count: self.constraint_filtered_count as u32,
            probe_failures: self.probe_failures.clone(),
            snapshot_age_seconds: self.candidate.snapshot_age_seconds(),
            snapshot_source: if self.candidate.fresh {
                SnapshotSource::Fresh
            } else {
                SnapshotSource::Cached
            },
            adaptive_metrics: self.adaptive_metrics.clone(),
        }
    }
}

/// Core selection algorithm - pure function
///
/// Input: list of (worker_entry, capabilities) + constraints
/// Output: selected worker OR error with specific reason
///
/// Per PLAN.md, this MUST be testable as a pure function with NO I/O.
pub fn select_worker<'a>(
    candidates: &'a [WorkerCandidate<'a>],
    constraints: &SelectionConstraints,
    host_protocol_range: (u32, u32),
) -> Result<SelectionResult<'a>, SelectionError> {
    if candidates.is_empty() {
        return Err(SelectionError::NoWorkersConfigured);
    }

    // Phase 1: Filter by tags
    let tag_refs: Vec<&str> = constraints.required_tags.iter().map(|s| s.as_str()).collect();
    let tag_filtered: Vec<&WorkerCandidate> = candidates
        .iter()
        .filter(|c| c.worker.has_tags(&tag_refs))
        .collect();

    if tag_filtered.is_empty() {
        return Err(SelectionError::NoTagMatch {
            tags: constraints.required_tags.clone(),
        });
    }

    let tag_filtered_count = tag_filtered.len();

    // Phase 2: Filter by Xcode constraint
    let mut filtered_no_xcode = Vec::new();
    let xcode_filtered: Vec<&WorkerCandidate> = tag_filtered
        .into_iter()
        .filter(|c| {
            if constraints.xcode.is_empty() {
                return true;
            }
            match resolve_toolchain(&c.capabilities, &constraints.xcode) {
                Ok(_) => true,
                Err(_) => {
                    filtered_no_xcode.push(c.worker.name.clone());
                    false
                }
            }
        })
        .collect();

    if xcode_filtered.is_empty() {
        return Err(SelectionError::NoXcodeMatch {
            constraint: format!("{:?}", constraints.xcode),
        });
    }

    // Phase 3: Filter by destination constraint (if specified)
    let mut filtered_no_destination = Vec::new();
    let destination_filtered: Vec<&WorkerCandidate> = if let Some(ref dest) = constraints.destination {
        xcode_filtered
            .into_iter()
            .filter(|c| {
                match resolve_destination(dest, &c.capabilities) {
                    Ok(_) => true,
                    Err(_) => {
                        filtered_no_destination.push(c.worker.name.clone());
                        false
                    }
                }
            })
            .collect()
    } else {
        xcode_filtered
    };

    if destination_filtered.is_empty() {
        return Err(SelectionError::NoDestinationMatch {
            destination: constraints
                .destination
                .as_ref()
                .map(|d| format!("{:?}", d))
                .unwrap_or_else(|| "unknown".to_string()),
        });
    }

    // Phase 4: Filter by protocol version compatibility
    let protocol_filtered: Vec<&WorkerCandidate> = destination_filtered
        .into_iter()
        .filter(|c| {
            // Check if there's an intersection between host and worker protocol ranges
            let worker_min = c.capabilities.protocol_min;
            let worker_max = c.capabilities.protocol_max;
            let (host_min, host_max) = host_protocol_range;
            worker_max >= host_min && host_max >= worker_min
        })
        .collect();

    if protocol_filtered.is_empty() {
        return Err(SelectionError::WorkerIncompatible {
            reason: "No protocol version intersection".to_string(),
        });
    }

    let constraint_filtered_count = protocol_filtered.len();

    // Phase 5: Sort by priority, then (if adaptive) by dynamic metrics, then by name
    let mut sorted = protocol_filtered;
    sorted.sort_by(|a, b| {
        let priority_cmp = a.worker.priority.cmp(&b.worker.priority);
        if priority_cmp != std::cmp::Ordering::Equal {
            return priority_cmp;
        }

        // In adaptive mode, use dynamic metrics as tie-breakers
        if constraints.mode == SelectionMode::Adaptive {
            // Prefer workers with more free disk (descending)
            let disk_cmp = b.capabilities.capacity.disk_free_bytes
                .cmp(&a.capabilities.capacity.disk_free_bytes);
            if disk_cmp != std::cmp::Ordering::Equal {
                return disk_cmp;
            }

            // Prefer workers with more free memory (descending), if available
            let a_mem = a.capabilities.capacity.memory_available_bytes.unwrap_or(0);
            let b_mem = b.capabilities.capacity.memory_available_bytes.unwrap_or(0);
            let mem_cmp = b_mem.cmp(&a_mem);
            if mem_cmp != std::cmp::Ordering::Equal {
                return mem_cmp;
            }
        }

        // Final tie-breaker: stable name (alphabetical)
        a.worker.name.cmp(&b.worker.name)
    });

    // Phase 6: Choose first
    let selected = sorted[0];

    // Calculate negotiated protocol version (highest common)
    let negotiated = std::cmp::min(
        selected.capabilities.protocol_max,
        host_protocol_range.1,
    );

    // Build adaptive metrics if in adaptive mode
    let adaptive_metrics = if constraints.mode == SelectionMode::Adaptive {
        // Determine if metrics were tiebreakers by checking if there were
        // other workers with same priority
        let same_priority_count = sorted
            .iter()
            .filter(|c| c.worker.priority == selected.worker.priority)
            .count();

        // If multiple workers had same priority, something broke the tie
        let disk_was_tiebreaker = same_priority_count > 1 && sorted.len() > 1 && {
            // Check if any other same-priority worker has different disk
            sorted.iter().any(|c| {
                c.worker.priority == selected.worker.priority
                    && c.capabilities.capacity.disk_free_bytes
                        != selected.capabilities.capacity.disk_free_bytes
            })
        };

        let memory_was_tiebreaker = same_priority_count > 1 && !disk_was_tiebreaker && {
            // Disk was same, check if memory differed
            sorted.iter().any(|c| {
                c.worker.priority == selected.worker.priority
                    && c.capabilities.capacity.memory_available_bytes
                        != selected.capabilities.capacity.memory_available_bytes
            })
        };

        Some(AdaptiveMetrics {
            disk_free_bytes: selected.capabilities.capacity.disk_free_bytes,
            memory_available_bytes: selected.capabilities.capacity.memory_available_bytes,
            disk_was_tiebreaker,
            memory_was_tiebreaker,
        })
    } else {
        None
    };

    Ok(SelectionResult {
        candidate: selected.clone(),
        tag_filtered_count,
        constraint_filtered_count,
        probe_failures: Vec::new(), // Probe failures tracked at higher level
        negotiated_protocol_version: negotiated,
        filtered_no_xcode,
        filtered_no_destination,
        selection_mode: constraints.mode,
        adaptive_metrics,
    })
}

/// Check if a cached snapshot is still valid
pub fn is_snapshot_valid(
    snapshot_time: DateTime<Utc>,
    ttl_seconds: u64,
) -> bool {
    let age = Utc::now() - snapshot_time;
    age.num_seconds() < ttl_seconds as i64
}

impl WorkerSelection {
    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Write to file atomically (write-then-rename)
    pub fn write_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        let json = self.to_json().map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;

        // Write to temp file then rename for atomicity
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "No parent directory")
        })?;

        let temp_path = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&temp_path, &json)?;
        std::fs::rename(&temp_path, path)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::WorkerEntry;
    use crate::worker::{Capacity, MacOSInfo, Runtime, XcodeVersion};

    fn sample_worker(name: &str, priority: i32, tags: Vec<&str>) -> WorkerEntry {
        WorkerEntry {
            name: name.to_string(),
            host: format!("{}.local", name),
            port: 22,
            user: "rch".to_string(),
            tags: tags.into_iter().map(|t| t.to_string()).collect(),
            ssh_key_path: None,
            priority,
            known_host_fingerprint: None,
            attestation_pubkey_fingerprint: None,
        }
    }

    fn sample_capabilities(xcode_versions: Vec<(&str, &str)>, runtimes: Vec<&str>) -> Capabilities {
        Capabilities {
            schema_version: 1,
            schema_id: "rch-xcode/capabilities@1".to_string(),
            created_at: Utc::now(),
            rch_xcode_lane_version: "0.1.0".to_string(),
            protocol_min: 1,
            protocol_max: 1,
            features: vec![],
            xcode_versions: xcode_versions
                .into_iter()
                .map(|(version, build)| XcodeVersion {
                    version: version.to_string(),
                    build: build.to_string(),
                    path: format!("/Applications/Xcode-{}.app", version),
                    developer_dir: format!("/Applications/Xcode-{}.app/Contents/Developer", version),
                })
                .collect(),
            active_xcode: None,
            macos: MacOSInfo {
                version: "15.3.1".to_string(),
                build: "24D60".to_string(),
                architecture: "arm64".to_string(),
            },
            simulators: vec![],
            runtimes: runtimes
                .into_iter()
                .map(|v| Runtime {
                    name: format!("iOS {}", v),
                    identifier: format!("com.apple.CoreSimulator.SimRuntime.iOS-{}", v.replace('.', "-")),
                    version: v.to_string(),
                    build: "test".to_string(),
                    available: true,
                })
                .collect(),
            tooling: Default::default(),
            capacity: Capacity {
                max_concurrent_jobs: 2,
                disk_free_bytes: 100_000_000_000,
                disk_total_bytes: None,
                memory_available_bytes: None,
            },
            limits: Default::default(),
        }
    }

    #[test]
    fn test_deterministic_selection_by_priority() {
        let worker_a = sample_worker("worker-a", 1, vec!["macos", "xcode"]);
        let worker_b = sample_worker("worker-b", 2, vec!["macos", "xcode"]);
        let worker_c = sample_worker("worker-c", 1, vec!["macos", "xcode"]);

        let caps = sample_capabilities(vec![("16.2", "16C5032a")], vec!["18.0"]);

        let candidates = vec![
            WorkerCandidate::new(&worker_a, caps.clone(), true),
            WorkerCandidate::new(&worker_b, caps.clone(), true),
            WorkerCandidate::new(&worker_c, caps.clone(), true),
        ];

        let constraints = SelectionConstraints::xcode_lane();
        let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();

        // worker-a should be selected (priority 1, alphabetically first)
        assert_eq!(result.candidate.worker.name, "worker-a");
    }

    #[test]
    fn test_deterministic_selection_same_priority() {
        let worker_c = sample_worker("worker-c", 1, vec!["macos", "xcode"]);
        let worker_a = sample_worker("worker-a", 1, vec!["macos", "xcode"]);
        let worker_b = sample_worker("worker-b", 1, vec!["macos", "xcode"]);

        let caps = sample_capabilities(vec![("16.2", "16C5032a")], vec!["18.0"]);

        // Pass in different order to test sorting
        let candidates = vec![
            WorkerCandidate::new(&worker_c, caps.clone(), true),
            WorkerCandidate::new(&worker_a, caps.clone(), true),
            WorkerCandidate::new(&worker_b, caps.clone(), true),
        ];

        let constraints = SelectionConstraints::xcode_lane();
        let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();

        // worker-a should be selected (same priority, alphabetically first)
        assert_eq!(result.candidate.worker.name, "worker-a");
    }

    #[test]
    fn test_tag_filter() {
        let worker_mac = sample_worker("worker-mac", 1, vec!["macos", "xcode"]);
        let worker_linux = sample_worker("worker-linux", 1, vec!["linux"]);

        let caps = sample_capabilities(vec![("16.2", "16C5032a")], vec!["18.0"]);

        let candidates = vec![
            WorkerCandidate::new(&worker_mac, caps.clone(), true),
            WorkerCandidate::new(&worker_linux, caps.clone(), true),
        ];

        let constraints = SelectionConstraints::xcode_lane();
        let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();

        assert_eq!(result.candidate.worker.name, "worker-mac");
        assert_eq!(result.tag_filtered_count, 1);
    }

    #[test]
    fn test_no_tag_match() {
        let worker_linux = sample_worker("worker-linux", 1, vec!["linux"]);

        let caps = sample_capabilities(vec![("16.2", "16C5032a")], vec!["18.0"]);

        let candidates = vec![
            WorkerCandidate::new(&worker_linux, caps, true),
        ];

        let constraints = SelectionConstraints::xcode_lane();
        let result = select_worker(&candidates, &constraints, (1, 1));

        assert!(matches!(result, Err(SelectionError::NoTagMatch { .. })));
    }

    #[test]
    fn test_xcode_constraint_filter() {
        let worker_a = sample_worker("worker-a", 1, vec!["macos", "xcode"]);
        let worker_b = sample_worker("worker-b", 1, vec!["macos", "xcode"]);

        let caps_with_16 = sample_capabilities(vec![("16.2", "16C5032a")], vec!["18.0"]);
        let caps_with_15 = sample_capabilities(vec![("15.4", "15F31d")], vec!["17.5"]);

        let candidates = vec![
            WorkerCandidate::new(&worker_a, caps_with_16, true),
            WorkerCandidate::new(&worker_b, caps_with_15, true),
        ];

        // Require Xcode 16.2
        let constraints = SelectionConstraints::xcode_lane()
            .with_xcode(XcodeConstraint::exact_version("16.2"));

        let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();

        assert_eq!(result.candidate.worker.name, "worker-a");
        assert_eq!(result.filtered_no_xcode, vec!["worker-b"]);
    }

    #[test]
    fn test_no_xcode_match() {
        let worker_a = sample_worker("worker-a", 1, vec!["macos", "xcode"]);

        let caps = sample_capabilities(vec![("15.4", "15F31d")], vec!["17.5"]);

        let candidates = vec![
            WorkerCandidate::new(&worker_a, caps, true),
        ];

        // Require nonexistent Xcode version
        let constraints = SelectionConstraints::xcode_lane()
            .with_xcode(XcodeConstraint::exact_version("99.0"));

        let result = select_worker(&candidates, &constraints, (1, 1));

        assert!(matches!(result, Err(SelectionError::NoXcodeMatch { .. })));
    }

    #[test]
    fn test_protocol_version_filter() {
        let worker_a = sample_worker("worker-a", 1, vec!["macos", "xcode"]);

        let mut caps = sample_capabilities(vec![("16.2", "16C5032a")], vec!["18.0"]);
        caps.protocol_min = 5;
        caps.protocol_max = 7;

        let candidates = vec![
            WorkerCandidate::new(&worker_a, caps, true),
        ];

        let constraints = SelectionConstraints::xcode_lane();

        // Host only supports v1-2, worker supports v5-7 - no intersection
        let result = select_worker(&candidates, &constraints, (1, 2));

        assert!(matches!(result, Err(SelectionError::WorkerIncompatible { .. })));
    }

    #[test]
    fn test_negotiated_protocol_version() {
        let worker_a = sample_worker("worker-a", 1, vec!["macos", "xcode"]);

        let mut caps = sample_capabilities(vec![("16.2", "16C5032a")], vec!["18.0"]);
        caps.protocol_min = 1;
        caps.protocol_max = 3;

        let candidates = vec![
            WorkerCandidate::new(&worker_a, caps, true),
        ];

        let constraints = SelectionConstraints::xcode_lane();

        // Host supports v1-2, worker supports v1-3 - should negotiate v2
        let result = select_worker(&candidates, &constraints, (1, 2)).unwrap();

        assert_eq!(result.negotiated_protocol_version, 2);
    }

    #[test]
    fn test_empty_candidates() {
        let constraints = SelectionConstraints::xcode_lane();
        let result = select_worker(&[], &constraints, (1, 1));

        assert!(matches!(result, Err(SelectionError::NoWorkersConfigured)));
    }

    #[test]
    fn test_selection_result_to_artifact() {
        let worker_a = sample_worker("worker-a", 1, vec!["macos", "xcode"]);

        let caps = sample_capabilities(vec![("16.2", "16C5032a")], vec!["18.0"]);

        let candidates = vec![
            WorkerCandidate::new(&worker_a, caps, true),
        ];

        let constraints = SelectionConstraints::xcode_lane();
        let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();

        let artifact = result.to_artifact("run-12345");

        assert_eq!(artifact.schema_version, 1);
        assert_eq!(artifact.schema_id, "rch-xcode/worker_selection@1");
        assert_eq!(artifact.run_id, "run-12345");
        assert_eq!(artifact.selected_worker, "worker-a");
        assert_eq!(artifact.selected_worker_host, "worker-a.local");
        assert_eq!(artifact.selection_mode, SelectionMode::Deterministic);
        assert_eq!(artifact.snapshot_source, SnapshotSource::Fresh);
    }

    #[test]
    fn test_snapshot_validity() {
        let now = Utc::now();
        let ttl = 300; // 5 minutes

        // Fresh snapshot is valid
        assert!(is_snapshot_valid(now, ttl));

        // Old snapshot is invalid
        let old = now - chrono::Duration::seconds(400);
        assert!(!is_snapshot_valid(old, ttl));

        // Edge case: exactly at TTL boundary
        let at_boundary = now - chrono::Duration::seconds(299);
        assert!(is_snapshot_valid(at_boundary, ttl));
    }

    #[test]
    fn test_selection_determinism() {
        // Run selection 10 times with same input
        let worker_a = sample_worker("worker-a", 1, vec!["macos", "xcode"]);
        let worker_b = sample_worker("worker-b", 2, vec!["macos", "xcode"]);
        let worker_c = sample_worker("worker-c", 1, vec!["macos", "xcode"]);

        let caps = sample_capabilities(vec![("16.2", "16C5032a")], vec!["18.0"]);

        let constraints = SelectionConstraints::xcode_lane();

        for _ in 0..10 {
            let candidates = vec![
                WorkerCandidate::new(&worker_a, caps.clone(), true),
                WorkerCandidate::new(&worker_b, caps.clone(), true),
                WorkerCandidate::new(&worker_c, caps.clone(), true),
            ];

            let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();
            assert_eq!(result.candidate.worker.name, "worker-a");
        }
    }

    #[test]
    fn test_artifact_serialization() {
        let artifact = WorkerSelection {
            schema_version: 1,
            schema_id: "rch-xcode/worker_selection@1".to_string(),
            created_at: Utc::now(),
            run_id: "run-12345".to_string(),
            negotiated_protocol_version: 1,
            worker_protocol_range: ProtocolRange { min: 1, max: 2 },
            selected_worker: "worker-a".to_string(),
            selected_worker_host: "worker-a.local".to_string(),
            selection_mode: SelectionMode::Deterministic,
            candidate_count: 3,
            probe_failures: vec![ProbeFailure {
                worker: "worker-b".to_string(),
                probe_error: "connection refused".to_string(),
                probe_duration_ms: 1500,
            }],
            snapshot_age_seconds: 120,
            snapshot_source: SnapshotSource::Cached,
            adaptive_metrics: None,
        };

        let json = artifact.to_json().unwrap();
        assert!(json.contains("rch-xcode/worker_selection@1"));
        assert!(json.contains("worker-a"));
        assert!(json.contains("connection refused"));

        let parsed = WorkerSelection::from_json(&json).unwrap();
        assert_eq!(parsed.selected_worker, "worker-a");
        assert_eq!(parsed.probe_failures.len(), 1);
    }

    // =============================================================================
    // Adaptive Mode Tests
    // =============================================================================

    fn sample_capabilities_with_capacity(
        xcode_versions: Vec<(&str, &str)>,
        disk_free_bytes: u64,
        memory_available_bytes: Option<u64>,
    ) -> Capabilities {
        let mut caps = sample_capabilities(xcode_versions, vec!["18.0"]);
        caps.capacity.disk_free_bytes = disk_free_bytes;
        caps.capacity.memory_available_bytes = memory_available_bytes;
        caps
    }

    #[test]
    fn test_adaptive_selection_by_disk() {
        // Three workers, same priority, different disk space
        let worker_a = sample_worker("worker-a", 1, vec!["macos", "xcode"]);
        let worker_b = sample_worker("worker-b", 1, vec!["macos", "xcode"]);
        let worker_c = sample_worker("worker-c", 1, vec!["macos", "xcode"]);

        // worker-b has most disk space
        let caps_a = sample_capabilities_with_capacity(vec![("16.2", "16C5032a")], 100_000_000_000, None);
        let caps_b = sample_capabilities_with_capacity(vec![("16.2", "16C5032a")], 500_000_000_000, None);
        let caps_c = sample_capabilities_with_capacity(vec![("16.2", "16C5032a")], 200_000_000_000, None);

        let candidates = vec![
            WorkerCandidate::new(&worker_a, caps_a, true),
            WorkerCandidate::new(&worker_b, caps_b, true),
            WorkerCandidate::new(&worker_c, caps_c, true),
        ];

        // Adaptive mode should prefer worker-b (most disk)
        let constraints = SelectionConstraints::xcode_lane().with_mode(SelectionMode::Adaptive);
        let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();

        assert_eq!(result.candidate.worker.name, "worker-b");
        assert_eq!(result.selection_mode, SelectionMode::Adaptive);

        // Should have adaptive metrics
        let metrics = result.adaptive_metrics.as_ref().unwrap();
        assert_eq!(metrics.disk_free_bytes, 500_000_000_000);
        assert!(metrics.disk_was_tiebreaker);
        assert!(!metrics.memory_was_tiebreaker);
    }

    #[test]
    fn test_adaptive_selection_by_memory_when_disk_equal() {
        // Three workers, same priority, same disk, different memory
        let worker_a = sample_worker("worker-a", 1, vec!["macos", "xcode"]);
        let worker_b = sample_worker("worker-b", 1, vec!["macos", "xcode"]);
        let worker_c = sample_worker("worker-c", 1, vec!["macos", "xcode"]);

        let disk = 100_000_000_000;
        // worker-c has most memory
        let caps_a = sample_capabilities_with_capacity(vec![("16.2", "16C5032a")], disk, Some(8_000_000_000));
        let caps_b = sample_capabilities_with_capacity(vec![("16.2", "16C5032a")], disk, Some(16_000_000_000));
        let caps_c = sample_capabilities_with_capacity(vec![("16.2", "16C5032a")], disk, Some(32_000_000_000));

        let candidates = vec![
            WorkerCandidate::new(&worker_a, caps_a, true),
            WorkerCandidate::new(&worker_b, caps_b, true),
            WorkerCandidate::new(&worker_c, caps_c, true),
        ];

        // Adaptive mode should prefer worker-c (most memory, since disk is equal)
        let constraints = SelectionConstraints::xcode_lane().with_mode(SelectionMode::Adaptive);
        let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();

        assert_eq!(result.candidate.worker.name, "worker-c");

        let metrics = result.adaptive_metrics.as_ref().unwrap();
        assert_eq!(metrics.memory_available_bytes, Some(32_000_000_000));
        assert!(!metrics.disk_was_tiebreaker); // disk was same
        assert!(metrics.memory_was_tiebreaker);
    }

    #[test]
    fn test_adaptive_falls_back_to_name_when_metrics_equal() {
        // Three workers, same priority, same disk, same memory
        let worker_c = sample_worker("worker-c", 1, vec!["macos", "xcode"]);
        let worker_a = sample_worker("worker-a", 1, vec!["macos", "xcode"]);
        let worker_b = sample_worker("worker-b", 1, vec!["macos", "xcode"]);

        let caps = sample_capabilities_with_capacity(vec![("16.2", "16C5032a")], 100_000_000_000, Some(16_000_000_000));

        // Pass in different order to test sorting
        let candidates = vec![
            WorkerCandidate::new(&worker_c, caps.clone(), true),
            WorkerCandidate::new(&worker_a, caps.clone(), true),
            WorkerCandidate::new(&worker_b, caps.clone(), true),
        ];

        // Adaptive mode should still fall back to alphabetical order
        let constraints = SelectionConstraints::xcode_lane().with_mode(SelectionMode::Adaptive);
        let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();

        // worker-a should be selected (alphabetically first after metrics are equal)
        assert_eq!(result.candidate.worker.name, "worker-a");

        let metrics = result.adaptive_metrics.as_ref().unwrap();
        assert!(!metrics.disk_was_tiebreaker);
        assert!(!metrics.memory_was_tiebreaker);
    }

    #[test]
    fn test_adaptive_priority_still_takes_precedence() {
        // Two workers: one with lower priority but less disk, one with higher priority and more disk
        let worker_high_priority = sample_worker("worker-z", 0, vec!["macos", "xcode"]); // priority 0 = highest
        let worker_low_priority = sample_worker("worker-a", 1, vec!["macos", "xcode"]); // priority 1 = lower

        let caps_high_priority = sample_capabilities_with_capacity(vec![("16.2", "16C5032a")], 50_000_000_000, None);
        let caps_low_priority = sample_capabilities_with_capacity(vec![("16.2", "16C5032a")], 500_000_000_000, None);

        let candidates = vec![
            WorkerCandidate::new(&worker_high_priority, caps_high_priority, true),
            WorkerCandidate::new(&worker_low_priority, caps_low_priority, true),
        ];

        // Even in adaptive mode, priority still wins
        let constraints = SelectionConstraints::xcode_lane().with_mode(SelectionMode::Adaptive);
        let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();

        // worker-z should be selected (higher priority, even though less disk)
        assert_eq!(result.candidate.worker.name, "worker-z");
    }

    #[test]
    fn test_adaptive_metrics_in_artifact() {
        let worker = sample_worker("worker-a", 1, vec!["macos", "xcode"]);
        let caps = sample_capabilities_with_capacity(vec![("16.2", "16C5032a")], 123_456_789, Some(9_876_543_210));

        let candidates = vec![
            WorkerCandidate::new(&worker, caps, true),
        ];

        let constraints = SelectionConstraints::xcode_lane().with_mode(SelectionMode::Adaptive);
        let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();

        let artifact = result.to_artifact("run-adaptive-test");

        assert_eq!(artifact.selection_mode, SelectionMode::Adaptive);
        assert!(artifact.adaptive_metrics.is_some());

        let metrics = artifact.adaptive_metrics.clone().unwrap();
        assert_eq!(metrics.disk_free_bytes, 123_456_789);
        assert_eq!(metrics.memory_available_bytes, Some(9_876_543_210));

        // Serialize to JSON and verify adaptive_metrics is included
        let json = artifact.to_json().unwrap();
        assert!(json.contains("\"adaptive_metrics\""));
        assert!(json.contains("123456789"));
    }

    #[test]
    fn test_deterministic_has_no_adaptive_metrics() {
        let worker = sample_worker("worker-a", 1, vec!["macos", "xcode"]);
        let caps = sample_capabilities_with_capacity(vec![("16.2", "16C5032a")], 100_000_000_000, Some(16_000_000_000));

        let candidates = vec![
            WorkerCandidate::new(&worker, caps, true),
        ];

        // Deterministic mode (default)
        let constraints = SelectionConstraints::xcode_lane();
        let result = select_worker(&candidates, &constraints, (1, 1)).unwrap();

        assert_eq!(result.selection_mode, SelectionMode::Deterministic);
        assert!(result.adaptive_metrics.is_none());

        let artifact = result.to_artifact("run-deterministic-test");
        assert!(artifact.adaptive_metrics.is_none());

        // JSON should NOT contain adaptive_metrics when None
        let json = artifact.to_json().unwrap();
        assert!(!json.contains("adaptive_metrics"));
    }
}
