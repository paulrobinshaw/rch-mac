Below are the strongest revisions I’d make to tighten the contract, improve determinism + reliability, reduce operator footguns, and make the lane more “CI-grade” (without expanding scope into signing/export/publishing).

I’ve kept each proposal as: **what/why** + a **git-diff** against your current README/PLAN text.

---

## 1) Make the operator story explicit: lane boundaries, “conservative classifier”, and config precedence

### Why this makes it better

Right now the README implies “intercept + remote” but doesn’t clearly set expectations for:

* **What happens to non-matching invocations** (reject vs passthrough)
* The lane’s **conservatism** (false negatives are expected/desired)
* **Config precedence**, which is the #1 source of “why did it pick that worker/destination?” confusion

This change reduces ambiguity, improves day-2 operations, and makes the system easier to reason about for both humans and agents.

### Diff — `README.md`

````diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## What it is
 An extension to Remote Compilation Helper (RCH) that offloads Xcode build/test to a remote macOS worker (e.g. a Mac mini).
 Execution can use either:
 - **Backend `xcodebuild` (MVP)**, or
 - **Backend `mcp` (preferred)** via XcodeBuildMCP for richer diagnostics/orchestration.
 
+## Status
+- **MVP**: `backend="xcodebuild"` remote build/test with deterministic JobSpecs + artifact sets.
+- **Preferred**: `backend="mcp"` when XcodeBuildMCP is installed on the worker.
+- The classifier is intentionally conservative: **false negatives are OK**.
+
+## How it fits into RCH (lane boundaries)
+- `rch xcode …` is a **lane** that builds a deterministic JobSpec, ships inputs to macOS, and materializes artifacts locally.
+- Only invocations accepted by the classifier are offloaded.
+- Non-matching invocations are either:
+  - **rejected** (with an explanation), or
+  - **not intercepted** (depending on subcommand and host capabilities).
+
+## Config precedence (high level)
+1. Repo config: `.rch/xcode.toml`
+2. Host config: `~/.config/rch/*` (workers, credentials, defaults)
+3. CLI flags / explicit overrides (highest precedence at runtime)
+
@@
 ## Quickstart
 Most common flows:
 
 ```bash
 # Validate setup without executing anything
 rch xcode verify --dry-run
 
 # Explain why a command will/won't be intercepted
 rch xcode explain -- xcodebuild test -workspace MyApp.xcworkspace -scheme MyApp
 
+# Run a single action as a one-step run (bypasses repo verify plan)
+rch xcode run --action test
+
 # Run the repo-defined verify lane (usually build+test)
 rch xcode verify
````

@@
Useful commands:

* `rch xcode explain -- <command...>`  (why it will/won't be intercepted)
* `rch xcode verify --dry-run`         (prints resolved plan + selected worker)
  +- `rch xcode run --action <build|test>`(run one step as its own run)
* `rch xcode tail <run_id|job_id>`     (stream logs/events while running)
* `rch xcode cancel <run_id|job_id>`   (best-effort cancellation)
* `rch xcode artifacts <run_id|job_id>`(print artifact locations + key files)
* `rch workers list --tag macos,xcode` (show matching workers)
* `rch workers probe <name>`           (fetch capabilities snapshot)
* `rch xcode doctor`                   (validate config, SSH, Xcode, destination)

````

---

## 2) Nail determinism at the “hash boundary”: canonical JSON for `job_key` + a lightweight JobSpec schema outline

### Why this makes it better
You already say `job_key` is derived from “effective_config + sanitized_invocation + toolchain”, but **you don’t define the byte-level canonicalization**. If any part is serialized differently (key order, whitespace, floats, etc.), you lose cross-platform determinism and cache hit rates.

Defining a canonical JSON scheme for hashing makes the system:
- reproducible across implementations/languages
- cache-friendly
- signature-friendly (attestations become robust)

Also: giving a **minimal JobSpec field outline** makes the spec easier to implement faithfully without accidentally relying on ambient defaults.

### Diff — `README.md` (docs map: schemas pointer)
```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Docs map
 - `PLAN.md` — **normative** contract (classifier, JobSpec, protocol, artifacts, caching, security)
 - `.rch/xcode.toml` — repo-scoped configuration (checked in)
 - `~/.config/rch/workers.toml` — host-scoped worker inventory + credentials
+- `schemas/rch-xcode/*` — JSON Schemas (recommended) for JobSpec + artifacts
````

### Diff — `PLAN.md` (canonical hashing + JobSpec outline + timestamp convention)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Deterministic JobSpec + Job Key
 Each remote run is driven by a `job.json` (JobSpec) generated on the host.
 The host computes:
 - `source_sha256` — SHA-256 of the sent source bundle (after canonicalization)
