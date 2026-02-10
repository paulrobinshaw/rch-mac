# PLAN — RCH Xcode Lane

## Vision
Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker (Mac mini) using XcodeBuildMCP, producing deterministic, machine-readable results.

## Goals
- Remote Xcode build/test via macOS workers only
- Deterministic configuration and attestation
- Agent-friendly JSON outputs and artifacts
- Safe-by-default interception (false negatives preferred)

## Non-goals
- Code signing, provisioning, notarization, export, TestFlight upload, or publishing
- Arbitrary script execution ("setup steps"), mutable environment bootstrap, or interactive shells
- Replacing Xcode IDE workflows (this is a gate/lane, not an IDE)

## Terminology
- **Host**: the machine running `rch` (may be Linux/macOS).
- **Worker**: macOS machine that executes the job.
- **Invocation**: user-provided command line (e.g. `xcodebuild test ...`).
- **Classifier**: deny-by-default gate that accepts/rejects invocations.
- **JobSpec** (`job.json`): deterministic, fully-resolved job description.
- **Job key** (`job_key`): stable hash used for caching and attestation.
- **Artifact set**: schema-versioned outputs written under `<job_id>/`.

## CLI surface (contract)
The lane MUST provide these user-facing entry points:
- `rch xcode verify` — run repo-defined `verify` actions (`build`, `test`).
- `rch xcode explain -- <cmd...>` — classifier explanation and effective constraints.
- `rch xcode verify --dry-run` — print resolved JobSpec + chosen worker, no execution.
Optional but recommended:
- `rch workers list --tag macos,xcode`
- `rch workers probe <worker>` — capture `capabilities.json` snapshot
- `rch xcode fetch <job_id>` — materialize remote artifacts locally if stored remotely

## Architecture (high level)
Pipeline stages:
1. **Classifier**: detects safe, supported Xcode build/test invocations (deny-by-default).
2. **JobSpec builder**: produces a fully specified, deterministic job description (no ambient defaults).
3. **Transport**: bundles inputs + sends to worker (integrity checked).
4. **Executor**: runs the job on macOS via a selected backend (**xcodebuild** or **XcodeBuildMCP**).
5. **Artifacts**: writes a schema-versioned artifact set + attestation.

## Backends
- **Backend: xcodebuild (MVP)** — minimal dependencies, fastest path to correctness.
- **Backend: XcodeBuildMCP (preferred)** — richer structure, better diagnostics, multi-step orchestration.

## Host↔Worker protocol (normative)
The system MUST behave as if there is a versioned protocol even if implemented over SSH.

