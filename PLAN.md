# RCH Xcode Lane — Specification

> **This document is normative.** It defines the contract for the RCH Xcode Lane: configuration semantics, job lifecycle, artifacts, safety rules, and performance requirements. If README.md conflicts with this document, this document wins.

## Vision

Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker via a stable worker harness (`rch-xcode-worker`), producing deterministic, machine-readable results. The harness may use XcodeBuildMCP or `xcodebuild` as its backend, but the host speaks one protocol and always receives structured NDJSON events.

## Goals

- Remote Xcode build/test via macOS workers only
- Deterministic configuration and attestation
- Agent-friendly JSON outputs and artifacts
- Safe-by-default interception (false negatives preferred)

---

## Terms

- **Host**: Machine running `rch` client + daemon.
- **Worker**: macOS machine running jobs over SSH.
- **Job**: One remote build/test execution with stable, addressable artifacts.
- **Profile**: Named configuration block (e.g., `ci`, `local`, `release`).
- **Lane**: The Xcode-specific subsystem within RCH.
- **Run ID**: Content-derived identifier (SHA-256 of canonical **config inputs** + source tree hash); identical inputs produce the same run ID.
- **Job ID**: Unique identifier per execution attempt; never reused.
- **Contract Version**: A stable semantic version of the *lane contract* that is included in `effective_config.inputs` (and therefore influences `run_id`). It MUST be bumped whenever the meaning of any hashed input changes (defaults, canonicalization, policy semantics, backend argument reconstruction), even if `lane_version` changes for other reasons.

---

## Core Architecture

### Host (RCH daemon)

- Classifies commands (strict allowlist)
- Selects an eligible worker (tags: `macos,xcode`)
- Syncs workspace snapshot (data-plane staging; rsync or bundle depending on profile)
- Executes by invoking the worker harness (`rch-xcode-worker run`) over SSH (single protocol)
- Collects artifacts back to host (data-plane fetch); emits manifest + attestation

### Worker (macOS)

- Provides a stable Xcode + simulator environment
- Maintains caches (DerivedData, SPM) keyed by effective config
- Returns xcresult + logs + tool metadata

### Worker Harness (Normative)

The lane MUST use the `rch-xcode-worker` harness for remote execution. This is a lightweight executable invoked over SSH that accepts a job request on stdin and emits structured results on stdout. The harness may use XcodeBuildMCP or `xcodebuild` as its backend, but the host always speaks one protocol and always receives a consistent event/log contract.

**Protocol (Verbs + Versioning):**

The harness MUST support two verbs:

1) `rch-xcode-worker probe`
   - Emits a single JSON object to stdout (capabilities) and exits 0 on success.
   - Used by `rch xcode verify` and worker selection.

**Probe output schema (Normative):**

The probe JSON MUST include at minimum:
- `protocol_versions` (array of strings, e.g. `["1"]`) — Supported protocol versions.
- `contract_versions` (array of SemVer strings) — Supported values of `effective_config.inputs.contract_version`.
- `harness_version` (SemVer string) — Version of the `rch-xcode-worker` harness.
- `lane_version` (SemVer string) — Lane specification version the harness implements.
- `worker` (object) — Stable identifiers: `hostname`, optional `machine_id`.
- `xcode` (object) — `path`, `version`, `build` (Xcode build number).
- `simulators` (object) — Available runtimes + device types (or a summarized digest).
- `backends` (object) — `xcodebuildmcp` availability + version, `xcodebuild` availability.
- `event_capabilities` (object) — Declares which event categories the harness can emit (e.g., phases/tests/xcresult-derived diagnostics). Enables the host/agents to understand expected event richness.
- `limits` (object) — `max_concurrent_jobs`, optional disk/space hints.
- `load` (object) — Best-effort current load snapshot:
  - `active_jobs` (integer) — Number of jobs currently executing.
  - `queued_jobs` (integer) — Number of jobs waiting for a lease slot.
  - `updated_at` (ISO 8601 timestamp) — When this snapshot was taken.
- `roots` (object) — Canonical harness roots used to derive per-job paths (especially in `--forced` mode): `stage_root`, `jobs_root`, `cache_root`.

The host MUST select a `protocol_version` supported by both sides. If no overlap exists, the lane MUST refuse with stable error code `protocol_version_unsupported`.

The host MUST also ensure the job's `effective_config.inputs.contract_version` is present in the harness `contract_versions`. If unsupported, the lane MUST refuse with stable error code `contract_version_unsupported`.

2) `rch-xcode-worker run`
   - Reads exactly one JSON object (the job request) from stdin.
   - Emits NDJSON events to stdout (one JSON object per line).
   - Exits after emitting a terminal `complete` event.

The harness SHOULD support an optional third verb:

3) `rch-xcode-worker cancel`
   - Reads a single JSON object from stdin containing at minimum `job_id` (and SHOULD include `lease_id` if known).
   - Terminates the active backend process group for that job (SIGTERM, then SIGKILL after 10s).
   - Emits a single JSON object to stdout describing whether a process was found and terminated.
   - Enables clean cancellation under forced-command mode without dropping the SSH session.

**Job request (stdin) MUST include:**
- `protocol_version` (string, e.g. `"1"`)
- `job_id`, `run_id`, `attempt`
- `config_inputs` (object) — exact copy of `effective_config.inputs`
- `config_resolved` (object) — execution-time resolved fields required to run (e.g., xcode path, destination UDID)
- `paths` (object). In `--forced` mode, the harness MUST ignore any host-supplied absolute paths and MUST derive paths from configured roots + `job_id` (see Path Confinement below).
  - `src`, `work`, `dd`, `result`, `spm`, `cache` MUST be representable (even if some are aliases of others depending on cache policy).

**Job request (stdin) SHOULD include:**
- `trace` (object) — correlation identifiers for cross-system log correlation:
  - `trace_id` (string) — stable ID for correlating host/harness/CI logs across attempts

**Path Confinement (Normative):**
- In `--forced` mode, the harness MUST compute `src/`, `work/`, `dd/` and related paths solely from (`jobs_root`, `stage_root`, `cache_root`) + `job_id` (and optional stable subpaths).
- The harness MUST reject any request whose effective paths would escape the configured roots (after resolving `..` and symlinks) with stable error code `path_out_of_bounds`.
- The harness MUST treat `paths` from the host as *hints only* (or ignore entirely) when `--forced` is active.

**Backend Selection (Normative):**
- `effective_config.backend.preferred` MAY be `"xcodebuildmcp"` or `"xcodebuild"`.
- The harness MUST select an available backend consistent with policy and capabilities.
- The harness MUST record the chosen backend in `effective_config.json` as `backend.actual`.
- If the preferred backend is unavailable and fallback is disallowed, the harness MUST fail with a terminal `complete` event and stable error code `backend_unavailable`.

**Stdout/Stderr Separation (Normative):**
- Stdout MUST contain ONLY NDJSON event objects. No banners, no debug lines, no progress text, no non-JSON output.
- Stderr MAY contain human-readable logs and MUST be treated as the source for `build.log` capture/streaming.
- The host MUST capture stderr separately and MUST NOT attempt to parse it as JSON.
- The harness SHOULD route backend build output (e.g., `xcodebuild` stdout/stderr) into harness stderr so it is captured in `build.log`.

**Event stream (stdout) requirements:**
- The FIRST event MUST be `{"type":"hello", ...}` and MUST include `protocol_version`, `lane_version`, `contract_version`, and the echoed `job_id`/`run_id`/`attempt`.
- The `hello` event MUST include `worker_paths` (object) describing the *actual derived* paths in use on the worker (`src`, `work`, `dd`, `result`, `spm`, `cache`) so audits/debugging can prove where execution happened.
- Every event MUST include: `type`, `timestamp`, `sequence`, `job_id`, `run_id`, `attempt`.
- Every event SHOULD include: `trace_id` — when provided in the job request, for cross-system correlation.
- Every event SHOULD include: `monotonic_ms` — Milliseconds since harness process start (monotonic clock) to provide stable ordering under clock skew.
- The FINAL event MUST be `{"type":"complete", ...}` and MUST be the last line. After emitting `complete`, the harness MUST exit promptly.

**`complete` event payload (Normative):**

The terminal `complete` event MUST include:
- `exit_code` (integer)
- `state` (string) — one of: `succeeded`, `failed`, `canceled`, `timed_out`
- `error_code` (string|null) — primary stable error code (null on success)
- `errors` (array) — array of error objects from the Error Model (may be empty on success)
- `backend` (object) — `{ "preferred": "...", "actual": "..." }`
- `events_sha256` (string|null) — digest over emitted event bytes (excluding the `complete` line itself)
- `event_chain_head_sha256` (string|null) — when hash chain enabled, the `event_sha256` of the last non-`complete` event
- `artifact_summary` (object) — small summary, e.g. `{ "xcresult": true, "xcresult_format": "directory|tar.zst" }`

This enables streaming consumers to know the terminal outcome without waiting for artifact assembly.

**Robustness (Normative):**
- The harness MUST NOT emit partial JSON lines or unterminated objects to stdout.
- If the harness detects it cannot encode a non-terminal event as valid JSON, it MUST attempt to:
  1) emit a terminal `complete` event with `error_code="event_stream_corrupt"`, then
  2) exit promptly.
- If the harness cannot emit a valid terminal `complete` event, it MUST exit non-zero and write a diagnostic marker to stderr.
  In this case, the host MUST record `event_stream_corrupt` in `summary.json` and include an error object indicating the stream ended without a valid terminal event.

