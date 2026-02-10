# PLAN — RCH Xcode Lane

## Vision
Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker (Mac mini) using XcodeBuildMCP, producing deterministic, machine-readable results.

## Goals
- Remote Xcode build/test via macOS workers only
- Deterministic configuration and attestation
- Agent-friendly JSON outputs and artifacts
- Safe-by-default interception (false negatives preferred)

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

## Deterministic JobSpec + Job Key
Each remote run is driven by a `job.json` (JobSpec) generated on the host.
The host computes:
- `source_sha256` — SHA-256 of the sent source bundle (after canonicalization)
- `job_key` — SHA-256 over: `source_sha256 + effective_config + sanitized_invocation + toolchain`
Artifacts include both values, enabling reproducible reruns and cache keys.

## Classifier (safety gate)
The classifier MUST:
- match only supported forms of `xcodebuild` invocations
- reject unknown flags / actions by default
- enforce repo config constraints (workspace/project, scheme, destination)
- emit a machine-readable explanation when rejecting (`summary.json` includes `rejection_reason`)

## Configuration
- Repo-scoped config: `.rch/xcode.toml` (checked in)
- Host/user config: `~/.config/rch/*` (workers, credentials, defaults)
`effective_config.json` MUST be emitted per job (post-merge, fully resolved).

## Failure taxonomy
`summary.json` MUST include:
- `status`: success | failed | rejected
- `failure_kind`: CLASSIFIER_REJECTED | SSH | TRANSFER | EXECUTOR | XCODEBUILD | MCP | ARTIFACTS
- `exit_code`: stable integer for scripting
- `human_summary`: short string for console output

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

## Worker capabilities
Worker reports a `capabilities.json` including:
- Xcode version(s) + build number, `DEVELOPER_DIR`
- available runtimes/devices (simctl)
- installed tooling versions (rch-worker, XcodeBuildMCP)
- capacity (max concurrent jobs, disk free)
Host stores the chosen worker capability snapshot in artifacts.

## Threat model / security notes
- Remote execution is limited to configured Xcode build/test actions.
- Worker SHOULD run under a dedicated user with constrained permissions.
- Prefer an implementation that does not require arbitrary interactive shell access.
- Not in scope: code signing, notarization, exporting archives, publishing.

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
