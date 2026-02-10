# APR Round 15 — rch-mac

## Focus
Version migration, mid-fleet upgrade resilience, observability for production debugging, third-party implementer completeness, and artifact schema consistency gaps that 14 rounds of refinement missed.

## Changes

### 15.1 — Unknown `schema_version` handling is undefined
**Problem:** Every artifact includes `schema_version`, and compatibility rules say when to bump it. But there's no guidance for consumers encountering a `schema_version` higher than they understand. A v1 host fetching artifacts from a v2 worker has no defined behavior — crash, ignore, warn?
**Fix:** Add normative consumer rule in "Schema compatibility rules": consumers encountering a `schema_version` higher than they support MUST NOT reject the artifact outright if the `schema_id` major version matches (i.e. `@1`). They MUST parse known fields and MUST ignore unknown fields. If the `schema_id` major version differs (e.g. consumer knows `@1`, artifact says `@2`), the consumer MUST reject with a diagnostic naming the expected vs actual `schema_id`. This enables forward compatibility for additive changes within a major version.

### 15.2 — `capabilities.json` missing `schema_id` (inconsistency with all other artifacts)
**Problem:** The normative requirement states `capabilities.json` MUST include `schema_version` and `created_at`, but unlike every other artifact it doesn't require `schema_id`. Third-party implementers will notice this inconsistency.
**Fix:** Add `schema_id` (`rch-xcode/capabilities@1`) to the `capabilities.json` normative requirements.

### 15.3 — Protocol negotiation result not persisted in any artifact
**Problem:** The host negotiates a `protocol_version` with the worker after probing, but this value is never recorded in any artifact. Operators debugging production issues (e.g. "why did the host use op X but the worker rejected it?") have no audit trail of what protocol version was active.
**Fix:** Add normative requirement: `worker_selection.json` MUST record `negotiated_protocol_version` (int) and `worker_protocol_range` (`{ min, max }`). Additionally, `run_plan.json` MUST record `protocol_version` (int) so resumption uses the same negotiated version.

### 15.4 — `worker_selection.json` has no normative schema definition
**Problem:** The spec says the host MUST write `worker_selection.json` and lists some contents (inputs, filtered set, chosen worker, reasons, probe failures), but never defines a schema table or requires `schema_version`/`schema_id`/`created_at` — unlike every other normative artifact. Third-party implementers must guess.
**Fix:** Add a normative schema outline for `worker_selection.json` with required fields: `schema_version` (int, `1`), `schema_id` (`rch-xcode/worker_selection@1`), `created_at`, `run_id`, `negotiated_protocol_version`, `worker_protocol_range`, `selected_worker`, `selected_worker_host`, `selection_mode`, `candidate_count`, `probe_failures[]`, `snapshot_age_seconds`, `snapshot_source` (`cached`|`fresh`).

### 15.5 — Worker upgraded mid-run during crash recovery
**Problem:** If the host crashes and resumes, it reconnects to the original worker (bound by `run_plan.json`). But if the worker was upgraded between crash and resume (new `rch_xcode_lane_version`, different `protocol_max`), the host re-probes and might negotiate a different protocol version. The spec doesn't address this. Worse, the worker's toolchain may have changed (Xcode update), invalidating `job_key_inputs`.
**Fix:** Add normative requirement in "Run resumption": on resume, the host MUST re-probe the worker and verify that `negotiated_protocol_version` matches the value persisted in `run_plan.json`. If it differs, the host MUST log a warning but continue if the persisted version is still within the worker's supported range. If the persisted version is outside the worker's range, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE` and `failure_subkind=PROTOCOL_DRIFT`. For toolchain changes: the host MUST verify that the worker still has the Xcode build recorded in `job_key_inputs.toolchain.xcode_build`; if absent, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE` and `failure_subkind=TOOLCHAIN_CHANGED`.

### 15.6 — Error `message` quality guidance missing for third-party implementers
**Problem:** The spec defines error codes (`INVALID_REQUEST`, `BUSY`, etc.) but provides no guidance on what `error.message` should contain. Third-party worker implementations will produce inconsistent, unhelpful messages (e.g. "error occurred" vs structured diagnostics).
**Fix:** Add recommended guidance in the error codes section: `error.message` SHOULD be a single-line, human-readable sentence describing the problem. It SHOULD include the specific field or value that caused the error when applicable (e.g. `"protocol_version 3 is outside supported range [1, 2]"`). `error.data` MAY include machine-readable details (the failing field name, expected vs actual values). Workers MUST NOT include secrets, file system paths outside the job directory, or stack traces in `error.message`.

### 15.7 — `source_manifest.json` entries don't record file type (symlink ambiguity)
**Problem:** The spec has detailed symlink handling rules (escape detection, preserve-vs-dereference), but `source_manifest.json` entries only record `path`, `size`, `sha256`. After bundling, there's no way to tell whether an entry was originally a symlink, what its target was, or whether it was dereferenced. This makes bundle reproducibility debugging nearly impossible.
**Fix:** Add optional but recommended fields to `source_manifest.json` entries: `type` (`file`|`symlink`|`directory`) and `symlink_target` (string, relative path, present only when `type=symlink` and symlinks are preserved). This enables auditing of symlink handling without breaking existing schemas.

