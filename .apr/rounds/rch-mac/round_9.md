Below are the highest-leverage revisions I’d make to strengthen the lane’s contract clarity, operational robustness, security posture, and performance/UX—without bloating the MVP.

I’m treating your current docs as already very strong; most changes are about (a) removing “undefined behavior” corners, (b) making artifacts more replayable/auditable, and (c) tightening interfaces so future backends/features don’t cause spec drift.

Change 1 — Make schema headers consistent everywhere (prevents silent divergence)
Why this improves the project

Right now PLAN.md later mandates schema_id/created_at for “all JSON artifacts,” but the earlier JobSpec outline doesn’t mention schema_id. That kind of mismatch is how consumers drift: some producers omit a field because the first place they looked didn’t include it.

This change makes the first schema outline reflect the global rule, which:

reduces implementation ambiguity,

makes schema validation in CI easier (no “special cases”),

improves third-party tooling interoperability.

Patch (PLAN.md)
diff
Copy code
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### JobSpec schema outline (v1)
 The canonical JobSpec (`job.json`) MUST include at least these fields:
 
 | Field               | Type     | Description                                      |
 |---------------------|----------|--------------------------------------------------|
 | `schema_version`    | int      | Always `1` for this version                      |
+| `schema_id`         | string   | Stable schema identifier (e.g. `rch-xcode/job@1`) |
 | `run_id`            | string   | Parent run identifier                            |
 | `job_id`            | string   | Unique step job identifier                       |
 | `action`            | string   | `build` or `test`                                |
 | `job_key_inputs`    | object   | Canonical key-object hashed to produce `job_key` |
 | `job_key`           | string   | SHA-256 hex digest of JCS(`job_key_inputs`)      |
 | `effective_config`  | object   | Merged repo + host config snapshot               |
 | `created_at`        | string   | RFC 3339 UTC timestamp                           |

Change 2 — README: add an “At a glance” scope/principles block (aligns operator expectations)
Why this improves the project

Your README is already good, but a fast “mental checksum” block reduces misuses and support load:

clarifies “gate not IDE” (so people don’t expect signing/export),

sets expectations about determinism/audit/safety,

makes it easier for someone skimming to decide if it fits their workflow.

Patch (README.md)
diff
Copy code
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Docs map
 - `PLAN.md` — **normative** contract (classifier, JobSpec, protocol, artifacts, caching, security)
 - `.rch/xcode.toml` — repo-scoped configuration (checked in)
 - `~/.config/rch/workers.toml` — host-scoped worker inventory + credentials
 - `schemas/rch-xcode/*` — machine-readable JSON Schemas for normative artifacts (see `schema_id` in each JSON)
 
+## At a glance
+- **Gate, not IDE:** this lane validates build/test under pinned Xcode; it is not a signing/export/publish pipeline.
+- **Deny-by-default:** false negatives are preferred; interception is intentionally conservative.
+- **Deterministic + auditable:** every job emits schema-versioned artifacts + a stable `job_key` for caching/attestation.
+- **Not a sandbox:** treat the worker like CI; build phases/plugins execute as the worker user.
+
 ## What it is
 An extension to Remote Compilation Helper (RCH) that offloads Xcode build/test to a remote macOS worker (e.g. a Mac mini).
 Execution can use either:
 - **Backend `xcodebuild` (MVP)**, or
 - **Backend `mcp` (preferred)** via XcodeBuildMCP for richer diagnostics/orchestration.
 Both backends MUST emit the same **minimum artifact contract** (see `PLAN.md` → "Backend contract").

Change 3 — Add artifact “two-phase commit” semantics (eliminates partial/ambiguous artifact sets)
Why this improves the project

You already require atomic updates for run_state.json / job_state.json, but the rest of the artifact set can still be partially written when:

the worker crashes mid-write,

a host fetch happens “too early,”

a tail/fetch client reads inconsistent manifest vs files.

A clean fix is to define a “commit marker” for job artifacts:

worker writes everything,

computes/writes manifest.json,

writes attestation.json,

writes job_index.json last (atomic rename),

consumers treat presence of job_index.json as “artifact set committed.”

This turns a lot of race conditions into deterministic, testable behavior.

Patch (PLAN.md)
diff
Copy code
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Artifact indices (normative)
 To make artifact discovery stable for tooling, the system MUST provide:
@@
 `job_index.json` MUST include pointers (relative paths) to the job's:
 - `job.json`, `job_state.json`, `summary.json`, `manifest.json`, `attestation.json`
 - primary human artifacts (`build.log`, `result.xcresult/` when present)
 and MUST record the `artifact_profile` produced.
 
+### Artifact completion + atomicity (normative)
+To prevent consumers from observing partially-written artifact sets, the worker MUST treat the job artifact
+directory as a two-phase commit:
+1) Write all job artifacts to their final relative paths (or write-then-rename per file).
+2) Write `manifest.json` enumerating the final set.
+3) Write `attestation.json` binding `manifest.json` (and any signing material).
+4) Write `job_index.json` **last** and atomically (write-then-rename).
+
+Consumers (host CLI, fetchers, tooling) MUST treat the existence of `job_index.json` as the commit marker that
+the job’s artifact set is complete and internally consistent.

