Below are the best “spec-level” upgrades I’d make to this plan to improve **robustness, performance, operator UX, and long-term evolvability**, without diluting the lane’s crisp scope.

---

## 1) Add stable artifact entrypoints: `run_index.json` + `job_index.json` (artifact discovery + future-proofing)

### Why this makes it better

Right now consumers “discover” artifacts by convention (file names + directory layout). That’s fine for humans, but brittle for tools and third-party integrations.

A **single stable entrypoint** per run and per job gives you:

* **Forward compatibility**: new artifacts can be added without breaking parsers.
* **Better tooling UX**: “where’s the xcresult?” becomes a pointer lookup, not convention knowledge.
* **Cleaner remote storage support**: indices can carry “fetch hints” later without changing every consumer.

### Git diff (README + PLAN)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Outputs
 Artifacts are written to:
 `~/.local/share/rch/artifacts/xcode/<run_id>/`
 
 Layout (example):
+- `run_index.json`                 # stable entrypoint: pointers to run + job artifacts
 - `run_summary.json`
 - `run_plan.json`
 - `run_state.json`
 - `worker_selection.json`
 - `capabilities.json`
-- `steps/build/<job_id>/...`
-- `steps/test/<job_id>/...`
+- `steps/build/<job_id>/job_index.json`
+- `steps/build/<job_id>/...`
+- `steps/test/<job_id>/job_index.json`
+- `steps/test/<job_id>/...`
@@
 Includes:
+- run_index.json
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
 - metrics.json
 - source_manifest.json
 - worker_selection.json
+- job_index.json (per job)
 - events.jsonl (recommended)
 - test_summary.json (recommended)
 - build_summary.json (recommended)
 - junit.xml (recommended, test jobs)
 - build.log
 - result.xcresult/
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Artifacts
+- run_index.json
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
 - destination.json (recommended)
 - metrics.json
 - source_manifest.json
 - worker_selection.json
+- job_index.json
 - events.jsonl (recommended)
 - test_summary.json (recommended)
 - build_summary.json (recommended)
 - junit.xml (recommended, test jobs)
 - build.log
 - result.xcresult/