**Benefits:**

- Decouples transport (SSH) from execution logic.
- Enables future transports (local socket, mTLS) without changing the worker.
- Structured output avoids fragile log parsing.
- Harness can enforce per-job resource limits and timeouts locally.

**Forced-command mode (Strongly Recommended):**

Operators SHOULD use an SSH key restricted via `authorized_keys command=...` so the remote account cannot execute arbitrary commands. The harness MUST support `rch-xcode-worker --forced`, which:

- Reads the requested verb from `SSH_ORIGINAL_COMMAND`
- Allows ONLY `probe`, `run`, and optionally `cancel` (exact match, no extra args) and rejects anything else with `complete` + error code `forbidden_ssh_command`
- Ignores argv verbs when `--forced` is set (to prevent bypass)

---

## Configuration Model

### Repo Config: `.rch/xcode.toml`

Configuration uses **named profiles** (`[profiles.<name>]`). Select a profile with `--profile <name>`.

### Structured Xcode Test Controls (Normative)

Profiles MAY define an `xcode_test` table to control *test selection* without permitting arbitrary flag passthrough:

```toml
[profiles.ci.xcode_test]
test_plan = "CI"                          # optional
only_testing = ["MyAppTests/FooTests"]    # optional array (replaces, no concat)
skip_testing = ["MyAppTests/FlakyTests"]  # optional array (replaces, no concat)
```

- These fields MUST be included in `effective_config.inputs` when present.
- The harness MUST translate these into backend-specific invocations (XcodeBuildMCP or xcodebuild) and MUST record the final applied selection in `effective_config.resolved`.

**Profile inheritance (Normative):**

A profile MAY declare `extends` to inherit fields from one or more other profiles:
- `extends` (string or array of strings) — parent profile name(s), applied in order

**Merge rules (Normative):**
- Tables are deep-merged (child keys override parent keys at each level)
- Scalars override (child value replaces parent value)
- Arrays replace (no concatenation) to keep resolution predictable
- The final resolved profile MUST be what is encoded into `effective_config.inputs`

```toml
# Example: .rch/xcode.toml

# Base profile with shared settings
[profiles.base]
workspace = "MyApp.xcworkspace"
scheme = "MyApp"
timeout_seconds = 1800

[profiles.base.safety]
allow_mutating = false
code_signing_allowed = false

# CI profile extends base (inherits workspace, scheme, timeout_seconds, safety)
[profiles.ci]
extends = "base"
action = "test"                        # "build" | "test"
configuration = "Debug"                # CI-specific override

[profiles.ci.destination]
platform = "iOS Simulator"
name = "iPhone 16"                     # friendly alias (used if device_type_id not specified)
os = "18.2"                            # friendly alias (used if runtime_id not specified)
device_type_id = "com.apple.CoreSimulator.SimDeviceType.iPhone-16"  # preferred when available
runtime_id = "com.apple.CoreSimulator.SimRuntime.iOS-18-2"          # preferred when available

[profiles.ci.xcode]
path = "/Applications/Xcode.app"       # optional; uses worker default if omitted
require_version = "16.2"               # optional; Xcode version constraint
require_build = "16C5032a"             # optional; strongest pin when available

[profiles.ci.worker]
require_tags = ["macos", "xcode"]      # default derives from lane, but profile may further restrict
min_macos = "15.0"                     # optional constraint
selection = "least_busy"               # "least_busy" | "warm_cache" | "random" (future)

[profiles.ci.cache]
derived_data = true
spm = true
mode = "read_only"                     # "off" | "read_only" | "read_write" (prevents cache poisoning)

[profiles.ci.backend]
preferred = "xcodebuildmcp"            # "xcodebuildmcp" | "xcodebuild"
allow_fallback = true                  # allow fallback to xcodebuild if preferred unavailable

[profiles.ci.env]
allow = ["CI", "RCH_*"]                # env vars forwarded to worker (default: none)

[profiles.ci.safety]
allow_mutating = false                 # disallow implicit clean/archive
code_signing_allowed = false           # CODE_SIGNING_ALLOWED=NO

[profiles.ci.determinism]
allow_floating_destination = false     # default false; must be true to use os = "latest" in CI

[profiles.ci.source]
mode = "vcs"                           # "vcs" (default) | "working_tree"
require_clean = true                   # reject dirty working trees
include_untracked = false              # include untracked files in source tree hash

[profiles.ci.artifacts]
store = "host"                         # "host" (default) | "worker" | "object_store"
xcresult_format = "directory"          # "directory" | "tar.zst"
compression = "none"                   # "none" | "zstd" (applies to large artifacts during transfer)

[profiles.ci.limits]
max_workspace_bytes = 50_000_000_000   # workspace + DerivedData cap; enforced after staging + before collection
max_artifact_bytes  = 10_000_000_000   # total collected artifacts cap; enforced during collection
max_log_bytes       = 200_000_000      # build.log cap; truncate with marker (see Quota Enforcement)
max_events_bytes    = 50_000_000       # events.ndjson cap; on exceed: fail with complete + error_code
max_event_line_bytes = 1_000_000       # per-event JSON line cap; on exceed: fail with complete + error_code

# Release profile extends base but overrides safety for signing
[profiles.release]
extends = "base"
action = "build"
configuration = "Release"
timeout_seconds = 3600                 # longer timeout for release builds

[profiles.release.safety]
allow_mutating = true                  # override base: allow archive
code_signing_allowed = true            # override base: enable signing
```

### Quota Enforcement (Normative)

When `limits.*` are configured, the harness/host MUST enforce them deterministically:

**`max_workspace_bytes`:**
- MUST be checked at least after staging completes and again before artifact collection begins.
- If exceeded, the job MUST terminate with `error_code="workspace_quota_exceeded"`.
- The terminal error `detail` MUST include `{ "limit_bytes": <int>, "observed_bytes": <int> }`.

**`max_artifact_bytes`:**
- MUST be enforced during collection; if collecting the next artifact would exceed the cap, collection MUST stop.
- The job MUST terminate with `error_code="artifact_quota_exceeded"` and preserve already-collected artifacts + manifest entries.
- The terminal error `detail` MUST include `{ "limit_bytes": <int>, "observed_bytes": <int>, "artifact": "<path>" }`.

**`max_log_bytes`:**
- When exceeded, `build.log` MUST be truncated with an explicit marker and `redaction_report.json` MUST record `log_truncated=true`.
- The job MAY still succeed; `summary.json.errors[]` SHOULD include a non-fatal error object with code `log_truncated`.

### Host Config: `~/.config/rch/workers.toml`

```toml
[[workers]]
name = "mac-mini-1"
host = "mac-mini-1.local"
tags = ["macos", "xcode"]
ssh_user = "rch"
ssh_port = 22
ssh_run_key = "~/.ssh/rch_run_ed25519"       # used only for harness probe/run
ssh_stage_key = "~/.ssh/rch_stage_ed25519"   # used only for staging (rrsync write-only, confined to stage_root)
ssh_fetch_key = "~/.ssh/rch_fetch_ed25519"   # used only for fetching artifacts (rrsync read-only, confined to jobs_root)
ssh_host_key_fingerprint = "SHA256:BASE64ENCODEDFINGERPRINT"

# Worker roots (used to compute per-job workspaces deterministically)
stage_root = "~/Library/Caches/rch-xcode-lane/stage"
jobs_root  = "~/Library/Caches/rch-xcode-lane/jobs"
cache_root = "~/Library/Caches/rch-xcode-lane/cache"
```

### Transport Trust (Normative)

When `ssh_host_key_fingerprint` is configured for a worker, the host MUST verify the remote SSH host key matches before executing any probe/run command. If verification fails, the lane MUST refuse to run and emit a clear error with code `ssh_host_key_mismatch`.

`attestation.json` MUST record the observed SSH host key fingerprint used for the session (even if not pinned), so audits can detect worker identity drift.

#### Profile-Level Host Key Enforcement

Profiles MAY define `[profiles.<name>.trust]` to require host key pinning:

```toml
[profiles.ci.trust]
require_pinned_host_key = true
posture = "auto"   # "auto" | "trusted" | "untrusted"
```

When `require_pinned_host_key = true`, the lane MUST refuse to run unless the selected worker has `ssh_host_key_fingerprint` configured in `workers.toml`. Refusal MUST use error code `unpinned_worker_disallowed` and include a hint describing how to pin the worker.

This enables CI profiles to enforce a stronger trust posture than local development profiles.

### Optional Integrity Controls (Recommended for CI / remote storage)

Profiles MAY enable an event hash chain for tamper-evident streaming verification:

```toml
[profiles.ci.integrity]
event_hash_chain = true   # default: false
```

When enabled, the harness MUST include hash chain fields in event output (see `events.ndjson` Requirements below). This provides:
- Streaming-time integrity verification (line-by-line validation)
- Tamper evidence even if someone edits the file and recomputes only a final digest
- Better forensic confidence for canceled/terminated mid-stream jobs

### Resolution Rules

- Effective config MUST be resolved deterministically (profile defaults + CLI overrides).
- Destination + Xcode identity MUST be pinned for CI profiles unless `allow_floating_destination = true`.
- If `os = "latest"` is specified and `allow_floating_destination` is false (or absent), lane MUST refuse to run and emit an error explaining the requirement.
- If a profile is not specified, lane MUST refuse to run (no implicit defaults).

### Destination Resolution Algorithm

