Below are the highest-leverage revisions I’d make to improve **determinism**, **cache utility**, **security posture**, and **operational robustness**, while keeping the lane’s scope tight (build/test gate only). For each change: rationale + a patch **against the original** README/PLAN you pasted.

---

## 1) Make config merge + “what affects job_key” explicit (and stop host-only config from poisoning cache keys)

### Why this makes it better

Right now, `job_key` is defined as hashing `effective_config`, but `effective_config` (as described in README) can include host-only operational details (worker inventory paths, timeouts, cache toggles, etc.) that **should not invalidate correctness-preserving caches** and can leak irrelevant host information into deterministic keys.
Fix: define **normative merge semantics**, emit `effective_config.json` for audit (redacted), and introduce a **canonical `job_key_inputs` object** that is *the only thing* hashed for `job_key`.

### Patch

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Config precedence
 Configuration is merged in this order (last wins):
 1. Built-in defaults (hardcoded in the RCH client)
 2. Host/user config (`~/.config/rch/`)
 3. Repo config (`.rch/xcode.toml`)
 4. CLI flags (e.g. `--action`, `--worker`)
 
-`effective_config.json` is emitted per job showing the final merged result.
+Config precedence + merge semantics are **normative** (see `PLAN.md` → “Configuration merge”).  
+`effective_config.json` is emitted per job showing the final merged result (with secrets redacted).
@@
 Includes:
 - run_summary.json
 - run_state.json
 - summary.json
 - attestation.json
 - manifest.json
 - effective_config.json
 - job.json
+- job_key_inputs.json
 - job_state.json
 - invocation.json
 - toolchain.json
 - metrics.json
 - source_manifest.json
 - worker_selection.json
@@
 ## Notes
 - Designed as a build/test gate, not a full IDE replacement
 - Safe-by-default: avoids intercepting setup or mutating commands
-- Deterministic: runs produce a JobSpec (`job.json`) and stable `job_key` used for caching and attestation
+- Deterministic: runs produce a JobSpec (`job.json`) plus the exact `job_key_inputs.json` used to compute a stable `job_key` for caching/attestation
 - Security posture: prefer a dedicated `rch` user; optionally use SSH forced-command; avoid signing/publishing workflows
 - Integrity: host verifies `manifest.json` digests; attestation binds worker identity + artifact set
