# APR Round 11 — rch-mac

**Reviewer:** Claude (spec reviewer)  
**Date:** 2026-02-10  
**Focus:** Undefined behavior corners, missing error paths, cross-reference inconsistencies, operational gaps

---

## Change 1: Host-side manifest verification is unspecified

### Rationale
PLAN.md defines `manifest.json` with per-file `sha256` digests and `artifact_root_sha256`, and states "host verifies `manifest.json` digests" in README notes—but the normative spec never defines **when** or **how** the host verifies, what happens on mismatch, or whether verification is MUST/SHOULD. This is a critical integrity gap: without normative verification, a corrupted or tampered artifact set could be silently accepted.

### Patch (PLAN.md)
After the "Artifact manifest (normative)" section's `artifact_root_sha256` computation paragraph, append:

```diff
+### Host manifest verification (normative)
+After fetching a job's artifacts, the host MUST:
+1. Recompute `artifact_root_sha256` from the fetched `manifest.json` entries and verify it matches the declared value.
+2. Verify `sha256` and `size` for every entry in `manifest.json` against the fetched files.
+3. Verify that no extra files exist in the artifact directory beyond those listed in `manifest.json`
+   (plus `manifest.json`, `attestation.json`, and `job_index.json` themselves).
+
+If any check fails, the host MUST:
+- Mark the job as failed with `failure_kind=ARTIFACTS` and `failure_subkind=INTEGRITY_MISMATCH`.
+- Record the mismatched paths/digests in `summary.json` under `integrity_errors[]`.
+- NOT use the artifacts for caching or attestation verification.
```

---

## Change 2: Concurrent runs on the same host are unspecified

### Rationale
Nothing prevents two `rch xcode verify` processes from running simultaneously on the same host. Without guidance, they could: write to the same artifact directory, race on cache locks, or exhaust worker capacity without coordinated backoff. The spec needs a normative stance.

### Patch (PLAN.md)
After the "Run plan artifact (normative)" section (before "Run resumption"), insert:

```diff
+### Concurrent host runs (normative)
+Multiple runs MAY execute concurrently on the same host. To prevent interference:
+- Each run MUST use a unique `run_id`, ensuring artifact directories do not collide.
+- The host MUST NOT hold filesystem locks on shared resources (e.g. the artifact root) across runs.
+- If the worker returns `WORKER_BUSY` (capacity exhausted), the host MUST respect `retry_after_seconds`
+  and MUST NOT busy-loop. The host SHOULD implement bounded retry with exponential backoff (recommended max: 3 retries).
+- Host-side artifact GC MUST acquire a lock before scanning/deleting, and MUST skip artifacts
+  belonging to any RUNNING run (determined by `run_state.json`).
```

---

## Change 3: `run_state.json` and `job_state.json` schema fields are incomplete

### Rationale
PLAN.md says these files have fields `run_id`/`job_id`, `state`, `updated_at`, `schema_version` and optional `seq`. But the global artifact schema rules require `schema_id` and `created_at` on ALL JSON artifacts. Additionally, `job_state.json` must include `job_key` per the job-scoped artifact rule. These are cross-reference inconsistencies that would cause schema validation failures.

### Patch (PLAN.md)

```diff
 ### State artifacts
-- `run_state.json`: persisted at `<run_id>/run_state.json` with fields: `run_id`, `state`, `updated_at`, `schema_version`.
-- `job_state.json`: persisted at `<run_id>/steps/<action>/<job_id>/job_state.json` with fields: `job_id`, `run_id`, `state`, `updated_at`, `schema_version`.
+- `run_state.json`: persisted at `<run_id>/run_state.json` with fields: `schema_version`, `schema_id` (`rch-xcode/run_state@1`), `run_id`, `state`, `created_at`, `updated_at`.
+- `job_state.json`: persisted at `<run_id>/steps/<action>/<job_id>/job_state.json` with fields: `schema_version`, `schema_id` (`rch-xcode/job_state@1`), `run_id`, `job_id`, `job_key`, `state`, `created_at`, `updated_at`.
```

---

## Change 4: Timeout configuration lacks normative bounds and defaults

### Rationale
PLAN.md references `overall_seconds` and `idle_log_seconds` timeouts in the config example and mentions "Executor timeout (overall + idle-log watchdog)" but never defines: default values, minimum/maximum bounds, or what happens when a timeout is set to 0 or a negative value. This is undefined behavior that implementations will handle inconsistently.

### Patch (PLAN.md)
After "Timeouts + retries" section, replace the existing content:

