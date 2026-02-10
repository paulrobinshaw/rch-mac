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
- **Run**: a top-level verification attempt (e.g. `rch xcode verify`) that may include multiple jobs.
- **Run ID** (`run_id`): unique opaque identifier for the run artifact directory (stable once chosen; not required to be deterministic).
- **Invocation**: user-provided command line (e.g. `xcodebuild test ...`).
- **Classifier**: deny-by-default gate that accepts/rejects invocations.
- **JobSpec** (`job.json`): deterministic, fully-resolved step job description.
- **Job ID** (`job_id`): unique identifier for a single step job within a run (used for cancellation/log streaming and artifact paths).
- **Job key** (`job_key`): stable hash used for caching and attestation. Computed using RFC 8785 (JSON Canonicalization Scheme) over the JobSpec fields that affect output (see "Job key computation" below).
- **Artifact set**: schema-versioned outputs written under `<job_id>/`.

### Timestamp and encoding conventions (normative)
- All timestamps in JSON artifacts MUST use RFC 3339 with UTC offset (`Z` suffix). Example: `2026-01-15T09:32:00Z`.
- All text fields and file content MUST be UTF-8 encoded.

### JobSpec schema outline (v1)
The canonical JobSpec (`job.json`) MUST include at least these fields:

| Field               | Type     | Description                                      |
|---------------------|----------|--------------------------------------------------|
| `schema_version`    | int      | Always `1` for this version                      |
| `run_id`            | string   | Parent run identifier                            |
| `job_id`            | string   | Unique step job identifier                       |
| `job_key`           | string   | Deterministic cache/attestation key              |
| `source_sha256`     | string   | Digest of the canonical source bundle            |
| `action`            | string   | `build` or `test`                                |
| `sanitized_argv`    | string[] | Canonical xcodebuild arguments                   |
| `toolchain`         | object   | `{xcode_version, xcode_build, developer_dir}`   |
| `destination`       | object   | Resolved destination (platform, device, OS)      |
| `effective_config`  | object   | Merged repo + host config snapshot               |
| `created_at`        | string   | RFC 3339 UTC timestamp                           |

### Job key computation (normative)
`job_key` is the SHA-256 hex digest of the RFC 8785 JSON Canonicalization of a key-object containing:
- `source_sha256`
- `effective_config` (merged, fully resolved)
- `sanitized_argv`
- `toolchain` (Xcode build number, DEVELOPER_DIR)

## CLI surface (contract)
The lane MUST provide these user-facing entry points:
- `rch xcode verify` — run repo-defined `verify` actions (`build`, `test`).
- `rch xcode run --action <build|test>` — run a single action as a one-step run.
- `rch xcode explain -- <cmd...>` — classifier explanation and effective constraints.
- `rch xcode verify --dry-run` — print resolved JobSpec + chosen worker, no execution.
- `rch xcode tail <run_id|job_id>` — stream logs/events with a cursor (falls back to polling if worker lacks tail).
- `rch xcode cancel <run_id|job_id>` — request best-effort cancellation and persist cancellation artifacts.
- `rch xcode artifacts <run_id|job_id>` — print the local artifact path(s) and key files (summary/xcresult/log).
Optional but recommended:
- `rch workers list --tag macos,xcode`
- `rch workers probe <worker>` — capture `capabilities.json` snapshot
- `rch xcode fetch <job_id>` — materialize remote artifacts locally if stored remotely
- `rch xcode doctor` — validate worker reachability, protocol, Xcode pinning, and destination availability

## Architecture (high level)
Pipeline stages:
1. **Classifier**: detects safe, supported Xcode build/test invocations (deny-by-default).
2. **Run builder**: resolves repo `verify` actions into an ordered run plan and chooses a worker once.
3. **Destination resolver**: resolves any destination constraints (e.g. `OS=latest`) using the chosen worker's `capabilities.json` snapshot and records the resolved destination.
4. **JobSpec builder**: produces a fully specified, deterministic step job description (no ambient defaults).
5. **Transport**: bundles inputs + sends to worker (integrity checked).
6. **Executor**: runs the job on macOS via a selected backend (**xcodebuild** or **XcodeBuildMCP**).
7. **Artifacts**: writes a schema-versioned artifact set + attestation.

## Backends
- **Backend: xcodebuild (MVP)** — minimal dependencies, fastest path to correctness.
- **Backend: XcodeBuildMCP (preferred)** — richer structure, better diagnostics, multi-step orchestration.

## Host↔Worker protocol (normative)
The system MUST behave as if there is a versioned protocol even if implemented over SSH.

