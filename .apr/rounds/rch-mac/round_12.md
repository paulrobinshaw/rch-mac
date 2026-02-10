# APR Round 12 — rch-mac

## Findings

### R12-1: Cancellation status contradiction
**Section:** Cancellation
**Issue:** The prose says "write artifacts with `status=failed` and `failure_kind=CANCELLED`" but the normative box immediately below says `status=cancelled`. Direct contradiction.
**Fix:** Change prose to `status=cancelled` to match the normative box and the failure taxonomy (`cancelled` is a distinct status).

### R12-2: `run_plan.json` missing `schema_id` in field list
**Section:** Run plan artifact
**Issue:** Every other normative artifact requires both `schema_version` and `schema_id`, and the schema compatibility section even lists `rch-xcode/run_plan@1` as an example. But `run_plan.json`'s own field list omits `schema_id`.
**Fix:** Add `schema_id` to the `run_plan.json` required fields.

### R12-3: Forced-command gate example writes error to stderr
**Section:** README → Hardening recommendations
**Issue:** The gate script writes the JSON error to `>&2` (stderr), but the RPC envelope spec requires responses on stdout. An SSH client reading stdout gets nothing; the error is lost.
**Fix:** Change `>&2` to `>&1` (stdout).

### R12-4: `effective_config.json` missing `job_key`
**Section:** Configuration merge → effective_config.json
**Issue:** The artifact versioning section says job-scoped artifacts MUST include `run_id`, `job_id`, AND `job_key`. `effective_config.json` is described as "emitted per job" and lists `run_id`, `job_id` — but omits `job_key`.
**Fix:** Add `job_key` to the required fields.

### R12-5: Missing artifacts from enumeration list
**Section:** Artifacts
**Issue:** Several artifacts defined in normative/recommended sections are absent from the master artifact list: `executor_env.json`, `classifier_policy.json`, `attestation_verification.json`.
**Fix:** Add all three to the enumeration.

### R12-6: `run_index.json` lacks normative schema fields
**Section:** Artifact indices
**Issue:** `run_index.json` is normative but has no schema field requirements. Unlike `job_index.json` (which at least specifies what it must point to), `run_index.json` has no `schema_version`, `schema_id`, `created_at`, `run_id` requirement stated, even though the artifact versioning section demands these for all run-scoped artifacts.
**Fix:** Add explicit field requirements.

### R12-7: `source_manifest.json` scope and schema ambiguity
**Section:** Source bundling canonicalization
**Issue:** `source_manifest.json` is listed in the artifact enumeration and referenced as normative, but it's unclear whether it's job-scoped or run-scoped, and it only mentions `schema_version` without `schema_id` or the other required fields per artifact versioning rules.
**Fix:** Clarify as job-scoped and add required schema fields.

### R12-8: `release` of unknown/expired lease has no defined behavior
**Section:** Worker lease / RPC error codes
**Issue:** The error code registry includes `LEASE_EXPIRED` but doesn't cover `release` of an already-expired or unknown lease. Should the worker return an error or silently succeed (idempotent)?
**Fix:** Define `release` as idempotent — worker returns `ok:true` even if the lease is unknown/expired (simplifies host cleanup).