1. Read `destination.platform`, `destination.name`, `destination.os` from effective config.
   - If `destination.device_type_id` and/or `destination.runtime_id` are present, prefer them for matching (stable CoreSimulator identifiers).
   - Profiles MAY require CoreSimulator identifiers via `destination.require_core_ids = true` (recommended for CI).
2. If `os` is `"latest"` and `allow_floating_destination` is false, reject with error.
3. Query worker for available simulators matching the destination spec.
4. If exactly one match, use its UDID for execution, but record it only under `effective_config.resolved.destination_udid`.
5. If zero matches, reject with error listing available runtimes.
6. If multiple matches:
   - Default: reject with an error describing the duplicates and how to fix/disambiguate.
   - If `destination.on_multiple = "select"` is set, select using `destination.selector` (default selector: `"highest_udid"`), and emit a warning.
7. Record device/runtime identifiers in `effective_config.inputs` and record resolved UDID in `effective_config.resolved`.

**CoreSimulator ID requirement (Recommended):**

If `destination.require_core_ids = true` and either `device_type_id` or `runtime_id` is missing, the lane SHOULD refuse with a stable error code `core_ids_required` and include a hint describing how to obtain CoreSimulator IDs from the worker probe output.

This catches human-friendly destination strings that may drift across Xcode/runtime versions, improving CI stability.

### Destination Disambiguation (Config)

Under `[profiles.<name>.destination]`, the following fields MAY be used:
- `on_multiple` — `"error"` (default) | `"select"`
- `selector` — `"highest_udid"` | `"lowest_udid"` (future: more selectors)
- `udid` — optional explicit simulator UDID (generally host/worker-specific; best for local-only profiles)

---

## Job Lifecycle

### States

```
1. created → 2. staging → 3. running → 4. terminal
                                         ├── succeeded
                                         ├── failed
                                         ├── canceled
                                         └── timed_out
```

### Job Identity (Normative)

Every job attempt MUST carry three identity fields:

- **`job_id`** — A unique identifier for this specific execution attempt (UUID v7 recommended). Never reused across attempts.
- **`run_id`** — A content-derived identifier: `SHA-256( JCS(config_inputs) || "\n" || source_tree_hash_hex )`. Two jobs with identical config inputs and source MUST produce the same `run_id`. This enables cache lookups and deduplication.
- **`attempt`** — A monotonically increasing integer (starting at 1) within a given `run_id`. If a job is retried with the same effective config and source, `run_id` stays the same but `attempt` increments.

All three fields MUST appear in `summary.json`, `effective_config.json`, and `attestation.json`.

### Effective Config Envelope (Normative)

`effective_config.json` is an *envelope* that separates **hashable inputs** from **execution-time resolution**.

```jsonc
{
  "kind": "effective_config",
  "schema_version": "1.0.0",
  "lane_version": "0.1.0",
  "inputs": { /* hashable, content-derived */ },
  "resolved": { /* execution-time details, NOT hashed */ }
}
```

**`inputs` (hashed):** MUST include all logical build/test inputs that affect results and caching, such as:
- `contract_version` — semantic contract version for the hashed inputs (see Terms)
- action, workspace/project, scheme, configuration, timeout
- destination *spec* (platform + device/runtime identifiers or pinned versions)
- toolchain identity once pinned (Xcode build number, selected runtime identifier)
- backend policy (preferred/fallback), safety policy, cache policy, env allowlist, redaction policy

**`resolved` (not hashed):** MUST include execution-time details that are expected to vary between attempts/workers, such as:
- selected worker name/identity (also captured in attestation)
- filesystem paths (`paths.*`), temp dirs
- simulator **UDID** (especially under `strategy = "per_job"`)
- timestamps, queue position, lease bookkeeping

### Canonicalization + `run_id` (Normative)

To make `run_id` stable across hosts/implementations, the lane MUST define canonicalization.

**Canonical JSON:** The lane MUST canonicalize **`effective_config.inputs`** using the JSON Canonicalization Scheme (RFC 8785 / "JCS") before hashing. `effective_config.inputs` MUST include `contract_version`.

**`run_id` bytes:** The lane MUST compute:
`run_id = SHA-256( JCS(effective_config.inputs) || "\n" || source_tree_hash_hex )`
where `JCS(effective_config.inputs)` is UTF-8 bytes and `source_tree_hash_hex` is the lowercase hex string (UTF-8).

Rationale: simple, deterministic, and implementation-portable. Separating inputs from resolved details ensures lane versioning and worker-local details (UDIDs, paths) do not affect run identity.

### `config_hash` (Recommended)

For cache addressing and reporting, the lane SHOULD compute:
`config_hash = SHA-256( JCS(effective_config.inputs) )` as lowercase hex.

This is distinct from `run_id` (which includes the source tree hash). The `config_hash` enables efficient cache sharing: jobs with identical configuration (toolchain, destination, scheme, build settings) but different source trees can share DerivedData caches.

### Requirements

- Host MUST preserve partial artifacts for non-success terminals.
- Terminal state MUST be recorded in `summary.json`.

### Timeouts + Cancellation

- Lane MUST support a per-job timeout (`timeout_seconds`).
- If a timeout triggers, terminal state MUST be `timed_out`.
- Lane SHOULD support user cancellation (best-effort SIGINT/remote termination).
- A canceled job MUST still emit `summary.json` + `manifest.json` referencing available artifacts.

### Cancellation Semantics (Normative)

When `rch xcode cancel <job_id>` is invoked:
- The host MUST mark `status.json.state = "canceled"` once cancellation is confirmed or best-effort attempted.
- The worker harness SHOULD terminate the active `xcodebuild` process group (SIGTERM, then SIGKILL after 10s).
- The lane MUST preserve partial artifacts collected so far and MUST still emit terminal `summary.json` and `manifest.json`.

### Worker Workspace Layout + Retention (Normative)

The worker MUST execute jobs inside a dedicated per-job workspace root, e.g.:
`~/Library/Caches/rch-xcode-lane/jobs/<job_id>/`

**Workspace subpaths (Normative):**
- `src/` — staged source tree
- `work/` — scratch/workdir for transient files
- `dd/` — DerivedData (or equivalent) root
- `result/` — result bundle root (xcresult output lives under here)
- `spm/` — cloned SourcePackages dir (when supported) or per-job package working dir

**Read-only source (Normative):**

After staging completes and (when enabled) the post-stage hash check passes, the harness MUST make `src/` read-only (best-effort via filesystem permissions) prior to backend execution. This prevents build scripts and plugins from silently mutating the staged source tree.

If backend execution modifies `src/` (detected via an optional post-run hash check when `verify_after_run = true`), the job MUST fail with stable error code `source_tree_modified`.

The lane MUST provide a garbage-collection mechanism:
- `rch xcode gc` cleans host job dirs and worker job workspaces according to retention policy.
- Default retention SHOULD be time-based (e.g., keep last N days) and SHOULD be configurable.

---

## Safety Rules

### Interception Policy

- Lane MUST NOT run mutating commands by implicit interception (e.g., `clean`, `archive`) unless `allow_mutating = true`.
- When classification is uncertain, lane MUST prefer **not** to intercept (false negatives are acceptable).
- Default signing policy: `CODE_SIGNING_ALLOWED=NO` unless explicitly enabled.

### Invocation Reconstruction (Normative)

When the lane intercepts an incoming command string, it MUST:
1. Parse/classify the request (for decision/audit), then
2. Construct the remote invocation exclusively from `effective_config.json`.

The lane MUST NOT pass through arbitrary user-provided `xcodebuild` flags or paths. Any overrides MUST be explicitly modeled as structured config fields (e.g., `xcode_test.only_testing`) and included in `effective_config.json`.

Rationale: prevents flag-based policy bypass and keeps runs deterministic/auditable.

### Secrets & Environment (Normative)

To prevent accidental secret leakage in build logs and environment artifacts:

- The host MUST NOT forward ambient host environment variables to the worker by default.
- If environment variables are required, profiles MUST declare an allowlist:

```toml
[profiles.ci.env]
allow = ["CI", "RCH_*"]     # Values forwarded if present (literal names or prefix globs)
```

- `environment.json` MUST be sanitized:
  - MUST NOT include secret values (tokens, private keys, credentials).
  - SHOULD include only non-sensitive machine/tool identifiers (Xcode, macOS, runtimes).
  - MAY include allowlisted env variable names with values omitted or redacted.

**Optional log redaction:**

```toml
[profiles.ci.redaction]
enabled = true              # Default: false (for local debugging)
patterns = ["ghp_*", "xox*-*"]  # Optional additional redaction patterns
```

When `redaction.enabled = true`, the lane SHOULD redact known secret patterns from `build.log` before storage.

**Redaction reporting (Recommended):**

When redaction is enabled, the lane SHOULD emit `redaction_report.json` containing:
- `enabled` (boolean) — Whether redaction was enabled
- `patterns` (array of strings) — Identifiers for patterns applied (no secret values)
- `replacements` (integer) — Best-effort count of redactions performed
- `log_truncated` (boolean) — Whether build.log was truncated

This enables auditing that redaction policy was applied without exposing what was redacted.

### Artifact Sensitivity (Normative for remote storage)

Artifacts MAY contain sensitive information (logs, crash dumps, test output).
When `store = "object_store"` (or any non-host persistence), the lane MUST:

- Apply configured redaction to `build.log` prior to upload
- Avoid uploading `environment.json` values (only identifiers/redacted fields)
- Never upload credentials; object store auth MUST remain on the host only

### Decision Artifact (Normative)

Every job MUST emit a `decision.json` file in the artifact directory, recording the classification and routing decision made by the host. This supports auditability and debugging of interception behavior.