-- `job_key` — SHA-256 over: `source_sha256 + effective_config + sanitized_invocation + toolchain`
+- `job_key` — SHA-256 over a canonical JSON object containing:
+  - `source_sha256`
+  - `effective_config`
+  - `sanitized_invocation`
+  - `toolchain`
+  Canonicalization MUST use RFC 8785 (JSON Canonicalization Scheme, UTF-8 bytes) so different implementations compute the same hash.
 Artifacts include both values, enabling reproducible reruns and cache keys.
+
+### Timestamp + encoding conventions (normative)
+- All `created_at` timestamps MUST be RFC3339 UTC (e.g. `2026-02-10T12:34:56Z`).
+- All JSON MUST be UTF-8.
+
+### JobSpec schema outline (v1) (normative)
+`job.json` MUST be fully-resolved (no ambient defaults) and MUST include at minimum:
+- Identity: `schema_version`, `created_at`, `run_id`, `job_id`, `job_key`
+- Action: `action` (`build`|`test`)
+- Backend: `backend` (`xcodebuild`|`mcp`)
+- Project: `workspace` or `project`, `scheme`, optional `configuration` (only if allowlisted/pinned)
+- Destination: `destination_resolved` (exact string used by the worker), and if applicable `destination_input`
+- Toolchain: resolved `DEVELOPER_DIR` (or equivalent), Xcode build number, macOS major/minor
+- Bundle: `source_sha256`, `source_manifest_sha256`, `bundle_mode`
+- Timeouts: `overall_seconds`, `idle_log_seconds`
+- Cache: `cache_namespace`, `derived_data_mode`, `spm_mode`
+- Invocation: `sanitized_argv` (the actual argv executed on the worker)
+
@@
 ## Artifact schemas + versioning
 All JSON artifacts MUST include:
 - `schema_version`
 - `created_at`
@@
 Job-scoped artifacts MUST include:
 - `run_id`
 - `job_id`
 - `job_key`
+
+### JSON Schemas (recommended)
+The project SHOULD ship JSON Schemas for JobSpec and each artifact under `schemas/rch-xcode/` (versioned per `schema_version`),
+so agents and external tooling can validate outputs.
```

---

## 3) Add an explicit run/job state machine + incremental state artifacts for crash-safe tail/cancel

### Why this makes it better

`tail` and `cancel` are where systems get flaky: daemon restarts, SSH drops, partial logs, “is it still running?”, etc.

A small, explicit **state machine** + an **incrementally updated state artifact** makes:

* `tail` reliable (even if the daemon restarts)
* cancellation auditable (request recorded immediately)
* post-mortems easier (you can see transitions)

### Diff — `PLAN.md` (state machine + artifacts)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### Job lifecycle + idempotency
 Worker MUST treat `job_id` as the idempotency key:
@@
 ### Cancellation
 - Host MUST be able to request cancellation.
 - Worker MUST attempt a best-effort cancel (terminate backend process tree) and write artifacts with `status=failed`
   and `failure_kind=CANCELLED`.
+
+## Run + Job state machine (normative)
+In addition to final `summary.json`, the system MUST maintain incremental state for reliable `tail`/`cancel`.
+
+### States
+- Job states: `QUEUED` → `RUNNING` → (`SUCCEEDED` | `FAILED` | `REJECTED` | `CANCELLED`)
+- Cancellation introduces `CANCEL_REQUESTED` as an intermediate state.
+
+### State artifacts
+- Host MUST write `run_state.json` under the run root and update it atomically on each transition.
+- Host MUST write `job_state.json` under each step job directory and update it atomically on each transition.
+- State artifacts MUST include: `schema_version`, `created_at`, `run_id`, and (for job) `job_id`, plus:
+  - `state`, optional `state_reason`
+  - `last_event_cursor` (if tail/events are supported)
+  - `last_log_offset_bytes` (if byte-offset tailing is supported)
 
@@
 ## Artifacts
 - run_summary.json
+- run_state.json
 - summary.json
 - attestation.json
 - manifest.json
 - effective_config.json
 - job.json
+- job_state.json
 - invocation.json
 - toolchain.json
 - metrics.json
 - source_manifest.json
 - worker_selection.json
 - events.jsonl (recommended)
```

### Diff — `README.md` (surface the new artifacts in the layout)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 Layout (example):
 - `run_summary.json`
+- `run_state.json`
 - `worker_selection.json`
 - `capabilities.json`
 - `steps/build/<job_id>/...`
 - `steps/test/<job_id>/...`
@@
 Includes:
 - run_summary.json
