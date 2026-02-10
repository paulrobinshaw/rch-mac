# IMPLEMENTATION — RCH Xcode Lane

> **Practical build plan.** PLAN.md is the normative spec; this document describes
> what will actually be built and in what order. Sections marked **(post-MVP)** are
> specced in PLAN.md but deferred past initial milestones.

## Architecture summary
```
Host (Linux/macOS)                          Worker (macOS)
┌──────────────────────┐                    ┌──────────────────────┐
│ CLI / rch xcode      │                    │ rch-worker xcode rpc │
│   ├─ Classifier      │   SSH + JSON RPC   │   ├─ Executor        │
│   ├─ Run builder     │ ◄────────────────► │   ├─ Artifact writer │
│   ├─ Source bundler   │                    │   └─ Cache manager   │
│   └─ Artifact store  │                    └──────────────────────┘
└──────────────────────┘
```

## MVP scope (M0–M4)

### M0: Worker connectivity
- Mac mini reachable via SSH, tagged `macos,xcode` in `~/.config/rch/workers.toml`
- Implement `rch workers probe <worker>` → captures `capabilities.json`
- SSH hardening: dedicated `rch` user, forced-command entrypoint preferred

### M1: Classifier
- Deny-by-default gate for `xcodebuild` invocations
- Allowed actions: `build`, `test` (constrained by `.rch/xcode.toml`)
- Reject: archive, export, signing, arbitrary flags, output path overrides
- Emit `invocation.json` with `original_argv`, `sanitized_argv`, `accepted_action`
- `rch xcode explain -- <cmd>` for human/agent-readable classifier decisions

### M1.5: Mock worker + protocol conformance
- Implement mock worker supporting: `probe`, `submit`, `status`, `tail`, `cancel`, `has_source`, `upload_source`
- JSON RPC envelope: request on stdin, response on stdout (SSH forced-command compatible)
- Protocol version negotiation via `probe` (see PLAN.md §Protocol version bootstrap)
- Use mock worker for all host-side integration tests

### M2: MVP remote execution (backend: xcodebuild)
- **Backend**: direct `xcodebuild` invocation (not XcodeBuildMCP)
- Source bundling: deterministic tar (sorted paths, normalized mtimes/uid/gid), SHA-256 digest
  - Respect `.rchignore`, exclude `.git/`, `DerivedData/`, `.build/`, `*.xcresult/`
  - Bundle mode: `worktree` (default) or `git_index`
  - Emit `source_manifest.json` per run
- Content-addressed source store on worker (skip re-upload if `source_sha256` matches)
- Binary framing for `upload_source` (JSON header + raw bytes; zstd compression)
- Run builder: resolve repo `verify` actions → `run_plan.json` with ordered step jobs
- Destination resolver: resolve constraints (e.g. OS=latest) against worker capabilities snapshot
  - Record resolved destination (platform, os_version, sim_runtime_identifier) in job_key_inputs
  - Emit destination.json per job
- State artifacts: emit run_state.json and job_state.json on every state transition (atomic write-then-rename)
  - Required for signal handling, resumption, and observability
- Sequential step execution; abort on first failure (unless continue_on_failure)
- Worker selection: filter by tags → filter by constraints → sort by priority/name → first
  - Emit `worker_selection.json`
- Job lifecycle: `submit` → poll `status` → fetch artifacts
- Timeouts: `overall_seconds`, `idle_log_seconds`, `connect_timeout_seconds`
- Retry: SSH connect (3x exponential backoff), upload (3x), executor (never)
- Host signal handling: SIGINT → cancel running jobs → persist state → exit 80
- CLI commands: `rch xcode verify`, `rch xcode run --action <build|test>`, `--dry-run`

### M3: XcodeBuildMCP backend
- Alternative backend selected via `backend = "mcp"` in `.rch/xcode.toml`
- Same artifact contract as xcodebuild backend
- Richer diagnostics, structured build/test events
- NOT a fallback — explicit choice per repo config

### M4: Artifacts + attestation
- All normative artifacts per PLAN.md (summary.json, manifest.json, attestation.json, etc.)
- Artifact completion protocol: write artifacts → manifest → attestation → job_index.json (commit marker)
- Host manifest verification: recompute digests, check for extra files
- Unsigned attestation binding worker identity + artifact set
- Stable exit codes (0=success, 10=rejected, 20=SSH, 30=transfer, 40=executor, 50=xcodebuild, 60=MCP, 70=artifacts, 80=cancelled, 90-93=worker/bundler/attestation)
- Failure taxonomy: `failure_kind` + optional `failure_subkind` in summary.json