**Required fields:**

- `command_raw` — The original command string as received.
- `command_classified` — The classified action (`build`, `test`, `clean`, `archive`, `unknown`).
- `command_parsed` — Optional structured parse result (recognized flags/fields), for debugging and audit.
- `profile_used` — The profile name selected.
- `intercepted` — Boolean: whether the command was intercepted and routed to a worker.
- `refusal_reason` — If not intercepted, a stable error code (e.g., `"uncertain_classification"`, `"mutating_disallowed"`). Null if intercepted. MUST use stable error codes from the Error Model.
- `worker_selected` — Worker name, or null if refused.
- `timestamp` — ISO 8601 timestamp of the decision.

**Optional fields (for CI explainability):**

- `worker_candidates` — Array describing considered workers and reject reasons.

`worker_candidates` element (when present) SHOULD include:
- `name` — Worker name.
- `eligible` — Boolean: whether the worker was eligible.
- `reasons` — Array of stable codes explaining rejection (e.g., `xcode_version_mismatch`, `runtime_not_found`, `lease_unavailable`).

---

## Error Model (Normative)

To support agent/CI automation, the lane MUST provide a stable machine-consumable error model.

### Error Object

When present, an error MUST be represented as:
```json
{ "code": "string", "message": "string", "retryable": "boolean", "hint": "string|null", "detail": "object|null" }
```

- `code` — Stable error code (snake_case). MUST NOT change across releases.
- `message` — Human-readable description. MAY change across releases.
- `retryable` — Boolean indicating if the operation may succeed on retry.
- `hint` — Optional human-readable guidance for resolution.
- `detail` — Optional structured data for debugging (e.g., available runtimes, expected vs actual values).

### Stable Error Codes

Error codes MUST be stable across releases (backward compatible). Codes MUST be snake_case.

**Standard error codes:**

| Code | Meaning | Retryable |
|------|---------|-----------|
| `destination_not_found` | Requested simulator destination not available | No |
| `destination_ambiguous` | Multiple matching destinations found | No |
| `core_ids_required` | Profile requires CoreSimulator IDs but they are missing | No |
| `ssh_host_key_mismatch` | Worker SSH host key does not match pinned fingerprint | No |
| `unpinned_worker_disallowed` | Profile requires pinned host key but worker has none | No |
| `lease_expired` | Worker lease TTL exceeded without renewal | Yes |
| `lease_unavailable` | No worker lease slots available | Yes |
| `worker_unreachable` | Cannot establish SSH connection to worker | Yes |
| `xcode_not_found` | Xcode not found at expected path on worker | No |
| `xcode_version_mismatch` | Xcode version does not match constraint | No |
| `backend_unavailable` | Preferred backend unavailable and fallback disallowed | No |
| `protocol_version_unsupported` | No common protocol version between host and harness | No |
| `contract_version_unsupported` | Harness does not support requested contract_version | No |
| `runtime_not_found` | Requested simulator runtime not installed | No |
| `uncertain_classification` | Command could not be confidently classified | No |
| `mutating_disallowed` | Mutating command refused by policy | No |
| `floating_destination_disallowed` | Floating destination (os="latest") refused by policy | No |
| `dirty_working_tree` | Working tree has uncommitted changes and require_clean=true | No |
| `timeout` | Job exceeded configured timeout_seconds | No |
| `canceled` | Job was canceled by user | No |
| `event_stream_corrupt` | Worker harness could not emit valid JSON event | No |
| `events_quota_exceeded` | Event stream exceeded configured byte cap | No |
| `event_line_too_long` | A single event exceeded configured per-line cap | No |
| `source_staging_failed` | Failed to stage source to worker | Yes |
| `source_hash_mismatch` | Staged source did not match expected source_tree_hash | Yes |
| `source_tree_modified` | Source tree was modified during execution | No |
| `artifact_collection_failed` | Failed to collect artifacts from worker | Yes |
| `forbidden_ssh_command` | SSH command rejected by harness in forced mode | No |
| `path_out_of_bounds` | Requested/derived path escaped configured roots | No |
| `workspace_quota_exceeded` | Worker workspace exceeded configured cap | No |
| `artifact_quota_exceeded` | Artifacts exceeded configured cap | No |
| `log_truncated` | build.log exceeded cap and was truncated | No |
| `untrusted_remote_storage_requires_redaction` | Untrusted posture requires redaction for remote artifact storage | No |
| `symlinks_disallowed` | Source tree contains symlinks but policy forbids them | No |
| `unsafe_symlink_target` | A symlink target is unsafe under allow_safe policy | No |

Implementations MAY define additional codes; consumers MUST tolerate unknown codes.

### Required Placement

- `summary.json` MUST include:
  - `error_code` (string|null) — Primary error code for the run (null on success).
  - `errors` (array) — Array of error objects; may be empty on success.
- `decision.json` MUST use stable codes for `refusal_reason` when refusing interception.

### Threat Model

- Repositories may contain build scripts/plugins that execute arbitrary code during build/test.
- Lane does not attempt to sandbox Xcode beyond best-effort OS/user isolation.
- Operators SHOULD deploy workers as dedicated, non-sensitive machines/accounts.
- Operators SHOULD use a dedicated macOS user account with minimal privileges for RCH runs.

### Repo Trust Posture (Normative)

Profiles MAY control trust posture explicitly via `[profiles.<name>.trust]`:
- `trust.posture = "trusted"` forces trusted posture
- `trust.posture = "untrusted"` forces untrusted posture
- `trust.posture = "auto"` (default) lets the host decide based on CI context / repo provenance

When posture is `untrusted`, the lane MUST enforce (regardless of profile defaults):
- `code_signing_allowed = false`
- `allow_mutating = false`
- `cache.mode = "read_only"` (or `"off"` if profile sets caches off)
- `source.symlinks = "forbid"` unless the operator explicitly opts out for a known-safe repository
- If `action = "test"`, the lane MUST set (or require) `simulator.strategy = "per_job"` unless explicitly overridden for local use.
- If artifacts may leave the host (`store = "worker"` or `"object_store"`), the lane MUST enable log redaction (`redaction.enabled = true`) or refuse with a stable error code `untrusted_remote_storage_requires_redaction`.

The applied posture MUST be recorded in `decision.json` (as `trust_posture`) and `effective_config.json` (as final resolved values). This ensures the lane is safer-by-default even when configs drift.

### Worker SSH Hardening (Strongly Recommended)

The lane assumes repos may execute arbitrary code during build/test. Operators SHOULD minimize SSH blast radius:

- Use a dedicated macOS user with minimal privileges and no interactive shell access where feasible.
- Prefer a **forced-command run key** restricted to executing `rch-xcode-worker`:
  - Key options: `no-pty,no-agent-forwarding,no-port-forwarding,no-X11-forwarding,restrict`
- Use separate, restricted **data-plane keys** (recommended):
  - **Stage key**: write-only, confined to the `stage_root/` (push source snapshot)
  - **Fetch key**: read-only, confined to the `jobs_root/` (pull artifacts back)

**Example `authorized_keys` entries (illustrative):**

```
# Run key (forced to harness in --forced mode; verb comes from SSH_ORIGINAL_COMMAND):
command="/usr/local/bin/rch-xcode-worker --forced",no-pty,no-agent-forwarding,no-port-forwarding,no-X11-forwarding,restrict ssh-ed25519 AAAA... rch-run

# Stage key (write-only, confined to staging root):
command="/usr/local/bin/rrsync -wo ~/Library/Caches/rch-xcode-lane/stage",no-pty,no-agent-forwarding,no-port-forwarding,no-X11-forwarding,restrict ssh-ed25519 AAAA... rch-stage

# Fetch key (read-only, confined to jobs root):
command="/usr/local/bin/rrsync -ro ~/Library/Caches/rch-xcode-lane/jobs",no-pty,no-agent-forwarding,no-port-forwarding,no-X11-forwarding,restrict ssh-ed25519 AAAA... rch-fetch
```

---

## Determinism Contract

Every run MUST emit:

- `effective_config.json` — Fully-resolved configuration used for the job (Xcode path, destination, build settings, cache policy).
- `attestation.json` — Environment fingerprint: macOS version, Xcode version/build, toolchain versions, worker identity, repo state.
- `manifest.json` — Artifact inventory + hashes.

### Artifact Schema + Versioning (Normative)

Every JSON artifact emitted by the lane MUST include these top-level fields:
- `kind` — A stable identifier for the artifact type (e.g., `"summary"`, `"manifest"`, `"attestation"`).
- `schema_version` — A SemVer-like string for the artifact schema (e.g., `"1.0.0"`).
- `lane_version` — The lane implementation version (SemVer).

The repository SHOULD provide JSON Schemas under `schemas/rch-xcode-lane/` and CI/agents SHOULD validate outputs against these schemas for early break detection.

Determinism inputs SHOULD include:

- Explicit Xcode selection (path or version constraint)
- Explicit destination resolution strategy (simulator UDIDs over human names)
- Explicit signing policy (default off)

### Source Snapshot (Normative)

The `attestation.json` MUST include a `source` object capturing the state of the source tree at job creation time:

- **`vcs_commit`** — The full commit SHA of HEAD (or null if not a VCS repo).
- **`dirty`** — Boolean: true if the working tree has uncommitted changes.
- **`source_tree_hash`** — A deterministic hash of the source files sent to the worker (see Canonical Source Manifest below).
- **`untracked_included`** — Boolean: whether untracked files were included in the hash and sync.
- **`lockfiles`** — Optional array of `{ path, sha256 }` for recognized dependency lockfiles (e.g., `Package.resolved`, `Podfile.lock`, `Cartfile.resolved`) when present in the staged source.