Change 4 — Define run-level exit code aggregation + record raw backend exit status (better automation + debugging)
Why this improves the project

You have stable exit codes (great), but two gaps remain:

What does the CLI exit with for multi-step runs? Tools will depend on that.

When xcodebuild fails, a stable mapping alone isn’t enough for diagnostics; you want the raw backend exit status too.

Adding:

run_summary.json.exit_code (aggregated),

summary.json.backend_exit_code (+ optional signal/termination),
makes automation reliable and debugging faster while preserving your stable taxonomy.

Patch (PLAN.md)
diff
Copy code
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Failure taxonomy
 `summary.json` MUST include:
 - `status`: success | failed | rejected | cancelled
 - `failure_kind`: CLASSIFIER_REJECTED | SSH | TRANSFER | EXECUTOR | XCODEBUILD | MCP | ARTIFACTS | CANCELLED | WORKER_INCOMPATIBLE | BUNDLER | ATTESTATION | WORKER_BUSY
 - `failure_subkind`: optional string for details (e.g. TIMEOUT_OVERALL | TIMEOUT_IDLE | PROTOCOL_ERROR)
 - `exit_code`: stable integer for scripting
+- `backend_exit_code`: integer exit status as reported by the backend process (when started)
+- `backend_term_signal`: optional string (e.g. `"SIGKILL"`) when terminated by signal
 - `human_summary`: short string for console output
@@
 ### Stable exit codes (normative)
@@
 - 93: ATTESTATION
+
+### Run-level exit code (normative)
+For multi-step runs, `run_summary.json` MUST include an `exit_code` suitable for scripting and the host CLI
+MUST exit with that value.
+
+Aggregation rule:
+- If any step has `status=rejected`, run `exit_code` MUST be 10.
+- Else if any step has `status=cancelled`, run `exit_code` MUST be 80.
+- Else if any step has `status=failed`, run `exit_code` MUST be the first failing step’s `exit_code` in run order.
+- Else run `exit_code` MUST be 0.

Change 5 — Add a classifier policy snapshot + hash (makes explain/audit truly replayable)
Why this improves the project

Today the classifier’s behavior depends on merged config + allowlists, but there’s no explicit “policy artifact” capturing exactly what was enforced. That makes it harder to:

debug why something was rejected,

compare behavior across versions,

prove “this job was constrained by policy X.”

A small classifier_policy.json artifact (job-scoped) + policy_sha256 recorded in invocation.json makes the lane auditable and makes explain outputs reproducible.

Patch (PLAN.md)
diff
Copy code
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Classifier (safety gate)
 The classifier MUST:
 - match only supported forms of `xcodebuild` invocations
 - reject unknown flags / actions by default
 - enforce repo config constraints (workspace/project, scheme, destination)
 - emit a machine-readable explanation when rejecting (`summary.json` includes `rejection_reason`)
@@
 ### Sanitized invocation (normative)
 If accepted, the host MUST emit `invocation.json` containing:
 - `original_argv` (as received)
 - `sanitized_argv` (canonical ordering, normalized quoting)
 - `accepted_action` (`build`|`test`)
 - `rejected_flags` (if any; for dry-run/explain)
+ - `classifier_policy_sha256` (sha256 hex of the effective classifier policy snapshot)
 `sanitized_argv` MUST NOT contain:
 - output path overrides
 - script hooks
 - unconstrained destinations
+
+### Classifier policy snapshot (recommended)
+For auditability and replayable `explain`, producers SHOULD emit `classifier_policy.json` per job capturing the
+effective allowlist/denylist and any pinned constraints enforced by the classifier (workspace/project, scheme,
+destination rules, allowed flags).
+`invocation.json.classifier_policy_sha256` SHOULD be the sha256 hex digest of the canonical JSON bytes of that snapshot.

Change 6 — Introduce explicit upload/bundle limits + advertise them in capabilities (prevents DoS foot-guns)
Why this improves the project

You already have bundle.max_bytes in README examples, but the normative spec doesn’t define:

who enforces it,

what happens when exceeded,

whether workers advertise hard caps.

This becomes important the first time someone points at a monorepo, or a malicious client tries to upload 200GB.

Defining limits as part of capabilities.json and enforcing them at both ends:

prevents resource exhaustion,

makes failures deterministic (failure_kind=BUNDLER / TRANSFER),

makes “doctor” checks meaningful.

Patch (PLAN.md)
diff
Copy code
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Source bundling canonicalization (normative)
 The host MUST create a canonical source bundle such that identical inputs yield identical `source_sha256`.
@@
 Transport note (non-normative but recommended):
 - The canonical tar MAY be compressed with zstd for transfer, but `source_sha256` MUST be computed
   over the canonical (pre-compression) tar bytes.
