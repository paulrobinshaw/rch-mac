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

**Protocol:**

1. Host opens an SSH connection and invokes `rch-xcode-worker`.
2. Host writes a single JSON object to stdin (the job request: effective config, source path, cache policy).
3. Harness validates the request, runs the Xcode action, and streams NDJSON events to stdout (one JSON object per line).
4. On completion, harness writes a final `{"type":"complete","exit_code":N}` event and exits.
5. Host collects events, extracts artifacts, and assembles the job directory.

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
```

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
6. If multiple matches, select the one with the highest UDID lexicographic order (deterministic tiebreak) and emit a warning.
7. Record resolved UDID, runtime version, and device type in `effective_config.json`.

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

### Requirements

- Host MUST preserve partial artifacts for non-success terminals.
- Terminal state MUST be recorded in `summary.json`.

### Timeouts + Cancellation

- Lane MUST support a per-job timeout (`timeout_seconds`).
- If a timeout triggers, terminal state MUST be `timed_out`.
- Lane SHOULD support user cancellation (best-effort SIGINT/remote termination).
- A canceled job MUST still emit `summary.json` + `manifest.json` referencing available artifacts.

---

## Safety Rules

### Interception Policy

- Lane MUST NOT run mutating commands by implicit interception (e.g., `clean`, `archive`) unless `allow_mutating = true`.
- When classification is uncertain, lane MUST prefer **not** to intercept (false negatives are acceptable).
- Default signing policy: `CODE_SIGNING_ALLOWED=NO` unless explicitly enabled.

### Decision Artifact (Normative)

Every job MUST emit a `decision.json` file in the artifact directory, recording the classification and routing decision made by the host. This supports auditability and debugging of interception behavior.

**Required fields:**

- `command_raw` — The original command string as received.
- `command_classified` — The classified action (`build`, `test`, `clean`, `archive`, `unknown`).
- `profile_used` — The profile name selected.
- `intercepted` — Boolean: whether the command was intercepted and routed to a worker.
- `refusal_reason` — If not intercepted, the reason (e.g., `"uncertain_classification"`, `"mutating_disallowed"`). Null if intercepted.
- `worker_selected` — Worker name, or null if refused.
- `timestamp` — ISO 8601 timestamp of the decision.

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

Determinism inputs SHOULD include:

- Explicit Xcode selection (path or version constraint)
- Explicit destination resolution strategy (simulator UDIDs over human names)
- Explicit signing policy (default off)

### Source Snapshot (Normative)

The `attestation.json` MUST include a `source` object capturing the state of the source tree at job creation time:

- **`vcs_commit`** — The full commit SHA of HEAD (or null if not a VCS repo).
- **`dirty`** — Boolean: true if the working tree has uncommitted changes.
- **`source_tree_hash`** — A deterministic hash of the source files sent to the worker (SHA-256 of sorted file paths + contents, excluding `.rch/`, `.git/`, and configured excludes).
- **`untracked_included`** — Boolean: whether untracked files were included in the hash and sync.

The source tree hash is a critical input to `run_id` computation (see Job Identity).

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
├── manifest.json          # Artifact index + SHA-256 hashes + byte sizes
├── decision.json          # Classification + routing decision record
├── environment.json       # Captured worker environment snapshot
├── timing.json            # Durations for stage/run/collect phases
├── events.ndjson          # Streaming event log (newline-delimited JSON)
├── status.json            # Current job status (updated in-place during execution)
├── build.log              # stdout/stderr capture
└── result.xcresult/       # When tests run and xcresult is produced
```

### `manifest.json` Requirements

- MUST include SHA-256 hashes for all material artifacts.
- MUST include byte sizes.
- SHOULD include a logical artifact type for each entry (log/json/xcresult/etc.).

### `environment.json` Requirements

- MUST include Xcode path + version + build number.
- MUST include macOS version.
- SHOULD include available simulator runtimes.
- SHOULD include Node.js version (if XcodeBuildMCP used).

### `timing.json` Requirements

- MUST include durations (seconds) for: `staging`, `running`, `collecting`.
- SHOULD include total wall-clock time.

### `events.ndjson` Requirements

The event stream is a newline-delimited JSON file where each line is a self-contained event object. Events are appended in real time during job execution.

**Required fields per event:**

- `type` — Event type string (e.g., `"build_started"`, `"test_case_passed"`, `"phase_completed"`, `"error"`, `"complete"`).
- `timestamp` — ISO 8601 timestamp.
- `sequence` — Monotonically increasing integer (1-based).

**Standard event types:**

| Type | Emitted when |
|------|-------------|
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

---

## Performance Design

### Incremental Staging

- Use rsync with excludes (`.git`, `DerivedData`, `*.xcresult`, etc.).
- Optional: git archive/clone strategy for pristine working copies.

### Cache Buckets

| Cache | Key Components |
|-------|----------------|
| DerivedData | Xcode identity + destination + repo content hash |
| SwiftPM | Xcode identity + resolved dependencies hash |

### Simulator Prewarm

- Boot simulator once, reuse across runs.
- Collect runtime info in `environment.json`.
- Avoid cold-start tax on every job.

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

---

## Milestones

- **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
- **M1**: `doctor` + worker capability probe + config validation (`verify`)
- **M2**: Classifier + policy: allowlist routing, refuse-on-uncertain, explain decisions
- **M3**: Workspace sync + remote runner MVP (fallback `xcodebuild`) with log streaming
- **M4**: XcodeBuildMCP backend with structured events (build phases, test events)
- **M5**: Determinism outputs: summary.json, effective_config.json, attestation.json, manifest.json, environment.json, timing.json
- **M5.1**: Decision artifact (`decision.json`) + event stream (`events.ndjson`) + live status (`status.json`)
- **M5.2**: Source snapshot in attestation + source policy enforcement
- **M6**: Caching + performance: DerivedData/SPM caches, incremental sync, simulator prewarm, concurrency control
- **M6.1**: Worker leases + crash recovery + queue-wait metrics
- **M7**: Ops hardening: timeouts, cancellation, worker harness (`rch-xcode-worker`), partial artifact preservation
- **M7.1**: Optional provenance (signed attestation + manifest)
- **M7.2**: Optional CI reports (`junit.xml`, `test_summary.json`)
- **M8**: Compatibility matrix + fixtures: golden configs, sample repos, reproducible failure cases

---

## Policies Summary

| Policy | Default | Override |
|--------|---------|----------|
| Code signing | `CODE_SIGNING_ALLOWED=NO` | `code_signing_allowed = true` |
| Mutating commands | Disallowed | `allow_mutating = true` |
| Uncertain classification | Refuse (false negative) | N/A |
| Floating destination (CI) | Disallowed | `allow_floating_destination = true` |
| Dirty working tree (CI) | Disallowed | `require_clean = false` |
| Worker user | Dedicated account | Operator responsibility |
| Provenance signing | Disabled | `[profiles.<name>.provenance] enabled = true` |

---

## Next Steps

1. Bring Mac mini worker online
2. Implement `rch xcode doctor` and `rch xcode verify`
3. Add classifier + routing + refusal/explanation paths + decision artifact
4. Implement workspace sync + remote runner + log streaming + worker harness
5. Add XcodeBuildMCP backend + event stream
6. Emit determinism artifacts + source snapshot
7. Add caching + performance optimizations + worker leases
8. Add optional provenance + CI reports