The source tree hash is a critical input to `run_id` computation (see Job Identity).

#### Post-stage Source Integrity Check (Recommended; Normative when enabled)

Profiles MAY enable a post-stage integrity check:

```toml
[profiles.ci.source]
verify_after_stage = true
```

When `verify_after_stage = true`:
- The worker harness MUST compute `observed_source_tree_hash` from the staged tree using the same Canonical Source Manifest rules.
- If `observed_source_tree_hash` does not equal the host-provided `source_tree_hash`, the harness MUST fail the job with stable error code `source_hash_mismatch` (retryable: true).
- `attestation.json` MUST record: `source.expected_tree_hash`, `source.observed_tree_hash`, and `source.verified` (true if check passed, false if skipped).

This check catches rsync drift, disk corruption, and staging misconfiguration before the build begins.

#### Canonical Source Manifest (Normative)

The lane MUST produce a canonical manifest of the staged source and MUST use it to compute `source_tree_hash`. This enables reproducibility verification and debugging.

**Artifact:** `source_manifest.json` (required)

**Required top-level fields:** `kind`, `schema_version`, `lane_version`, `entries`

Each entry in `entries` MUST be an object with:
- `path` (string) — Normalized path: POSIX `/` separators, no leading `./`, UTF-8, sorted lexicographically (byte order).
- `type` (string) — `"file"` or `"symlink"`.
- `mode` (string) — Stable file mode string (e.g., `"100644"` for regular file, `"100755"` for executable).
- `sha256` (string) — Lowercase hex SHA-256 of file contents. For symlinks, hash the link target path bytes (not the target file contents).
- `bytes` (integer) — File size in bytes (symlink: length of target path).
- `link_target` (string|null) — REQUIRED when `type="symlink"`; MUST be the symlink target path as stored in the filesystem. Null for regular files.

**Hash computation:**

```
source_tree_hash = SHA-256( JCS(source_manifest.entries) )
```

Where `JCS` is the JSON Canonicalization Scheme (RFC 8785) and the result is lowercase hex.

**Normalization rules:**

- Paths MUST use `/` as separator (convert from platform-native).
- Paths MUST NOT have leading `./` or trailing `/`.
- Paths MUST be sorted lexicographically by UTF-8 byte values.
- Symlinks MUST be represented as `type: "symlink"` with `link_target` recorded and `sha256` computed over `link_target` bytes; the lane MUST NOT follow symlinks.
- Binary files are hashed as-is (no newline normalization).
- Default excludes: `.git/`, `.rch/`, `*.xcresult/`, `DerivedData/`. Additional excludes via `[profiles.<name>.source].excludes`.

#### Excludes + Submodules

Under `[profiles.<name>.source]`, the following fields MAY be used:
- `excludes` (array of strings) — Additional path globs to exclude from hashing/staging (applied after defaults).
- `submodules` (string) — `"forbid"` (default) | `"include"`. If `"include"`, submodules MUST be staged at their recorded commit and represented in the manifest as normal files.

### Source Policy

Source snapshot behavior is configured under `[profiles.<name>.source]`:

- **`mode`** — `"vcs"` (default): hash and sync only VCS-tracked files. `"working_tree"`: hash and sync all files (respecting excludes).
- **`require_clean`** — If true, lane MUST refuse to run when the working tree is dirty. Default: false for `local` profiles, true for `ci` profiles.
- **`include_untracked`** — If true, untracked files are included in hash and sync. Default: false.
- **`symlinks`** — `"forbid"` | `"allow_safe"` | `"allow_all"`. Default: `"forbid"` for untrusted posture, `"allow_safe"` otherwise.
  - `forbid`: Source tree MUST NOT contain symlinks; if any are found, refuse with `symlinks_disallowed`.
  - `allow_safe`: Allow symlinks only if target is relative, does not start with `/`, and does not contain `..` path segments. Unsafe symlinks cause `unsafe_symlink_target`.
  - `allow_all`: Allow all symlinks (use with caution).

### Provenance (Optional, Strongly Recommended)

For builds that require a verifiable chain of custody, the lane MAY emit a `provenance/` directory alongside the standard artifacts:

```
~/.local/share/rch/artifacts/<job_id>/
├── ...standard artifacts...
└── provenance/
    ├── attestation.sig        # Detached signature over attestation.json
    ├── manifest.sig           # Detached signature over manifest.json
    └── verify.json            # Public key reference + algorithm + instructions
```

**Requirements (when provenance is enabled):**

- Signatures MUST use Ed25519 or ECDSA P-256.
- `verify.json` MUST include: `algorithm`, `public_key` (or `public_key_url`), and `signed_files` (list of filename + hash pairs).
- Signing key MUST NOT be stored on the worker; host signs after collecting artifacts.
- Provenance is opt-in via `[profiles.<name>.provenance]` with `enabled = true` and `key_path`.

---

## Required Artifacts

Artifacts are written under a per-job directory on the host. The directory is **append-only during execution**, except for `status.json`, which is updated in-place using atomic replacement. The host SHOULD also maintain a stable run index keyed by `run_id` for deduplication and agent/CI ergonomics.

```
~/.local/share/rch/artifacts/jobs/<job_id>/
├── probe.json             # Captured worker harness probe output used for selection/verification
├── summary.json           # Machine-readable outcome + timings
├── effective_config.json  # Fully-resolved config used for the job
├── attestation.json       # Toolchain + environment fingerprint
├── source_manifest.json   # Canonical file list + per-entry hashes used to compute source_tree_hash
├── manifest.json          # Artifact index + SHA-256 hashes + byte sizes
├── decision.json          # Classification + routing decision record
├── environment.json       # Captured worker environment snapshot
├── timing.json            # Durations for stage/run/collect phases
├── metrics.json           # Resource + transfer metrics + queue stats
├── events.ndjson          # Streaming event log (newline-delimited JSON)
├── status.json            # Current job status (updated in-place during execution)
├── build.log              # Captured harness stderr (human logs + backend output). Stdout is reserved for NDJSON events.
├── backend_invocation.json# Structured backend invocation record (safe; no secret values)
├── redaction_report.json  # Optional: redaction/truncation report (no secret values; emitted when redaction enabled)
└── result.xcresult/       # When tests run and xcresult is produced (or result.xcresult.tar.zst when compression enabled)
```

### Run Index (Recommended)

The host SHOULD create:
`~/.local/share/rch/artifacts/runs/<run_id>/attempt-<attempt>/`
as a symlink or pointer to `jobs/<job_id>/` for that attempt.

This enables:
- Easy deduplication and retention decisions ("keep last N runs")
- Straightforward retrieval of "latest attempt for run_id"
- Cleaner correlation with caches (run_id/config hashes)

### `manifest.json` Requirements

- MUST include SHA-256 hashes for all material artifacts.
- MUST include byte sizes.
- SHOULD include a logical artifact type for each entry (log/json/xcresult/etc.).
- SHOULD include `kind`/`schema_version` for JSON artifacts (or inferable mapping) to aid verifiers.

Each manifest entry MUST include:
- `path` (string) — Relative path within the artifact directory.
- `sha256` (string) — Lowercase hex SHA-256 hash.
- `bytes` (integer) — Size in bytes.

Each manifest entry SHOULD include:
- `artifact_type` (string) — e.g. `"json"` | `"log"` | `"xcresult"` | `"provenance"` | `"other"`.
- `content_type` (string) — e.g. `"application/json"`, `"text/plain"`, `"application/vnd.apple.xcresult"`.
- `encoding` (string|null) — e.g. `"utf-8"` for text files.

Each manifest entry MAY include:
- `storage` (string) — `"host"` | `"worker"` | `"object_store"` (where the artifact is stored).
- `uri` (string|null) — For remote storage, a stable URI for retrieval.
- `compression` (string) — `"none"` | `"zstd"` (compression applied to the stored artifact).
- `logical_name` (string|null) — Stable name for the logical artifact (e.g. `"result.xcresult"` even if stored as `result.xcresult.tar.zst`).

### Artifact Storage & Compression (Normative)

Artifact storage and compression are controlled via `[profiles.<name>.artifacts]`.

#### Storage Modes

- `store = "host"` (default): Host collects all configured artifacts into `~/.local/share/rch/artifacts/<job_id>/`. All artifacts are stored locally.
- `store = "worker"`: Worker retains large artifacts (xcresult, logs); host stores JSON metadata artifacts + manifest with pointers. `rch xcode fetch <job_id>` retrieves on demand.
- `store = "object_store"`: Host uploads artifacts after collection; manifest MUST include stable URIs for retrieval. Requires additional `[profiles.<name>.artifacts.object_store]` configuration (endpoint, bucket, credentials).

#### Compression Options

- `xcresult_format = "directory"` (default): xcresult stored as directory tree.
- `xcresult_format = "tar.zst"`: xcresult compressed as `result.xcresult.tar.zst` for transfer and storage.
- `compression = "none"` (default): No compression for other artifacts.
- `compression = "zstd"`: Apply zstd compression to large text artifacts (logs) during transfer.

When compression is enabled, the `manifest.json` entry MUST indicate the compression type so consumers can decompress correctly.

#### Fetch Semantics