+- run_state.json
 - summary.json
 - attestation.json
 - manifest.json
 - effective_config.json
 - job.json
+- job_state.json
 - invocation.json
 - toolchain.json
 - metrics.json
 - source_manifest.json
 - worker_selection.json
```

---

## 4) Make toolchain pinning first-class: multi-Xcode support + deterministic `DEVELOPER_DIR` resolution

### Why this makes it better

You already warn about silent updates and `DEVELOPER_DIR`, but the contract doesn’t fully encode:

* multiple Xcodes installed on one worker
* selecting a specific Xcode by **build number**
* ensuring the worker actually ran under the selected toolchain

This change makes “pinned Xcode conditions” real, not aspirational.

### Diff — `README.md` (repo config example: toolchain section)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 [project]
 workspace = "MyApp.xcworkspace" # or project = "MyApp.xcodeproj"
 scheme = "MyApp"
 
+[toolchain]
+mode = "constraints"         # pinned | constraints
+# Prefer pinning by Xcode build number for stability (avoids silent updates).
+xcode_build = "16E000"       # example placeholder
+# Optional: constrain macOS major version when relevant
+macos_major = 15
+
 [actions]
 verify = ["build", "test"]
```

### Diff — `PLAN.md` (capabilities + selection)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Worker capabilities
 Worker reports a `capabilities.json` including:
-- Xcode version(s) + build number, `DEVELOPER_DIR`
+- Installed Xcodes (array), each with: version, build number, app path, and corresponding `DEVELOPER_DIR`
+- Current default `DEVELOPER_DIR` (for informational purposes only; jobs MUST use the resolved toolchain)
 - macOS version + architecture
@@
 ## Worker selection (normative)
@@
 Selection inputs:
 - required tags: `macos,xcode` (and any repo-required tags)
-- constraints: Xcode version/build, platform (iOS/macOS), destination availability
+- constraints: Xcode build (preferred), Xcode version, macOS major, destination availability
@@
 Selection algorithm (default):
 1. Filter by required tags.
 2. Probe or load cached `capabilities.json` snapshots (bounded staleness).
-3. Filter by constraints (destination exists, required Xcode available).
+3. Filter by constraints (destination exists, required toolchain available).
 4. Sort deterministically by:
    - explicit worker priority (host config)
    - then stable worker name
 5. Choose first.
+
+### Toolchain resolution (normative)
+For a chosen worker, the host MUST resolve the concrete toolchain (specific Xcode install / `DEVELOPER_DIR`)
+and record it in:
+- `toolchain.json` (artifact)
+- `job.json` (JobSpec)
+The worker MUST execute under that resolved `DEVELOPER_DIR`.
```

---

## 5) Turn the bundle store into an actual protocol feature: `has_source` + `upload_source`

### Why this makes it better

You describe the content-addressed store, but the protocol doesn’t expose a clean, explicit handshake.

Making `has_source` and `upload_source` explicit:

* drastically improves performance on repeat runs
* reduces “protocol implied by SSH scripts”
* makes the lane easier to implement consistently (and test)

### Diff — `PLAN.md` (protocol ops)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 Request:
 - `protocol_version` (int)
-- `op` (string: probe|submit|status|tail|cancel|fetch)
+- `op` (string: probe|has_source|upload_source|submit|status|tail|cancel|fetch)
 - `request_id` (string, caller-chosen)
 - `payload` (object)
@@
 Worker SHOULD implement these operations with JSON request/response payloads:
 - `probe` → returns `capabilities.json`
+- `has_source` → checks whether `source_sha256` exists in the worker bundle store
+- `upload_source` → accepts the canonical bundle bytes (optionally compressed) and stores under `source_sha256`
 - `submit` → accepts `job.json` (+ optional bundle upload reference), returns ACK and initial status
 - `status` → returns current job status and pointers to logs/artifacts
 - `tail` → streams logs/events with a cursor
 - `cancel` → requests best-effort cancellation
 - `fetch` → returns artifacts (or a signed manifest + download hints)
@@
 ## Source bundle store (performance, correctness-preserving)
 Workers SHOULD maintain a content-addressed store keyed by `source_sha256`.
 
 Protocol expectations:
-- Host MAY query whether the worker already has `source_sha256`.
-- If present, host SHOULD skip re-uploading the bundle and submit only `job.json`.
-- If absent, host uploads the canonical bundle once; worker stores it under `source_sha256`.
+- Host SHOULD call `has_source` before uploading.
+- If present, host SHOULD skip re-uploading and proceed to `submit`.
+- If absent, host SHOULD call `upload_source` exactly once per `source_sha256`, then `submit`.
```

---

