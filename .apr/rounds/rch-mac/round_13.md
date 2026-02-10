# APR Round 13 — rch-mac

## Focus
Subtle cross-subsystem interactions: bootstrapping paradoxes, missing execution semantics, cache materialization gaps, partial-state recovery, framing ambiguity.

## Findings & Patches

### 13.1 — Probe protocol_version bootstrap paradox
**File:** PLAN.md (RPC envelope / probe)
**Issue:** The RPC envelope requires `protocol_version` on every request, but `probe` is the operation that negotiates the version. The host cannot know the version before probing. No bootstrap rule is defined.
**Patch:** Add normative rule: `probe` requests MUST use `protocol_version: 0` (sentinel). Worker MUST accept `protocol_version: 0` exclusively for `op: "probe"` and MUST reject it for all other ops.

### 13.2 — Step execution semantics missing (sequential, early-abort)
**File:** PLAN.md (Run plan / Architecture)
**Issue:** `run_plan.json` defines an "ordered list of step jobs" but the spec never states whether steps execute sequentially or in parallel, nor whether a failed/rejected step aborts subsequent steps. An implementer must guess.
**Patch:** Add normative execution semantics: steps execute sequentially in plan order; if a step fails or is rejected, subsequent steps MUST be skipped (marked as not started) unless the run is explicitly configured for `continue_on_failure`.

### 13.3 — Result cache materialization must rewrite job-scoped fields
**File:** PLAN.md (Result cache)
**Issue:** When materializing cached results into a new `job_id` directory, artifacts like `summary.json`, `job_state.json`, `toolchain.json`, etc. contain the original `job_id`, `run_id`, and `created_at`. The spec doesn't require rewriting these fields, which would produce artifacts with stale/wrong identifiers.
**Patch:** Add normative requirement: when materializing from result cache, the worker MUST rewrite `run_id`, `job_id`, and `created_at` in all job-scoped artifacts to reflect the new job context. `cached_from_job_id` MUST be recorded in `summary.json`.

### 13.4 — Worker job in crashed/partial state on re-submit
**File:** PLAN.md (Job lifecycle + idempotency)
**Issue:** The idempotency rules cover COMPLETE and RUNNING states for a re-submitted `job_id`, but not the case where the worker crashed mid-execution leaving partial artifacts (neither COMPLETE nor RUNNING). On host resumption, re-submitting the same `job_id` hits an undefined state.
**Patch:** Add rule: if a `job_id` exists on the worker but is in a non-terminal, non-running state (e.g. orphaned after worker crash), the worker MUST clean up partial artifacts and re-execute the job from scratch, preserving idempotency of the `job_id`.

### 13.5 — Binary framing discriminator missing
**File:** PLAN.md (Binary framing)
**Issue:** The worker receives bytes on stdin but has no way to distinguish a plain JSON request from a framed request (JSON header + binary payload). Both start with `{`. There's no discriminator field specified.
**Patch:** Add normative discriminator: if the request `payload` contains a `stream` object, the worker MUST treat stdin as framed (JSON header line + raw bytes). If `payload.stream` is absent, the request is plain JSON. The JSON header line MUST be terminated by `\n` and MUST be the only line before the binary payload.

### 13.6 — Lease recovery on run resumption undefined
**File:** PLAN.md (Run resumption / Worker lease)
**Issue:** If the host crashes and resumes a run, the original lease may have expired and the worker may now be leased to another host. The spec says "renew or re-reserve if the lease expires" but doesn't specify behavior when re-reserve fails because the worker is busy—and the run_plan is already committed to that specific worker.
**Patch:** Add to run resumption: if the host cannot re-acquire a lease on the original worker (WORKER_BUSY or unreachable), and the run has incomplete jobs, the host MUST fail the run with `failure_kind=WORKER_BUSY` and `human_summary` explaining the lease could not be recovered. The host MUST NOT silently switch workers mid-run (job_key_inputs are bound to a specific toolchain/destination from the original worker).

### 13.7 — manifest.json entry inclusion scope undefined
**File:** PLAN.md (Artifact manifest)
**Issue:** The spec says manifest.json enumerates "produced artifacts" and the host verification excludes `manifest.json`, `attestation.json`, and `job_index.json`. But it never states which other artifacts MUST vs MAY appear in entries. If an implementer omits `effective_config.json` or `toolchain.json` from entries, verification would flag them as "extra files."
**Patch:** Add normative rule: `manifest.json` entries MUST include every file in the job artifact directory EXCEPT `manifest.json`, `attestation.json`, and `job_index.json` themselves. This makes the inclusion rule exhaustive and the "no extra files" check unambiguous.

### 13.8 — run_plan.json lacks worker binding
**File:** PLAN.md (Run plan artifact)
**Issue:** `run_plan.json` records steps and job_ids but not which worker was selected. On resumption, the host must know which worker to query for job status, but this isn't in the run_plan. The host would need to also read `worker_selection.json`, creating an implicit ordering dependency that isn't documented.
**Patch:** Add `selected_worker` (worker name) and `selected_worker_host` to `run_plan.json` schema. Document that on resumption, the host MUST read `run_plan.json` for worker identity (or fall back to `worker_selection.json` if `selected_worker` is absent for backward compatibility).