+
+### Bundle size limits (normative)
+The host MUST enforce an upper bound on bundle size to prevent accidental or malicious resource exhaustion.
+If `.rch/xcode.toml` sets `bundle.max_bytes`, the host MUST reject bundles that exceed it with `failure_kind=BUNDLER`.
+Workers MAY additionally enforce a hard maximum upload size; if exceeded, the worker MUST reject with a structured
+error (`failure_kind=TRANSFER` or `failure_kind=PROTOCOL_ERROR`) and include the limit in diagnostics.
@@
 ## Worker capabilities
 Worker reports a `capabilities.json` including:
@@
 - installed tooling versions (rch-worker, XcodeBuildMCP)
 - capacity (max concurrent jobs, disk free)
+ - limits (recommended): `max_upload_bytes`, `max_artifact_bytes`, `max_log_bytes`
 Normative:
 - `capabilities.json` MUST include `schema_version` and `created_at` (RFC 3339 UTC).

Change 7 — Make env allowlist/redaction auditable with a dedicated artifact (security posture you can prove)
Why this improves the project

You correctly state “env allowlist (drop-by-default)” and “redact secrets,” but without an artifact that shows what happened, you can’t:

confirm which env keys actually reached the backend,

prove secrets weren’t leaked,

debug “works locally but not in lane” env issues.

Adding an executor_env.json artifact (keys only by default) gives you:

deterministic auditability,

better debugging,

a place to document/env-pin behavior without stuffing it into logs.

Patch (PLAN.md)
diff
Copy code
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Execution environment (normative)
 - Worker MUST execute each job in an isolated working directory unique per `job_id`.
 - Worker MUST set `DEVELOPER_DIR` to the resolved toolchain `developer_dir` prior to execution.
 - Worker MUST apply an environment allowlist (drop-by-default) when launching the backend.
 - Worker MUST redact secrets from logs/artifacts to the extent feasible (at minimum: do not emit env vars outside the allowlist).
+
+### Executor environment audit (recommended)
+The worker SHOULD emit `executor_env.json` containing:
+- `schema_version`, `schema_id`, `created_at`, `run_id`, `job_id`, `job_key`
+- `passed_keys[]`: environment variable keys passed to the backend (no values by default)
+- `dropped_keys[]`: keys present in the worker process environment but intentionally not passed
+- `overrides[]`: keys the worker set explicitly (e.g. `DEVELOPER_DIR`)
+If values are ever recorded (not recommended), they MUST be explicitly opt-in and MUST be redacted by default.

Change 8 — Specify cache directory layout formulas (prevents collisions; makes cleanup/GC sane)
Why this improves the project

You have correct high-level cache keying requirements, but without a layout contract you’ll end up with:

ad hoc directory naming,

collisions across repos/workers,

brittle GC/eviction code.

A minimal layout spec keeps caches predictable and makes metrics.json easier to reason about.

Patch (PLAN.md)
diff
Copy code
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Caching
 Caching MUST be correctness-preserving:
@@
 ### Cache keying details (normative)
 - Any cache directory that can be reused across jobs MUST be additionally keyed by toolchain identity
   (at minimum: Xcode build number and macOS major version) to prevent cross-toolchain corruption.
 - `metrics.json` SHOULD record the concrete cache key components used (job_key, xcode_build, macos_version, etc.).
+
+### Cache directory layout (recommended, stabilizes operability)
+To prevent collisions and simplify GC, workers SHOULD use a predictable cache root layout such as:
+- `caches/<namespace>/spm/<toolchain_key>/...`
+- `caches/<namespace>/derived_data/shared/<toolchain_key>/...`
+- `caches/<namespace>/derived_data/per_job/<job_key>/...`
+Where:
+- `namespace` is `cache.namespace` (filesystem-safe)
+- `toolchain_key` is a filesystem-safe composite derived from toolchain identity (e.g. `xcode_<build>__macos_<major>__<arch>`)
+Workers SHOULD record the resolved cache paths in `metrics.json` for transparency.

Change 9 — Standardize RPC error.code values (improves client behavior + retry logic)
Why this improves the project

You already have a good envelope shape, but without a stable error code registry you’ll end up encoding behavior in free-form strings (hard to automate, hard to test).

A small, normative set of error.code values makes:

retries safe (BUSY, LEASE_EXPIRED),

user messaging consistent (INVALID_REQUEST, UNSUPPORTED_PROTOCOL),

conformance tests straightforward.

Patch (PLAN.md)
diff
Copy code
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### RPC envelope (normative, recommended)
 All worker operations SHOULD accept a single JSON request on stdin and emit a single JSON response on stdout.
@@
 Response:
 - `protocol_version` (int; selected by host after probe)
 - `request_id` (string)
 - `ok` (bool)
 - `payload` (object, when ok=true)
 - `error` (object, when ok=false) containing: `code`, `message`, optional `data`
+
+#### Error codes (recommended, stabilizes automation)
+Workers SHOULD restrict `error.code` to a small, documented registry (examples):
+- `INVALID_REQUEST` (malformed JSON, missing fields)
+- `UNSUPPORTED_PROTOCOL` (no version intersection)
+- `FEATURE_MISSING` (required feature absent)
+- `BUSY` (capacity exceeded; include `retry_after_seconds`)
+- `