When `store` is `"worker"` or `"object_store"`, the `rch xcode fetch <job_id>` command MUST:
1. Read `manifest.json` from the local artifact directory.
2. For each entry with `storage` != `"host"`, retrieve the artifact from the indicated location.
3. Verify SHA-256 hash matches the manifest entry.
4. Decompress if `compression` indicates compressed format.
5. Update `manifest.json` to reflect local storage after successful fetch.

### `backend_invocation.json` Requirements (Normative)

A small, structured record of what the harness actually invoked. MUST be safe to store remotely.

**Required fields:**
- `kind`: `"backend_invocation"`
- `schema_version`, `lane_version`
- `job_id`, `run_id`, `attempt`
- `backend` (object): `{ "preferred": "...", "actual": "..." }`

**Backend-specific fields:**

For `backend.actual = "xcodebuild"`:
- `argv` (array of strings) — The full argument vector passed to xcodebuild.
- `cwd` (string) — The working directory.

For `backend.actual = "xcodebuildmcp"`:
- `mcp_request` (object) — A structured, redacted/safe representation of the MCP request sent.

**Additional required fields:**
- `paths` (object) — The effective confined paths used (`dd`, `result`, `spm`) for auditability and reproducibility.
- `env_names` (array of strings) — Allowlisted environment variable names passed (values MUST NOT be included).

**Prohibited:**
- MUST NOT include secret values (env values, tokens, credentials).
- Environment MUST be represented as names/allowlist only.

### `environment.json` Requirements

- MUST include Xcode path + version + build number.
- MUST include macOS version.
- SHOULD include available simulator runtimes.
- SHOULD include Node.js version (if XcodeBuildMCP used).

### `timing.json` Requirements

- MUST include durations (seconds) for: `staging`, `running`, `collecting`.
- SHOULD include total wall-clock time.

### `metrics.json` Requirements

`metrics.json` captures resource and transfer metrics for each job run.

**Required fields (when emitted):**
- `staging_bytes_sent` — Total bytes transferred to worker during staging.
- `artifact_bytes_received` — Total bytes transferred back from worker.
- `queue_wait_seconds` — Time spent waiting for a worker lease (mirrors `status.json`).

**Optional fields:**
- `peak_memory_bytes` — Peak resident memory of the `xcodebuild` process on the worker.
- `cpu_seconds` — Total CPU time consumed by the build/test.
- `disk_usage_bytes` — Workspace disk usage at completion.
- `cache_hit` — Object with `derived_data` (boolean) and `spm` (boolean) cache hit/miss status.
- `cache_keys` — Object with `config_hash`, `derived_data_key`, `spm_key` (strings) when available.

### `events.ndjson` Requirements

The event stream is a newline-delimited JSON file where each line is a self-contained event object. Events are appended in real time during job execution.

**Append safety (Normative):**
- Writers MUST append complete, newline-terminated JSON objects only.
- Writers MUST NOT emit partial JSON lines.
- Writers MUST ensure each event line is <= `max_event_line_bytes` when configured; if the next event would exceed the cap, the harness MUST terminate the job with a final `complete` event using error code `event_line_too_long`.
- When `max_events_bytes` is configured, if emitting the next event would exceed the cap, the harness MUST terminate the job with a final `complete` event using error code `events_quota_exceeded`.
- Readers MUST tolerate the file growing during reads, and SHOULD ignore a non-terminal final line only if it is not newline-terminated (defensive tail behavior).

**Required fields per event:**

- `type` — Event type string (e.g., `"build_started"`, `"test_case_passed"`, `"phase_completed"`, `"error"`, `"complete"`).
- `timestamp` — ISO 8601 timestamp.
- `sequence` — Monotonically increasing integer (1-based).
- `job_id` — Job identifier (MUST match artifacts).
- `run_id` — Content-derived identifier.
- `attempt` — Attempt number for this run_id.

**Optional event hash chain (Normative when enabled):**

When `[profiles.<name>.integrity].event_hash_chain = true`:
- Every non-`complete` event MUST include:
  - `prev_event_sha256` (string) — lowercase hex SHA-256 of the previous non-`complete` event's JSON line bytes
  - `event_sha256` (string) — lowercase hex SHA-256 of this event, computed as:
    `SHA-256( prev_event_sha256 || "\n" || JCS(event_without_chain_fields) )`
  - Where `event_without_chain_fields` is the full event object with `prev_event_sha256` and `event_sha256` removed.
- The first event (`hello`) MUST use `prev_event_sha256` = 64 zero hex characters (`"0000...0000"`).
- The terminal `complete` event MUST include `event_chain_head_sha256` equal to the last non-`complete` event's `event_sha256`.

This enables streaming consumers to verify event integrity incrementally without waiting for the final `events_sha256` digest.

**Sequence integrity (Normative):**

- `sequence` MUST start at 1 and MUST increase by exactly 1 for each subsequent event (no gaps).

**Heartbeat (Recommended):**

- The harness SHOULD emit `{"type":"heartbeat",...}` at least every 10 seconds while running.

**Standard event types:**

| Type | Emitted when |
|------|-------------|
| `hello` | First event; echoes protocol_version, lane_version, job identity |
| `queued` | Job is waiting for a lease slot (periodic, at least every 10s) |
| `lease_acquired` | Lease slot acquired; job may begin backend execution |
| `job_started` | Job execution begins on worker |
| `phase_started` | A build phase begins (compile, link, etc.) |
| `phase_completed` | A build phase ends (includes duration) |
| `test_suite_started` | A test suite begins execution |
| `test_case_passed` | A single test case passes |
| `test_case_failed` | A single test case fails (includes failure message) |
| `test_suite_completed` | A test suite finishes (includes pass/fail counts) |
| `warning` | Non-fatal issue detected |
| `error` | Fatal error during execution |
| `heartbeat` | Periodic liveness signal (at least every 10s) |
| `complete` | Final event; includes terminal state + error model + `exit_code` |

**Optional stream digest (Recommended):**

- The `complete` event MAY include `events_sha256` computed over the exact UTF-8 bytes of `events.ndjson` (excluding the `complete` event itself).
- When present, `summary.json` SHOULD copy the same `events_sha256` for easy validation.

Consumers MUST tolerate unknown event types (forward compatibility).

### `status.json` Requirements

`status.json` is a mutable file updated in-place throughout job execution. It provides a polling-friendly snapshot of current job state.

**Atomic update (Normative):**
- Writers MUST update `status.json` by writing a complete JSON document to a temporary path (e.g., `status.json.tmp`) and then atomically renaming it over `status.json`.
- Readers MUST tolerate missing/empty `status.json` during very early job creation.

**Required fields:**

- `job_id`, `run_id`, `attempt` — Job identity fields.
- `state` — Current lifecycle state (`created`, `staging`, `running`, `succeeded`, `failed`, `canceled`, `timed_out`).
- `updated_at` — ISO 8601 timestamp of last update.
- `queued_at` — ISO 8601 timestamp of when the job entered the queue (null if never queued).
- `started_at` — ISO 8601 timestamp of when execution began on the worker (null if not yet started).
- `queue_wait_seconds` — Elapsed seconds between `queued_at` and `started_at` (null if not applicable).

**Optional fields:**

- `progress` — Free-form string (e.g., `"Compiling 42/128 files"`).
- `worker` — Name of the assigned worker.

---

## Optional Artifacts

The following artifacts are not required but SHOULD be emitted when the information is available.

### `repro/` (Recommended)

A small, standardized reproduction bundle intended to make failed runs easy to re-run.

When emitted, the lane SHOULD include:
- `repro/inputs.json` — Exact copy of `effective_config.inputs` (enables re-running with identical hashable inputs).
- `repro/attestation_excerpt.json` — Minimal toolchain + destination identifiers needed to reason about reproducibility.
- `repro/README.txt` — Short instructions (no secrets) describing how to re-run with `rch xcode plan`/`test`.
- OPTIONAL: `repro/source.tar.zst` — Exact staged source snapshot (opt-in; may be disallowed in CI/object_store).

Profiles MAY control this via:

```toml
[profiles.ci.repro]
enabled = true
include_source_bundle = false
```

The repro bundle is especially valuable for failed runs, enabling developers to immediately reproduce the failure with identical inputs.

### `junit.xml`

Standard JUnit XML test report for integration with CI systems (Jenkins, GitHub Actions, etc.). Emitted when the job action is `test` and results are available.

- MUST conform to the JUnit XML schema (testsuite/testcase elements).
- SHOULD be generated from xcresult data or event stream test events.

### `test_summary.json`

Machine-readable test summary for agent consumption. Emitted when the job action is `test`.

**Required fields (when emitted):**

- `total` — Total test count.
- `passed` — Number of passed tests.
- `failed` — Number of failed tests.
- `skipped` — Number of skipped tests.
- `duration_seconds` — Total test execution time.
- `failures` — Array of `{ suite, test_case, message, file, line }` objects.

### `sarif.json` (Recommended for CI)

Standard SARIF output for build diagnostics and (optionally) test failures:
- SHOULD be derived from `xcresult` when available (preferred over log scraping).
- MUST be stable and schema-valid SARIF so CI systems can ingest it without custom tooling.
- Enables GitHub code scanning, PR annotations, and other SARIF-compatible integrations.

### `annotations.json` (Recommended for agent workflows)

A small, lane-defined diagnostic summary intended for UIs and agents:
- `{ kind, schema_version, lane_version, items: [...] }`
- Each item SHOULD include `{ severity, message, file, line, code|null, category }`
- Provides "file:line + message" format for agent consumption without bespoke xcresult parsing.

---

## Commands