### Versioning
- Host and worker MUST each report `rch_xcode_lane_version` and `protocol_version`.
- If `protocol_version` is incompatible, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE`.

### RPC envelope (normative, recommended)
All worker operations SHOULD accept a single JSON request on stdin and emit a single JSON response on stdout.
This maps directly to an SSH forced-command entrypoint.

Request:
- `protocol_version` (int)
- `op` (string: probe|submit|status|tail|cancel|fetch|has_source|upload_source)
- `request_id` (string, caller-chosen)
- `payload` (object)

Response:
- `protocol_version` (int)
- `request_id` (string)
- `ok` (bool)
- `payload` (object, when ok=true)
- `error` (object, when ok=false) containing: `code`, `message`, optional `data`

### Worker RPC surface (recommended, maps cleanly to SSH forced-command)
Worker SHOULD implement these operations with JSON request/response payloads:
- `probe` → returns `capabilities.json`
- `submit` → accepts `job.json` (+ optional bundle upload reference), returns ACK and initial status
- `status` → returns current job status and pointers to logs/artifacts
- `tail` → streams logs/events with a cursor
- `cancel` → requests best-effort cancellation
- `fetch` → returns artifacts (or a signed manifest + download hints)
- `has_source` → returns `{exists: bool}` for a given `source_sha256`
- `upload_source` → accepts a source bundle upload for a given `source_sha256`

### Job lifecycle + idempotency
Worker MUST treat `job_id` as the idempotency key:
- If a job with the same `job_id` is already COMPLETE, worker MAY return the existing artifacts.
- If a job with the same `job_id` is RUNNING, worker MUST report status and allow log tailing.
- If `job_id` already exists but the submitted `job_key` differs, worker MUST reject to prevent artifact confusion.

Worker MAY additionally maintain a correctness-preserving *result cache* keyed by `job_key`:
- On submit of a new `job_id` with a previously completed `job_key`, worker MAY materialize artifacts
  from the cached result into the new `<job_id>/` artifact directory and record `cached_from_job_id`.

### Cancellation
- Host MUST be able to request cancellation.
- Worker MUST attempt a best-effort cancel (terminate backend process tree) and write artifacts with `status=failed`
  and `failure_kind=CANCELLED`.

### Log streaming (recommended)
- Worker SHOULD support a "tail" mode so host can stream logs while running.
- If not supported, host MUST still periodically fetch/append logs to avoid silent hangs.

### Structured events (recommended)
Worker SHOULD emit `events.jsonl` (JSON Lines) for machine-readable progress.
Each event SHOULD include: `ts`, `stage`, `kind`, and optional `data`.
Idle-log watchdog SHOULD treat either new log bytes OR new events as "activity".

## Run + Job state machine (normative)

### Run states
A run transitions through:
```
QUEUED → RUNNING → { SUCCEEDED | FAILED | CANCELLED }
```
- `QUEUED`: run is accepted but no jobs have started.
- `RUNNING`: at least one job is executing.
- `SUCCEEDED`: all jobs completed with `status=success`.
- `FAILED`: at least one job ended with `status=failed` (and no cancellation).
- `CANCELLED`: cancellation was requested and acknowledged.

### Job states
Each step job transitions through:
```
QUEUED → RUNNING → { SUCCEEDED | FAILED | CANCELLED }
            ↑              |
            └─ CANCEL_REQUESTED