+
+## Artifact indices (normative)
+To make artifact discovery stable for tooling, the system MUST provide:
+
+- `run_index.json` at `<run_id>/run_index.json`
+- `job_index.json` at `<run_id>/steps/<action>/<job_id>/job_index.json`
+
+`run_index.json` MUST include pointers (relative paths) to:
+- `run_plan.json`, `run_state.json`, `run_summary.json`
+- `worker_selection.json` and the selected `capabilities.json` snapshot
+- an ordered list of step jobs with `{ index, action, job_id, job_index_path }`
+
+`job_index.json` MUST include pointers (relative paths) to the job’s:
+- `job.json`, `job_state.json`, `summary.json`, `manifest.json`, `attestation.json`
+- primary human artifacts (`build.log`, `result.xcresult/` when present)
+and MUST record the `artifact_profile` produced.
```

---

## 2) Introduce a **worker lease** (`reserve`/`release`) to prevent cross-host races and improve capacity behavior

### Why this makes it better

Your plan already selects a worker once per run, but without a “reservation” primitive you can still get:

* **mid-run contention surprises** (two hosts saturate the same worker)
* **flaky timeouts** when capacity is exceeded between plan creation and submit
* messy semantics around “WORKER_BUSY” (is it per job? per run?).

A **time-bounded lease** makes capacity a first-class concept:

* Host reserves capacity once, then submits jobs under that lease.
* Worker can enforce fairness and clearer “busy” behavior.
* Greatly improves resume behavior after host restarts.

### Git diff (README + PLAN)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Mental model (operator view)
 - You run `rch xcode verify` locally (even on Linux).
 - RCH classifies/sanitizes the invocation, builds a deterministic `job.json`, bundles inputs, and ships to macOS.
+- The host selects a worker once and (if supported) acquires a time-bounded **lease** to avoid cross-host contention.
 - Worker executes and returns schema-versioned artifacts (`summary.json`, logs, `xcresult`, etc.).
 - `rch xcode verify` is a **run** that may contain multiple **step jobs** (e.g. `build` then `test`).
 - The host persists a `run_plan.json` up front so runs can be resumed after interruption.
 - The run produces a **run summary** that links to each step job's artifact set.
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Architecture (high level)
 Pipeline stages:
 1. **Classifier**: detects safe, supported Xcode build/test invocations (deny-by-default).
-2. **Run builder**: resolves repo `verify` actions into an ordered run plan, allocates stable `job_id`s, persists `run_plan.json`, and chooses a worker once.
+2. **Run builder**: resolves repo `verify` actions into an ordered run plan, allocates stable `job_id`s, persists `run_plan.json`, chooses a worker once, and (when supported) acquires a time-bounded **worker lease**.
@@
 ### Worker RPC surface (recommended, maps cleanly to SSH forced-command)
 Worker SHOULD implement these operations with JSON request/response payloads:
 - `probe` → returns protocol range/features + `capabilities.json`
+- `reserve` → requests a lease for a run (capacity reservation; optional but recommended)
+- `release` → releases a lease early (optional)
 - `submit` → accepts `job.json` (+ optional bundle upload reference), returns ACK and initial status
 - `status` → returns current job status and pointers to logs/artifacts
@@
 ### Concurrency + capacity (normative)
 - Worker MUST enforce `max_concurrent_jobs`.
 - If capacity exceeded, worker MUST respond with a structured "busy" state so host can retry/backoff.
   - Response SHOULD include `retry_after_seconds`.
+
+## Worker lease (recommended)
+If the worker advertises feature `lease`, the host SHOULD:
+- call `reserve` once per run (before submitting any jobs),
+- include the returned `lease_id` on each `submit`,
+- renew or re-reserve if the lease expires before completion,
+- and call `release` when the run finishes.
+
+If the worker is at capacity, `reserve` SHOULD fail with `failure_kind=WORKER_BUSY` and include `retry_after_seconds`.
```

---

## 3) Make uploads reliable + performant: **framed binary payloads** and optional **resumable upload** for `upload_source`

### Why this makes it better

You already have a content-addressed source store keyed by `source_sha256` (excellent). The missing piece is transfer robustness:

* Large repos + spotty links can make uploads painful.
* Encoding bundles into JSON (base64) is a trap (slow + memory heavy).
* Over SSH stdin/stdout you *can* do clean, deterministic framing.

This upgrade keeps your “JSON request/response” model while allowing streaming bytes safely.

### Git diff (PLAN)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### RPC envelope (normative, recommended)
 All worker operations SHOULD accept a single JSON request on stdin and emit a single JSON response on stdout.
 This maps directly to an SSH forced-command entrypoint.
@@
 Response:
 - `protocol_version` (int; selected by host after probe)
 - `request_id` (string)
 - `ok` (bool)
 - `payload` (object, when ok=true)
 - `error` (object, when ok=false) containing: `code`, `message`, optional `data`
+
+### Binary framing (recommended)
+Some operations (notably `upload_source`) may need to transmit bytes efficiently.
+Implementations SHOULD support a framed stdin format:
+1) A single UTF-8 JSON header line terminated by `\n`
+2) Immediately followed by exactly `content_length` raw bytes (if `payload.stream` is present)
+
+The header MUST include:
+- `payload.stream.content_length` (int)
+- `payload.stream.content_sha256` (sha256 hex of the raw streamed bytes)
+- `payload.stream.compression` (`none`|`zstd`) and `payload.stream.format` (`tar`)
+
+The worker MUST verify `content_sha256` before processing, then (if compressed) decompress
+and verify the resulting canonical bundle digest matches `source_sha256`.
@@
 ### Worker RPC surface (recommended, maps cleanly to SSH forced-command)
 Worker SHOULD implement these operations with JSON request/response payloads:
@@
 - `has_source` → returns `{exists: bool}` for a given `source_sha256`
 - `upload_source` → accepts a source bundle upload for a given `source_sha256`
