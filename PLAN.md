# RCH Xcode Lane — Specification

> **This document is normative.** It defines the contract for the RCH Xcode Lane: configuration semantics, job lifecycle, artifacts, safety rules, and performance requirements. If README.md conflicts with this document, this document wins.

## Vision

Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker (Mac mini) using XcodeBuildMCP, producing deterministic, machine-readable results.

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
- **Run ID**: Content-derived identifier (SHA-256 of effective config + source tree hash); identical inputs produce the same run ID.
- **Job ID**: Unique identifier per execution attempt; never reused.

---

## Core Architecture

### Host (RCH daemon)

- Classifies commands (strict allowlist)
- Selects an eligible worker (tags: `macos,xcode`)
- Syncs workspace snapshot (rsync + excludes)
- Executes via backend (XcodeBuildMCP preferred, `rch-xcode-worker` harness recommended)
- Streams logs; assembles artifacts; emits manifest + attestation

### Worker (macOS)

- Provides a stable Xcode + simulator environment
- Maintains caches (DerivedData, SPM) keyed by effective config
- Returns xcresult + logs + tool metadata

### Worker Harness (Normative, Recommended)

The recommended way to execute jobs on a worker is via the `rch-xcode-worker` harness — a lightweight executable invoked over SSH that accepts a job request on stdin and emits structured results on stdout.

**Protocol (Verbs + Versioning):**

The harness MUST support two verbs:

1) `rch-xcode-worker probe`
   - Emits a single JSON object to stdout (capabilities) and exits 0 on success.
   - Used by `rch xcode verify` and worker selection.

2) `rch-xcode-worker run`
   - Reads exactly one JSON object (the job request) from stdin.
   - Emits NDJSON events to stdout (one JSON object per line).
   - Exits after emitting a terminal `complete` event.

**Job request (stdin) MUST include:**
- `protocol_version` (string, e.g. `"1"`)
- `job_id`, `run_id`, `attempt`
- `effective_config` (object)
- `paths` (object: source/workdir/cache roots)

**Stdout/Stderr Separation (Normative):**
- Stdout MUST contain ONLY NDJSON event objects. No banners, no debug lines, no progress text, no non-JSON output.
- Stderr MAY contain human-readable logs and MUST be treated as the source for `build.log` capture/streaming.
- The host MUST capture stderr separately and MUST NOT attempt to parse it as JSON.

**Event stream (stdout) requirements:**
- The FIRST event MUST be `{"type":"hello", ...}` and MUST include `protocol_version`, `lane_version`, and the echoed `job_id`/`run_id`/`attempt`.
- Every event MUST include: `type`, `timestamp`, `sequence`, `job_id`.
- Every event SHOULD include: `monotonic_ms` — Milliseconds since harness process start (monotonic clock) to provide stable ordering under clock skew.
- The FINAL event MUST be `{"type":"complete","exit_code":N,...}` and MUST be the last line. After emitting `complete`, the harness MUST exit promptly.

**Robustness (Normative):**
- If the harness cannot emit valid JSON for an event, it MUST fail the job with a terminal `complete` event whose payload includes error code `event_stream_corrupt`.
- The harness MUST NOT emit partial JSON lines or unterminated objects to stdout.

**Benefits:**

- Decouples transport (SSH) from execution logic.
- Enables future transports (local socket, mTLS) without changing the worker.
- Structured output avoids fragile log parsing.
- Harness can enforce per-job resource limits and timeouts locally.

---

## Configuration Model

### Repo Config: `.rch/xcode.toml`

Configuration uses **named profiles** (`[profiles.<name>]`). Select a profile with `--profile <name>`.