```diff
 ## Timeouts + retries
-- SSH/connect retries with backoff
-- Transfer retries (idempotent)
-- Executor timeout (overall + idle-log watchdog)
-- On failure, artifacts MUST still include logs + diagnostics if available.
+### Timeout defaults and bounds (normative)
+- `overall_seconds`: maximum wall-clock time for a single job execution. Default: 1800 (30 min). MUST be > 0 and ≤ 86400 (24h).
+- `idle_log_seconds`: maximum time without new log output before the host kills the job. Default: 300 (5 min). MUST be > 0 and ≤ `overall_seconds`.
+- `connect_timeout_seconds`: SSH/transport connection timeout. Default: 30. MUST be > 0 and ≤ 300.
+- If any timeout value is out of bounds, the host MUST reject the configuration and exit with a diagnostic (before run execution).
+
+### Retry policy (normative)
+- SSH/connect: retry with exponential backoff. Default: 3 attempts, initial delay 2s, max delay 30s.
+- Transfer (upload_source): retry with backoff (idempotent by `source_sha256`). Default: 3 attempts.
+- Executor: MUST NOT retry automatically (non-idempotent). Retries require a new run.
+- On failure at any stage, artifacts MUST still include logs + diagnostics if available.
```

---

## Change 5: `destination.json` missing from README artifact list

### Rationale
PLAN.md defines `destination.json` as a normative per-job artifact with a full schema, but README's artifact listing omits it entirely. This cross-reference inconsistency means operators won't know to look for it.

### Patch (README.md)

```diff
 Includes:
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
+- destination.json
 - metrics.json
 - source_manifest.json
 - worker_selection.json
```

Also in the PLAN.md Artifacts list:

```diff
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
+- destination.json
 - metrics.json
 - source_manifest.json
 - worker_selection.json
```

---

## Change 6: Bundle `max_bytes` enforcement and worker-side limits are unspecified

### Rationale
The config example shows `max_bytes = 0` meaning "unlimited" and capabilities include `max_upload_bytes`, but: (1) the host never validates bundle size against `max_bytes` before upload, (2) the worker never validates incoming bundle size against `max_upload_bytes` before accepting, (3) there's no normative failure kind for oversized bundles. A multi-GB repo could silently exhaust worker disk or hang on slow connections.

### Patch (PLAN.md)
After "Bundle modes (recommended)" section, append:

```diff
+### Bundle size enforcement (normative)
+- If `bundle.max_bytes` > 0, the host MUST reject the bundle before upload if it exceeds the limit,
+  with `failure_kind=BUNDLER` and `failure_subkind=SIZE_EXCEEDED`.
+- If the worker's `capabilities.json` includes `max_upload_bytes`, the host MUST reject the bundle
+  before upload if it exceeds that limit (same failure kind/subkind).
+- The effective limit is `min(bundle.max_bytes, worker.max_upload_bytes)` where 0 means "no limit from that source".
+- The worker MUST reject an `upload_source` request if the `content_length` exceeds `max_upload_bytes`,
+  returning error code `PAYLOAD_TOO_LARGE` with the limit in `data.max_bytes`.
```

---

## Change 7: Worker clock skew can corrupt timestamp ordering

### Rationale
Artifacts are timestamped with RFC 3339 UTC, and `run_state.json`/`job_state.json` use `updated_at` for ordering (with `seq` as a recommended alternative). But the host and worker have independent clocks. If worker clock is skewed, `created_at` in worker-produced artifacts (toolchain.json, destination.json, attestation.json) could be before the host's `run_plan.json.created_at`, breaking monotonicity assumptions. The spec should acknowledge and mitigate this.

### Patch (PLAN.md)
After "Timestamp and encoding conventions (normative)", append:

```diff
+### Clock skew tolerance (normative)
+- Host and worker clocks are independent; implementations MUST NOT assume cross-machine timestamp monotonicity.
+- For ordering within a single machine's artifacts, prefer `seq` (monotonic counter) over timestamps.
+- The host SHOULD log a warning if the worker's `probe` response timestamp differs from the host clock
+  by more than 30 seconds, as this may cause confusing artifact timelines.
+- Consumers MUST NOT reject artifacts solely because cross-machine timestamps are non-monotonic.
```

---

## Change 8: `status` values in `summary.json` vs state machine are inconsistent

### Rationale
The failure taxonomy says `summary.json` MUST include `status`: `success | failed | rejected | cancelled`. But the job state machine has states: `QUEUED | RUNNING | SUCCEEDED | FAILED | CANCELLED`. Note: (1) `rejected` appears in summary status but not in the state machine, (2) state machine uses `SUCCEEDED` but summary uses `success`. This inconsistency will cause bugs in consumers mapping between the two.

### Patch (PLAN.md)
In the failure taxonomy section:

```diff
 `summary.json` MUST include:
-- `status`: success | failed | rejected | cancelled
+- `status`: `success` | `failed` | `rejected` | `cancelled`
+  - `rejected` is used when the classifier rejects the invocation (no job execution occurs;
+    the job state machine is not entered). `rejected` jobs MUST NOT have a `job_state.json`.
+  - `success` corresponds to the `SUCCEEDED` terminal state in the job state machine.
+  - `failed` corresponds to the `FAILED` terminal state.
+  - `cancelled` corresponds to the `CANCELLED` terminal state.
```

And in the run summary schema, add `rejected` as a possible status:

```diff
-| `status`         | string | `success` \| `failed` \| `cancelled`                  |
+| `status`         | string | `success` \| `failed` \| `rejected` \| `cancelled`    |
```
