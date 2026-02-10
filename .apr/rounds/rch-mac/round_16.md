# APR Round 16 — Forensic Spec Review

**Focus:** Gaps that 15 rounds missed — ambiguities two implementers would resolve differently, production failure modes, missing normative details.

## Changes

### 1. `summary.json` has no normative schema table
**Problem:** Every major artifact (`toolchain.json`, `destination.json`, `metrics.json`, `run_summary.json`, etc.) has a normative schema table. `summary.json` is referenced dozens of times as the primary job result artifact but its required fields are never enumerated in one place — they're scattered across Failure taxonomy, Cancellation, and other sections. Two implementers would produce incompatible `summary.json` files.
**Fix:** Add a normative schema table for `summary.json` consolidating all required fields.

### 2. `manifest.json` violates the "all JSON artifacts" schema rule
**Problem:** The "Artifact schemas + versioning" section says ALL JSON artifacts MUST include `schema_version`, `schema_id`, `created_at`, and job-scoped artifacts MUST also include `run_id`, `job_id`, `job_key`. But the `manifest.json` section only specifies `path`, `size`, `sha256` entries and `artifact_root_sha256` — missing all mandatory envelope fields. An implementer following only the manifest section would produce non-compliant artifacts.
**Fix:** Add mandatory envelope fields to the manifest.json specification.

### 3. `job_index.json` pointers are incomplete
**Problem:** `job_index.json` lists pointers to only 7 artifacts (job.json, job_state.json, summary.json, manifest.json, attestation.json, build.log, result.xcresult). But a job produces up to 15+ artifacts. Missing: `toolchain.json`, `destination.json`, `effective_config.json`, `invocation.json`, `job_key_inputs.json`, `metrics.json`. Without these pointers, tooling must guess paths or scan directories, defeating the purpose of the index.
**Fix:** Expand `job_index.json` required and optional pointers.

### 4. `run_index.json` missing `source_manifest.json` pointer
**Problem:** `source_manifest.json` is a run-scoped normative artifact, but `run_index.json` doesn't include a pointer to it. It's the only run-scoped normative artifact omitted from the index.
**Fix:** Add `source_manifest.json` to `run_index.json` pointers.

### 5. TOCTOU race: `has_source` → `submit` with concurrent GC
**Problem:** Host calls `has_source` (true), skips upload, then calls `submit`. Between those calls, worker GC could evict the bundle. The worker would then fail with a confusing error. No specified recovery path.
**Fix:** Add normative requirement: worker MUST check bundle availability on `submit` and return a specific error code if missing, and host MUST handle by re-uploading.

### 6. Rejected steps in multi-step runs are underspecified
**Problem:** The classifier runs on the host before submission. If a step's invocation is rejected, the spec says `rejected` jobs "MUST NOT have a `job_state.json`" and the run-level exit code handles `rejected`. But: is a rejected step even added to `run_plan.json`? Does it get a `job_id`? Where does its `summary.json` live if it has no `job_state.json`? The step execution semantics only mention `failed` and `rejected` as skip triggers but don't define the rejection artifact path.
**Fix:** Clarify that rejection happens at plan time, rejected steps are recorded in `run_plan.json` with a `rejected: true` flag, and their `summary.json` is still emitted at the standard step path.

### 7. `capabilities.json` snapshot has no defined artifact path
**Problem:** `run_index.json` says it includes a pointer to "the selected capabilities.json snapshot" and worker_selection.json references it, but no canonical path is defined for where this snapshot is stored in the run artifact directory. Two implementers would put it in different places.
**Fix:** Define the canonical path as `<run_id>/capabilities.json`.

### 8. `fetch` RPC op transfer format unspecified
**Problem:** The `fetch` op is listed in the worker RPC surface as "returns artifacts (or a signed manifest + download hints)" but the actual response format — how artifact bytes are transferred — is never specified. Is it binary framing like `upload_source`? A tar stream? Individual file requests? This is the most underspecified RPC op.
**Fix:** Add a normative specification for `fetch` response format using the same binary framing mechanism as `upload_source`.
