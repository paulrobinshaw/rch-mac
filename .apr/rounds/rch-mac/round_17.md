# APR Round 17 — Spec Review (Hostile Implementer / Fuzz Perspective)

**Reviewer role:** Spec reviewer (Claude)
**Focus:** Interop-breaking ambiguities, fuzz-test edge cases, under-specified behavior that two independent implementations would disagree on.

## Findings

### 1. manifest.json directory entries: sha256/size undefined
**Severity:** Interop-breaking
**Location:** PLAN.md → "Artifact manifest"

The inclusion rule says entries MUST include "every file and directory" but `sha256` and `size` are only meaningful for files. Two implementations would produce different manifest entries for directories (empty sha256? omitted? sha256 of empty string? size=0?). This breaks `artifact_root_sha256` determinism.

**Fix:** Define directory entry semantics: `type` field (`file`|`directory`), directories use `sha256: null` and `size: 0`, entries sorted by path as before.

### 2. `fetch` response conflicts with RPC envelope
**Severity:** Interop-breaking
**Location:** PLAN.md → "fetch response format" vs "RPC envelope"

The RPC envelope spec says all responses have `{protocol_version, request_id, ok, payload}` on stdout. But `fetch` uses binary framing (JSON header line + raw bytes). A compliant implementation could wrap the binary-framed response inside the standard envelope (double JSON), or bypass the envelope entirely. The spec doesn't say which.

**Fix:** Explicitly state that `fetch` (and any binary-framed response) replaces the standard envelope — the JSON header line IS the response (containing `ok`, `request_id`, `protocol_version` alongside `stream`). No outer envelope wrapping.

### 3. `probe` response `protocol_version` value undefined
**Severity:** Interop ambiguity
**Location:** PLAN.md → "Protocol version bootstrap"

Request uses sentinel `protocol_version: 0`. What does the worker put in the response `protocol_version` field? Options: echo `0`, use `protocol_max`, omit it. Two implementations would disagree.

**Fix:** Worker MUST echo `0` in probe responses (since no version is negotiated yet).

### 4. Rejected steps: required `job_key` field may not exist
**Severity:** Schema violation
**Location:** PLAN.md → "Classifier rejection in multi-step runs" + "summary.json schema"

Rejected steps get a `summary.json` with `status=rejected`. But `job_key` is listed as a required field in the summary schema. For rejected steps, the classifier fires before job_key computation (source bundling may not have happened). The spec doesn't say whether job_key is still computed or what value to use.

**Fix:** For rejected steps, `job_key` MUST be set to `null` in `summary.json`. Consumers MUST accept `null` for `job_key` when `status=rejected`.

### 5. `run_plan.json` schema missing `rejected` field
**Severity:** Interop ambiguity
**Location:** PLAN.md → "Run plan artifact" vs "Classifier rejection"

The classifier rejection section says rejected steps appear in `run_plan.json` with `rejected: true`, but the run_plan schema defines steps as only `{ index, action, job_id }`. The `rejected` field isn't in the schema.

**Fix:** Add `rejected` (bool, default false) to the step schema in run_plan.json.

### 6. `run_summary.json` status field aggregation undefined
**Severity:** Interop ambiguity
**Location:** PLAN.md → "Run-level exit code"

The exit_code aggregation is well-defined, but the `status` field aggregation is not. If step 1 is `rejected` and step 2 is `failed`, what is `run_summary.status`? The exit code would be 10 (rejected takes priority), but does status follow the same priority? The status enum includes `rejected` but the mapping isn't stated.

**Fix:** Add explicit status aggregation: priority order is `rejected` > `cancelled` > `failed` > `success`. Status follows the same priority as exit_code aggregation.

### 7. Binary framing: newline in JSON header edge case
**Severity:** Fuzz-test failure
**Location:** PLAN.md → "Binary framing"

The spec says "a single UTF-8 JSON header line terminated by `\n`" but doesn't forbid newlines within JSON string values in the header. A fuzzer could put `\n` in `source_sha256` or other string fields, breaking naive line-based parsers. 

**Fix:** The JSON header MUST be serialized as a single line (no unescaped newline characters within JSON string values). Implementations MUST use standard JSON string escaping which already escapes `\n` as `\\n`, but state this explicitly to prevent implementations that use pretty-printed JSON.

## Patches Applied
- PLAN.md: All 7 fixes above
- README.md: No changes needed (non-normative)