```
- `QUEUED`: job is pending execution on the worker.
- `RUNNING`: job is actively executing.
- `CANCEL_REQUESTED`: cancellation signal sent; job is still running until the backend terminates.
- `SUCCEEDED`: job completed successfully.
- `FAILED`: job completed with a failure.
- `CANCELLED`: job was terminated due to cancellation.

### State artifacts
- `run_state.json`: persisted at `<run_id>/run_state.json` with fields: `run_id`, `state`, `updated_at`, `schema_version`.
- `job_state.json`: persisted at `<run_id>/steps/<action>/<job_id>/job_state.json` with fields: `job_id`, `run_id`, `state`, `updated_at`, `schema_version`.

Both MUST be updated atomically (write-then-rename) on every state transition.

## Deterministic JobSpec + Job Key
Each remote run is driven by a `job.json` (JobSpec) generated on the host.
The host computes:
- `source_sha256` — SHA-256 of the sent source bundle (after canonicalization)
- `job_key` — SHA-256 of the RFC 8785 (JCS) canonicalization of the key-object (see "Job key computation" above)
Artifacts include both values, enabling reproducible reruns and cache keys.

## Source bundle store (performance, correctness-preserving)
Workers SHOULD maintain a content-addressed store keyed by `source_sha256`.

Protocol expectations:
- Host MAY query whether the worker already has `source_sha256` (via `has_source` RPC op).
- If present, host SHOULD skip re-uploading the bundle and submit only `job.json`.
- If absent, host uploads the canonical bundle once (via `upload_source` RPC op); worker stores it under `source_sha256`.

GC expectations:
- Bundle GC MUST NOT remove bundles referenced by RUNNING jobs.
- Bundle GC policy MAY align with cache eviction policy (age/size based).

## Source bundling canonicalization (normative)
The host MUST create a canonical source bundle such that identical inputs yield identical `source_sha256`.

Rules:
- Use a deterministic `tar` archive (PAX recommended) with:
  - sorted file paths (lexicographic, UTF-8)
  - normalized mtimes (e.g. 0) and uid/gid (0)
  - stable file modes (preserve executable bit; normalize others)
  - fixed pax headers where applicable (avoid host-dependent extended attributes)
- Exclude by default:
  - `.git/`, `.DS_Store`, `DerivedData/`, `.build/`, `**/*.xcresult/`, `**/.swiftpm/` (build artifacts)
  - any host-local RCH artifact directories
- Include repo config `.rch/xcode.toml` in the bundle (so worker always has the same constraints)
- The host MUST support a repo ignore file (recommended: `.rchignore`) for additional excludes.
- Symlink handling MUST be safe:
  - symlinks that escape the repo root MUST be rejected (`failure_kind=BUNDLER`)
  - host MUST choose either "preserve symlink" or "dereference within root" deterministically per config
- The host MUST emit `source_manifest.json` listing:
  - `path`, `size`, `sha256` per file
  - manifest `schema_version`

Transport note (non-normative but recommended):
- The canonical tar MAY be compressed with zstd for transfer, but `source_sha256` MUST be computed
  over the canonical (pre-compression) tar bytes.

Compliance (recommended):
- Provide a fixture-based reproducibility test: identical repo inputs on Linux/macOS produce identical `source_sha256`.

### Bundle modes (recommended)
- `worktree`: include tracked + untracked files (except excluded patterns).
- `git_index`: include only git-index tracked files (plus `.rch/xcode.toml` and ignore file).

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
- `failure_kind`: CLASSIFIER_REJECTED | SSH | TRANSFER | EXECUTOR | XCODEBUILD | MCP | ARTIFACTS | CANCELLED | WORKER_INCOMPATIBLE | BUNDLER | ATTESTATION | WORKER_BUSY
- `failure_subkind`: optional string for details (e.g. TIMEOUT_OVERALL | TIMEOUT_IDLE | PROTOCOL_ERROR)
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
- 90: WORKER_BUSY
- 91: WORKER_INCOMPATIBLE
- 92: BUNDLER
- 93: ATTESTATION

## Agent-friendly summaries (recommended)
In addition to `summary.json`, the worker SHOULD emit:
- `test_summary.json` (counts, failing tests, duration, top failures)
- `build_summary.json` (targets, warnings/errors counts, first error location if available)
- `junit.xml` — JUnit XML report (recommended for test jobs; enables integration with CI dashboards and standard tooling)
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

### Cache namespace (recommended)
Repo config SHOULD provide a stable `cache_namespace` used as part of shared cache directory names,
to prevent collisions across unrelated repos on the same worker.

### Cache keying details (normative)
- Any cache directory that can be reused across jobs MUST be additionally keyed by toolchain identity
  (at minimum: Xcode build number and macOS major version) to prevent cross-toolchain corruption.
- `metrics.json` SHOULD record the concrete cache key components used (job_key, xcode_build, macos_version, etc.).

### Result cache (recommended)
Worker SHOULD maintain an optional result cache keyed by `job_key`:
- If present and complete, a submit MAY be satisfied by materializing artifacts from the cached result.
- The worker MUST still emit a correct `attestation.json` for the new `job_id` referencing the same `job_key`.

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
  - Response SHOULD include `retry_after_seconds`.

## Worker capabilities
Worker reports a `capabilities.json` including:
- Installed Xcode versions as an array of objects, each containing:
  - `version` (e.g. `"16.2"`)
  - `build` (e.g. `"16C5032a"`)
  - `path` (e.g. `"/Applications/Xcode-16.2.app"`)
  - `developer_dir` (e.g. `"/Applications/Xcode-16.2.app/Contents/Developer"`)
- Active Xcode: currently selected `DEVELOPER_DIR`
- macOS version + architecture
- available runtimes/devices (simctl)
- installed tooling versions (rch-worker, XcodeBuildMCP)
- capacity (max concurrent jobs, disk free)
Optional but recommended:
- worker identity material (SSH host key fingerprint and/or attestation public key fingerprint)
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

### Toolchain resolution (normative)
When a job requires a specific Xcode toolchain:
1. The repo config or CLI MAY specify a required Xcode build number (preferred), version, or range.
2. The host matches the requirement against the worker's `capabilities.json` Xcode array.
3. If multiple Xcodes match, the host MUST prefer the entry whose `build` number exactly matches.
   If no exact build match, the host MUST prefer the highest matching version and log a warning.
4. The resolved Xcode entry (`version`, `build`, `developer_dir`) is recorded in `toolchain.json` and used
   to set `DEVELOPER_DIR` on the worker before execution.
5. If no installed Xcode satisfies the constraint, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE`.