```toml
# Example: .rch/xcode.toml
[profiles.ci]
action = "test"                        # "build" | "test"
workspace = "MyApp.xcworkspace"        # or project = "MyApp.xcodeproj"
scheme = "MyApp"
configuration = "Debug"
timeout_seconds = 1800

[profiles.ci.destination]
platform = "iOS Simulator"
name = "iPhone 16"
os = "18.2"                            # pinned version; required for CI unless allow_floating_destination = true

[profiles.ci.xcode]
path = "/Applications/Xcode.app"       # optional; uses worker default if omitted

[profiles.ci.cache]
derived_data = true
spm = true

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

[profiles.release]
action = "build"
workspace = "MyApp.xcworkspace"
scheme = "MyApp"
configuration = "Release"
timeout_seconds = 3600

[profiles.release.safety]
allow_mutating = true
code_signing_allowed = true
```

### Host Config: `~/.config/rch/workers.toml`

```toml
[[workers]]
name = "mac-mini-1"
host = "mac-mini-1.local"
tags = ["macos", "xcode"]
ssh_user = "rch"
ssh_key = "~/.ssh/rch_ed25519"
ssh_host_key_fingerprint = "SHA256:BASE64ENCODEDFINGERPRINT"
```

### Transport Trust (Normative)

When `ssh_host_key_fingerprint` is configured for a worker, the host MUST verify the remote SSH host key matches before executing any probe/run command. If verification fails, the lane MUST refuse to run and emit a clear error with code `ssh_host_key_mismatch`.

`attestation.json` MUST record the observed SSH host key fingerprint used for the session (even if not pinned), so audits can detect worker identity drift.

#### Profile-Level Host Key Enforcement

Profiles MAY define `[profiles.<name>.trust]` to require host key pinning:

```toml
[profiles.ci.trust]
require_pinned_host_key = true
```

When `require_pinned_host_key = true`, the lane MUST refuse to run unless the selected worker has `ssh_host_key_fingerprint` configured in `workers.toml`. Refusal MUST use error code `unpinned_worker_disallowed` and include a hint describing how to pin the worker.

This enables CI profiles to enforce a stronger trust posture than local development profiles.

### Resolution Rules

- Effective config MUST be resolved deterministically (profile defaults + CLI overrides).
- Destination + Xcode identity MUST be pinned for CI profiles unless `allow_floating_destination = true`.
- If `os = "latest"` is specified and `allow_floating_destination` is false (or absent), lane MUST refuse to run and emit an error explaining the requirement.
- If a profile is not specified, lane MUST refuse to run (no implicit defaults).

### Destination Resolution Algorithm

1. Read `destination.platform`, `destination.name`, `destination.os` from effective config.
2. If `os` is `"latest"` and `allow_floating_destination` is false, reject with error.
3. Query worker for available simulators matching platform + name + os.
4. If exactly one match, use its UDID.
5. If zero matches, reject with error listing available runtimes.
6. If multiple matches:
   - Default: reject with an error describing the duplicates and how to fix/disambiguate.
   - If `destination.on_multiple = "select"` is set, select using `destination.selector` (default selector: `"highest_udid"`), and emit a warning.
7. Record resolved UDID, runtime version, and device type in `effective_config.json`.

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
- **`run_id`** — A content-derived identifier: `SHA-256(canonical(effective_config) || source_tree_hash)`. Two jobs with identical config and source MUST produce the same `run_id`. This enables cache lookups and deduplication.
- **`attempt`** — A monotonically increasing integer (starting at 1) within a given `run_id`. If a job is retried with the same effective config and source, `run_id` stays the same but `attempt` increments.

All three fields MUST appear in `summary.json`, `effective_config.json`, and `attestation.json`.

### Canonicalization + `run_id` (Normative)

To make `run_id` stable across hosts/implementations, the lane MUST define canonicalization.

**Canonical JSON:** The lane MUST canonicalize `effective_config.json` using the JSON Canonicalization Scheme (RFC 8785 / "JCS") before hashing.

**`run_id` bytes:** The lane MUST compute:
`run_id = SHA-256( JCS(effective_config) || "\n" || source_tree_hash_hex )`
where `JCS(effective_config)` is UTF-8 bytes and `source_tree_hash_hex` is the lowercase hex string (UTF-8).

Rationale: simple, deterministic, and implementation-portable.

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