## Post-MVP (M5–M7)

### M5: Caching + performance
- DerivedData modes: `off` | `per_job` | `shared` (with toolchain-keyed directories + locking)
- SPM cache: `off` | `shared` (keyed by Package.resolved + toolchain)
- Result cache: keyed by `job_key`, materializes artifacts without re-execution
- Emit `metrics.json` with timings, cache hits, sizes
- Bundle size enforcement (`bundle.max_bytes` + worker `max_upload_bytes`)
- Host artifact retention / GC (`rch xcode gc`)

### M6: Worker capabilities + selection
- Deterministic vs adaptive worker selection modes
- Toolchain resolution: match Xcode build number against capabilities
- Worker leases (reserve/release) for multi-worker contention
- Probe staleness TTL + cache invalidation
- Run resumption: recover from host restart via `run_plan.json` + `job_index.json` commit markers
- Concurrent host runs (unique `run_id` isolation)

### M7: Hardening + conformance
- Attestation signing (Ed25519) + key pinning
- Ephemeral simulator provisioning (clean sim per job)
- Artifact profiles (`minimal` / `rich`)
- Resumable uploads
- Conformance test suite: classifier, JobSpec determinism, source bundle reproducibility, protocol replay
- JSON Schema files under `schemas/rch-xcode/`
- Fixture project for golden-file assertions

## Key components

### Classifier (`src/xcode/classifier`)
- Pattern-match `xcodebuild` argv against allowlist
- Validate workspace/project/scheme against `.rch/xcode.toml`
- Emit structured rejection reasons
- Policy snapshot for auditability (`classifier_policy.json`)

### Destination resolver (src/xcode/destination)
- Resolve destination constraints against worker capabilities.json
- Map OS=latest to concrete version + runtime identifiers
- Validate destination availability on selected worker

### Source bundler (`src/xcode/bundler`)
- Deterministic tar (PAX format recommended)
- Content-addressed by `source_sha256`
- Symlink safety: reject escaping symlinks
- `.rchignore` support

### Host RPC client (`src/xcode/rpc`)
- JSON-over-SSH: single request → single response per operation
- Operations: probe, submit, status, tail, cancel, has_source, upload_source, fetch
- Binary framing for upload_source/fetch (JSON header line + raw bytes)
- Protocol version negotiation

### Worker entrypoint (`src/worker/xcode`)
- `rch-worker xcode rpc`: stdin/stdout JSON RPC handler
- Job execution: set `DEVELOPER_DIR`, env allowlist, isolated workdir per `job_id`
- Artifact writer: two-phase commit (artifacts → manifest → attestation → job_index)
- Source store: content-addressed, atomic writes
- Concurrency: enforce `max_concurrent_jobs`, return BUSY with `retry_after_seconds`

### Config (`src/xcode/config`)
- Merge: defaults → host config → repo config → CLI flags
- Deep-merge objects, replace arrays, override scalars
- Emit `effective_config.json` per job (secrets redacted)

## Error handling
- Failure taxonomy per PLAN.md: `failure_kind` enum (CLASSIFIER_REJECTED, SSH, TRANSFER, EXECUTOR, XCODEBUILD, MCP, ARTIFACTS, CANCELLED, WORKER_INCOMPATIBLE, BUNDLER, ATTESTATION, WORKER_BUSY)
- Stable integer exit codes for scripting (see M4 above)
- No fail-open: if the worker is unreachable, the job fails (exit 20). Local Xcode is not a fallback.
- `human_summary` in summary.json for console output

## Testing strategy
- **Unit**: classifier (corpus of accepted/rejected invocations), config merge, source bundler determinism
- **Integration**: mock worker + host RPC round-trips, artifact verification, state machine transitions
- **E2E** (optional): real Mac mini, real Xcode project, full verify cycle
- **Fixtures**: minimal Xcode project (build + test targets), classifier edge cases

## Decisions log
- Backend `xcodebuild` is MVP (M2); XcodeBuildMCP is M3. No auto-fallback between them.
- Source bundling uses deterministic tar, not rsync/git-push. Enables content-addressing + caching.
- No local Xcode fallback ("fail-open"). The lane is a remote gate; if remote fails, the job fails.
- Worker leases, attestation signing, ephemeral sims, and result cache are post-MVP.
- One worker, one run at a time for MVP. Concurrency and multi-worker come in M6.