## Threat model / security notes
- Remote execution is limited to configured Xcode build/test actions.
- Worker SHOULD run under a dedicated user with constrained permissions.
- Prefer an implementation that does not require arbitrary interactive shell access.
- Not in scope: code signing, notarization, exporting archives, publishing.

Clarification (normative):
- The lane does not attempt to sandbox Xcode builds. Repo-defined build phases/plugins may execute on the worker.
  Operators MUST treat the worker as a CI machine and scope secrets accordingly.

Recommended mitigations:
- Executor MUST use an environment-variable allowlist and pass through only known-safe variables. Obvious secrets MUST be redacted in logs/artifacts.
- Worker SHOULD avoid unlocking or accessing user keychains during execution.

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
- `manifest_sha256` (digest of `manifest.json`)

Optional but recommended:
- Worker signs `attestation.json` with a worker-held key (e.g. Ed25519).
- Host verifies signature and records `attestation_verification.json`.
If signature verification fails, host MUST mark the run as failed (`failure_kind=ATTESTATION`).

## Artifact manifest (normative)
`manifest.json` MUST enumerate produced artifacts with at least:
- `path` (relative), `size`, `sha256`
`manifest.json` SHOULD also include `artifact_root_sha256` (digest over ordered entries) to bind the set.

## Milestones
- **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
- **M1**: Classifier detects Xcode build/test safely
- **M2**: MVP remote execution with `xcodebuild`
- **M3**: Switch to XcodeBuildMCP backend
- **M4**: Emit summary.json, attestation.json, manifest.json
- **M5**: Remote caching (DerivedData, SPM) and performance tuning
- **M6**: Worker capability handshake + deterministic worker selection
- **M7**: Conformance and regression test suite passes

## Conformance / regression tests
The project SHOULD maintain a conformance test suite that validates:
- Classifier correctness: known-good invocations accepted, known-bad rejected.
- JobSpec determinism: identical inputs produce identical `job_key`.
- Source bundle reproducibility: identical repo state → identical `source_sha256` (cross-platform).
- Artifact schema compliance: all emitted JSON artifacts validate against their JSON Schemas.
- Protocol round-trip: a mock worker can handle all RPC ops and return valid responses.
- State machine transitions: run and job states follow the defined state diagrams.
- Cache correctness: result cache hits produce artifacts identical to fresh runs.

Tests SHOULD be runnable without a live worker (use mocks/fixtures) and integrated into CI.

## JSON Schemas (recommended)
The project SHOULD ship machine-readable JSON Schemas for all normative artifacts under `schemas/rch-xcode/`.
Schema files SHOULD follow the naming convention `<artifact_name>.schema.json` (e.g. `schemas/rch-xcode/job.schema.json`).
Schemas enable automated validation in CI and by third-party tooling.

## Artifacts
- run_summary.json
- run_state.json
- summary.json
- attestation.json
- manifest.json
- effective_config.json
- job.json
- job_state.json
- invocation.json
- toolchain.json
- metrics.json
- source_manifest.json
- worker_selection.json
- events.jsonl (recommended)
- test_summary.json (recommended)
- build_summary.json (recommended)
- junit.xml (recommended, test jobs)
- build.log
- result.xcresult/

## Artifact schemas + versioning
All JSON artifacts MUST include:
- `schema_version`
- `created_at`

Run-scoped artifacts MUST include:
- `run_id`

Job-scoped artifacts MUST include:
- `run_id`
- `job_id`
- `job_key`

## Next steps
1. Bring Mac mini worker online
2. Implement `rch xcode verify`
3. Add classifier + routing
4. Add XcodeBuildMCP backend