The lane MUST NOT pass through arbitrary user-provided `xcodebuild` flags or paths. Any CLI overrides MUST be explicitly modeled as structured config fields and included in `effective_config.json`.

Rationale: prevents flag-based policy bypass and keeps runs deterministic/auditable.

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
| `ssh_host_key_mismatch` | Worker SSH host key does not match pinned fingerprint | No |
| `unpinned_worker_disallowed` | Profile requires pinned host key but worker has none | No |
| `lease_expired` | Worker lease TTL exceeded without renewal | Yes |
| `lease_unavailable` | No worker lease slots available | Yes |
| `worker_unreachable` | Cannot establish SSH connection to worker | Yes |
| `xcode_not_found` | Xcode not found at expected path on worker | No |
| `xcode_version_mismatch` | Xcode version does not match constraint | No |
| `runtime_not_found` | Requested simulator runtime not installed | No |
| `uncertain_classification` | Command could not be confidently classified | No |
| `mutating_disallowed` | Mutating command refused by policy | No |
| `floating_destination_disallowed` | Floating destination (os="latest") refused by policy | No |
| `dirty_working_tree` | Working tree has uncommitted changes and require_clean=true | No |
| `timeout` | Job exceeded configured timeout_seconds | No |
| `canceled` | Job was canceled by user | No |
| `event_stream_corrupt` | Worker harness could not emit valid JSON event | No |
| `source_staging_failed` | Failed to stage source to worker | Yes |
| `artifact_collection_failed` | Failed to collect artifacts from worker | Yes |

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

The source tree hash is a critical input to `run_id` computation (see Job Identity).

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

**Hash computation:**

```
source_tree_hash = SHA-256( JCS(source_manifest.entries) )
```

Where `JCS` is the JSON Canonicalization Scheme (RFC 8785) and the result is lowercase hex.

**Normalization rules:**

- Paths MUST use `/` as separator (convert from platform-native).
- Paths MUST NOT have leading `./` or trailing `/`.
- Paths MUST be sorted lexicographically by UTF-8 byte values.
- Symlinks MUST be represented as `type: "symlink"` with hash of target path; the lane MUST NOT follow symlinks.
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

Artifacts are written under an immutable job directory on the host:

```
~/.local/share/rch/artifacts/<job_id>/
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
├── build.log              # stdout/stderr capture
└── result.xcresult/       # When tests run and xcresult is produced (or result.xcresult.tar.zst when compression enabled)
```

### `manifest.json` Requirements

- MUST include SHA-256 hashes for all material artifacts.
- MUST include byte sizes.
- SHOULD include a logical artifact type for each entry (log/json/xcresult/etc.).
- SHOULD include `kind`/`schema_version` for JSON artifacts (or inferable mapping) to aid verifiers.

Each manifest entry MUST include:
- `path` (string) — Relative path within the artifact directory.
- `sha256` (string) — Lowercase hex SHA-256 hash.
- `bytes` (integer) — Size in bytes.

Each manifest entry MAY include:
- `storage` (string) — `"host"` | `"worker"` | `"object_store"` (where the artifact is stored).
- `uri` (string|null) — For remote storage, a stable URI for retrieval.
- `compression` (string) — `"none"` | `"zstd"` (compression applied to the stored artifact).

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

### `events.ndjson` Requirements

The event stream is a newline-delimited JSON file where each line is a self-contained event object. Events are appended in real time during job execution.

**Required fields per event:**

- `type` — Event type string (e.g., `"build_started"`, `"test_case_passed"`, `"phase_completed"`, `"error"`, `"complete"`).
- `timestamp` — ISO 8601 timestamp.
- `sequence` — Monotonically increasing integer (1-based).

**Standard event types:**

| Type | Emitted when |
|------|-------------|
| `hello` | First event; echoes protocol_version, lane_version, job identity |
| `job_started` | Job execution begins on worker |
| `phase_started` | A build phase begins (compile, link, etc.) |
| `phase_completed` | A build phase ends (includes duration) |
| `test_suite_started` | A test suite begins execution |
| `test_case_passed` | A single test case passes |
| `test_case_failed` | A single test case fails (includes failure message) |
| `test_suite_completed` | A test suite finishes (includes pass/fail counts) |
| `warning` | Non-fatal issue detected |
| `error` | Fatal error during execution |
| `complete` | Final event; includes `exit_code` |