@@
 ## Source bundle store (performance, correctness-preserving)
 Workers SHOULD maintain a content-addressed store keyed by `source_sha256`.
@@
 Protocol expectations:
 - Host MAY query whether the worker already has `source_sha256` (via `has_source` RPC op).
 - If present, host SHOULD skip re-uploading the bundle and submit only `job.json`.
-- If absent, host uploads the canonical bundle once (via `upload_source` RPC op); worker stores it under `source_sha256`.
+- If absent, host uploads the canonical bundle once (via `upload_source` RPC op); worker stores it under `source_sha256`.
+
+`upload_source` payload (recommended):
+- `source_sha256` (string)
+- `stream` (object; see "Binary framing") describing the streamed bytes
+- optional `resume` object (if feature `upload_resumable` is present):
+  - `upload_id` (string), `offset` (int)
+Worker SHOULD respond with `next_offset` to support resumable uploads.
```

---

## 4) Improve crash/retry correctness: add monotonic `seq` to run/job state artifacts (race-free resume)

### Why this makes it better

Your state machine is solid, but on restarts you can still encounter “last writer wins” ambiguity if timestamps collide or updates arrive out of order.

A simple monotonic `seq`:

* makes resumption logic deterministic
* prevents stale status overwriting newer state
* is backwards compatible (optional field)

### Git diff (PLAN)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### State artifacts
 - `run_state.json`: persisted at `<run_id>/run_state.json` with fields: `run_id`, `state`, `updated_at`, `schema_version`.
 - `job_state.json`: persisted at `<run_id>/steps/<action>/<job_id>/job_state.json` with fields: `job_id`, `run_id`, `state`, `updated_at`, `schema_version`.
+
+Both MAY additionally include:
+- `seq` (int): monotonic sequence number incremented on every state transition for the artifact.
+If present, consumers SHOULD prefer `seq` over timestamps for ordering.
 
 Both MUST be updated atomically (write-then-rename) on every state transition.
```

---

## 5) Promote `destination.json` to a required artifact and extend it to cover provisioning details (debuggability + determinism audits)

### Why this makes it better

You already encode destination identity into `job_key_inputs` (great). But operators will *debug* destination issues constantly (OS=latest resolution, runtime identifiers, ephemeral sims).

Making `destination.json` **always present** gives:

* a canonical, human-readable “what actually ran”
* a clean place to record ephemeral provisioning outcomes (UDID, cleanup, timings) **without polluting `job_key_inputs`**

### Git diff (PLAN)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Artifacts
@@
 - invocation.json
 - toolchain.json
-- destination.json (recommended)
+- destination.json
@@
 ### Destination resolver
@@
-   - **Destination provisioning (optional, recommended)**: if `destination.provisioning="ephemeral"` and the destination is a Simulator, the worker provisions a clean simulator per job and records the created UDID in artifacts (UDID MUST NOT affect `job_key`).
+   - **Destination provisioning (optional, recommended)**: if `destination.provisioning="ephemeral"` and the destination is a Simulator, the worker provisions a clean simulator per job and records the created UDID and cleanup outcome in `destination.json` (UDID MUST NOT affect `job_key`).
```

*(If you also want this visible in README, the “Includes:” list already contains `destination.json`; the real change is making it non-optional in PLAN.)*

---

## 6) Attestation you can actually trust: pin worker attestation key + emit `attestation_verification.json`

### Why this makes it better

You already have great attestation structure. The missing operational piece is **trust bootstrap**:

* Where does the host learn the worker’s signing key?
* How do operators pin/rotate it?
* Where do you record verification results?

Adding key pinning to `workers.toml` plus a verification artifact makes this auditable and automation-friendly.

### Git diff (README + PLAN)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 [[worker]]
 name = "macmini-01"
 host = "macmini.local"
 user = "rch"
 port = 22
 tags = ["macos","xcode"]
 known_host_fingerprint = "SHA256:..."
+attestation_pubkey_fingerprint = "SHA256:..." # optional pin for signed attestation
 ssh_key_path = "~/.ssh/rch_macmini"
 priority = 10
@@
 ### Hardening recommendations
@@
 - Host SHOULD pin worker host keys (no TOFU surprises).
+- If attestation signing is enabled, pin the worker’s attestation public key fingerprint and require verification.
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Worker capabilities
 Worker reports a `capabilities.json` including:
@@
 Optional but recommended:
 - worker identity material (SSH host key fingerprint and/or attestation public key fingerprint)
 Host stores the chosen worker capability snapshot in artifacts.
@@
 ## Artifact attestation (normative + optional signing)
 Artifacts MUST include `attestation.json` with:
@@
 Optional but recommended:
 - Worker signs `attestation.json` with a worker-held key (e.g. Ed25519).
 - Host verifies signature and records `attestation_verification.json`.
 If signature verification fails, host MUST mark the run as failed (`failure_kind=ATTESTATION`).
+
+### Attestation key pinning (recommended)
+Host worker inventory MAY pin an `attestation_pubkey_fingerprint`.
+If pinned and the worker provides a signing key, the host MUST verify the fingerprint match
+before accepting signed attestations.
```

