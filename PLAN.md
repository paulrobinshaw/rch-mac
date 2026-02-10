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
- **Run plan** (`run_plan.json`): persisted, ordered list of step jobs (actions + allocated `job_id`s) for a run.
- **Job key** (`job_key`): stable hash used for caching and attestation. Computed using RFC 8785 (JSON Canonicalization Scheme) over a canonical **job key inputs** object (`job_key_inputs`) containing only output-affecting, fully-resolved fields (see "Job key computation" below).
- **Artifact set**: schema-versioned outputs written under `<job_id>/`.

### Identifier formats (normative)
`run_id`, `job_id`, and `request_id` MUST be treated as opaque strings, but MUST be filesystem-safe:
- MUST NOT contain `/`, `\`, whitespace, control characters, or `..`
- MUST match: `^[A-Za-z0-9][A-Za-z0-9_-]{9,63}$`
- MUST be safe to embed in file paths without additional escaping

Generation guidance:
- Host SHOULD generate `run_id` and `job_id` using a sortable unique ID (ULID or UUIDv7 recommended).
- `job_id` MUST be globally unique per host over time (host MUST NOT reuse a `job_id` across runs).
- `request_id` MUST be unique per RPC request (per host process) to allow reliable correlation and retries.

### Timestamp and encoding conventions (normative)
- All timestamps in JSON artifacts MUST use RFC 3339 with UTC offset (`Z` suffix). Example: `2026-01-15T09:32:00Z`.
- All text fields and file content MUST be UTF-8 encoded.

### Clock skew tolerance (normative)
- Host and worker clocks are independent; implementations MUST NOT assume cross-machine timestamp monotonicity.
- For ordering within a single machine's artifacts, prefer `seq` (monotonic counter) over timestamps.
- The host SHOULD log a warning if the worker's `probe` response timestamp differs from the host clock
  by more than 30 seconds, as this may cause confusing artifact timelines.
- Consumers MUST NOT reject artifacts solely because cross-machine timestamps are non-monotonic.

### JobSpec schema outline (v1)
The canonical JobSpec (`job.json`) MUST include at least these fields:

| Field               | Type     | Description                                      |
|---------------------|----------|--------------------------------------------------|
| `schema_version`    | int      | Always `1` for this version                      |
| `schema_id`         | string   | Stable schema identifier (e.g. `rch-xcode/job@1`) |
| `run_id`            | string   | Parent run identifier                            |
| `job_id`            | string   | Unique step job identifier                       |
| `action`            | string   | `build` or `test`                                |
| `job_key_inputs`    | object   | Canonical key-object hashed to produce `job_key` |
| `job_key`           | string   | SHA-256 hex digest of JCS(`job_key_inputs`)      |
| `effective_config`  | object   | Merged repo + host config snapshot               |
| `created_at`        | string   | RFC 3339 UTC timestamp                           |

Optional but recommended fields (v1, backward-compatible):
| `artifact_profile`  | string   | `minimal` or `rich` (defaults to `minimal`)      |

### Job key computation (normative)
`job_key_inputs` MUST be an object containing the fully-resolved, output-affecting inputs for the job, and MUST include at least:
- `source_sha256` (digest of the canonical source bundle)
- `sanitized_argv` (canonical xcodebuild arguments; no output-path overrides)
- `toolchain` (resolved toolchain identity; see below)
- `destination` (fully-resolved destination identity; see below)

`job_key` is the SHA-256 hex digest of the RFC 8785 JSON Canonicalization (JCS) of `job_key_inputs`.

Toolchain identity (normative):
`job_key_inputs.toolchain` MUST include:
- `xcode_build` (e.g. `"16C5032a"`)
- `developer_dir` (absolute path on worker)
- `macos_version` (e.g. `"15.3.1"`)
- `macos_build` (e.g. `"24D60"`)
- `arch` (e.g. `"arm64"`)

Destination identity (normative):
`job_key_inputs.destination` MUST include enough detail to prevent cross-runtime confusion.
At minimum it MUST include:
- `platform` (e.g. `"iOS Simulator"` or `"macOS"`)
- `name` (device name when applicable)
- `os_version` (resolved concrete version; MUST NOT be `"latest"`)
- `provisioning` (`"existing"` | `"ephemeral"`)

For iOS/tvOS/watchOS Simulator destinations, it MUST ALSO include:
- `sim_runtime_identifier` (e.g. `com.apple.CoreSimulator.SimRuntime.iOS-19-2`)
- `sim_runtime_build` (runtime build string if available from `simctl`)
- `device_type_identifier` (e.g. `com.apple.CoreSimulator.SimDeviceType.iPhone-16`)

`job_key_inputs` MUST NOT include host-only operational settings that should not invalidate correctness-preserving caches,
including (non-exhaustive): timeouts, worker inventory/SSH details, cache toggles, worker selection metadata, backend selection.

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
2. **Run builder**: resolves repo `verify` actions into an ordered run plan, allocates stable `job_id`s, persists `run_plan.json`, chooses a worker once, and (when supported) acquires a time-bounded **worker lease**.
3. **Destination resolver**: resolves any destination constraints (e.g. `OS=latest`) using the chosen worker's `capabilities.json` snapshot and records the resolved destination (including simulator runtime identifiers/builds).
   - **Destination provisioning (optional, recommended)**: if `destination.provisioning="ephemeral"` and the destination is a Simulator, the worker provisions a clean simulator per job and records the created UDID in artifacts (UDID MUST NOT affect `job_key`).
   - **Ephemeral cleanup (normative)**: when `provisioning="ephemeral"`, the worker MUST delete the provisioned simulator after artifact collection completes (or after cancellation). If cleanup fails, the worker MUST log a warning in `build.log` and record `ephemeral_cleanup_failed: true` in `summary.json`. Workers SHOULD implement a startup sweep that deletes any orphaned ephemeral simulators from previous crashed runs (identifiable by a naming convention, e.g. `rch-ephemeral-<job_id>`).
4. **JobSpec builder**: produces a fully specified, deterministic step job description (no ambient defaults).
5. **Transport**: bundles inputs + sends to worker (integrity checked).
6. **Executor**: runs the job on macOS via a selected backend (**xcodebuild** or **XcodeBuildMCP**).
7. **Artifacts**: writes a schema-versioned artifact set + attestation.

## Backends
- **Backend: xcodebuild (MVP)** — minimal dependencies, fastest path to correctness.
- **Backend: XcodeBuildMCP (preferred)** — richer structure, better diagnostics, multi-step orchestration.

## Backend contract (normative)
Regardless of backend, the worker MUST:
- execute the action described by `job.json` under the resolved toolchain + destination
- write the normative artifacts (`summary.json`, `manifest.json`, `attestation.json`, `job_state.json`, logs)
- control output paths (host/user args MUST NOT override artifact locations)

Minimum artifact expectations (normative):
- `build.log` MUST be present for all jobs
- `result.xcresult/` MUST be present for `test` jobs (backend may generate via `-resultBundlePath`)
- `summary.json` MUST include backend identity (`backend=...`) and a stable exit_code mapping

## Artifact profiles (recommended)
Jobs MAY request an `artifact_profile`:
- `minimal` (default): only the minimum artifact expectations are required.
- `rich`: in addition to `minimal`, the worker MUST emit:
  - `events.jsonl`
  - `build_summary.json` (for build jobs)
  - `test_summary.json` + `junit.xml` (for test jobs)

`summary.json` SHOULD record the `artifact_profile` actually produced.

### toolchain.json schema (normative)
`toolchain.json` MUST be emitted per job and MUST include:

| Field             | Type   | Description                                        |
|-------------------|--------|----------------------------------------------------|
| `schema_version`  | int    | Always `1`                                         |
| `schema_id`       | string | `rch-xcode/toolchain@1`                            |
| `run_id`          | string | Parent run identifier                              |
| `job_id`          | string | Job identifier                                     |
| `job_key`         | string | Job key                                            |
| `created_at`      | string | RFC 3339 UTC                                       |
| `xcode_version`   | string | e.g. `"16.2"`                                      |
| `xcode_build`     | string | e.g. `"16C5032a"`                                  |
| `developer_dir`   | string | Absolute path on worker                            |
| `macos_version`   | string | e.g. `"15.3.1"`                                    |
| `macos_build`     | string | e.g. `"24D60"`                                     |
| `arch`            | string | e.g. `"arm64"`                                     |

### destination.json schema (normative)
`destination.json` MUST be emitted per job and MUST include:

| Field             | Type   | Description                                        |
|-------------------|--------|----------------------------------------------------|
| `schema_version`  | int    | Always `1`                                         |
| `schema_id`       | string | `rch-xcode/destination@1`                          |
| `run_id`          | string | Parent run identifier                              |
| `job_id`          | string | Job identifier                                     |
| `job_key`         | string | Job key                                            |
| `created_at`      | string | RFC 3339 UTC                                       |
| `platform`        | string | e.g. `"iOS Simulator"`                             |
| `name`            | string | Device name when applicable                        |
| `os_version`      | string | Resolved concrete version                          |
| `provisioning`    | string | `"existing"` or `"ephemeral"`                      |
| `original_constraint` | string | The raw destination string from config          |
| `sim_runtime_identifier` | string | (Simulator only) Runtime identifier         |
| `sim_runtime_build` | string | (Simulator only) Runtime build string           |
| `device_type_identifier` | string | (Simulator only) Device type identifier     |
| `udid`            | string | (Optional) Actual device/simulator UDID used       |

## Host↔Worker protocol (normative)
The system MUST behave as if there is a versioned protocol even if implemented over SSH.

### Versioning + feature negotiation
### Protocol version bootstrap (normative)
`probe` requests MUST use `protocol_version: 0` (sentinel value). The worker MUST accept `protocol_version: 0`
exclusively for `op: "probe"` and MUST reject `protocol_version: 0` for all other operations with error code
`UNSUPPORTED_PROTOCOL`. After probe, the host selects a concrete version within the negotiated range and uses
it for all subsequent requests.

On `probe`, the worker MUST report:
- `rch_xcode_lane_version` (string)
- `protocol_min` / `protocol_max` (int; inclusive range supported by the worker)
- `features` (array of strings; see below)

The host MUST select a single `protocol_version` within the intersection of host- and worker-supported ranges
and use that value for every subsequent request in the run.

If there is no intersection, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE`.
If the worker lacks a host-required feature, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE`
and `failure_subkind=FEATURE_MISSING`.

`features` is a capability list used for forward-compatible optional behavior. Example feature strings:
`tail`, `fetch`, `events`, `has_source`, `upload_source`, `attestation_signing`.

### RPC envelope (normative, recommended)
All worker operations SHOULD accept a single JSON request on stdin and emit a single JSON response on stdout.
This maps directly to an SSH forced-command entrypoint.

Request:
- `protocol_version` (int; selected by host after probe)
- `op` (string: probe|submit|status|tail|cancel|fetch|has_source|upload_source)
- `request_id` (string, caller-chosen)
- `payload` (object)

Response:
- `protocol_version` (int; selected by host after probe)
- `request_id` (string)
- `ok` (bool)
- `payload` (object, when ok=true)
- `error` (object, when ok=false) containing: `code`, `message`, optional `data`

#### Error codes (recommended, stabilizes automation)
Workers SHOULD restrict `error.code` to a small, documented registry (examples):
- `INVALID_REQUEST` (malformed JSON, missing fields)
- `UNSUPPORTED_PROTOCOL` (no version intersection)
- `FEATURE_MISSING` (required feature absent)
- `BUSY` (capacity exceeded; include `retry_after_seconds`)
- `LEASE_EXPIRED` (job lease timed out)

### Binary framing (recommended)
Some operations (notably `upload_source`) may need to transmit bytes efficiently.
Implementations SHOULD support a framed stdin format:
1) A single UTF-8 JSON header line terminated by `\n`
2) Immediately followed by exactly `content_length` raw bytes (if `payload.stream` is present)

Discriminator: if the request `payload` contains a `stream` object, the worker MUST parse stdin as framed
(a single JSON header line terminated by `\n`, followed by exactly `content_length` raw bytes).
If `payload.stream` is absent, the entire stdin is a single JSON object (no binary payload follows).

The header MUST include:
- `payload.stream.content_length` (int)
- `payload.stream.content_sha256` (sha256 hex of the raw streamed bytes)
- `payload.stream.compression` (`none`|`zstd`) and `payload.stream.format` (`tar`)

The worker MUST verify `content_sha256` before processing, then (if compressed) decompress
and verify the resulting canonical bundle digest matches `source_sha256`.

### Worker RPC surface (recommended, maps cleanly to SSH forced-command)
Worker SHOULD implement these operations with JSON request/response payloads:
- `probe` → returns protocol range/features + `capabilities.json`
- `reserve` → requests a lease for a run (capacity reservation; optional but recommended)
- `release` → releases a lease early (optional)
- `submit` → accepts `job.json` (+ optional bundle upload reference), returns ACK and initial status
- `status` → returns current job status and pointers to logs/artifacts
- `tail` → returns the next chunk of logs/events given a cursor (repeatable; host loops to "stream")
- `cancel` → requests best-effort cancellation
- `fetch` → returns artifacts (or a signed manifest + download hints)
- `has_source` → returns `{exists: bool}` for a given `source_sha256`
- `upload_source` → accepts a source bundle upload for a given `source_sha256`

### Job lifecycle + idempotency
Worker MUST treat `job_id` as the idempotency key:
- If a job with the same `job_id` is already COMPLETE, worker MAY return the existing artifacts.
- If a job with the same `job_id` is RUNNING, worker MUST report status and allow log tailing.
- If `job_id` already exists but the submitted `job_key` differs, worker MUST reject to prevent artifact confusion.
- If `job_id` exists but is in a non-terminal, non-running state (e.g. orphaned after a worker crash with partial artifacts),
  the worker MUST clean up the partial artifact directory and re-execute the job from scratch.
  Workers SHOULD detect this state on startup or on submit by checking for a `job_id` directory that lacks a `job_index.json`
  commit marker and is not actively running.

On `submit`, worker MUST validate:
- `job_key` matches SHA-256(JCS(`job_key_inputs`)) as defined in "Job key computation"
- `job_key_inputs.source_sha256` matches the stored source bundle digest for the bundle used (uploaded or previously present)
If validation fails, worker MUST fail the job with `failure_kind=PROTOCOL_ERROR` (or a more specific subkind) and emit diagnostics.

Worker MAY additionally maintain a correctness-preserving *result cache* keyed by `job_key`:
- On submit of a new `job_id` with a previously completed `job_key`, worker MAY materialize artifacts
  from the cached result into the new `<job_id>/` artifact directory and record `cached_from_job_id`.

### Cancellation
- Host MUST be able to request cancellation.
- Worker MUST attempt a best-effort cancel (terminate backend process tree) and write artifacts with `status=cancelled`
  and `failure_kind=CANCELLED`.

On cancellation, `summary.json` MUST set:
- `status=cancelled`
- `failure_kind=CANCELLED`
- `exit_code=80`

### Host signal handling (normative)
On receiving SIGINT or SIGTERM, the host MUST:
1. Send `cancel` for all RUNNING jobs in the current run.
2. Wait up to a bounded grace period (recommended: 10 seconds) for cancellation acknowledgement.
3. Persist `run_state.json` with `state=CANCELLED` and `run_summary.json` with available results.
4. Exit with code 80 (`CANCELLED`).

On receiving a second SIGINT (double-interrupt), the host MAY exit immediately without waiting for
cancellation acknowledgement, but MUST still persist `run_state.json`. The first SIGINT MUST dispatch
`cancel` RPCs before returning (fire-and-forget is acceptable); second SIGINT only skips the wait for ack.

Lease-based backstop (normative): if the worker supports leases, it MUST auto-cancel any RUNNING jobs
whose lease expires without a `release`, providing a safety net for host crashes or unclean exits.

### Log streaming (recommended)
- Worker SHOULD support a "tail" mode so host can stream logs while running.
- If not supported, host MUST still periodically fetch/append logs to avoid silent hangs.

`tail` MUST be defined as cursor-based chunk retrieval compatible with the single request/response envelope:
- Request payload SHOULD include: `job_id`, `cursor` (nullable), and optional limits (`max_bytes`, `max_events`)
- Response payload SHOULD include: `next_cursor` (nullable), plus either/both:
  - `log_chunk` (UTF-8 text) and/or
  - `events` (array of event objects or JSONL strings)

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
- `run_state.json`: persisted at `<run_id>/run_state.json` with fields: `schema_version`, `schema_id` (`rch-xcode/run_state@1`), `run_id`, `state`, `created_at`, `updated_at`.
  `run_state.json` SHOULD also include `current_step` (`{ index, job_id, action }` or null) indicating the
  currently executing step, to allow lightweight observability without scanning per-job state files.
- `job_state.json`: persisted at `<run_id>/steps/<action>/<job_id>/job_state.json` with fields: `schema_version`, `schema_id` (`rch-xcode/job_state@1`), `run_id`, `job_id`, `job_key`, `state`, `created_at`, `updated_at`.

Both MAY additionally include:
- `seq` (int): monotonic sequence number incremented on every state transition for the artifact.
If present, consumers SHOULD prefer `seq` over timestamps for ordering.

Both MUST be updated atomically (write-then-rename) on every state transition.

### Step execution semantics (normative)
Steps in `run_plan.json` MUST execute sequentially in plan order.
If a step ends with `status=failed` or `status=rejected`, subsequent steps MUST be skipped
(their `job_state.json` MUST NOT be created) unless the run is explicitly configured with
`run.continue_on_failure = true`, in which case all steps execute regardless of prior failures.
Skipped steps MUST be recorded in `run_summary.json` under `steps_skipped` (int).

### Run plan artifact (normative)
The host MUST emit `run_plan.json` at `<run_id>/run_plan.json` before starting execution.
It MUST include at least:
- `schema_version`, `schema_id` (`rch-xcode/run_plan@1`), `created_at`, `run_id`
- `steps`: ordered array of `{ index, action, job_id }`
- `selected_worker`: worker name chosen for this run
- `selected_worker_host`: SSH host of the chosen worker
- `continue_on_failure`: bool (whether failed steps should skip remaining steps; from config at plan creation time).
  On resumption, the host MUST use the value persisted in `run_plan.json`, not the current config.

`run_plan.json` is the authoritative source for which `job_id`s belong to a run. If the daemon restarts,
it MUST be able to resume by reading `run_plan.json` and reusing the same `job_id`s (preserving worker idempotency guarantees).

### Concurrent host runs (normative)
Multiple runs MAY execute concurrently on the same host. To prevent interference:
- Each run MUST use a unique `run_id`, ensuring artifact directories do not collide.
- The host MUST NOT hold filesystem locks on shared resources (e.g. the artifact root) across runs.
- If the worker returns `WORKER_BUSY` (capacity exhausted), the host MUST respect `retry_after_seconds`
  and MUST NOT busy-loop. The host SHOULD implement bounded retry with exponential backoff (recommended max: 3 retries).
- Host-side artifact GC MUST acquire a lock before scanning/deleting, and MUST skip artifacts
  belonging to any RUNNING run (determined by `run_state.json`).

### Run resumption (normative)
On restart, the host MUST:
1. Read `run_plan.json` to recover the step list and `job_id`s.
2. For each step job, check whether `job_index.json` exists (the commit marker):
   - If present: treat the job as COMPLETE (success or failure per its `summary.json`).
   - If absent: query the worker via `status` using the original `job_id`.
     - If worker reports RUNNING: resume tailing/waiting.
     - If worker reports COMPLETE: fetch artifacts normally.
     - If worker reports unknown `job_id` or is unreachable: re-submit the job with the same `job_id`.
3. The host MUST NOT skip a failed job on resumption unless the run is already CANCELLED.
4. `run_state.json` MUST record `resumed_at` (RFC 3339 UTC) when a run is resumed.
5. If the original worker requires a lease and the host cannot re-acquire it (e.g. `WORKER_BUSY` or unreachable),
   the host MUST fail the run with `failure_kind=WORKER_BUSY` (or `SSH` if unreachable).
   The host MUST NOT silently switch to a different worker mid-run, because `job_key_inputs` are bound
   to the original worker's toolchain and destination identity.

## Deterministic JobSpec + Job Key
Each remote run is driven by a `job.json` (JobSpec) generated on the host.
The host computes:
- `source_sha256` — SHA-256 of the canonical source bundle bytes
- `job_key_inputs` — canonical, output-affecting inputs (see "Job key computation")
- `job_key` — SHA-256 of JCS(`job_key_inputs`)
Artifacts include both values, enabling reproducible reruns and cache keys.

The host MUST emit a standalone `job_key_inputs.json` artifact (byte-for-byte identical to the `job_key_inputs` object embedded in `job.json`)
to make cache/attestation inputs directly inspectable.

## Source bundle store (performance, correctness-preserving)
Workers SHOULD maintain a content-addressed store keyed by `source_sha256`.

Protocol expectations:
- Host MAY query whether the worker already has `source_sha256` (via `has_source` RPC op).
- If present, host SHOULD skip re-uploading the bundle and submit only `job.json`.
- If absent, host uploads the canonical bundle once (via `upload_source` RPC op); worker stores it under `source_sha256`.

`upload_source` payload (recommended):
- `source_sha256` (string)
- `stream` (object; see "Binary framing") describing the streamed bytes
- optional `resume` object (if feature `upload_resumable` is present):
  - `upload_id` (string), `offset` (int)
Worker SHOULD respond with `next_offset` to support resumable uploads.

Atomicity (normative): `upload_source` MUST be atomic by `source_sha256`. If the bundle already
exists when the upload completes (e.g. concurrent upload from another host), the worker MUST discard
the duplicate and return success. Workers SHOULD use write-to-temp-then-rename to prevent partial visibility.

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
- The host MUST emit `source_manifest.json` (run-scoped, at `<run_id>/source_manifest.json`) listing:
  - `schema_version`, `schema_id` (`rch-xcode/source_manifest@1`), `created_at`, `run_id`, `source_sha256`
  - `entries[]`: each with `path`, `size`, `sha256`

Transport note (non-normative but recommended):
- The canonical tar MAY be compressed with zstd for transfer, but `source_sha256` MUST be computed
  over the canonical (pre-compression) tar bytes.

Compliance (recommended):
- Provide a fixture-based reproducibility test: identical repo inputs on Linux/macOS produce identical `source_sha256`.

### Bundle modes (recommended)
- `worktree`: include tracked + untracked files (except excluded patterns).
- `git_index`: include only git-index tracked files (plus `.rch/xcode.toml` and ignore file).

If the bundler cannot apply canonicalization, the job MUST be rejected (`failure_kind=BUNDLER`).

### Bundle size enforcement (normative)
- If `bundle.max_bytes` > 0, the host MUST reject the bundle before upload if it exceeds the limit,
  with `failure_kind=BUNDLER` and `failure_subkind=SIZE_EXCEEDED`.
- If the worker's `capabilities.json` includes `max_upload_bytes`, the host MUST reject the bundle
  before upload if it exceeds that limit (same failure kind/subkind).
- The effective limit is `min(bundle.max_bytes, worker.max_upload_bytes)` where 0 means "no limit from that source".
- The worker MUST reject an `upload_source` request if the `content_length` exceeds `max_upload_bytes`,
  returning error code `PAYLOAD_TOO_LARGE` with the limit in `data.max_bytes`.

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
- `classifier_policy_sha256` (sha256 hex of the effective classifier policy snapshot)
`sanitized_argv` MUST NOT contain:
- output path overrides
- script hooks
- unconstrained destinations

### Classifier policy snapshot (recommended)
For auditability and replayable `explain`, producers SHOULD emit `classifier_policy.json` per job capturing the
effective allowlist/denylist and any pinned constraints enforced by the classifier (workspace/project, scheme,
destination rules, allowed flags).
`invocation.json.classifier_policy_sha256` SHOULD be the sha256 hex digest of the canonical JSON bytes of that snapshot.

## Configuration merge (normative)
### Sources + precedence (last wins)
1. Built-in lane defaults
2. Host/user config (`~/.config/rch/…`)
3. Repo config (`.rch/xcode.toml`)
4. CLI flags

### Merge semantics
- Config MUST be decoded into a JSON-compatible object model (maps, arrays, scalars).
- Objects MUST deep-merge by key.
- Arrays MUST replace (no concatenation) to avoid host-dependent ordering surprises.
- Scalars MUST override.
- Merge MUST be deterministic.

### `effective_config.json` (audit, not a cache key)
`effective_config.json` MUST be emitted per job and MUST:
- include `schema_version`, `created_at`, `run_id`, `job_id`, `job_key`
- include the merged config object
- record the contributing sources (origin + optional file path + a digest of raw bytes)
- redact secrets (private keys, tokens, passwords). Any redaction MUST be recorded in a `redactions[]` list.

`effective_config` MUST NOT be used for `job_key` computation (only `job_key_inputs` is hashed).

## Failure taxonomy
`summary.json` MUST include:
- `status`: `success` | `failed` | `rejected` | `cancelled`
  - `rejected` is used when the classifier rejects the invocation (no job execution occurs;
    the job state machine is not entered). `rejected` jobs MUST NOT have a `job_state.json`.
  - `success` corresponds to the `SUCCEEDED` terminal state in the job state machine.
  - `failed` corresponds to the `FAILED` terminal state.
  - `cancelled` corresponds to the `CANCELLED` terminal state.
- `failure_kind`: CLASSIFIER_REJECTED | SSH | TRANSFER | EXECUTOR | XCODEBUILD | MCP | ARTIFACTS | CANCELLED | WORKER_INCOMPATIBLE | BUNDLER | ATTESTATION | WORKER_BUSY
- `failure_subkind`: optional string for details (e.g. TIMEOUT_OVERALL | TIMEOUT_IDLE | PROTOCOL_ERROR)
- `exit_code`: stable integer for scripting
- `backend_exit_code`: integer exit status as reported by the backend process (when started)
- `backend_term_signal`: optional string (e.g. `"SIGKILL"`) when terminated by signal
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

### Run-level exit code (normative)
For multi-step runs, `run_summary.json` MUST include an `exit_code` suitable for scripting and the host CLI
MUST exit with that value.

Aggregation rule:
- If any step has `status=rejected`, run `exit_code` MUST be 10.
- Else if any step has `status=cancelled`, run `exit_code` MUST be 80.
- Else if any step has `status=failed`, run `exit_code` MUST be the first failing step's `exit_code` in run order.
- Else run `exit_code` MUST be 0.

### run_summary.json schema (normative)
`run_summary.json` MUST include at least:

| Field            | Type   | Description                                         |
|------------------|--------|-----------------------------------------------------|
| `schema_version` | int    | Always `1` for this version                         |
| `schema_id`      | string | `rch-xcode/run_summary@1`                           |
| `run_id`         | string | Run identifier                                      |
| `created_at`     | string | RFC 3339 UTC                                        |
| `status`         | string | `success` \| `failed` \| `rejected` \| `cancelled`    |
| `exit_code`      | int    | Aggregated per run-level exit code rules             |
| `step_count`     | int    | Total steps in the run                               |
| `steps_succeeded`| int    | Count of steps with `status=success`                 |
| `steps_failed`   | int    | Count of steps with `status=failed`                  |
| `steps_cancelled`| int    | Count of steps with `status=cancelled`               |
| `steps_skipped` | int    | Count of steps skipped due to early-abort              |
| `steps_rejected`| int    | Count of steps with `status=rejected`                 |
| `duration_ms`    | int    | Wall-clock duration of the entire run                |
| `human_summary`  | string | One-line human-readable summary                      |

## Agent-friendly summaries (recommended)
In addition to `summary.json`, the worker SHOULD emit:
- `test_summary.json` (counts, failing tests, duration, top failures)
- `build_summary.json` (targets, warnings/errors counts, first error location if available)
- `junit.xml` — JUnit XML report (recommended for test jobs; enables integration with CI dashboards and standard tooling)
These MUST be derived from authoritative sources (`xcresult` when present; logs as fallback).

## Timeouts + retries

### Timeout defaults and bounds (normative)
- `overall_seconds`: maximum wall-clock time for a single job execution. Default: 1800 (30 min). MUST be > 0 and ≤ 86400 (24h).
- `idle_log_seconds`: maximum time without new log output before the host kills the job. Default: 300 (5 min). MUST be > 0 and ≤ `overall_seconds`.
- `connect_timeout_seconds`: SSH/transport connection timeout. Default: 30. MUST be > 0 and ≤ 300.
- If any timeout value is out of bounds, the host MUST reject the configuration and exit with a diagnostic (before run execution).

### Retry policy (normative)
- SSH/connect: retry with exponential backoff. Default: 3 attempts, initial delay 2s, max delay 30s.
- Transfer (upload_source): retry with backoff (idempotent by `source_sha256`). Default: 3 attempts.
- Executor: MUST NOT retry automatically (non-idempotent). Retries require a new run.
- On failure at any stage, artifacts MUST still include logs + diagnostics if available.

## Caching
Caching MUST be correctness-preserving:
- Cache keys derive from `job_key` (or documented sub-keys).
- DerivedData modes: `off` | `per_job` | `shared` (shared requires safe keying + locking).
- SPM cache mode: `off` | `shared` (shared keyed by resolved Package.resolved + toolchain).
`metrics.json` includes cache hit/miss + sizes + timing.

### Cache namespace (recommended)
Repo config SHOULD provide a stable `cache.namespace` used as part of shared cache directory names,
to prevent collisions across unrelated repos on the same worker.

`cache.namespace` MUST be filesystem-safe:
- MUST match: `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`
- MUST NOT contain `/`, `\`, whitespace, or `..`

### Cache keying details (normative)
- Any cache directory that can be reused across jobs MUST be additionally keyed by toolchain identity
  (at minimum: Xcode build number and macOS major version) to prevent cross-toolchain corruption.
- `metrics.json` SHOULD record the concrete cache key components used (job_key, xcode_build, macos_version, etc.).

### Cache directory layout (recommended, stabilizes operability)
To prevent collisions and simplify GC, workers SHOULD use a predictable cache root layout such as:
- `caches/<namespace>/spm/<toolchain_key>/...`
- `caches/<namespace>/derived_data/shared/<toolchain_key>/...`
- `caches/<namespace>/derived_data/per_job/<job_key>/...`
Where:
- `namespace` is `cache.namespace` (filesystem-safe)
- `toolchain_key` is a filesystem-safe composite derived from toolchain identity (e.g. `xcode_<build>__macos_<major>__<arch>`)
Workers SHOULD record the resolved cache paths in `metrics.json` for transparency.

### Result cache (recommended)
Worker SHOULD maintain an optional result cache keyed by `job_key`:
- If present and complete, a submit MAY be satisfied by materializing artifacts from the cached result.
- The worker MUST still emit a correct `attestation.json` for the new `job_id` referencing the same `job_key`.

Profile-aware reuse (recommended, prevents surprise omissions):
- When materializing from cache, the worker MUST rewrite `run_id`, `job_id`, `created_at`, and `job_key`
  in all job-scoped artifacts to reflect the new job context. The worker MUST record `cached_from_job_id`
  (the original job_id) in `summary.json`.
- Each cached entry SHOULD record `artifact_profile` it satisfies.
- A submit MAY be satisfied from cache ONLY if cached `artifact_profile` is >= requested `artifact_profile`.
- Otherwise, the worker MUST execute the job to produce the missing richer artifacts.

### Locking + isolation (normative)
- `per_job` DerivedData MUST be written under a directory derived from `job_key`.
- `shared` caches MUST use a lock to prevent concurrent writers corrupting state.
  - Lock MUST have a timeout and emit diagnostics if contention occurs.
- Worker MUST execute each job in an isolated working directory (unique per job_id).

### metrics.json schema (recommended)
`metrics.json` SHOULD include at least:

| Field               | Type   | Description                                          |
|---------------------|--------|------------------------------------------------------|
| `schema_version`    | int    | Always `1`                                           |
| `schema_id`         | string | `rch-xcode/metrics@1`                                |
| `run_id`            | string | Parent run identifier                                |
| `job_id`            | string | Job identifier                                       |
| `job_key`           | string | Job key                                              |
| `created_at`        | string | RFC 3339 UTC                                         |
| `timings`           | object | `{ bundle_ms, upload_ms, queue_ms, execute_ms, fetch_ms, total_ms }` |
| `cache`             | object | `{ derived_data_hit: bool, spm_hit: bool, result_cache_hit: bool }` |
| `cache_paths`       | object | Resolved cache directory paths used                  |
| `sizes`             | object | `{ source_bundle_bytes, artifact_bytes, xcresult_bytes }` |
| `cache_key_components` | object | Concrete key components used (job_key, xcode_build, etc.) |

### Eviction / garbage collection (normative)
Worker MUST implement at least one:
- size-based eviction (e.g. keep under N GB)
- age-based eviction (e.g. delete items unused for N days)
Eviction MUST NOT delete caches that are currently locked/in use.

### Host artifact retention (recommended)
The host SHOULD implement artifact retention to bound disk usage under the local artifact root:
- Retention policy SHOULD support age-based and/or count-based limits (e.g. keep last N runs or last N days).
- The host MUST NOT delete artifacts for RUNNING runs.
- `rch xcode gc` (optional CLI) MAY trigger manual garbage collection.
- The host SHOULD log when artifacts are evicted and record the policy in `effective_config.json`.

### Concurrency + capacity (normative)
- Worker MUST enforce `max_concurrent_jobs`.
- If capacity exceeded, worker MUST respond with a structured "busy" state so host can retry/backoff.
  - Response SHOULD include `retry_after_seconds`.

## Worker lease (recommended)
If the worker advertises feature `lease`, the host SHOULD:
- call `reserve` once per run (before submitting any jobs),
- include the returned `lease_id` on each `submit`,
- renew or re-reserve if the lease expires before completion,
- and call `release` when the run finishes.

`release` MUST be idempotent: if the lease is unknown or already expired, the worker MUST return `ok: true` (no error). This simplifies host cleanup after crashes or lease expiry.

If the worker is at capacity, `reserve` SHOULD fail with `failure_kind=WORKER_BUSY` and include `retry_after_seconds`.

Lease TTL guidance (normative): workers SHOULD set lease TTL >= `connect_timeout_seconds + overall_seconds`
to minimize spurious expiry during slow uploads. If `submit` fails with `LEASE_EXPIRED`, the host MUST
attempt a single `reserve` + re-submit cycle before failing the run. `run_state.json` SHOULD record
`lease_renewed: true` if re-reservation occurs.

## Worker capabilities
Worker reports a `capabilities.json` including:
- Installed Xcode versions as an array of objects, each containing:
  - `version` (e.g. `"16.2"`)
  - `build` (e.g. `"16C5032a"`)
  - `path` (e.g. `"/Applications/Xcode-16.2.app"`)
  - `developer_dir` (e.g. `"/Applications/Xcode-16.2.app/Contents/Developer"`)
- Active Xcode: currently selected `DEVELOPER_DIR`
- macOS version + architecture
- available runtimes/devices (simctl), including runtime identifiers and runtime build strings when available
- installed tooling versions (rch-worker, XcodeBuildMCP)
- capacity (max concurrent jobs, disk free)
- limits (recommended): `max_upload_bytes`, `max_artifact_bytes`, `max_log_bytes`
Normative:
- `capabilities.json` MUST include `schema_version` and `created_at` (RFC 3339 UTC).
Optional but recommended:
- worker identity material (SSH host key fingerprint and/or attestation public key fingerprint)
Host stores the chosen worker capability snapshot in artifacts.

## Worker selection (normative)
Given a set of eligible workers (tag match + reachable), host MUST choose deterministically
unless user explicitly requests randomness.

Selection mode (normative):
- Default: `worker_selection.mode = "deterministic"`
- Optional: `worker_selection.mode = "adaptive"`

In `deterministic` mode, dynamic metrics (e.g. current load, free disk) MUST NOT affect ordering/choice.
In `adaptive` mode, dynamic metrics MAY be used as tie-breakers, but the host MUST record the exact
metric values used in `worker_selection.json`.

Selection inputs:
- required tags: `macos,xcode` (and any repo-required tags)
- constraints: Xcode version/build, platform (iOS/macOS), destination availability
- preference: only in `adaptive` mode, lowest load / highest free disk MAY be used as a tie-breaker

Selection algorithm (default):
1. Filter by required tags.
2. Probe or load cached `capabilities.json` snapshots (bounded staleness).
   - If a probe fails (timeout, connection error, or protocol error), the host MUST exclude that worker from the candidate set for this run and record `{ worker, probe_error, probe_duration_ms }` in `worker_selection.json` under a `probe_failures[]` array.
   - If ALL probes fail, the host MUST fail with `failure_kind=SSH` and `human_summary` listing the unreachable workers.
   - If a cached snapshot exists but is stale (older than TTL), and the fresh probe fails, the host MUST NOT fall back to the stale snapshot (to prevent using outdated capability data).
3. Filter by constraints (destination exists, required Xcode available).
4. Sort deterministically by:
   - explicit worker priority (host config)
   - then stable worker name
5. Choose first.

The host MUST write:
- `worker_selection.json` (inputs, filtered set, chosen worker, reasons)
- `capabilities.json` snapshot as used for the decision

Staleness policy (normative):
- Host MUST define a TTL for cached capability snapshots (e.g. `probe_ttl_seconds`).
- If a cached snapshot is older than TTL, host MUST re-probe before selecting a worker.
- `worker_selection.json` MUST record whether the snapshot was cached or freshly probed and the snapshot age.

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

## Execution environment (normative)
- Worker MUST execute each job in an isolated working directory unique per `job_id`.
- Worker MUST set `DEVELOPER_DIR` to the resolved toolchain `developer_dir` prior to execution.
- Worker MUST apply an environment allowlist (drop-by-default) when launching the backend.
- Worker MUST redact secrets from logs/artifacts to the extent feasible (at minimum: do not emit env vars outside the allowlist).

### Executor environment audit (recommended)
The worker SHOULD emit `executor_env.json` containing:
- `schema_version`, `schema_id`, `created_at`, `run_id`, `job_id`, `job_key`
- `passed_keys[]`: environment variable keys passed to the backend (no values by default)
- `dropped_keys[]`: keys present in the worker process environment but intentionally not passed
- `overrides[]`: keys the worker set explicitly (e.g. `DEVELOPER_DIR`)
If values are ever recorded (not recommended), they MUST be explicitly opt-in and MUST be redacted by default.

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

### Attestation key pinning (recommended)
Host worker inventory MAY pin an `attestation_pubkey_fingerprint`.
If pinned and the worker provides a signing key, the host MUST verify the fingerprint match
before accepting signed attestations.

## Artifact manifest (normative)
`manifest.json` MUST enumerate produced artifacts with at least:
- `path` (relative), `size`, `sha256`
`manifest.json` MUST also include `artifact_root_sha256` to bind the set.

`entries` MUST be sorted lexicographically by `path` (UTF-8).
`artifact_root_sha256` MUST be computed as:
- `sha256_hex( JCS(entries) )`
where `entries` is the exact array written to `manifest.json`.

### Host manifest verification (normative)
After fetching a job's artifacts, the host MUST:
1. Recompute `artifact_root_sha256` from the fetched `manifest.json` entries and verify it matches the declared value.
2. Verify `sha256` and `size` for every entry in `manifest.json` against the fetched files.
3. Verify that no extra files exist in the artifact directory beyond those listed in `manifest.json`
   (plus `manifest.json`, `attestation.json`, and `job_index.json` themselves).

Inclusion rule (normative): `manifest.json` entries MUST include every file and directory in the job artifact
directory EXCEPT `manifest.json`, `attestation.json`, and `job_index.json` themselves. This makes the
"no extra files" check above unambiguous: any file not in entries and not in the excluded triple is a violation.

If any check fails, the host MUST:
- Mark the job as failed with `failure_kind=ARTIFACTS` and `failure_subkind=INTEGRITY_MISMATCH`.
- Record the mismatched paths/digests in `summary.json` under `integrity_errors[]`.
- NOT use the artifacts for caching or attestation verification.

## Milestones
- **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
- **M1**: Classifier detects Xcode build/test safely
- **M1.5**: Mock worker implements protocol ops (probe/submit/status/tail/cancel/has_source/upload_source) for conformance tests
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
- Protocol replay: recorded request/response transcripts can be replayed deterministically against host logic.
- State machine transitions: run and job states follow the defined state diagrams.
- Cache correctness: result cache hits produce artifacts identical to fresh runs.

Tests SHOULD be runnable without a live worker (use a mock worker + fixtures) and integrated into CI.
The project SHOULD ship a minimal mock worker implementation that validates request/response schemas and exercises host logic deterministically.

Recommended fixtures:
- A minimal Xcode fixture project (build + test) with stable outputs suitable for golden assertions.
- Classifier fixtures: a corpus of accepted/rejected `xcodebuild` invocations (including tricky edge cases).

## JSON Schemas (recommended)
The project SHOULD ship machine-readable JSON Schemas for all normative artifacts under `schemas/rch-xcode/`.
Schema files SHOULD follow the naming convention `<artifact_name>.schema.json` (e.g. `schemas/rch-xcode/job.schema.json`).
Schemas enable automated validation in CI and by third-party tooling.

Schema authoring recommendations:
- Each schema SHOULD include a JSON Schema `$id` that corresponds to `schema_id`.
- Use a single JSON Schema draft consistently across the project (2020-12 recommended).

## Artifacts
- run_index.json
- run_summary.json
- run_plan.json
- run_state.json
- summary.json
- attestation.json
- manifest.json
- effective_config.json
- job_key_inputs.json
- job.json
- job_state.json
- invocation.json
- toolchain.json
- destination.json
- metrics.json
- source_manifest.json
- worker_selection.json
- job_index.json
- executor_env.json (recommended)
- classifier_policy.json (recommended)
- attestation_verification.json (recommended)
- events.jsonl (recommended)
- test_summary.json (recommended)
- build_summary.json (recommended)
- junit.xml (recommended, test jobs)
- build.log
- result.xcresult/

## Artifact indices (normative)
To make artifact discovery stable for tooling, the system MUST provide:

- `run_index.json` at `<run_id>/run_index.json`
- `job_index.json` at `<run_id>/steps/<action>/<job_id>/job_index.json`

`run_index.json` MUST include `schema_version`, `schema_id` (`rch-xcode/run_index@1`), `created_at`, `run_id`, and pointers (relative paths) to:
- `run_plan.json`, `run_state.json`, `run_summary.json`
- `worker_selection.json` and the selected `capabilities.json` snapshot
- an ordered list of step jobs with `{ index, action, job_id, job_index_path }`

`job_index.json` MUST include pointers (relative paths) to the job's:
- `job.json`, `job_state.json`, `summary.json`, `manifest.json`, `attestation.json`
- primary human artifacts (`build.log`, `result.xcresult/` when present)
and MUST record the `artifact_profile` produced.

### Artifact completion + atomicity (normative)
To prevent consumers from observing partially-written artifact sets, the worker MUST treat the job artifact
directory as a two-phase commit:
1) Write all job artifacts to their final relative paths (or write-then-rename per file).
2) Write `manifest.json` enumerating the final set.
3) Write `attestation.json` binding `manifest.json` (and any signing material).
4) Write `job_index.json` **last** and atomically (write-then-rename).

Consumers (host CLI, fetchers, tooling) MUST treat the existence of `job_index.json` as the commit marker that
the job's artifact set is complete and internally consistent.

## Artifact schemas + versioning
All JSON artifacts MUST include:
- `schema_version`
- `schema_id`
- `created_at`

Run-scoped artifacts MUST include:
- `run_id`

Job-scoped artifacts MUST include:
- `run_id`
- `job_id`
- `job_key`

### Schema compatibility rules (normative)
- Adding new optional fields is a backward-compatible change and MUST NOT require bumping `schema_version`.
- Removing fields, changing meanings/types, or tightening constraints in a way that breaks old producers/consumers
  MUST bump `schema_version`.
- `schema_id` MUST be a stable string identifier for the artifact schema, e.g.:
  - `rch-xcode/job@1`
  - `rch-xcode/summary@1`
  - `rch-xcode/run_plan@1`

## Next steps
1. Bring Mac mini worker online
2. Implement `rch xcode verify`
3. Add classifier + routing
4. Add XcodeBuildMCP backend