| Command | Purpose |
|---------|---------|
| `rch xcode doctor` | Validate host setup (daemon, config, SSH tooling) |
| `rch xcode workers [--refresh]` | Enumerate/probe workers; show capability summaries |
| `rch xcode verify [--profile <name>]` | Probe worker + validate config against capabilities |
| `rch xcode plan --profile <name>` | Deterministically resolve effective config, worker selection, destination resolution (no staging/run) |
| `rch xcode build [--profile <name>]` | Remote build gate |
| `rch xcode test [--profile <name>]` | Remote test gate |
| `rch xcode fetch <job_id>` | Pull artifacts (if stored remotely) |
| `rch xcode validate <job_id\|path>` | Verify artifacts: schema validation + manifest hashes + event stream integrity (+ provenance if enabled) |
| `rch xcode watch <job_id>` | Stream events + follow logs for a running job |
| `rch xcode cancel <job_id>` | Best-effort cancel (preserve partial artifacts) |
| `rch xcode explain <job_id\|run_id\|path>` | Explain selection/verification/refusal decisions (human + `--json`) |
| `rch xcode retry <job_id>` | Retry a failed job with incremented attempt (preserves run_id when unchanged) |
| `rch xcode gc` | Garbage-collect old runs + worker workspaces |

**Machine-readable CLI output (Normative):**

Commands SHOULD accept `--json` to emit a single JSON object to stdout instead of human-readable text.

JSON outputs MUST use a standard envelope with:
- `kind` (string) — Command-specific result type, e.g. `"doctor_result"`, `"verify_result"`, `"plan_result"`, `"validate_result"`
- `schema_version` (string) — Schema version for this result type
- `lane_version` (string) — Lane implementation version
- `ok` (boolean) — Whether the command succeeded
- `error_code` (string|null) — Stable error code on failure (null on success)
- `errors` (array) — Array of error objects from the Error Model

This enables agent/CI integration without log scraping and provides consistent error handling across all commands.

### `explain` Output (Normative when `--json`)

`rch xcode explain --json` MUST emit a single envelope object:
- `kind`: `"explain_result"`
- `schema_version`, `lane_version`, `ok`, `error_code`, `errors` (standard envelope fields)
- `job` (object|null): `{ job_id, run_id, attempt }`
- `decision` (object|null): parsed/normalized `decision.json`
- `selection` (object|null): worker candidates + reasons (when available)
- `pins` (object|null): resolved Xcode build, runtime/device IDs, backend actual, and whether host key was pinned

This provides a one-command path for agents to understand worker selection, pinning decisions, and refusal reasons without parsing multiple artifacts.

### `doctor` Checks (Host)

- RCH daemon running
- SSH tooling available
- Config parseable
- Workers reachable

### `verify` Checks (Worker)

- Worker reachable via SSH
- Xcode installed at expected path (or discoverable)
- Requested destination available (simulator runtime + device)
- XcodeBuildMCP available (if configured as backend)
- Node.js version compatible

### `validate` Checks (Artifacts)

The `validate` command verifies artifact integrity and consistency for a completed job.

**Required validations:**

- All files listed in `manifest.json` exist and have matching SHA-256 hashes.
- All JSON artifacts parse successfully and include required `kind`, `schema_version`, `lane_version` fields.
- `probe.json` parses successfully and includes required capability fields (`protocol_versions`, `harness_version`, `lane_version`, `roots`, `backends`).
- JSON artifacts validate against their schemas (when schemas are available).
- Recompute `source_tree_hash` from `source_manifest.entries` using the Normative rules and verify it matches `attestation.json.source.source_tree_hash`.
- Recompute `run_id` as `SHA-256( JCS(effective_config.inputs) || "\n" || source_tree_hash_hex )` and verify it matches `summary.json.run_id` (and the `run_id` echoed in `events.ndjson`).
- `events.ndjson` parses as valid NDJSON, has contiguous `sequence` (no gaps), and ends with a terminal `complete` event.
- If `events_sha256` is present in the `complete` event, recompute and verify it matches.
- If hash chain fields are present, verify the chain (`prev_event_sha256`/`event_sha256`) and that `event_chain_head_sha256` matches the final non-`complete` event.
- `job_id`, `run_id`, `attempt` are consistent across artifacts.
- `summary.json` terminal state is consistent with `events.ndjson` terminal `complete` event (exit_code, error_code).
- If a run index (`runs/<run_id>/attempt-<n>`) exists, it MUST point to the matching `jobs/<job_id>` directory.

**Optional validations (when applicable):**

- Provenance signatures verify against `verify.json` public key.
- `source_manifest.json` entries hash to the recorded `source_tree_hash`.

**Exit codes:**

- `0` — All validations passed.
- `1` — One or more validations failed (details in stdout as JSON).
- `2` — Artifact directory not found or unreadable.

---

## Performance Design

### Incremental Staging

- Use rsync with excludes (`.git`, `DerivedData`, `*.xcresult`, etc.).
- Optional: git archive/clone strategy for pristine working copies.

### Cache Buckets

| Cache | Key Components |
|-------|----------------|
| DerivedData | cache_namespace + config_hash |
| SwiftPM | Xcode build + resolved dependencies hash (derived from lockfiles such as `Package.resolved`) (+ optional toolchain constraints) |

### Cache Policy (Normative)

Cache usage MUST be controlled via `[profiles.<name>.cache]` and MUST be reflected in `effective_config.json`. The lane MUST record cache decisions and hit/miss stats in `metrics.json` (at minimum: derived data + SPM).

#### Cache Write Policy (Normative)

To prevent cache poisoning from untrusted PRs/forks, profiles MUST declare cache write intent:

```toml
[profiles.ci.cache]
derived_data = true
spm = true
mode = "read_only"          # "off" | "read_only" | "read_write"
```

- `off`: Do not read or write caches.
- `read_only` (recommended for CI with untrusted code): May read existing caches but MUST NOT write/update them. Prevents poisoning.
- `read_write` (default for trusted repos): May read and write caches.

The worker MUST enforce the cache mode:
- When `mode = "off"`, the worker MUST NOT use cached DerivedData or SPM packages.
- When `mode = "read_only"`, the worker MUST use cached data if available but MUST NOT write new cache entries.
- When `mode = "read_write"`, the worker MAY write cache entries.

`metrics.json` MUST record:
- `cache_mode` — The effective cache mode for the job.
- `cache_writable` — Boolean: whether the cache was writable for this job (false for `off` and `read_only`).

#### Atomic Cache Promotion (Normative)

To prevent cache corruption and poisoning:
- The worker MUST NOT write directly into the shared cache namespace during a job.
- The worker MUST use per-job cache work directories (e.g., under `jobs/<job_id>/cache-work/` or `cache_root/tmp/<job_id>/`).
- When `cache.mode = "read_write"`:
  - The worker MAY promote updated cache entries ONLY if the job terminal state is `succeeded`, unless explicitly overridden.
  - Promotion MUST be atomic (e.g., write to a new directory then `rename(2)` swap) so readers never observe partial state.
- When `cache.mode = "read_only"`:
  - The worker MUST ensure shared caches are not modified (best-effort via permissions and/or copy-on-write cloning).
  - Any attempted modification detected by the worker SHOULD emit a warning event and MUST NOT mutate the shared cache namespace.

Profiles MAY opt into "promote on failure" for specialized workflows:

```toml
[profiles.<name>.cache]
promote_on_failure = false   # default: false
```

**Performance note (Non-normative):** On macOS, `clonefile(2)` can provide fast copy-on-write snapshots for DerivedData.

#### Cache Isolation (Normative)

To reduce cross-run contamination and cache poisoning, profiles MAY define cache trust boundaries:

```toml
[profiles.ci.cache]
derived_data = true
spm = true
trust_domain = "per_profile"           # "shared" | "per_repo" | "per_profile"
```

- `trust_domain = "shared"`: All jobs share the same cache namespace. Use only for fully trusted repos.
- `trust_domain = "per_repo"` (default for CI): Caches are segregated by repository identity (e.g., repo URL hash).
- `trust_domain = "per_profile"`: Caches are segregated by profile name within a repo.

The worker MUST segregate caches by the effective `trust_domain` boundary. At minimum, DerivedData and SPM caches MUST be isolated.

`metrics.json` SHOULD record:
- `cache_namespace` — The computed cache namespace key.
- `cache_writable` — Boolean indicating if the cache was writable for this job.

### Simulator Prewarm

- Boot simulator once, reuse across runs.
- Collect runtime info in `environment.json`.
- Avoid cold-start tax on every job.

### Simulator Hygiene (Normative)

To prevent test pollution and flaky behavior from shared simulator state, profiles MAY define simulator lifecycle policies:

```toml
[profiles.ci.simulator]
strategy = "shared_prebooted"          # "shared_prebooted" | "per_job"
erase_on_start = false                 # Erase simulator data before test run
shutdown_on_end = true                 # Shutdown simulator after job completion
```

- `strategy = "shared_prebooted"` (default): Reuse prebooted simulators across jobs. Faster but may accumulate state.
- `strategy = "per_job"`: Create a dedicated simulator device for the job, delete on completion (best-effort). Maximum isolation.

When `strategy = "per_job"`:
- The worker MUST create a new simulator device matching the destination spec.
- The worker SHOULD name the device deterministically for cleanup and debugging (e.g., `rch-<job_id>` or `rch-<run_id>-<attempt>`).
- The worker MUST record the created UDID in `effective_config.json`.
- The worker MUST delete the simulator device on job completion (best-effort cleanup).

When `erase_on_start = true`:
- The worker MUST erase the simulator device data before the test run begins.
- This is slower but ensures a clean state for each job.