---

## 7) Strengthen integrity: require `artifact_root_sha256` in `manifest.json` and define its computation

### Why this makes it better

You already verify `manifest.json` digests, but you’ll eventually want:

* **fast integrity checks** (“did I get the same artifact set?”)
* remote fetch/storage flows where you validate the set before unpacking everything
* a single digest to bind the manifest entries deterministically

Make `artifact_root_sha256` mandatory and define it precisely (JCS like you did for `job_key_inputs`).

### Git diff (PLAN)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Artifact manifest (normative)
 `manifest.json` MUST enumerate produced artifacts with at least:
 - `path` (relative), `size`, `sha256`
-`manifest.json` SHOULD also include `artifact_root_sha256` (digest over ordered entries) to bind the set.
+`manifest.json` MUST also include `artifact_root_sha256` to bind the set.
+
+`entries` MUST be sorted lexicographically by `path` (UTF-8).
+`artifact_root_sha256` MUST be computed as:
+- `sha256_hex( JCS(entries) )`
+where `entries` is the exact array written to `manifest.json`.
```

---

## 8) Make the conformance suite “real”: add a tiny fixture Xcode project + protocol replay harness

### Why this makes it better

You already call out conformance tests (excellent). Two additions make it much more compelling and reliable:

1. **A minimal fixture Xcode project** (tiny Swift package / app) that can build and run tests deterministically.
2. A **protocol replay harness**: record a golden “probe → reserve → upload → submit → tail → fetch” transcript and replay it against host logic. This massively reduces regressions without requiring a live Mac in every test environment.

### Git diff (PLAN)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Conformance / regression tests
 The project SHOULD maintain a conformance test suite that validates:
@@
 - Protocol round-trip: a mock worker can handle all RPC ops and return valid responses.
+- Protocol replay: recorded request/response transcripts can be replayed deterministically against host logic.
 - State machine transitions: run and job states follow the defined state diagrams.
@@
 Tests SHOULD be runnable without a live worker (use a mock worker + fixtures) and integrated into CI.
 The project SHOULD ship a minimal mock worker implementation that validates request/response schemas and exercises host logic deterministically.
+
+Recommended fixtures:
+- A minimal Xcode fixture project (build + test) with stable outputs suitable for golden assertions.
+- Classifier fixtures: a corpus of accepted/rejected `xcodebuild` invocations (including tricky edge cases).
```

---

### If you only take *two* of these

If you want the biggest “leverage per line of spec,” do:

1. **Artifact indices** (`run_index.json` / `job_index.json`)
2. **Worker lease + better transfer framing** (leases + framed `upload_source`)

Those two upgrades dramatically improve the lane’s *operational* quality without expanding scope into CI/publishing.

If you want, I can also propose a **single consolidated patch** that applies all changes cleanly at once (less merge-conflict-prone than per-change patches).