```

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Terminology
@@
-- **Job key** (`job_key`): stable hash used for caching and attestation. Computed using RFC 8785 (JSON Canonicalization Scheme) over the JobSpec fields that affect output (see "Job key computation" below).
+- **Job key** (`job_key`): stable hash used for caching and attestation. Computed using RFC 8785 (JSON Canonicalization Scheme) over a canonical **job key inputs** object (`job_key_inputs`) containing only output-affecting, fully-resolved fields (see "Job key computation" below).
@@
 ### JobSpec schema outline (v1)
 The canonical JobSpec (`job.json`) MUST include at least these fields:
 
 | Field               | Type     | Description                                      |
 |---------------------|----------|--------------------------------------------------|
 | `schema_version`    | int      | Always `1` for this version                      |
 | `run_id`            | string   | Parent run identifier                            |
 | `job_id`            | string   | Unique step job identifier                       |
-| `job_key`           | string   | Deterministic cache/attestation key              |
-| `source_sha256`     | string   | Digest of the canonical source bundle            |
 | `action`            | string   | `build` or `test`                                |
-| `sanitized_argv`    | string[] | Canonical xcodebuild arguments                   |
-| `toolchain`         | object   | `{xcode_version, xcode_build, developer_dir}`   |
-| `destination`       | object   | Resolved destination (platform, device, OS)      |
+| `job_key_inputs`    | object   | Canonical key-object hashed to produce `job_key` |
+| `job_key`           | string   | SHA-256 hex digest of JCS(`job_key_inputs`)      |
 | `effective_config`  | object   | Merged repo + host config snapshot               |
 | `created_at`        | string   | RFC 3339 UTC timestamp                           |
 
 ### Job key computation (normative)
-`job_key` is the SHA-256 hex digest of the RFC 8785 JSON Canonicalization of a key-object containing:
-- `source_sha256`
-- `effective_config` (merged, fully resolved)
-- `sanitized_argv`
-- `toolchain` (Xcode build number, DEVELOPER_DIR)
+`job_key_inputs` MUST be an object containing the fully-resolved, output-affecting inputs for the job, and MUST include at least:
+- `source_sha256` (digest of the canonical source bundle)
+- `sanitized_argv` (canonical xcodebuild arguments; no output-path overrides)
+- `toolchain` (at minimum: Xcode build number + `developer_dir`)
+- `destination` (resolved destination details; may also be present inside `sanitized_argv`)
+
+`job_key` is the SHA-256 hex digest of the RFC 8785 JSON Canonicalization (JCS) of `job_key_inputs`.
+
+`job_key_inputs` MUST NOT include host-only operational settings that should not invalidate correctness-preserving caches,
+including (non-exhaustive): timeouts, worker inventory/SSH details, cache toggles, worker selection metadata, backend selection.
@@
 ## Deterministic JobSpec + Job Key
 Each remote run is driven by a `job.json` (JobSpec) generated on the host.
 The host computes:
-- `source_sha256` — SHA-256 of the sent source bundle (after canonicalization)
-- `job_key` — SHA-256 of the RFC 8785 (JCS) canonicalization of the key-object (see "Job key computation" above)
+- `source_sha256` — SHA-256 of the canonical source bundle bytes
+- `job_key_inputs` — canonical, output-affecting inputs (see "Job key computation")
+- `job_key` — SHA-256 of JCS(`job_key_inputs`)
 Artifacts include both values, enabling reproducible reruns and cache keys.
+
+The host MUST emit a standalone `job_key_inputs.json` artifact (byte-for-byte identical to the `job_key_inputs` object embedded in `job.json`)
+to make cache/attestation inputs directly inspectable.
@@
-## Configuration
-- Repo-scoped config: `.rch/xcode.toml` (checked in)
-- Host/user config: `~/.config/rch/*` (workers, credentials, defaults)
-`effective_config.json` MUST be emitted per job (post-merge, fully resolved).
+## Configuration merge (normative)
+### Sources + precedence (last wins)
+1. Built-in lane defaults
+2. Host/user config (`~/.config/rch/…`)
+3. Repo config (`.rch/xcode.toml`)
+4. CLI flags
+
+### Merge semantics
+- Config MUST be decoded into a JSON-compatible object model (maps, arrays, scalars).
+- Objects MUST deep-merge by key.
+- Arrays MUST replace (no concatenation) to avoid host-dependent ordering surprises.
+- Scalars MUST override.
+- Merge MUST be deterministic.
+
+### `effective_config.json` (audit, not a cache key)
+`effective_config.json` MUST be emitted per job and MUST:
+- include `schema_version`, `created_at`, `run_id`, `job_id`
+- include the merged config object
+- record the contributing sources (origin + optional file path + a digest of raw bytes)
+- redact secrets (private keys, tokens, passwords). Any redaction MUST be recorded in a `redactions[]` list.
+
+`effective_config` MUST NOT be used for `job_key` computation (only `job_key_inputs` is hashed).
@@
 ## Artifacts
 - run_summary.json
 - run_state.json
 - summary.json
 - attestation.json
 - manifest.json
 - effective_config.json
+- job_key_inputs.json
 - job.json
 - job_state.json
 - invocation.json
 - toolchain.json
 - metrics.json
 - source_manifest.json
 - worker_selection.json
```

---

## 2) Fix cancellation semantics mismatch (state machine says CANCELLED, but `summary.status` can’t be cancelled)

### Why this makes it better

You currently define **run/job states including `CANCELLED`**, but `summary.json.status` omits `cancelled` and the cancellation section says “write `status=failed`”. That makes downstream automation messy (“was it a failure or a cancel?”).

### Patch

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Failure taxonomy
 `summary.json` MUST include:
-- `status`: success | failed | rejected
+- `status`: success | failed | rejected | cancelled
 - `failure_kind`: CLASSIFIER_REJECTED | SSH | TRANSFER | EXECUTOR | XCODEBUILD | MCP | ARTIFACTS | CANCELLED | WORKER_INCOMPATIBLE | BUNDLER | ATTESTATION | WORKER_BUSY
@@
 ### Cancellation
 - Host MUST be able to request cancellation.
 - Worker MUST attempt a best-effort cancel (terminate backend process tree) and write artifacts with `status=failed`
-  and `failure_kind=CANCELLED`.
+  and `failure_kind=CANCELLED`.
+
+On cancellation, `summary.json` MUST set:
+- `status=cancelled`
+- `failure_kind=CANCELLED`
+- `exit_code=80`
```

---

## 3) Add a normative `run_plan.json` (and define stable job_id allocation + resume semantics)

### Why this makes it better

You describe “a run may contain multiple step jobs” but you don’t currently define a **single authoritative run plan artifact**. That plan is the lynchpin for:

* daemon restarts / resume
* idempotent resubmits (reusing the same `job_id`)
* clear operator introspection (“what exactly will verify do?”)

### Patch

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Mental model (operator view)
@@
-- `rch xcode verify` is a **run** that may contain multiple **step jobs** (e.g. `build` then `test`).
+- `rch xcode verify` is a **run** that may contain multiple **step jobs** (e.g. `build` then `test`).
+- The host persists a `run_plan.json` up front so runs can be resumed after interruption.
@@
 Layout (example):
 - `run_summary.json`
+- `run_plan.json`
 - `run_state.json`
 - `worker_selection.json`
 - `capabilities.json`
@@
 Includes:
 - run_summary.json
+- run_plan.json
 - run_state.json
```

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Terminology
@@
 - **Run**: a top-level verification attempt (e.g. `rch xcode verify`) that may include multiple jobs.
@@
 - **Job ID** (`job_id`): unique identifier for a single step job within a run (used for cancellation/log streaming and artifact paths).
+- **Run plan** (`run_plan.json`): persisted, ordered list of step jobs (actions + allocated `job_id`s) for a run.
@@
 ## Architecture (high level)
 Pipeline stages:
 1. **Classifier**: detects safe, supported Xcode build/test invocations (deny-by-default).
-2. **Run builder**: resolves repo `verify` actions into an ordered run plan and chooses a worker once.
+2. **Run builder**: resolves repo `verify` actions into an ordered run plan, allocates stable `job_id`s, persists `run_plan.json`, and chooses a worker once.
@@
 ## Run + Job state machine (normative)
@@
 ### State artifacts
 - `run_state.json`: persisted at `<run_id>/run_state.json` with fields: `run_id`, `state`, `updated_at`, `schema_version`.
 - `job_state.json`: persisted at `<run_id>/steps/<action>/<job_id>/job_state.json` with fields: `job_id`, `run_id`, `state`, `updated_at`, `schema_version`.
+
+### Run plan artifact (normative)
+The host MUST emit `run_plan.json` at `<run_id>/run_plan.json` before starting execution.
+It MUST include at least:
+- `schema_version`, `created_at`, `run_id`
+- `steps`: ordered array of `{ index, action, job_id }`
+
+`run_plan.json` is the authoritative source for which `job_id`s belong to a run. If the daemon restarts,
+it MUST be able to resume by reading `run_plan.json` and reusing the same `job_id`s (preserving worker idempotency guarantees).
@@
 ## Artifacts
 - run_summary.json
+- run_plan.json
 - run_state.json
@@
```

---

## 4) Make `tail` compatible with the “single request/response” RPC envelope (cursor-based chunking)

### Why this makes it better

You say RPC is “one JSON request stdin / one JSON response stdout” *and* you say “tail streams logs.” Those two conflict unless “tail” is defined as **repeatable chunk fetch** with a cursor (which is also easier to implement over SSH forced-command).

### Patch

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### Worker RPC surface (recommended, maps cleanly to SSH forced-command)
 Worker SHOULD implement these operations with JSON request/response payloads:
@@
-- `tail` → streams logs/events with a cursor
+- `tail` → returns the next chunk of logs/events given a cursor (repeatable; host loops to “stream”)
@@
 ### Log streaming (recommended)
 - Worker SHOULD support a "tail" mode so host can stream logs while running.
 - If not supported, host MUST still periodically fetch/append logs to avoid silent hangs.
+
+`tail` MUST be defined as cursor-based chunk retrieval compatible with the single request/response envelope:
+- Request payload SHOULD include: `job_id`, `cursor` (nullable), and optional limits (`max_bytes`, `max_events`)
+- Response payload SHOULD include: `next_cursor` (nullable), plus either/both:
+  - `log_chunk` (UTF-8 text) and/or
+  - `events` (array of event objects or JSONL strings)
```

---

## 5) Require worker-side verification of `job_key` + `source_sha256` (prevents “artifact confusion” and subtle cache poisoning)

### Why this makes it better

You already require rejecting when `job_id` exists with a different `job_key`, but you don’t require the worker to **recompute** `job_key` from the canonical inputs. That’s an easy hardening win:

* catches buggy hosts
* prevents tampering between host build + submit
* reduces “wrong cache entry” risk

### Patch

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### Job lifecycle + idempotency
 Worker MUST treat `job_id` as the idempotency key:
@@
 - If `job_id` already exists but the submitted `job_key` differs, worker MUST reject to prevent artifact confusion.
+
+On `submit`, worker MUST validate:
+- `job_key` matches SHA-256(JCS(`job_key_inputs`)) as defined in "Job key computation"
+- `job_key_inputs.source_sha256` matches the stored source bundle digest for the bundle used (uploaded or previously present)
+If validation fails, worker MUST fail the job with `failure_kind=PROTOCOL_ERROR` (or a more specific subkind) and emit diagnostics.
```

---

## 6) Capabilities snapshot staleness needs a TTL + timestamps (otherwise “bounded staleness” is underspecified)

### Why this makes it better

You say “probe or load cached capabilities (bounded staleness)” but don’t define how staleness is bounded. Add:

* `created_at` in `capabilities.json`
* a host-side TTL policy
* record whether cached snapshot or fresh probe was used

### Patch

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Worker capabilities
 Worker reports a `capabilities.json` including:
@@
 - capacity (max concurrent jobs, disk free)
+Normative:
+- `capabilities.json` MUST include `schema_version` and `created_at` (RFC 3339 UTC).
 
 ## Worker selection (normative)
@@
 Selection algorithm (default):
 1. Filter by required tags.
-2. Probe or load cached `capabilities.json` snapshots (bounded staleness).
+2. Probe or load cached `capabilities.json` snapshots (bounded staleness).
 3. Filter by constraints (destination exists, required Xcode available).
@@
 The host MUST write:
 - `worker_selection.json` (inputs, filtered set, chosen worker, reasons)
 - `capabilities.json` snapshot as used for the decision
+
+Staleness policy (normative):
+- Host MUST define a TTL for cached capability snapshots (e.g. `probe_ttl_seconds`).
+- If a cached snapshot is older than TTL, host MUST re-probe before selecting a worker.
+- `worker_selection.json` MUST record whether the snapshot was cached or freshly probed and the snapshot age.
```

---

## 7) Define a backend contract so `xcodebuild` vs `mcp` doesn’t fragment artifacts and tooling

### Why this makes it better

If the backend changes the artifact surface area, your caching + automation become brittle. A small “backend contract” section ensures:

* consistent minimum artifact set
* identical semantics for `summary.json`, `manifest.json`, `xcresult` location, etc.
* “MVP xcodebuild” can evolve without breaking consumers

### Patch

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## What it is
 An extension to Remote Compilation Helper (RCH) that offloads Xcode build/test to a remote macOS worker (e.g. a Mac mini).
 Execution can use either:
 - **Backend `xcodebuild` (MVP)**, or
 - **Backend `mcp` (preferred)** via XcodeBuildMCP for richer diagnostics/orchestration.
+Both backends MUST emit the same **minimum artifact contract** (see `PLAN.md` → “Backend contract”).
```

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Backends
 - **Backend: xcodebuild (MVP)** — minimal dependencies, fastest path to correctness.
 - **Backend: XcodeBuildMCP (preferred)** — richer structure, better diagnostics, multi-step orchestration.
+
+## Backend contract (normative)
+Regardless of backend, the worker MUST:
+- execute the action described by `job.json` under the resolved toolchain + destination
+- write the normative artifacts (`summary.json`, `manifest.json`, `attestation.json`, `job_state.json`, logs)
+- control output paths (host/user args MUST NOT override artifact locations)
+
+Minimum artifact expectations (normative):
+- `build.log` MUST be present for all jobs
+- `result.xcresult/` MUST be present for `test` jobs (backend may generate via `-resultBundlePath`)
+- `summary.json` MUST include backend identity (`backend=...`) and a stable exit_code mapping
```

---

## 8) Promote env allowlist + per-job isolation from “recommended” to “normative” (it’s essential to your threat model)

### Why this makes it better

You correctly warn “not a sandbox,” but you can still materially reduce accidental secret leakage and cross-job interference by requiring:

* per-job working directory
* env allowlist (drop unknown vars)
* log redaction obligations

This is low complexity and high impact.

### Patch

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Threat model / security notes
@@
 Recommended mitigations:
-- Executor MUST use an environment-variable allowlist and pass through only known-safe variables. Obvious secrets MUST be redacted in logs/artifacts.
+- Executor MUST use an environment-variable allowlist and pass through only known-safe variables. Obvious secrets MUST be redacted in logs/artifacts.
 - Worker SHOULD avoid unlocking or accessing user keychains during execution.
+
+## Execution environment (normative)
+- Worker MUST execute each job in an isolated working directory unique per `job_id`.
+- Worker MUST set `DEVELOPER_DIR` to the resolved toolchain `developer_dir` prior to execution.
+- Worker MUST apply an environment allowlist (drop-by-default) when launching the backend.
+- Worker MUST redact secrets from logs/artifacts to the extent feasible (at minimum: do not emit env vars outside the allowlist).
```

---

## 9) Make the “mock worker” a first-class milestone so conformance tests don’t require a real Mac

### Why this makes it better

You already say “tests runnable without a live worker (use mocks/fixtures)”—great. Make it explicit in milestones so the project can ship a reliable spec-driven core *before* wrangling macOS edge cases.

### Patch

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Milestones
 - **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
 - **M1**: Classifier detects Xcode build/test safely
+- **M1.5**: Mock worker implements protocol ops (probe/submit/status/tail/cancel/has_source/upload_source) for conformance tests
 - **M2**: MVP remote execution with `xcodebuild`
@@
 ## Conformance / regression tests
@@
-Tests SHOULD be runnable without a live worker (use mocks/fixtures) and integrated into CI.
+Tests SHOULD be runnable without a live worker (use a mock worker + fixtures) and integrated into CI.
+The project SHOULD ship a minimal mock worker implementation that validates request/response schemas and exercises host logic deterministically.
```

---

If you want one additional “big swing” architectural refinement after these: **separate “result cache” from “artifact cache.”** I.e., allow `job_key` to represent correctness-preserving results, while allowing backend-specific *extra* artifacts to be generated (or post-processed) without invalidating the result cache. That keeps MVP fast, MCP rich, and caching coherent.