`effective_config.json` MUST record the actual `simulator_strategy` and `simulator_udid` used.

### Backend Path Discipline (Normative)

The harness MUST ensure backend outputs are confined to the per-job workspace by construction:

- When invoking `xcodebuild`, the harness MUST pass:
  - `-derivedDataPath` = `worker_paths.dd`
  - `-resultBundlePath` = `worker_paths.result/result.xcresult` (or equivalent under `result/`)
  - When supported and when `cache.spm` is enabled: `-clonedSourcePackagesDirPath` = `worker_paths.spm`
- When invoking XcodeBuildMCP, the harness MUST set equivalent fields in the MCP request so the same confinement holds.

The harness MUST record the effective paths in `effective_config.resolved.paths` and echo them in the `hello.worker_paths`.

### Concurrency Control

- Avoid thrashing by limiting concurrent jobs per worker.
- Queue jobs if worker is busy.
- Report queue position in job status.

#### Worker Leases (Normative)

To prevent resource exhaustion and ensure crash recovery, the lane uses a lease-based concurrency model:

- **Lease acquisition**: Before backend execution begins, the **harness MUST acquire a lease slot locally**. The harness is the source of truth for concurrency/queueing because it has perfect local visibility. The host does not maintain separate lease state; it observes harness `queued`/`lease_acquired` events and reflects them into `status.json`.
- **Lease TTL**: Every lease MUST have a time-to-live (TTL), defaulting to `timeout_seconds + 300` (5-minute grace). TTL is a hard upper bound to prevent runaway jobs.
- **Liveness**: Lease liveness MUST be tied to the active `rch-xcode-worker run` session (SSH connection + harness process). If the session is lost unexpectedly, the worker MUST treat the job as abandoned.
- **Crash recovery**: If liveness is lost (session drop) or TTL is exceeded:
  - Worker MUST terminate the associated job process (SIGTERM, then SIGKILL after 10s).
  - Worker MUST mark the job workspace for cleanup.
  - Host MUST transition the job to `failed` with reason `"lease_expired"` (retryable) when it cannot observe a clean terminal completion.
- **Concurrency limit**: Each worker advertises a maximum concurrent job count (default: 1). If the worker is at capacity, the harness MUST queue the job and emit periodic `queued` events (at least every 10s) until a lease slot is acquired.
- **`status.json` integration**: `queued_at`, `started_at`, and `queue_wait_seconds` fields (see status.json Requirements) MUST reflect actual lease acquisition timing.

**Hello event (lease fields, Normative):**
- The harness `hello` event MUST include `lease_id` and `lease_ttl_seconds` so the host can correlate execution with concurrency state and debug lease failures.

**Lease acquisition protocol (Normative):**

Before starting backend execution, the harness MUST acquire a lease slot consistent with `limits.max_concurrent_jobs`.

- If no slot is available, the harness MUST emit periodic `queued` events (at least every 10s) including:
  - `queue_position` (integer|null) — best-effort position in queue
  - `queue_wait_seconds` (number) — elapsed time queued
- Once a slot is acquired, the harness MUST emit `lease_acquired` including:
  - `lease_id` (string)
  - `lease_ttl_seconds` (integer)
  - `queue_wait_seconds` (number)
- Backend execution MUST NOT begin until after `lease_acquired` is emitted.

### Retry Policy (Normative)

To handle transient infrastructure failures gracefully, profiles MAY define automatic retry behavior:

```toml
[profiles.ci.retry]
max_attempts = 3                       # Maximum total attempts (including first try)
retry_on = ["lease_expired", "worker_unreachable", "source_staging_failed", "artifact_collection_failed"]
```

- `max_attempts` (integer, default: 1) — Maximum number of attempts. Value of 1 means no retries.
- `retry_on` (array of strings) — Error codes that trigger automatic retry. Empty means no automatic retries.

**Retry semantics:**

- Automatic retries MUST keep the same `run_id` (content-derived, unchanged).
- Each retry MUST increment `attempt` (1, 2, 3, ...).
- Each retry MUST allocate a new `job_id` (unique per attempt).
- Retries MUST respect `max_attempts`; after exhausting retries, the job fails with the last error.

**Event recording:**

- Each attempt MUST emit `attempt_started` and `attempt_complete` events in `events.ndjson`.
- `summary.json` MUST include `attempt` field and `attempts` array with per-attempt summaries when retries occur.

**Manual retry:**

`rch xcode retry <job_id>` SHOULD be provided to manually retry a failed job with `attempt` incremented. The new attempt inherits the original `run_id` if source and config are unchanged.

---

## Milestones

- **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
- **M1**: `doctor` + worker capability probe + config validation (`verify`)
- **M2**: Classifier + policy: allowlist routing, refuse-on-uncertain, explain decisions
- **M3**: Workspace sync + remote runner MVP (fallback `xcodebuild`) with log streaming
- **M4**: XcodeBuildMCP backend with structured events (build phases, test events)
- **M5**: Determinism outputs: summary.json, effective_config.json, attestation.json, manifest.json, environment.json, timing.json
- **M5.1**: Decision artifact (`decision.json`) + event stream (`events.ndjson`) + live status (`status.json`)
- **M5.2**: Source snapshot in attestation + source policy enforcement + `source_manifest.json`
- **M5.3**: Artifact schema versioning (`kind`, `schema_version`, `lane_version`) + metrics.json
- **M5.4**: Error model (stable error codes in summary.json, decision.json)
- **M6**: Caching + performance: DerivedData/SPM caches, incremental sync, simulator prewarm, concurrency control
- **M6.1**: Worker leases + crash recovery + queue-wait metrics
- **M6.2**: Cache isolation (trust_domain boundaries)
- **M6.3**: Simulator hygiene (per-job strategy, erase/shutdown policies)
- **M7**: Ops hardening: timeouts, cancellation, worker harness (`rch-xcode-worker`), partial artifact preservation
- **M7.1**: Optional provenance (signed attestation + manifest)
- **M7.2**: Optional CI reports (`junit.xml`, `test_summary.json`)
- **M7.3**: Garbage collection (`rch xcode gc`) + worker workspace retention
- **M7.4**: Retry policy (automatic retries on transient errors)
- **M7.5**: Artifact storage + compression (worker/object_store modes, xcresult compression)
- **M7.6**: `validate` command (artifact integrity verification)
- **M7.7**: Transport trust enforcement (`require_pinned_host_key`)
- **M8**: Compatibility matrix + fixtures: golden configs, sample repos, reproducible failure cases

---

## Conformance & Fixtures (Recommended; Normative for the repo implementing this spec)

To prevent contract drift between host and harness:
- The repository implementing the lane SHOULD include a `fixtures/` directory with:
  - `fixtures/probe/` — probe JSON examples (multiple protocol versions if supported)
  - `fixtures/events/` — NDJSON event streams covering success/failure/cancel/timeout and corruption cases
  - `fixtures/jobs/` — minimal artifact directories used to exercise `rch xcode validate`
- CI SHOULD run a conformance job that:
  - Validates all JSON artifacts against schemas
  - Recomputes and verifies `run_id`, `config_hash`, and `source_tree_hash`
  - Runs `validate` against `fixtures/jobs/*` and asserts stable error codes for known-bad fixtures

---

## Policies Summary

| Policy | Default | Override |
|--------|---------|----------|
| Code signing | `CODE_SIGNING_ALLOWED=NO` | `code_signing_allowed = true` |
| Mutating commands | Disallowed | `allow_mutating = true` |
| Uncertain classification | Refuse (false negative) | N/A |
| Floating destination (CI) | Disallowed | `allow_floating_destination = true` |
| Duplicate destinations | Error | `destination.on_multiple = "select"` |
| Dirty working tree (CI) | Disallowed | `require_clean = false` |
| Worker user | Dedicated account | Operator responsibility |
| Provenance signing | Disabled | `[profiles.<name>.provenance] enabled = true` |
| Flag passthrough | Disallowed | N/A (must model as config fields) |
| SSH host key pinning | Optional (recorded) | `ssh_host_key_fingerprint` in workers.toml |
| CI requires pinning | Disabled | `[profiles.<name>.trust] require_pinned_host_key = true` |
| Cache write policy | `read_write` | `[profiles.<name>.cache] mode = "read_only"` |
| Environment passthrough | Disallowed | `[profiles.<name>.env] allow = [...]` |
| Log redaction | Disabled | `[profiles.<name>.redaction] enabled = true` |
| Backend preference | `xcodebuildmcp` | `[profiles.<name>.backend] preferred = "xcodebuild"` |
| Trust posture | `auto` | `[profiles.<name>.trust] posture = "untrusted"` |
| Event hash chain | Disabled | `[profiles.<name>.integrity] event_hash_chain = true` |
| Cache promote on failure | Disabled | `[profiles.<name>.cache] promote_on_failure = true` |
| Symlinks in source | `forbid` (untrusted) / `allow_safe` (trusted) | `[profiles.<name>.source] symlinks = "allow_all"` |

---

## Next Steps

1. Bring Mac mini worker online
2. Implement `rch xcode doctor` and `rch xcode verify`
3. Add classifier + routing + refusal/explanation paths + decision artifact
4. Implement workspace sync + remote runner + log streaming + worker harness
5. Add XcodeBuildMCP backend + event stream
6. Emit determinism artifacts + source snapshot + schema versioning
7. Add caching + performance optimizations + worker leases
8. Add optional provenance + CI reports
9. Implement garbage collection + retention policies