### Versioning
- Host and worker MUST each report `rch_xcode_lane_version` and `protocol_version`.
- If `protocol_version` is incompatible, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE`.

### Job lifecycle + idempotency
Worker MUST treat `(job_id, job_key)` as an idempotency key:
- If a job with the same `(job_id, job_key)` is already COMPLETE, worker MAY return the existing artifacts.
- If a job with the same `(job_id, job_key)` is RUNNING, worker MUST report status and allow log tailing.
- If `(job_id, job_key)` mismatches (same job_id, different key), worker MUST reject to prevent artifact confusion.

### Cancellation
- Host MUST be able to request cancellation.
- Worker MUST attempt a best-effort cancel (terminate backend process tree) and write artifacts with `status=failed`
  and `failure_kind=CANCELLED`.

### Log streaming (recommended)
- Worker SHOULD support a "tail" mode so host can stream logs while running.
- If not supported, host MUST still periodically fetch/append logs to avoid silent hangs.

## Deterministic JobSpec + Job Key
Each remote run is driven by a `job.json` (JobSpec) generated on the host.
The host computes:
- `source_sha256` — SHA-256 of the sent source bundle (after canonicalization)
- `job_key` — SHA-256 over: `source_sha256 + effective_config + sanitized_invocation + toolchain`
Artifacts include both values, enabling reproducible reruns and cache keys.

## Source bundling canonicalization (normative)
The host MUST create a canonical source bundle such that identical inputs yield identical `source_sha256`.

Rules:
- Use a deterministic archive format (e.g. `tar`) with:
  - sorted file paths (lexicographic, UTF-8)
  - normalized mtimes (e.g. 0) and uid/gid (0)
  - stable file modes (preserve executable bit; normalize others)
- Exclude by default:
  - `.git/`, `.DS_Store`, `DerivedData/`, `.build/`, `**/*.xcresult/`, `**/.swiftpm/` (build artifacts)
  - any host-local RCH artifact directories
- Include repo config `.rch/xcode.toml` in the bundle (so worker always has the same constraints)
- The host MUST emit `source_manifest.json` listing:
  - `path`, `size`, `sha256` per file
  - manifest `schema_version`

If the bundler cannot apply canonicalization, the job MUST be rejected (`failure_kind=BUNDLER`).

## Classifier (safety gate)
The classifier MUST:
- match only supported forms of `xcodebuild` invocations
- reject unknown flags / actions by default
- enforce repo config constraints (workspace/project, scheme, destination)
- emit a machine-readable explanation when rejecting (`summary.json` includes `rejection_reason`)

### Supported actions (initial contract)
Allowed (when fully constrained by repo config):
- `build`
- `test`
Explicitly denied:
- `archive`, `-exportArchive`, `-exportNotarizedApp`, notarization/signing flows
- `-resultBundlePath` to arbitrary locations (worker controls output paths)
- `-derivedDataPath` to arbitrary locations (worker controls paths per cache mode)

### Supported flags (initial contract)
The classifier MAY allow a minimal safe subset (example):
- `-workspace` OR `-project` (must match repo config)
- `-scheme` (must match repo config)
- `-destination` (must match resolved/pinned destination)
- `-configuration` (optional; if allowed must be pinned or whitelisted)
All other flags MUST be rejected unless explicitly added to the allowlist.

### Sanitized invocation (normative)
If accepted, the host MUST emit `invocation.json` containing:
- `original_argv` (as received)
- `sanitized_argv` (canonical ordering, normalized quoting)
- `accepted_action` (`build`|`test`)
- `rejected_flags` (if any; for dry-run/explain)
`sanitized_argv` MUST NOT contain:
- output path overrides
- script hooks
- unconstrained destinations

## Configuration
- Repo-scoped config: `.rch/xcode.toml` (checked in)
- Host/user config: `~/.config/rch/*` (workers, credentials, defaults)
`effective_config.json` MUST be emitted per job (post-merge, fully resolved).

## Failure taxonomy
`summary.json` MUST include:
- `status`: success | failed | rejected
- `failure_kind`: CLASSIFIER_REJECTED | SSH | TRANSFER | EXECUTOR | XCODEBUILD | MCP | ARTIFACTS | CANCELLED | WORKER_INCOMPATIBLE | BUNDLER | ATTESTATION
- `exit_code`: stable integer for scripting
- `human_summary`: short string for console output

### Stable exit codes (normative)
- 0: SUCCESS
- 10: CLASSIFIER_REJECTED
- 20: SSH/CONNECT
- 30: TRANSFER
- 40: EXECUTOR
- 50: XCODEBUILD_FAILED
- 60: MCP_FAILED
- 70: ARTIFACTS_FAILED
- 80: CANCELLED

## Agent-friendly summaries (recommended)
In addition to `summary.json`, the worker SHOULD emit:
- `test_summary.json` (counts, failing tests, duration, top failures)
- `build_summary.json` (targets, warnings/errors counts, first error location if available)
These MUST be derived from authoritative sources (`xcresult` when present; logs as fallback).

## Timeouts + retries
- SSH/connect retries with backoff
- Transfer retries (idempotent)
- Executor timeout (overall + idle-log watchdog)
On failure, artifacts MUST still include logs + diagnostics if available.

## Caching
Caching MUST be correctness-preserving:
- Cache keys derive from `job_key` (or documented sub-keys).
- DerivedData modes: `off` | `per_job` | `shared` (shared requires safe keying + locking).
- SPM cache mode: `off` | `shared` (shared keyed by resolved Package.resolved + toolchain).
`metrics.json` includes cache hit/miss + sizes + timing.

### Locking + isolation (normative)
- `per_job` DerivedData MUST be written under a directory derived from `job_key`.
- `shared` caches MUST use a lock to prevent concurrent writers corrupting state.
  - Lock MUST have a timeout and emit diagnostics if contention occurs.
- Worker MUST execute each job in an isolated working directory (unique per job_id).

### Eviction / garbage collection (normative)
Worker MUST implement at least one:
- size-based eviction (e.g. keep under N GB)
- age-based eviction (e.g. delete items unused for N days)
Eviction MUST NOT delete caches that are currently locked/in use.

### Concurrency + capacity (normative)
- Worker MUST enforce `max_concurrent_jobs`.
- If capacity exceeded, worker MUST respond with a structured "busy" state so host can retry/backoff.

## Worker capabilities
Worker reports a `capabilities.json` including:
- Xcode version(s) + build number, `DEVELOPER_DIR`
- available runtimes/devices (simctl)
- installed tooling versions (rch-worker, XcodeBuildMCP)
- capacity (max concurrent jobs, disk free)
Host stores the chosen worker capability snapshot in artifacts.

## Worker selection (normative)
Given a set of eligible workers (tag match + reachable), host MUST choose deterministically
unless user explicitly requests randomness.

Selection inputs:
- required tags: `macos,xcode` (and any repo-required tags)
- constraints: Xcode version/build, platform (iOS/macOS), destination availability
- preference: lowest load / highest free disk MAY be used only as a tie-breaker

Selection algorithm (default):
1. Filter by required tags.
2. Probe or load cached `capabilities.json` snapshots (bounded staleness).
3. Filter by constraints (destination exists, required Xcode available).
4. Sort deterministically by:
   - explicit worker priority (host config)
   - then stable worker name
5. Choose first.

The host MUST write:
- `worker_selection.json` (inputs, filtered set, chosen worker, reasons)
- `capabilities.json` snapshot as used for the decision

## Threat model / security notes
- Remote execution is limited to configured Xcode build/test actions.
- Worker SHOULD run under a dedicated user with constrained permissions.
- Prefer an implementation that does not require arbitrary interactive shell access.
- Not in scope: code signing, notarization, exporting archives, publishing.

## SSH hardening (recommended)
- Use a dedicated `rch` user on the worker.
- Prefer SSH keys restricted with:
  - forced-command that only runs the worker entrypoint (no shell)
  - disable agent forwarding, no-pty, restrictive source addresses where possible
- Host SHOULD pin worker host keys (no TOFU surprises).

## Artifact attestation (normative + optional signing)
Artifacts MUST include `attestation.json` with:
- `job_id`, `job_key`, `source_sha256`
- worker identity (name, stable fingerprint) + `capabilities.json` digest
- backend identity (xcodebuild/XcodeBuildMCP version)

Optional but recommended:
- Worker signs `attestation.json` with a worker-held key (e.g. Ed25519).
- Host verifies signature and records `attestation_verification.json`.
If signature verification fails, host MUST mark the run as failed (`failure_kind=ATTESTATION`).

## Milestones
- **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
- **M1**: Classifier detects Xcode build/test safely
- **M2**: MVP remote execution with `xcodebuild`
- **M3**: Switch to XcodeBuildMCP backend
- **M4**: Emit summary.json, attestation.json, manifest.json
- **M5**: Remote caching (DerivedData, SPM) and performance tuning
- **M6**: Worker capability handshake + deterministic worker selection

## Artifacts
- summary.json
- attestation.json
- manifest.json
- effective_config.json
- job.json
- invocation.json
- toolchain.json
- metrics.json
- source_manifest.json
- worker_selection.json
- test_summary.json (recommended)
- build_summary.json (recommended)
- build.log
- result.xcresult/

## Artifact schemas + versioning
All JSON artifacts MUST include:
- `schema_version`
- `created_at`
- `job_id` and `job_key`

## Next steps
1. Bring Mac mini worker online
2. Implement `rch xcode verify`
3. Add classifier + routing
4. Add XcodeBuildMCP backend