Consumers MUST tolerate unknown event types (forward compatibility).

### `status.json` Requirements

`status.json` is a mutable file updated in-place throughout job execution. It provides a polling-friendly snapshot of current job state.

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

---

## Commands

| Command | Purpose |
|---------|---------|
| `rch xcode doctor` | Validate host setup (daemon, config, SSH tooling) |
| `rch xcode verify [--profile <name>]` | Probe worker + validate config against capabilities |
| `rch xcode build [--profile <name>]` | Remote build gate |
| `rch xcode test [--profile <name>]` | Remote test gate |
| `rch xcode fetch <job_id>` | Pull artifacts (if stored remotely) |
| `rch xcode validate <job_id\|path>` | Verify artifacts: schema validation + manifest hashes + event stream integrity (+ provenance if enabled) |
| `rch xcode watch <job_id>` | Stream events + follow logs for a running job |
| `rch xcode cancel <job_id>` | Best-effort cancel (preserve partial artifacts) |
| `rch xcode gc` | Garbage-collect old runs + worker workspaces |

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
- JSON artifacts validate against their schemas (when schemas are available).
- `events.ndjson` parses as valid NDJSON and ends with a terminal `complete` event.
- `job_id`, `run_id`, `attempt` are consistent across artifacts.

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
| DerivedData | Xcode build + destination runtime + scheme/config + source_tree_hash (or run_id) |
| SwiftPM | Xcode build + resolved dependencies hash (+ optional toolchain constraints) |

### Cache Policy (Normative)

Cache usage MUST be controlled via `[profiles.<name>.cache]` and MUST be reflected in `effective_config.json`. The lane MUST record cache decisions and hit/miss stats in `metrics.json` (at minimum: derived data + SPM).

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
- The worker MUST record the created UDID in `effective_config.json`.
- The worker MUST delete the simulator device on job completion (best-effort cleanup).

When `erase_on_start = true`:
- The worker MUST erase the simulator device data before the test run begins.
- This is slower but ensures a clean state for each job.

`effective_config.json` MUST record the actual `simulator_strategy` and `simulator_udid` used.

### Concurrency Control

- Avoid thrashing by limiting concurrent jobs per worker.
- Queue jobs if worker is busy.
- Report queue position in job status.

#### Worker Leases (Normative)

To prevent resource exhaustion and ensure crash recovery, the lane uses a lease-based concurrency model:

- **Lease acquisition**: Before a job begins execution on a worker, the host MUST acquire a lease. A lease grants exclusive (or counted) access to worker job slots.
- **Lease TTL**: Every lease MUST have a time-to-live (TTL), defaulting to `timeout_seconds + 300` (5-minute grace). If the lease expires without renewal, the worker MUST consider the job abandoned.
- **Lease renewal**: The host MUST renew the lease periodically (at least every `TTL / 2` seconds) while the job is active. Renewal is a heartbeat proving the host is alive.
- **Crash recovery**: If a lease expires without graceful release:
  - Worker MUST terminate the associated job process (SIGTERM, then SIGKILL after 10s).
  - Worker MUST mark the job workspace for cleanup.
  - Host MUST detect the expired lease on reconnection and transition the job to `failed` with reason `"lease_expired"`.
- **Concurrency limit**: Each worker advertises a maximum concurrent job count (default: 1). The host MUST NOT acquire a lease if the worker is at capacity; instead, the job enters a queue.
- **`status.json` integration**: `queued_at`, `started_at`, and `queue_wait_seconds` fields (see status.json Requirements) MUST reflect actual lease acquisition timing.

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

`rch xcode retry <job_id>` MAY be provided to manually retry a failed job with `attempt` incremented. The new attempt inherits the original `run_id` if source and config are unchanged.

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