## 6) Strengthen the security knobs that prevent accidental footguns: env allowlist (normative) + concrete SSH forced-command example

### Why this makes it better

You already state the key truth: **this isn’t a sandbox**. The practical next step is ensuring the lane doesn’t *accidentally* make things worse by:

* leaking secrets via env/logs
* allowing broader SSH access than intended

Two high-leverage tweaks:

* make **env allowlisting** part of the normative executor contract
* show a copy/paste-ready **authorized_keys** restriction

### Diff — `PLAN.md` (env allowlist becomes required)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 Recommended mitigations:
-- Executor SHOULD use an environment-variable allowlist and redact obvious secrets in logs/artifacts.
+- Executor MUST use an environment-variable allowlist (start from a clean env and only pass allowlisted vars).
+- Executor SHOULD redact obvious secrets in logs/artifacts (best-effort).
 - Worker SHOULD avoid unlocking or accessing user keychains during execution.
```

### Diff — `README.md` (forced-command example)

````diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ### Hardening recommendations
@@
 - Prefer an env allowlist for the executor (only pass through known-safe vars), and redact secrets from logs.
 - Consider running the worker user with reduced permissions (no admin), and keep artifacts + caches on a dedicated volume.
+
+#### Example: SSH authorized_keys restriction (recommended)
+On the worker, restrict the key used by the host:
+```text
+command="/usr/local/bin/rch-worker xcode rpc",no-pty,no-agent-forwarding,no-port-forwarding,no-X11-forwarding ssh-ed25519 AAAA...
+```
````

---

## 7) CI/agent friendliness: emit `junit.xml` for test jobs (recommended) + document it

### Why this makes it better

You already have `test_summary.json` (great for agents), but CI ecosystems still revolve around JUnit XML.

Adding `junit.xml` as a recommended artifact:

* makes adoption easier (GitHub Actions, Buildkite, Jenkins, etc.)
* reduces “write your own converter” friction
* stays in-scope (it’s derived from xcresult / logs, not signing/export)

### Diff — `PLAN.md` (artifact + summary)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Agent-friendly summaries (recommended)
 In addition to `summary.json`, the worker SHOULD emit:
 - `test_summary.json` (counts, failing tests, duration, top failures)
 - `build_summary.json` (targets, warnings/errors counts, first error location if available)
 These MUST be derived from authoritative sources (`xcresult` when present; logs as fallback).
+
+Additionally, for test jobs the worker SHOULD emit:
+- `junit.xml` (CI-friendly test report derived from `xcresult` when present)
@@
 ## Artifacts
@@
 - test_summary.json (recommended)
+- junit.xml (recommended for test jobs)
 - build_summary.json (recommended)
```

### Diff — `README.md` (mention it in outputs)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 Includes:
@@
 - test_summary.json (recommended)
+- junit.xml (recommended for CI)
 - build_summary.json (recommended)
```

---

## 8) Add a small conformance/regression suite to keep determinism real over time

### Why this makes it better

This project’s core value proposition is *deterministic, auditable remote Xcode gating*. That’s brittle unless you lock in a few “never regress” tests.

A conformance suite pays for itself by preventing:

* accidental bundler nondeterminism across platforms
* protocol incompatibilities
* classifier allowlist drift (security regression)

### Diff — `PLAN.md` (new conformance section + milestone)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Milestones
 - **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
 - **M1**: Classifier detects Xcode build/test safely
 - **M2**: MVP remote execution with `xcodebuild`
 - **M3**: Switch to XcodeBuildMCP backend
 - **M4**: Emit summary.json, attestation.json, manifest.json
 - **M5**: Remote caching (DerivedData, SPM) and performance tuning
 - **M6**: Worker capability handshake + deterministic worker selection
+- **M7**: Conformance/regression suite (bundling determinism, protocol compatibility, classifier fixtures)
+
+## Conformance / regression tests (recommended)
+The project SHOULD include a small suite that runs in CI:
+- **Bundler determinism fixture**: identical inputs on Linux/macOS yield identical `source_sha256`.
+- **Classifier fixtures**: allowlist/reject cases with stable `rejection_reason` outputs.
+- **Protocol compatibility**: host↔worker protocol version mismatch produces `failure_kind=WORKER_INCOMPATIBLE`.
+- **Artifact schema validation**: emitted artifacts validate against `schemas/rch-xcode/*` (when present).
```

---

If you want one more “high impact” follow-on, I’d do **a dedicated section in PLAN.md that specifies the *exact* `capabilities.json` shape (v1)** (at least the toolchain + sim runtime parts). That’s the piece that tends to drift and break destination/toolchain resolution unless it’s nailed down early.
