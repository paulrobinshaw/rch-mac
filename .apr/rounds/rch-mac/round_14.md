# APR Round 14 — rch-mac

## Focus
Race conditions between subsystems, implicit ordering assumptions, partial failure paths, observability gaps, and schema completeness issues that an implementer would discover the hard way.

## Changes

### 14.1 — `run_summary.json` schema missing `steps_skipped` and `steps_rejected`
**Problem:** Step execution semantics reference `steps_skipped` in `run_summary.json`, but the schema table omits it. Also, `rejected` is a valid step status but has no counter.
**Fix:** Add `steps_skipped` (int) and `steps_rejected` (int) to the `run_summary.json` schema table.

### 14.2 — `source_manifest.json` is job-scoped but the source bundle is run-scoped
**Problem:** The source bundle is created once per run (same `source_sha256` for all steps), but `source_manifest.json` includes `job_id` and `job_key`, making it job-scoped. In multi-step runs this means either confusing duplication or ambiguous ownership.
**Fix:** Make `source_manifest.json` run-scoped (remove `job_id`/`job_key`, keep `run_id`). Emit once at `<run_id>/source_manifest.json`. Update artifact list and `run_index.json` pointers.

### 14.3 — Concurrent `upload_source` lacks atomic store semantics
**Problem:** Two hosts may both call `has_source` → false, then both upload. The spec doesn't require `upload_source` to be idempotent by `source_sha256`, risking partial overwrites or races.
**Fix:** Add normative requirement: `upload_source` MUST be atomic by `source_sha256` — if the bundle already exists when the upload completes, the worker MUST discard the duplicate and return success. Workers SHOULD use write-to-temp-then-rename.

### 14.4 — Result cache materialization must regenerate `manifest.json` and `attestation.json`
**Problem:** When materializing from cache, the worker rewrites `run_id`, `job_id`, `created_at`, etc. in artifacts. But `manifest.json` contains `sha256` digests of each file. After rewriting, those digests are stale. The spec doesn't clarify that `manifest.json` must be regenerated.
**Fix:** Add normative note: after rewriting job-scoped fields during cache materialization, the worker MUST regenerate `manifest.json` (recomputing all digests) and `attestation.json` (rebinding the new manifest). The four-phase commit sequence applies to materialized jobs.

### 14.5 — `run_plan.json` does not record `continue_on_failure`
**Problem:** The step execution semantics branch on `continue_on_failure`, but `run_plan.json` doesn't capture this flag. On resumption, the host cannot determine whether to skip remaining steps after a failure without re-reading config (which may have changed).
**Fix:** Add `continue_on_failure` (bool) to `run_plan.json` required fields. The host MUST use the value from `run_plan.json` (not current config) on resumption.

### 14.6 — `run_state.json` has no current-step pointer (observability gap)
**Problem:** During a multi-step run, determining which step is active requires scanning all `job_state.json` files. External tooling (dashboards, `rch xcode tail`) has no lightweight way to find the active job.
**Fix:** Add optional `current_step` object (`{ index, job_id, action }`) to `run_state.json`, updated on each step transition. Null/absent when run is terminal.

### 14.7 — Lease expiry between `reserve` and first `submit` is unspecified
**Problem:** A lease may expire if the host takes too long between `reserve` and first `submit` (e.g., slow source upload). The worker would reject the submit with `LEASE_EXPIRED`, but the host has no recovery path defined.
**Fix:** Add normative guidance: if `submit` fails with `LEASE_EXPIRED`, the host MUST attempt a single `reserve` + re-submit cycle before failing. `run_state.json` SHOULD record `lease_renewed: true` if this occurs. Workers SHOULD set lease TTL >= `connect_timeout_seconds + overall_seconds` to minimize spurious expiry.

### 14.8 — Double-interrupt leaves orphaned jobs on worker
**Problem:** On second SIGINT, the host MAY exit immediately without waiting for cancellation acknowledgement. But it only "persists `run_state.json`" — it doesn't mention whether `cancel` RPCs were already sent (they were sent on first SIGINT). If the first cancel RPC hasn't completed, the worker job runs to completion with no one collecting artifacts.
**Fix:** Clarify: the first SIGINT MUST send `cancel` RPCs and not return until they are dispatched (fire-and-forget is acceptable). Second SIGINT skips waiting for acknowledgement but cancels ARE already in-flight. Add: workers MUST auto-cancel jobs whose lease expires without a `release`, providing a backstop for host crashes.

