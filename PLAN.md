# RCH Xcode Lane — Specification

> **This document is normative.** It defines the contract for the RCH Xcode Lane: configuration semantics, job lifecycle, artifacts, safety rules, and performance requirements. If README.md conflicts with this document, this document wins.

## Vision

Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker via a stable worker harness (`rch-xcode-worker`), producing deterministic, machine-readable results. The harness may use XcodeBuildMCP or `xcodebuild` as its backend, but the host speaks one protocol and always receives structured NDJSON events.

## Goals

- Remote Xcode build/test via macOS workers only
- Deterministic configuration and attestation
- Agent-friendly JSON outputs and artifacts
- Safe-by-default interception (false negatives preferred)

---

## Terms

- **Host**: Machine running `rch` client + daemon.
- **Worker**: macOS machine running jobs over SSH.
- **Stage Root**: A worker-confined root (`probe.roots.stage_root`) where the host can upload a source snapshot using a restricted stage key.
- **Job**: One remote build/test execution with stable, addressable artifacts.
- **Profile**: Named configuration block (e.g., `ci`, `local`, `release`).
- **Lane**: The Xcode-specific subsystem within RCH.
- **Run ID**: Content-derived identifier (SHA-256 of canonical **config inputs** + source tree hash); identical inputs produce the same run ID.
- **Job ID**: Unique identifier per execution attempt; never reused.
- **Repo Key**: A stable *host-side* namespace used to segregate artifacts and (optionally) caches per repository/workspace. It MUST NOT be included in `effective_config.inputs` (so it does not affect `run_id`), but MUST be recorded in `attestation.json`. Derived from VCS identity when available (e.g., normalized origin URL hash), otherwise from a workspace identity hash.
- **Repo Identity**: A cross-host stable identity for cache partitioning and audit (e.g., normalized VCS origin + optional repo fingerprint). See Cache Namespace section.
- **Cache Namespace**: A worker-visible, explicit namespace string used to segregate caches per trust boundary. Computed by the host and supplied in the job request.
- **Contract Version**: A stable semantic version of the *lane contract* that is included in `effective_config.inputs` (and therefore influences `run_id`). It MUST be bumped whenever the meaning of any hashed input changes (defaults, canonicalization, policy semantics, backend argument reconstruction), even if `lane_version` changes for other reasons.

---

## Versioning & Compatibility (Normative)

### Version Streams

There are four distinct version streams:

- **`protocol_version`**: Negotiated for host↔harness verbs (`probe`, `run`, `cancel`, etc.). Backward compatible changes MUST NOT change the meaning of existing fields.
- **`contract_version`**: Required overlap; defines semantics of `effective_config.inputs` (hashed). Any semantic change to hashed inputs MUST bump `contract_version`. This includes changes to domain separation prefixes used in hash computations (`run_id`, `config_hash`, `source_tree_hash`, `capabilities_sha256`, event chain, etc.).
- **`lane_version`**: Implementation/spec version. MAY change without changing `contract_version`.
- **`schema_version`**: Per-artifact schema version. Validators MUST key off `kind` + `schema_version`.

### Negotiation Rules

- Host MUST select the **highest** common `protocol_version` from the intersection of host-supported and harness-advertised versions.
- Host MUST refuse if the job's `effective_config.inputs.contract_version` is not present in the harness `contract_versions`. Error code: `contract_version_unsupported`.
- Host MUST record the selected `protocol_version` and enforced `contract_version` in `attestation.json`.

### Deprecation Policy

- Harness/host SHOULD support at least the current and immediately prior **minor** `protocol_version` (when feasible).
- Harness MUST be allowed to support multiple `contract_versions` concurrently, but MUST advertise them explicitly in `probe.contract_versions`.
- When dropping support for a `contract_version`, implementations SHOULD provide at least one release cycle of deprecation warning.

---

## Core Architecture

### Host (RCH daemon)

- Classifies commands (strict allowlist)
- Selects an eligible worker (tags: `macos,xcode`)
- Syncs workspace snapshot (data-plane staging; rsync or bundle depending on profile)
- Executes by invoking the worker harness (`rch-xcode-worker run`) over SSH (single protocol)
- Collects artifacts back to host (data-plane fetch); emits manifest + attestation

### Worker (macOS)

- Provides a stable Xcode + simulator environment
- Maintains caches (DerivedData, SPM) keyed by effective config
- Returns xcresult + logs + tool metadata

### Worker Harness (Normative)

The lane MUST use the `rch-xcode-worker` harness for remote execution. This is a lightweight executable invoked over SSH that accepts a job request on stdin and emits structured results on stdout. The harness may use XcodeBuildMCP or `xcodebuild` as its backend, but the host always speaks one protocol and always receives a consistent event/log contract.

**Protocol (Verbs + Versioning):**

The harness exposes a small set of verbs. Implementations MUST be explicit about what is supported via `probe.verbs`.

**Verb requirement matrix (Normative):**

| Verb | Always required | Required when `rch-xcode-worker --forced` is used |
|------|------------------|---------------------------------------------------|
| `probe` | ✅ | ✅ |
| `run` | ✅ | ✅ |
| `cancel` | Optional | ✅ |
| `status` | Optional | ✅ |
| `cache_query` | Optional | Optional |
| `prewarm` | Optional | Optional |

The harness MUST support two verbs:

1) `rch-xcode-worker probe`
   - Emits a single JSON object to stdout (capabilities) and exits 0 on success.
   - Used by `rch xcode verify` and worker selection.

**Probe output schema (Normative):**

The probe JSON MUST be a self-describing, versioned object and MUST include at minimum:
- `kind` (string) — MUST be `"probe"`.
- `schema_version` (SemVer string) — Schema version for the probe payload (e.g., `"1.0.0"`).
- `protocol_versions` (array of strings, e.g. `["1"]`) — Supported protocol versions.
- `contract_versions` (array of SemVer strings) — Supported values of `effective_config.inputs.contract_version`.
- `harness_version` (SemVer string) — Version of the `rch-xcode-worker` harness.
- `harness_binary_sha256` (string) — Lowercase hex SHA-256 of the harness executable bytes (or signed package payload).
- `codesign` (object|null) — Best-effort code signing identity info when available:
  - `identifier` (string|null)
  - `team_id` (string|null)
  - `signing_sha256` (string|null) — Optional digest of the signing blob/requirement.
  - `requirement_sha256` (string|null) — SHA-256 of the designated requirement string (or equivalent stable form). Provides a stronger identity pin than `team_id` alone.
- `lane_version` (SemVer string) — Lane specification version the harness implements.
- `verbs` (array of strings) — Supported verbs, e.g. `["probe","run","cancel","status"]` (and optionally `"cache_query"`, `"prewarm"`).
- `features` (object) — Feature flags/capabilities beyond versions, e.g.:
  - `event_hash_chain` (boolean) — Whether hash-chained events are supported when enabled in config.
  - `cache_query` (boolean) — Whether the optional cache query verb is supported (see Cache Query verb below).
  - `cache_namespace` (boolean) — Whether the harness supports explicit cache namespaces supplied by the host.
  - `inline_redaction` (boolean) — Whether the harness can apply configured redaction inline to stdout/stderr and durable artifacts (`events.ndjson`, `build.log`) before integrity hashing.
- `capabilities_sha256` (string) — Lowercase hex SHA-256 of **stable** capability fields (MUST exclude `load` and `health`). Enables capability drift detection.

**`capabilities_sha256` computation (Normative):**

The harness MUST compute `capabilities_sha256` as:
`capabilities_sha256 = SHA-256( "rch-xcode-worker/capabilities_sha256/v1\n" || JCS(stable_capabilities) )` (lowercase hex),
where `stable_capabilities` is an object containing ONLY stable fields:
- `kind`, `schema_version`, `protocol_versions`, `contract_versions`
- `harness_version`, `harness_binary_sha256`, `codesign`
- `lane_version`, `verbs`, `features`, `limits`
- `worker`, `xcode`, `simulators`, `backends`, `event_capabilities`, `roots`

and MUST exclude volatile fields such as `load` and `health`.

Before applying JCS, the harness MUST normalize any set-like arrays inside `stable_capabilities`
(remove duplicates, sort lexicographically by UTF-8 byte order). If these rules change, the harness
MUST bump `probe.schema_version`.
- `worker` (object) — Stable identifiers: `hostname`, optional `machine_id`.
- `xcode` (object) — `path`, `version`, `build` (Xcode build number).
- `simulators` (object) — Available runtimes + device types (Normative minimum):
  - `runtimes` (array) — entries include `runtime_id`, `name`, `version`, optional `build`
  - `device_types` (array) — entries include `device_type_id`, `name`
  The probe MUST NOT be required to enumerate all *devices* / UDIDs.
- `backends` (object) — `xcodebuildmcp` availability + version, `xcodebuild` availability.
- `event_capabilities` (object) — Declares which event categories the harness can emit (e.g., phases/tests/diagnostics). Enables the host/agents to understand expected event richness.
- `limits` (object) — `max_concurrent_jobs`, optional disk/space hints.
- `load` (object) — Best-effort current load snapshot (**volatile; excluded from `capabilities_sha256`**):
  - `active_jobs` (integer) — Number of jobs currently executing.
  - `queued_jobs` (integer) — Number of jobs waiting for a lease slot.
  - `updated_at` (ISO 8601 timestamp) — When this snapshot was taken.
- `roots` (object) — Canonical harness roots used to derive per-job paths (especially in `--forced` mode): `stage_root`, `jobs_root`, `cache_root`.
- `health` (object) — Best-effort worker health snapshot (**volatile; excluded from `capabilities_sha256`**):
  - `disk_free_bytes` (integer|null)
  - `disk_total_bytes` (integer|null)
  - `degraded` (boolean) — true if the harness believes the worker is not in a healthy execution state
  - `notes` (array of strings) — human hints; MUST NOT contain secrets

The host MUST select a `protocol_version` supported by both sides. If no overlap exists, the lane MUST refuse with stable error code `protocol_version_unsupported`.

The host MUST also ensure the job's `effective_config.inputs.contract_version` is present in the harness `contract_versions`. If unsupported, the lane MUST refuse with stable error code `contract_version_unsupported`.

**Capability drift (Recommended):**
- The host SHOULD record `capabilities_sha256` in `attestation.json`.
- Profiles MAY require `capabilities_sha256` stability across a CI run to detect worker/toolchain drift early (report via `decision.json`/`summary.json.errors[]`).

2) `rch-xcode-worker run`
   - Reads exactly one JSON object (the job request) from stdin.
   - Emits NDJSON events to stdout (one JSON object per line).
   - Exits after emitting a terminal `complete` event.

The harness SHOULD support a third verb:

3) `rch-xcode-worker cancel`
   - Reads a single JSON object from stdin containing at minimum `job_id` (and SHOULD include `lease_id` if known).
   - Terminates the active backend process group for that job (SIGTERM, then SIGKILL after 10s).
   - Emits a single self-describing JSON object to stdout describing whether a process was found and terminated.
   - **Idempotency (Normative):** If the job is already terminal (or no backend process is found), `cancel` MUST still exit 0 and report
     `{ "ok": true, "already_terminal": true }` (or `{ "ok": true, "found": false }`) rather than failing.
   - **REQUIRED for deployments using `rch-xcode-worker --forced`** so the host can cancel without arbitrary remote commands.

**Cancel/status bookkeeping (Normative):**
- During `run`, the harness MUST write a small, non-secret control record under the per-job workspace (e.g., `jobs_root/<job_id>/control.json`)
  containing at minimum `{ job_id, lease_id, backend_pid, backend_pgid, started_at }`. This enables `cancel` and `status` to operate
  without scanning process tables or relying on transport state.

The harness MAY support an optional fourth verb:

4) `rch-xcode-worker cache_query` (Optional; Recommended)
   - Reads a single JSON object from stdin containing at minimum:
     - `protocol_version`
     - `config_hash` (lowercase hex SHA-256 of `JCS(effective_config.inputs)`)
     - `kinds` (array of strings) — e.g. `["derived_data","spm"]`
     - `cache_namespace` (string) — REQUIRED when `features.cache_namespace=true`
   - Emits a single JSON object to stdout and exits 0 on success:
     - `config_hash`
     - `cache_namespace` (string) — effective cache namespace/trust boundary
     - `namespace` (string) — DEPRECATED alias for `cache_namespace` (for backward compatibility)
     - `derived_data` (object|null): `{ "present": bool, "bytes": int|null, "mtime": string|null }`
     - `spm` (object|null): `{ "present": bool, "bytes": int|null, "mtime": string|null }`
   - MUST NOT reveal any paths outside configured cache roots.
   - Enables warm-cache worker selection (see Worker Selection below).

The harness MAY support an optional fifth verb:

5) `rch-xcode-worker prewarm` (Optional; Recommended)
   - Reads a single JSON object from stdin containing:
     - `protocol_version`
     - `destination` (optional) — runtime/device identifiers to prewarm
     - `kinds` (array) — e.g., `["simulator_boot", "spm_prefetch"]`
   - Emits a single JSON object to stdout and exits 0 on success:
     - `ok` (boolean)
     - `performed` (array) — list of prewarm actions completed
     - `warnings` (array) — non-fatal issues encountered
   - MUST NOT mutate shared caches when `trust.posture` is `"untrusted"` (best-effort).
   - Enables reducing cold-start tax for CI and agent loops.

The harness SHOULD support a sixth verb:

6) `rch-xcode-worker status` (Normative when `--forced` is used; optional otherwise)
   - Reads a single JSON object from stdin containing at minimum `job_id`.
   - Emits a single JSON object describing best-effort job state (`queued`, `running`, `terminal`, `unknown`),
     and the latest observed `events.sequence` written to disk (when available).
   - Enables watch/fetch resume after transport drops without arbitrary remote commands.
   - MUST NOT reveal paths outside configured roots.
   - **REQUIRED for deployments using `rch-xcode-worker --forced`** so the host can recover from transport drops without arbitrary remote commands.

**Status output schema (Normative):**

The status verb MUST emit a single self-describing JSON object:
- `kind` = `"status"`
- `schema_version` (SemVer string)
- `job_id`, `run_id`, `attempt`
- `state` (string) — `queued` | `running` | `terminal` | `unknown`
- `updated_at` (ISO 8601 string|null)
- `latest_sequence` (integer|null) — last sequence written to durable `events.ndjson` (best-effort)
- `events_bytes` (integer|null) — best-effort current byte size of `events.ndjson` (for resume tooling)
- `build_log_bytes` (integer|null) — best-effort current byte size of `build.log` (for resume tooling)
- `lease_id` (string|null)
- `hints` (array of strings) — human-safe hints (MUST NOT contain secrets)
- `terminal` (object|null) — Present only when `state="terminal"`:
  - `state` (string) — `succeeded` | `failed` | `canceled` | `timed_out`
  - `exit_code` (integer|null)
  - `error_code` (string|null)
  - `errors` (array) — array of Error Objects (MAY be truncated; MUST remain JSON-valid)

**Job request (stdin) MUST be a self-describing, versioned object and MUST include:**
- `kind` (string) — MUST be `"job_request"`.
- `schema_version` (SemVer string) — Schema version for the job request payload (e.g., `"1.0.0"`).
- `protocol_version` (string, e.g. `"1"`)
- `job_id`, `run_id`, `attempt`
- `source_tree_hash` (string) — Lowercase hex hash of the staged source snapshot (`source_manifest.entries` hash). Used for staging verification and (optionally) for recomputing/validating `run_id`.
- `config_inputs` (object) — exact copy of `effective_config.inputs`
- `config_resolved` (object) — execution-time resolved fields required to run (e.g., xcode path, resolved destination IDs).
  The destination UDID MAY be omitted; when omitted, the harness MUST resolve/select/create an appropriate simulator device
  according to `effective_config.inputs.simulator.*` and record the chosen UDID under `effective_config.resolved`.
  - MUST include `repo_identity` and `cache_namespace` when caching is enabled (see Cache Namespace section).
- `paths` (object). In `--forced` mode, the harness MUST ignore any host-supplied absolute paths and MUST derive paths from configured roots + `job_id` (see Path Confinement below).
  - `src`, `work`, `dd`, `result`, `spm`, `cache` MUST be representable (even if some are aliases of others depending on cache policy).

**Job request (stdin) SHOULD include:**
- `trace` (object) — correlation identifiers for cross-system log correlation:
  - `trace_id` (string) — stable ID for correlating host/harness/CI logs across attempts
- `required_features` (array of strings) — policy-critical features the host expects the harness to enforce.
  The harness MUST refuse if any required feature is unsupported (see Required Feature Negotiation below).
- `job_request_sha256` (string) — Lowercase hex SHA-256 of `"rch-xcode-lane/job_request_sha256/v1\n" || JCS(job_request_without_digest)` computed by the host, where `job_request_without_digest` is the full job request object with the `job_request_sha256` field removed. The harness MUST echo this value in `hello` and `complete` so the host can detect any request corruption.

**Path Confinement (Normative):**
- In `--forced` mode, the harness MUST compute `src/`, `work/`, `dd/` and related paths solely from (`jobs_root`, `stage_root`, `cache_root`) + `job_id` (and optional stable subpaths).
- The harness MUST validate `job_id`, `run_id`, and `attempt` before using them for path derivation:
  - `job_id` MUST match `^[0-9a-fA-F-]{16,64}$` (UUID v7 recommended).
  - `run_id` MUST be 64 lowercase hex chars.
  - `attempt` MUST be an integer >= 1.
  On validation failure: fail with stable error code `invalid_job_identity`.
- The harness MUST reject any request whose effective paths would escape the configured roots (after resolving `..` and symlinks) with stable error code `path_out_of_bounds`.
- The harness MUST treat `paths` from the host as *hints only* (or ignore entirely) when `--forced` is active.

**Job ID single-use (Normative):**
- The harness MUST treat `job_id` as single-use.
- If the derived job workspace root already exists and appears to contain a prior run (e.g., has a terminal `complete` event in `events.ndjson` or a non-empty artifact directory), the harness MUST refuse with stable error code `job_id_reused`.
- The harness MUST NOT overwrite or "recycle" an existing job workspace.

**Backend Selection (Normative):**
- `effective_config.backend.preferred` MAY be `"xcodebuildmcp"` or `"xcodebuild"`.
- The harness MUST select an available backend consistent with policy and capabilities.
- The harness MUST record the chosen backend in `effective_config.json` as `backend.actual`.
- If the preferred backend is unavailable and fallback is disallowed, the harness MUST fail with a terminal `complete` event and stable error code `backend_unavailable`.

**Stdout/Stderr Separation (Normative):**
- Stdout MUST contain ONLY NDJSON event objects. No banners, no debug lines, no progress text, no non-JSON output.
- Stderr MAY contain human-readable logs and MUST be treated as the source for `build.log` capture/streaming.
- The host MUST capture stderr separately and MUST NOT attempt to parse it as JSON.
- The harness SHOULD route backend build output (e.g., `xcodebuild` stdout/stderr) into harness stderr so it is captured in `build.log`.

**Durable stream tee (Normative):**

The harness MUST tee (write) the same bytes it emits to durable files under the per-job workspace root:
- `events.ndjson` — Same content as stdout (NDJSON events), written as each event is emitted.
- `build.log` — Same content as harness stderr (human logs + backend output).

This ensures runs remain inspectable and artifacts are fetchable even if the SSH session is interrupted mid-execution. The `status` verb (when supported) can report the latest durable `events.sequence` to enable resume.

---

## Staging Contract (Normative)

Deployments commonly use split SSH keys:
- **Stage key** (write-only) confined to `stage_root/` to upload source
- **Run key** (forced-command) confined to `rch-xcode-worker --forced`
- **Fetch key** (read-only) confined to `jobs_root/` to pull artifacts

To make this safe and deterministic, staging MUST be an explicit contract between host and harness.

### Worker Stage Layout

For a given `job_id`, the host MUST stage into:
- `stage_root/<job_id>/src.tmp/` (upload target)
- then atomically rename to `stage_root/<job_id>/src/`

After the rename, the host MUST write (atomically):
- `stage_root/<job_id>/stage_receipt.json` (self-describing JSON; schema below)
- `stage_root/<job_id>/STAGE_READY` (empty sentinel file)

**Atomic write rule (Normative):**
- The host MUST write `stage_receipt.json` via `stage_receipt.json.tmp` + atomic rename.
- The host MUST create `STAGE_READY` via `STAGE_READY.tmp` + atomic rename (empty file).

The harness MUST treat staging as complete **only** when `STAGE_READY` exists.

### `stage_receipt.json` (Normative)

The stage receipt MUST be a self-describing JSON object:
```jsonc
{
  "kind": "stage_receipt",
  "schema_version": "1.0.0",
  "lane_version": "0.1.0",
  "job_id": "…",
  "run_id": "…",
  "attempt": 1,
  "method": "rsync|git_snapshot",
  "source_tree_hash": "…",
  "excludes": ["…"],
  "files_total": 789,
  "bytes_total": 987654321,
  "bytes_sent": 123,
  "files_changed": 456,
  "created_at": "2026-02-08T00:00:00Z"
}
```

### Harness Staging Behavior (Normative)

Before backend execution, the harness MUST:
1. Refuse with stable error code `source_staging_incomplete` if `STAGE_READY` or `stage_receipt.json` is missing.
2. Refuse with stable error code `stage_receipt_mismatch` if `stage_receipt.json.job_id/run_id/attempt` or `stage_receipt.json.source_tree_hash` do not match the job request (`job_id/run_id/attempt/source_tree_hash`).
3. Copy `stage_receipt.json` into the job workspace as `stage_receipt.json` for audit.
3. Populate the job `src/` by atomically swapping a fully materialized tree:
   - Recommended: stage to `jobs_root/<job_id>/src.tmp/` then rename to `src/`
4. Apply the existing **read-only source** rule to the final `src/`.

If `[profiles.<name>.source].verify_after_stage = true`, the harness MUST emit `stage_verification.json`
recording expected vs observed hashes and fail with `source_hash_mismatch` when they differ.

---

## Required Feature Negotiation (Normative)

The probe `features` object is the authoritative declaration of optional harness capabilities.
If the host relies on a policy-affecting optional capability, it MUST list it in `job_request.required_features`.

The harness MUST:
1. Compare `job_request.required_features[]` against `probe.features`
2. Refuse before execution with stable error code `required_feature_unsupported` on any missing feature

This prevents "silent downgrade" when new safety knobs are introduced. Without this, an older harness might silently ignore a new enforcement field (e.g., `sandbox.network = deny_all` or `redaction.enabled = true` requiring `features.inline_redaction`), causing the job to run unsafely or leak secrets.

---

## Cache Namespace (Normative)

When `cache.derived_data` and/or `cache.spm` is enabled, the host MUST compute and supply a `cache_namespace`
string under `job_request.config_resolved.cache_namespace`. The harness MUST use it to segregate all cache reads/writes.

The harness MUST refuse with stable error code `cache_namespace_missing` if caching is enabled but
`cache_namespace` is absent or empty.

### `repo_identity` (Normative)

The host MUST supply `job_request.config_resolved.repo_identity` as:
```jsonc
{ "kind": "git", "origin": "<normalized-url>", "repo_fingerprint_sha256": "<optional>" }
```

`origin` normalization MUST:
- Strip credentials/userinfo
- Lowercase hostname
- Remove trailing `.git`
- Normalize ssh/scp-like forms to `ssh://host/path`

### Cache Namespace Derivation (Recommended; MUST be recorded when used)

```text
cache_namespace = "rch-xcode-lane/cache_ns/v1/" +
  SHA256( normalized_origin + "\n" + trust_posture + "\n" + trust_domain + "\n" + profile_name )
```

The host MUST record the computed `cache_namespace` in `metrics.json` and `attestation.json`.

---

**Event stream (stdout) requirements:**
- The FIRST event MUST be `{"type":"hello", ...}` and MUST include `protocol_version`, `lane_version`, `contract_version`, and the echoed `job_id`/`run_id`/`attempt`.
- The `hello` event MUST include `event_schema_version` (SemVer string) declaring the schema for all subsequent NDJSON events in the stream.
- The `hello` event MUST include `job_request_sha256` when provided in the job request.
- The `hello` event MUST include `worker_paths` (object) describing the *actual derived* paths in use on the worker (`src`, `work`, `dd`, `result`, `spm`, `cache`) so audits/debugging can prove where execution happened.
- Every event MUST include: `type`, `timestamp`, `sequence`, `job_id`, `run_id`, `attempt`.
- Every event SHOULD include: `trace_id` — when provided in the job request, for cross-system correlation.
- Every event SHOULD include: `monotonic_ms` — Milliseconds since harness process start (monotonic clock) to provide stable ordering under clock skew.
- The FINAL event MUST be `{"type":"complete", ...}` and MUST be the last line. After emitting `complete`, the harness MUST exit promptly.

**`complete` event payload (Normative):**

The terminal `complete` event MUST include:
- `exit_code` (integer)
- `state` (string) — one of: `succeeded`, `failed`, `canceled`, `timed_out`
- `error_code` (string|null) — primary stable error code (null on success)
- `errors` (array) — array of error objects from the Error Model (may be empty on success)
- `backend` (object) — `{ "preferred": "...", "actual": "..." }`
- `events_sha256` (string|null) — digest over emitted event bytes (excluding the `complete` line itself)
- `event_chain_head_sha256` (string|null) — when hash chain enabled, the `event_sha256` of the last non-`complete` event
- `artifact_summary` (object) — small summary, e.g. `{ "xcresult": true, "xcresult_format": "directory|tar.zst" }`
- `job_request_sha256` (string|null) — Echoed from `hello` when present; null if not provided in the job request.

This enables streaming consumers to know the terminal outcome without waiting for artifact assembly.

**Robustness (Normative):**
- The harness MUST NOT emit partial JSON lines or unterminated objects to stdout.
- If the harness detects it cannot encode a non-terminal event as valid JSON, it MUST attempt to:
  1) emit a terminal `complete` event with `error_code="event_stream_corrupt"`, then
  2) exit promptly.
- If the harness cannot emit a valid terminal `complete` event, it MUST exit non-zero and write a diagnostic marker to stderr.
  In this case, the host MUST record `event_stream_corrupt` in `summary.json` and include an error object indicating the stream ended without a valid terminal event.

**Benefits:**

- Decouples transport (SSH) from execution logic.
- Enables future transports (local socket, mTLS) without changing the worker.
- Structured output avoids fragile log parsing.
- Harness can enforce per-job resource limits and timeouts locally.

**Forced-command mode (Strongly Recommended):**

Operators SHOULD use an SSH key restricted via `authorized_keys command=...` so the remote account cannot execute arbitrary commands. The harness MUST support `rch-xcode-worker --forced`, which:

- Reads the requested verb from `SSH_ORIGINAL_COMMAND`
- Allows ONLY `probe`, `run`, `cancel`, `status`, and optionally `cache_query` (exact match, no extra args) and rejects anything else with `complete` + error code `forbidden_ssh_command`
- Ignores argv verbs when `--forced` is set (to prevent bypass)
- **Note:** `cancel` and `status` MUST be supported in `--forced` mode to enable cancellation + recovery without arbitrary remote commands

---

## Configuration Model

### Repo Config: `.rch/xcode.toml`

Configuration uses **named profiles** (`[profiles.<name>]`). Select a profile with `--profile <name>`.

### Structured Xcode Test Controls (Normative)

Profiles MAY define an `xcode_test` table to control *test selection* without permitting arbitrary flag passthrough:

```toml
[profiles.ci.xcode_test]
test_plan = "CI"                          # optional
only_testing = ["MyAppTests/FooTests"]    # optional array (replaces, no concat)
skip_testing = ["MyAppTests/FlakyTests"]  # optional array (replaces, no concat)
```

- These fields MUST be included in `effective_config.inputs` when present.
- The harness MUST translate these into backend-specific invocations (XcodeBuildMCP or xcodebuild) and MUST record the final applied selection in `effective_config.resolved`.

### Structured Execution Controls (Recommended; Normative when present)

Profiles MAY define `xcode_run` controls for commonly-needed behaviors that remain safe when modeled explicitly:

```toml
[profiles.ci.xcode_run]
code_coverage = false                  # optional; enable code coverage collection
parallel_testing_enabled = true        # optional (test only); enable parallel test execution
max_concurrent_test_simulators = 2     # optional (test only); limit concurrent simulator destinations
retry_tests_on_failure = true          # optional (test only); Xcode automatic test retry
test_iterations = 1                    # optional (test only); must be >= 1
test_repetition_mode = "retry_on_failure" # optional enum: "none"|"retry_on_failure"|"until_failure"
```

- When present, these fields MUST be included in `effective_config.inputs`.
- The harness MUST apply them to the selected backend deterministically and MUST record the final applied values in `effective_config.resolved.xcode_run`.
- The lane MUST reject unknown keys under `xcode_run` (no free-form passthrough).

**Profile inheritance (Normative):**

A profile MAY declare `extends` to inherit fields from one or more other profiles:
- `extends` (string or array of strings) — parent profile name(s), applied in order

**Merge rules (Normative):**
- Tables are deep-merged (child keys override parent keys at each level)
- Scalars override (child value replaces parent value)
- Arrays replace (no concatenation) to keep resolution predictable
- The final resolved profile MUST be what is encoded into `effective_config.inputs`

```toml
# Example: .rch/xcode.toml

# Base profile with shared settings
[profiles.base]
workspace = "MyApp.xcworkspace"
scheme = "MyApp"
timeout_seconds = 1800

[profiles.base.safety]
allow_mutating = false
code_signing_allowed = false

# CI profile extends base (inherits workspace, scheme, timeout_seconds, safety)
[profiles.ci]
extends = "base"
action = "test"                        # "build" | "test"
configuration = "Debug"                # CI-specific override

[profiles.ci.destination]
platform = "iOS Simulator"
name = "iPhone 16"                     # friendly alias (used if device_type_id not specified)
os = "18.2"                            # friendly alias (used if runtime_id not specified)
device_type_id = "com.apple.CoreSimulator.SimDeviceType.iPhone-16"  # preferred when available
runtime_id = "com.apple.CoreSimulator.SimRuntime.iOS-18-2"          # preferred when available

[profiles.ci.xcode]
path = "/Applications/Xcode.app"       # optional; uses worker default if omitted
require_version = "16.2"               # optional; Xcode version constraint
require_build = "16C5032a"             # optional; strongest pin when available

[profiles.ci.worker]
require_tags = ["macos", "xcode"]      # default derives from lane, but profile may further restrict
min_macos = "15.0"                     # optional constraint
selection = "least_busy"               # "least_busy" | "warm_cache" | "random"
min_disk_free_bytes = 20_000_000_000   # refuse/de-prioritize if worker is low on disk
disallow_degraded = true               # refuse workers reporting health.degraded=true

[profiles.ci.cache]
derived_data = true
spm = true
mode = "read_only"                     # "off" | "read_only" | "read_write" (prevents cache poisoning)

[profiles.ci.backend]
preferred = "xcodebuildmcp"            # "xcodebuildmcp" | "xcodebuild"
allow_fallback = true                  # allow fallback to xcodebuild if preferred unavailable

[profiles.ci.env]
allow = ["CI", "RCH_BUILD_MODE"]       # explicit names recommended for CI
strict = true                          # forbid globs/prefix patterns (CI SHOULD enable)
include_value_fingerprints = true      # include hashed fingerprints of forwarded env values in inputs

[profiles.ci.safety]
allow_mutating = false                 # disallow implicit clean/archive
code_signing_allowed = false           # CODE_SIGNING_ALLOWED=NO

[profiles.ci.determinism]
allow_floating_destination = false     # default false; must be true to use os = "latest" in CI

[profiles.ci.source]
mode = "vcs"                           # "vcs" (default) | "working_tree"
require_clean = true                   # reject dirty working trees
include_untracked = false              # include untracked files in source tree hash

[profiles.ci.artifacts]
store = "host"                         # "host" (default) | "worker" | "object_store"
xcresult_format = "directory"          # "directory" | "tar.zst"
compression = "none"                   # "none" | "zstd" (applies to large artifacts during transfer)

[profiles.ci.limits]
max_workspace_bytes = 50_000_000_000   # workspace + DerivedData cap; enforced after staging + before collection
max_artifact_bytes  = 10_000_000_000   # total collected artifacts cap; enforced during collection
max_log_bytes       = 200_000_000      # build.log cap; truncate with marker (see Quota Enforcement)
max_events_bytes    = 50_000_000       # events.ndjson cap; on exceed: fail with complete + error_code
max_event_line_bytes = 1_000_000       # per-event JSON line cap; on exceed: fail with complete + error_code

# Release profile extends base but overrides safety for signing
[profiles.release]
extends = "base"
action = "build"
configuration = "Release"
timeout_seconds = 3600                 # longer timeout for release builds

[profiles.release.safety]
allow_mutating = true                  # override base: allow archive
code_signing_allowed = true            # override base: enable signing
```

### Quota Enforcement (Normative)

When `limits.*` are configured, the harness/host MUST enforce them deterministically:

**`max_workspace_bytes`:**
- MUST be checked at least after staging completes and again before artifact collection begins.
- If exceeded, the job MUST terminate with `error_code="workspace_quota_exceeded"`.
- The terminal error `detail` MUST include `{ "limit_bytes": <int>, "observed_bytes": <int> }`.

**`max_artifact_bytes`:**
- MUST be enforced during collection; if collecting the next artifact would exceed the cap, collection MUST stop.
- The job MUST terminate with `error_code="artifact_quota_exceeded"` and preserve already-collected artifacts + manifest entries.
- The terminal error `detail` MUST include `{ "limit_bytes": <int>, "observed_bytes": <int>, "artifact": "<path>" }`.

**`max_log_bytes`:**
- When exceeded, `build.log` MUST be truncated with an explicit marker and `redaction_report.json` MUST record `log_truncated=true`.
- The job MAY still succeed; `summary.json.errors[]` SHOULD include a non-fatal error object with code `log_truncated`.

**`source.max_bytes` / `source.max_file_bytes`:**
- The host MUST compute source snapshot size from `source_manifest.entries` (sum of `bytes`, and max per-entry `bytes`) before staging begins.
- If exceeded, the lane MUST refuse with `error_code="source_quota_exceeded"` and include `detail` at least:
  `{ "limit_bytes": <int>, "observed_bytes": <int>, "largest_file_bytes": <int>, "largest_file_path": "<path>" }`.
- The harness MUST re-check the staged tree before backend execution (best-effort) and fail with the same error code if exceeded.

### Host Config: `~/.config/rch/workers.toml`

```toml
[[workers]]
name = "mac-mini-1"
host = "mac-mini-1.local"
tags = ["macos", "xcode"]
ssh_user = "rch"
ssh_port = 22
ssh_run_key = "~/.ssh/rch_run_ed25519"       # used only for harness probe/run
ssh_stage_key = "~/.ssh/rch_stage_ed25519"   # used only for staging (rrsync write-only, confined to stage_root)
ssh_fetch_key = "~/.ssh/rch_fetch_ed25519"   # used only for fetching artifacts (rrsync read-only, confined to jobs_root)
ssh_host_key_fingerprint = "SHA256:BASE64ENCODEDFINGERPRINT"
ssh_host_key_ca_public_key = "~/.config/rch/ssh_host_ca.pub"  # optional: trust worker host certs signed by this CA
expected_harness_binary_sha256 = "0123...abcd"       # optional pin for supply-chain integrity
expected_codesign_team_id = "ABCDE12345"             # optional pin when codesign is used
expected_codesign_requirement_sha256 = "deadbeef..."  # optional stronger pin (SHA-256 of designated requirement)
probe_cache_ttl_seconds = 30                         # optional; host may reuse probe results for this TTL

# Worker roots (used to compute per-job workspaces deterministically)
stage_root = "~/Library/Caches/rch-xcode-lane/stage"
jobs_root  = "~/Library/Caches/rch-xcode-lane/jobs"
cache_root = "~/Library/Caches/rch-xcode-lane/cache"
```

### Transport Trust (Normative)

The host MUST support two worker host-key verification modes:
1. **Pinned fingerprint** via `ssh_host_key_fingerprint` — exact match required
2. **SSH host CA** via `ssh_host_key_ca_public_key` — trust host certificates signed by the specified CA

If either verification mode is configured, the host MUST verify worker identity before any probe/run command.
If verification fails, the lane MUST refuse to run and emit a clear error with code `ssh_host_key_untrusted`.

`attestation.json` MUST record:
- The observed SSH host key fingerprint used for the session (even if not pinned)
- `ssh_host_key_verification` = `"fingerprint"` | `"ca"` | `"none"` indicating which mode was used

This enables audits to detect worker identity drift.

#### Harness Identity Enforcement (Recommended)

If `expected_harness_binary_sha256` and/or `expected_codesign_team_id` are configured for a worker, the host SHOULD verify probe-reported values match before running. On mismatch, the lane MUST refuse with stable error code `harness_identity_mismatch`.

#### Profile-Level Host Key Enforcement

Profiles MAY define `[profiles.<name>.trust]` to require host key pinning:

```toml
[profiles.ci.trust]
require_pinned_host_key = true
posture = "auto"   # "auto" | "trusted" | "untrusted"
require_stable_capabilities_sha256 = true   # CI may require no capability drift between selection and run
```

When `require_pinned_host_key = true`, the lane MUST refuse to run unless the selected worker has `ssh_host_key_fingerprint` configured in `workers.toml`. Refusal MUST use error code `unpinned_worker_disallowed` and include a hint describing how to pin the worker.

This enables CI profiles to enforce a stronger trust posture than local development profiles.

#### Capability Freeze (Normative when enabled)

When `trust.require_stable_capabilities_sha256 = true`:
- The host MUST record the selected worker's `capabilities_sha256` at selection time in `decision.json`.
- Immediately before invoking `run`, the host MUST re-probe the selected worker (ignoring probe cache) and compare `capabilities_sha256`.
- If it differs, the lane MUST refuse with stable error code `capabilities_drift` and include details `{ "expected": "...", "observed": "..." }`.

### Optional Integrity Controls (Recommended for CI / remote storage)

Profiles MAY enable an event hash chain for tamper-evident streaming verification:

```toml
[profiles.ci.integrity]
event_hash_chain = true   # default: false
```

When enabled, the harness MUST include hash chain fields in event output (see `events.ndjson` Requirements below). This provides:
- Streaming-time integrity verification (line-by-line validation)
- Tamper evidence even if someone edits the file and recomputes only a final digest
- Better forensic confidence for canceled/terminated mid-stream jobs

### Resolution Rules

- Effective config MUST be resolved deterministically (profile defaults + CLI overrides).
- Destination + Xcode identity MUST be pinned for CI profiles unless `allow_floating_destination = true`.
- If `os = "latest"` is specified and `allow_floating_destination` is false (or absent), lane MUST refuse to run and emit an error explaining the requirement.
- If a profile is not specified, lane MUST refuse to run (no implicit defaults).

### Destination Resolution Algorithm

**Host responsibilities (before job submission):**

1. Read `destination.platform`, `destination.name`, `destination.os` from effective config.
   - If `destination.device_type_id` and/or `destination.runtime_id` are present, prefer them for matching (stable CoreSimulator identifiers).
   - Profiles MAY require CoreSimulator identifiers via `destination.require_core_ids = true` (recommended for CI).
2. If `os` is `"latest"` and `allow_floating_destination` is false, reject with error.
3. Using `probe.simulators.runtimes` + `probe.simulators.device_types`, resolve the destination spec to stable CoreSimulator IDs:
   - `runtime_id` and `device_type_id` (preferred), or refuse with stable errors if not resolvable.
4. If zero matches in the probe output, reject with error listing available runtimes.
5. If multiple matches:
   - Default: reject with an error describing the duplicates and how to fix/disambiguate.
   - If `destination.on_multiple = "select"` is set, select using `destination.selector` (default selector: `"highest_udid"`), and emit a warning.
6. Record `runtime_id` + `device_type_id` in `effective_config.inputs`.

**Harness responsibilities (at run time):**

7. The harness MUST select/create the actual simulator device (UDID) at run time based on `simulator.strategy` and the resolved `runtime_id`/`device_type_id`.
8. The harness MUST record the chosen UDID under `effective_config.resolved.destination_udid` (and emit it in `hello.worker_paths` or equivalent).

**CoreSimulator ID requirement (Recommended):**

If `destination.require_core_ids = true` and either `device_type_id` or `runtime_id` is missing, the lane SHOULD refuse with a stable error code `core_ids_required` and include a hint describing how to obtain CoreSimulator IDs from the worker probe output.

This catches human-friendly destination strings that may drift across Xcode/runtime versions, improving CI stability.

### Destination Disambiguation (Config)

Under `[profiles.<name>.destination]`, the following fields MAY be used:
- `on_multiple` — `"error"` (default) | `"select"`
- `selector` — `"highest_udid"` | `"lowest_udid"` (future: more selectors)
- `udid` — optional explicit simulator UDID (generally host/worker-specific; best for local-only profiles)

### Worker Selection (Normative)

Worker selection is controlled via `[profiles.<name>.worker].selection`:

- `"least_busy"` (default): Select the eligible worker with lowest `probe.load.active_jobs`.
- `"warm_cache"`: Select based on cache warmth (see below).
- `"random"`: Random selection among eligible workers.

**Worker Selection: `warm_cache` (Normative)**

When `worker.selection = "warm_cache"`:
- The host MUST compute `config_hash = SHA-256( JCS(effective_config.inputs) )` as lowercase hex.
- For each eligible worker that advertises `"cache_query"` in `probe.verbs` (or `features.cache_query=true`), the host SHOULD invoke `cache_query` and score the worker:
  - Scoring SHOULD prioritize DerivedData presence over SPM presence.
  - Implementations MAY use weighted scoring (e.g., +100 for DerivedData present, +50 for SPM present) but MUST document their algorithm.
- Tie-breakers (in order): lower `probe.load.active_jobs`, then lexicographic worker name.
- If no worker supports `cache_query`, the host MUST fall back to `least_busy` selection.
- The selected worker and selection rationale SHOULD be recorded in `decision.json.worker_candidates`.

**Default scoring algorithm (Recommended; MUST be recorded if used):**
```
score = 0
if DerivedData present: score += 100
if SPM present: score += 50
score -= (probe.load.active_jobs * 10)
tie-breakers: lower active_jobs, then lexicographic worker name
```

**Selection recording (Normative):**
The host MUST record in `decision.json.worker_candidates[]`:
- `selection_strategy` (string) — e.g., `"warm_cache"`, `"least_busy"`
- `selection_algorithm` (string|null) — algorithm identifier when applicable, e.g., `"default_v1"`
- Per-candidate: `cache_query` summary (when queried) + computed `score`

---

## Job Lifecycle

### States

```
1. created → 2. staging → 3. queued → 4. running → 5. collecting → 6. uploading → 7. terminal
                                                                                     ├── succeeded
                                                                                     ├── failed
                                                                                     ├── canceled
                                                                                     └── timed_out
```

**State descriptions:**
- `created`: Job initialized, not yet started.
- `staging`: Source snapshot being transferred to worker.
- `queued`: Waiting for a worker lease slot (harness is source of truth).
- `running`: Backend execution in progress on worker.
- `collecting`: Artifacts being collected from worker.
- `uploading`: Artifacts being uploaded to object store (when `store = "object_store"`).
- Terminal states: `succeeded`, `failed`, `canceled`, `timed_out`.

### Job Identity (Normative)

Every job attempt MUST carry three identity fields:

- **`job_id`** — A unique identifier for this specific execution attempt (UUID v7 recommended). Never reused across attempts.
- **`run_id`** — A content-derived identifier: `SHA-256( JCS(config_inputs) || "\n" || source_tree_hash_hex )`. Two jobs with identical config inputs and source MUST produce the same `run_id`. This enables cache lookups and deduplication.
- **`attempt`** — A monotonically increasing integer (starting at 1) within a given `run_id`. If a job is retried with the same effective config and source, `run_id` stays the same but `attempt` increments.

All three fields MUST appear in `summary.json`, `effective_config.json`, and `attestation.json`.

### Effective Config Envelope (Normative)

`effective_config.json` is an *envelope* that separates **hashable inputs** from **execution-time resolution**.

```jsonc
{
  "kind": "effective_config",
  "schema_version": "1.0.0",
  "lane_version": "0.1.0",
  "inputs": { /* hashable, content-derived */ },
  "resolved": { /* execution-time details, NOT hashed */ }
}
```

**`inputs` (hashed):** MUST include all logical build/test inputs that affect results and caching, such as:
- `contract_version` — semantic contract version for the hashed inputs (see Terms)
- action, workspace/project, scheme, configuration, timeout
- destination *spec* (platform + device/runtime identifiers or pinned versions)
- toolchain identity once pinned (Xcode build number, selected runtime identifier)
- backend policy (preferred/fallback), safety policy, cache policy, env allowlist, redaction policy

**`resolved` (not hashed):** MUST include execution-time details that are expected to vary between attempts/workers, such as:
- selected worker name/identity (also captured in attestation)
- filesystem paths (`paths.*`), temp dirs
- simulator **UDID** (especially under `strategy = "per_job"`)
- timestamps, queue position, lease bookkeeping

### Canonicalization + `run_id` (Normative)

To make `run_id` stable across hosts/implementations, the lane MUST define canonicalization.

**Canonical JSON:** The lane MUST canonicalize **`effective_config.inputs`** using the JSON Canonicalization Scheme (RFC 8785 / "JCS") before hashing. `effective_config.inputs` MUST include `contract_version`.

#### Semantic Normalization (Normative)

Before applying JCS, the lane MUST normalize "set-like" arrays in `effective_config.inputs` by:
1. Removing duplicates
2. Sorting lexicographically by UTF-8 byte order

**Fields treated as sets (MUST be documented and versioned):**
- `worker.require_tags`
- `env.allow`
- `retry.retry_on`
- `source.excludes`
- `redaction.patterns`

If the meaning of this normalization changes (fields added/removed, sort order changed), `contract_version` MUST be bumped.

Rationale: JCS does not normalize array order. Without semantic normalization, two semantically identical configs could produce different `run_id`s purely due to ordering, causing cache fragmentation.

**`run_id` bytes:** The lane MUST compute:
`run_id = SHA-256( "rch-xcode-lane/run_id/v1\n" || JCS(effective_config.inputs) || "\n" || source_tree_hash_hex )`
where `JCS(effective_config.inputs)` is UTF-8 bytes and `source_tree_hash_hex` is the lowercase hex string (UTF-8).

Rationale: simple, deterministic, and implementation-portable. The domain prefix (`"rch-xcode-lane/run_id/v1\n"`) prevents accidental digest reuse with other hash computations. Separating inputs from resolved details ensures lane versioning and worker-local details (UDIDs, paths) do not affect run identity.

**Note:** Changing the domain prefix or hash algorithm MUST bump `contract_version`.

### `config_hash` (Recommended)

For cache addressing and reporting, the lane SHOULD compute:
`config_hash = SHA-256( "rch-xcode-lane/config_hash/v1\n" || JCS(effective_config.inputs) )` as lowercase hex.

This is distinct from `run_id` (which includes the source tree hash). The `config_hash` enables efficient cache sharing: jobs with identical configuration (toolchain, destination, scheme, build settings) but different source trees can share DerivedData caches.

### Requirements

- Host MUST preserve partial artifacts for non-success terminals.
- Terminal state MUST be recorded in `summary.json`.

### Timeouts + Cancellation

- Lane MUST support a per-job timeout (`timeout_seconds`).
- If a timeout triggers, terminal state MUST be `timed_out`.
- Lane SHOULD support user cancellation (best-effort SIGINT/remote termination).
- A canceled job MUST still emit `summary.json` + `manifest.json` referencing available artifacts.

### Cancellation Semantics (Normative)

When `rch xcode cancel <job_id>` is invoked:
- The host MUST mark `status.json.state = "canceled"` once cancellation is confirmed or best-effort attempted.
- The worker harness SHOULD terminate the active backend process group (SIGTERM, then SIGKILL after 10s).
- If the worker is in `--forced` mode, the host MUST invoke `rch-xcode-worker cancel` and record its response.
- If the worker does not support `cancel` (not in `verbs`), the host MUST record `cancel_not_supported` in `summary.json.errors[]`.
- The lane MUST preserve partial artifacts collected so far and MUST still emit terminal `summary.json` and `manifest.json`.

### Worker Workspace Layout + Retention (Normative)

The worker MUST execute jobs inside a dedicated per-job workspace root, e.g.:
`~/Library/Caches/rch-xcode-lane/jobs/<job_id>/`

**Workspace subpaths (Normative):**
- `src/` — staged source tree
- `work/` — scratch/workdir for transient files
- `dd/` — DerivedData (or equivalent) root
- `result/` — result bundle root (xcresult output lives under here)
- `spm/` — cloned SourcePackages dir (when supported) or per-job package working dir

**Atomic staging (Normative):**

The harness MUST stage into a temporary directory (e.g., `src.tmp/`) and then atomically replace `src/` (rename swap), so readers never observe a partially-staged tree. If staging is interrupted, the harness MUST remove `src.tmp/` on next run.

**Read-only source (Normative):**

After staging completes and (when enabled) the post-stage hash check passes, the harness MUST make `src/` read-only (best-effort via filesystem permissions) prior to backend execution. This prevents build scripts and plugins from silently mutating the staged source tree.

If backend execution modifies `src/` (detected via an optional post-run hash check when `verify_after_run = true`), the job MUST fail with stable error code `source_tree_modified`.

The lane MUST provide a garbage-collection mechanism:
- `rch xcode gc` cleans host job dirs and worker job workspaces according to retention policy.
- Default retention SHOULD be time-based (e.g., keep last N days) and SHOULD be configurable.

---

## Safety Rules

### Interception Policy

- Lane MUST NOT run mutating commands by implicit interception (e.g., `clean`, `archive`) unless `allow_mutating = true`.
- When classification is uncertain, lane MUST prefer **not** to intercept (false negatives are acceptable).
- Default signing policy: `CODE_SIGNING_ALLOWED=NO` unless explicitly enabled.

### Invocation Reconstruction (Normative)

When the lane intercepts an incoming command string, it MUST:
1. Parse/classify the request (for decision/audit), then
2. Construct the remote invocation exclusively from `effective_config.json`.

The lane MUST NOT pass through arbitrary user-provided `xcodebuild` flags or paths. Any overrides MUST be explicitly modeled as structured config fields (e.g., `xcode_test.only_testing`) and included in `effective_config.json`.

Rationale: prevents flag-based policy bypass and keeps runs deterministic/auditable.

### Environment Forwarding (Normative)

Profiles MAY forward specific environment variables to the worker. Because env values can affect build outputs, the lane provides deterministic controls that avoid recording raw values.

```toml
[profiles.ci.env]
allow = ["CI", "RCH_BUILD_MODE"]          # explicit names recommended for CI
strict = true                             # forbid globs/prefix patterns (CI SHOULD enable)
include_value_fingerprints = true         # include hashed fingerprints of forwarded env values in inputs
```

**Rules (Normative):**
- If `env.strict = true`, `env.allow` entries MUST be exact variable names (no `*` globs, no prefix patterns). If violated, refuse with `env_globs_disallowed`.
- The host MUST NOT forward any env vars by default.
- If `env.allow` is present, the host MUST forward **only** vars matching `env.allow` (and MUST NOT forward "ambient" vars).
- If `env.include_value_fingerprints = true`, the host MUST include in `effective_config.inputs` an `env.forwarded` map containing, for each forwarded env var name:
  - `present` (boolean)
  - `value_sha256` (string|null) — lowercase hex SHA-256 of:
    `SHA-256( "rch-xcode-lane/env_value_sha256/v1\n" || <NAME> || "\n" || <VALUE_BYTES> )`
    where `<VALUE_BYTES>` are the exact UTF-8 bytes of the env value (empty string allowed).
- `environment.json` MUST continue to omit env values (names only); fingerprints live only in `effective_config.inputs`.

### Secrets & Environment (Normative)

To prevent accidental secret leakage in build logs and environment artifacts:

- The host MUST NOT forward ambient host environment variables to the worker by default.
- If environment variables are required, profiles MUST declare an allowlist:

```toml
[profiles.ci.env]
allow = ["CI", "RCH_*"]     # Values forwarded if present (literal names or prefix globs)
strict = false              # When true: forbid globs/prefix (recommended for CI determinism)
include_value_fingerprints = false  # When true: hash forwarded env values into effective_config.inputs
```

- `environment.json` MUST be sanitized:
  - MUST NOT include secret values (tokens, private keys, credentials).
  - SHOULD include only non-sensitive machine/tool identifiers (Xcode, macOS, runtimes).
  - MAY include allowlisted env variable names with values omitted or redacted.

**Optional log + event redaction:**

```toml
[profiles.ci.redaction]
enabled = true              # Default: false (for local debugging)
patterns = ["ghp_*", "xox*-*"]  # Optional additional redaction patterns
```

When `redaction.enabled = true`, the lane SHOULD redact known secret patterns from:
- `build.log`
- `events.ndjson`
- any derived text reports emitted by the lane (e.g., `junit.xml`, `sarif.json`, `annotations.json`) when those reports may embed raw messages

**Ordering (Normative):**
- Redaction MUST be applied **before** any integrity digests are computed (event hash chain, `events_sha256`) and before any manifest hashes are computed for storage/upload.
- To preserve end-to-end integrity validation, the harness MUST apply redaction **inline** (before emitting bytes to stdout/stderr and before writing durable `events.ndjson` / `build.log`).
- When `redaction.enabled = true`, the host MUST include `"inline_redaction"` in `job_request.required_features`.
- The host MUST treat `events.ndjson` and `build.log` as immutable bytes once collected. If additional post-processing is required for export,
  implementations MUST write new artifacts (e.g., `events.redacted.ndjson`) and record them separately in `manifest.json`.

**Redaction reporting (Recommended):**

When redaction is enabled, the lane SHOULD emit `redaction_report.json` containing:
- `enabled` (boolean) — Whether redaction was enabled
- `patterns` (array of strings) — Identifiers for patterns applied (no secret values)
- `replacements` (integer) — Best-effort count of redactions performed
- `log_truncated` (boolean) — Whether build.log was truncated
- `events_truncated` (boolean) — Whether events.ndjson was truncated (if a truncation policy exists)
- `targets` (array of strings) — Which artifacts were processed (e.g., `["build.log", "events.ndjson", "sarif.json"]`)

This enables auditing that redaction policy was applied without exposing what was redacted.

### Artifact Sensitivity (Normative for remote storage)

Artifacts MAY contain sensitive information (logs, crash dumps, test output, event messages).
When `store = "object_store"` (or any non-host persistence), the lane MUST:

- Ensure configured redaction is applied **inline on the worker** (requires `probe.features.inline_redaction = true` and MUST be listed in `job_request.required_features`)
  so secrets are not transmitted over the control-plane stream.
- Apply configured redaction to derived text reports (e.g., `junit.xml`, `sarif.json`, `annotations.json`) prior to upload
- Avoid uploading `environment.json` values (only identifiers/redacted fields)
- Never upload credentials; object store auth MUST remain on the host only

### Decision Artifact (Normative)

Every job MUST emit a `decision.json` file in the artifact directory, recording the classification and routing decision made by the host. This supports auditability and debugging of interception behavior.

**Required fields:**

- `command_raw` — The original command string as received.
- `command_normalized` — Normalized form used for classification (whitespace/quoting normalized).
- `command_classified` — The classified action (`build`, `test`, `clean`, `archive`, `unknown`).
- `command_parsed` — Optional structured parse result (recognized flags/fields), for debugging and audit.

**Classifier provenance (Required):**
- `classifier` (object):
  - `name` (string) — Classifier identifier
  - `version` (string) — Classifier version
  - `policy_sha256` (string) — Digest of the active allowlist/policy bundle
  - `policy_artifact` (string) — Relative path to the captured policy bundle artifact (see `policy.json`)
  - `confidence` (number) — Classification confidence (0..1)
- `profile_used` — The profile name selected.
- `intercepted` — Boolean: whether the command was intercepted and routed to a worker.
- `refusal_reason` — If not intercepted, a stable error code (e.g., `"uncertain_classification"`, `"mutating_disallowed"`). Null if intercepted. MUST use stable error codes from the Error Model.
- `worker_selected` — Worker name, or null if refused.
- `timestamp` — ISO 8601 timestamp of the decision.

**Optional fields (for CI explainability):**

- `worker_candidates` — Array describing considered workers and reject reasons.

`worker_candidates` element (when present) SHOULD include:
- `name` — Worker name.
- `eligible` — Boolean: whether the worker was eligible.
- `reasons` — Array of stable codes explaining rejection (e.g., `xcode_version_mismatch`, `runtime_not_found`, `lease_unavailable`).

### Policy Bundle Artifact (Normative)

Every job MUST also emit a captured policy bundle artifact so audits do not depend on a hash alone.

**Artifact:** `policy.json` (required)

**Required fields:**
```jsonc
{
  "kind": "policy",
  "schema_version": "1.0.0",
  "lane_version": "0.1.0",
  "policy": {
    "name": "string",
    "version": "string",
    "sha256": "lowercase hex",
    "rules": { /* canonical, serialized allowlist/policy representation */ }
  }
}
```

**Rules (Normative):**
- `policy.policy.sha256` MUST equal `decision.json.classifier.policy_sha256`.
- `decision.json.classifier.policy_artifact` MUST equal `"policy.json"`.
- The policy artifact MUST NOT contain secrets.

---

## Error Model (Normative)

To support agent/CI automation, the lane MUST provide a stable machine-consumable error model.

### Error Object

When present, an error MUST be represented as:
```json
{ "code": "string", "message": "string", "retryable": "boolean", "hint": "string|null", "detail": "object|null" }
```

- `code` — Stable error code (snake_case). MUST NOT change across releases.
- `message` — Human-readable description. MAY change across releases.
- `retryable` — Boolean indicating if the operation may succeed on retry.
- `hint` — Optional human-readable guidance for resolution.
- `detail` — Optional structured data for debugging (e.g., available runtimes, expected vs actual values).

### Stable Error Codes

Error codes MUST be stable across releases (backward compatible). Codes MUST be snake_case.

**Standard error codes:**

| Code | Meaning | Retryable |
|------|---------|-----------|
| `destination_not_found` | Requested simulator destination not available | No |
| `destination_ambiguous` | Multiple matching destinations found | No |
| `core_ids_required` | Profile requires CoreSimulator IDs but they are missing | No |
| `ssh_host_key_untrusted` | Worker SSH host key/cert not trusted by configured pins | No |
| `harness_identity_mismatch` | Harness binary hash or codesign identity does not match expected pins | No |
| `capabilities_drift` | Worker capabilities_sha256 changed between selection and run | No |
| `insufficient_disk_space` | Worker did not meet `min_disk_free_bytes` threshold | Yes |
| `worker_degraded` | Worker reported degraded execution state | Yes |
| `unpinned_worker_disallowed` | Profile requires pinned host key but worker has none | No |
| `lease_expired` | Worker lease TTL exceeded without renewal | Yes |
| `lease_unavailable` | No worker lease slots available | Yes |
| `worker_unreachable` | Cannot establish SSH connection to worker | Yes |
| `xcode_not_found` | Xcode not found at expected path on worker | No |
| `xcode_version_mismatch` | Xcode version does not match constraint | No |
| `backend_unavailable` | Preferred backend unavailable and fallback disallowed | No |
| `cache_namespace_missing` | Caching enabled but cache_namespace missing/invalid | No |
| `protocol_version_unsupported` | No common protocol version between host and harness | No |
| `contract_version_unsupported` | Harness does not support requested contract_version | No |
| `required_feature_unsupported` | Host required a harness feature that is unavailable | No |
| `runtime_not_found` | Requested simulator runtime not installed | No |
| `uncertain_classification` | Command could not be confidently classified | No |
| `mutating_disallowed` | Mutating command refused by policy | No |
| `floating_destination_disallowed` | Floating destination (os="latest") refused by policy | No |
| `env_globs_disallowed` | env.strict=true forbids glob/prefix patterns in env.allow | No |
| `dirty_working_tree` | Working tree has uncommitted changes and require_clean=true | No |
| `timeout` | Job exceeded configured timeout_seconds | No |
| `canceled` | Job was canceled by user | No |
| `cancel_failed` | Cancel command executed but worker could not terminate the job | Yes |
| `cancel_not_supported` | Worker/harness does not support cancel (disallowed in `--forced` deployments) | No |
| `event_stream_corrupt` | Worker harness could not emit valid JSON event | No |
| `events_quota_exceeded` | Event stream exceeded configured byte cap | No |
| `event_line_too_long` | A single event exceeded configured per-line cap | No |
| `source_staging_failed` | Failed to stage source to worker | Yes |
| `source_staging_incomplete` | Stage receipt/sentinel missing; staging not complete | Yes |
| `stage_receipt_mismatch` | Stage receipt identity/hash does not match the job request | Yes |
| `source_hash_mismatch` | Staged source did not match expected source_tree_hash | Yes |
| `source_tree_modified` | Source tree was modified during execution | No |
| `artifact_collection_failed` | Failed to collect artifacts from worker | Yes |
| `forbidden_ssh_command` | SSH command rejected by harness in forced mode | No |
| `path_out_of_bounds` | Requested/derived path escaped configured roots | No |
| `invalid_job_identity` | job_id/run_id/attempt failed validation | No |
| `job_id_reused` | job_id workspace already exists; job_id is single-use | No |
| `workspace_quota_exceeded` | Worker workspace exceeded configured cap | No |
| `artifact_quota_exceeded` | Artifacts exceeded configured cap | No |
| `log_truncated` | build.log exceeded cap and was truncated | No |
| `source_quota_exceeded` | Source snapshot exceeded configured `source.max_*` limits | No |
| `untrusted_remote_storage_requires_redaction` | Untrusted posture requires redaction for remote artifact storage | No |
| `object_store_encryption_required` | Remote storage requires explicit encryption policy (untrusted posture disallows none) | No |
| `symlinks_disallowed` | Source tree contains symlinks but policy forbids them | No |
| `unsafe_symlink_target` | A symlink target is unsafe under allow_safe policy | No |
| `hardlinks_disallowed` | Source tree contains hardlinks but policy forbids them | No |
| `directory_artifact_remote_disallowed` | Directory artifact cannot be stored remotely without packing | No |

Implementations MAY define additional codes; consumers MUST tolerate unknown codes.

### Required Placement

- `summary.json` MUST include:
  - `error_code` (string|null) — Primary error code for the run (null on success).
  - `errors` (array) — Array of error objects; may be empty on success.
- `decision.json` MUST use stable codes for `refusal_reason` when refusing interception.

### Threat Model

- Repositories may contain build scripts/plugins that execute arbitrary code during build/test.
- Lane does not attempt to sandbox Xcode beyond best-effort OS/user isolation.
- Operators SHOULD deploy workers as dedicated, non-sensitive machines/accounts.
- Operators SHOULD use a dedicated macOS user account with minimal privileges for RCH runs.

### Repo Trust Posture (Normative)

Profiles MAY control trust posture explicitly via `[profiles.<name>.trust]`:
- `trust.posture = "trusted"` forces trusted posture
- `trust.posture = "untrusted"` forces untrusted posture
- `trust.posture = "auto"` (default) lets the host decide based on CI context / repo provenance

When posture is `untrusted`, the lane MUST enforce (regardless of profile defaults):
- `code_signing_allowed = false`
- `allow_mutating = false`
- `cache.mode = "off"` unless the profile explicitly opts in via `cache.allow_untrusted_reads = true`
- `cache.trust_domain = "per_repo_and_posture"` unless the profile explicitly sets `allow_cross_posture_reads = true`
- If `allow_cross_posture_reads = false`, the worker MUST NOT read cache entries produced under a different posture partition.
- `source.symlinks = "forbid"` unless the operator explicitly opts out for a known-safe repository
- If `action = "test"`, the lane MUST set (or require) `simulator.strategy = "per_job"` unless explicitly overridden for local use.
- If `action = "test"`, the lane MUST set (or require) `simulator.device_set = "per_job"` to isolate CoreSimulator device database state.
- If artifacts may leave the host (`store = "worker"` or `"object_store"`), the lane MUST enable log redaction (`redaction.enabled = true`) or refuse with a stable error code `untrusted_remote_storage_requires_redaction`.
  When enabled, the host MUST require `features.inline_redaction` from the harness to avoid leaking secrets over the run stream.

#### Untrusted Cache Read Opt-in (Normative)

Profiles MAY explicitly opt in to cache reads under untrusted posture:

```toml
[profiles.ci.cache]
allow_untrusted_reads = false  # default: false
```

If `trust.posture` resolves to `"untrusted"` and `allow_untrusted_reads = false` (the default), the lane MUST force `cache.mode = "off"`.

Rationale: Reading caches in untrusted posture can leak information from prior trusted builds (private symbols, paths, embedded data from proprietary dependencies).

The applied posture MUST be recorded in `decision.json` (as `trust_posture`) and `effective_config.json` (as final resolved values). This ensures the lane is safer-by-default even when configs drift.

### Worker SSH Hardening (Strongly Recommended)

The lane assumes repos may execute arbitrary code during build/test. Operators SHOULD minimize SSH blast radius:

- Use a dedicated macOS user with minimal privileges and no interactive shell access where feasible.
- Prefer a **forced-command run key** restricted to executing `rch-xcode-worker`:
  - Key options: `no-pty,no-agent-forwarding,no-port-forwarding,no-X11-forwarding,restrict`
- Use separate, restricted **data-plane keys** (recommended):
  - **Stage key**: write-only, confined to the `stage_root/` (push source snapshot)
  - **Fetch key**: read-only, confined to the `jobs_root/` (pull artifacts back)

**Example `authorized_keys` entries (illustrative):**

```
# Run key (forced to harness in --forced mode; verb comes from SSH_ORIGINAL_COMMAND):
command="/usr/local/bin/rch-xcode-worker --forced",no-pty,no-agent-forwarding,no-port-forwarding,no-X11-forwarding,restrict ssh-ed25519 AAAA... rch-run

# Stage key (write-only, confined to staging root):
command="/usr/local/bin/rrsync -wo ~/Library/Caches/rch-xcode-lane/stage",no-pty,no-agent-forwarding,no-port-forwarding,no-X11-forwarding,restrict ssh-ed25519 AAAA... rch-stage

# Fetch key (read-only, confined to jobs root):
command="/usr/local/bin/rrsync -ro ~/Library/Caches/rch-xcode-lane/jobs",no-pty,no-agent-forwarding,no-port-forwarding,no-X11-forwarding,restrict ssh-ed25519 AAAA... rch-fetch
```

---

## Determinism Contract

Every run MUST emit:

- `effective_config.json` — Fully-resolved configuration used for the job (Xcode path, destination, build settings, cache policy).
- `attestation.json` — Environment fingerprint: macOS version, Xcode version/build, toolchain versions, worker identity, repo state.
- `manifest.json` — Artifact inventory + hashes.

### Artifact Schema + Versioning (Normative)

Every JSON artifact emitted by the lane MUST include these top-level fields:
- `kind` — A stable identifier for the artifact type (e.g., `"summary"`, `"manifest"`, `"attestation"`).
- `schema_version` — A SemVer-like string for the artifact schema (e.g., `"1.0.0"`).
- `lane_version` — The lane implementation version (SemVer).

The repository SHOULD provide JSON Schemas under `schemas/rch-xcode-lane/` and CI/agents SHOULD validate outputs against these schemas for early break detection.

Determinism inputs SHOULD include:

- Explicit Xcode selection (path or version constraint)
- Explicit destination resolution strategy (simulator UDIDs over human names)
- Explicit signing policy (default off)

### Source Snapshot (Normative)

The `attestation.json` MUST include a `source` object capturing the state of the source tree at job creation time:

- **`vcs_commit`** — The full commit SHA of HEAD (or null if not a VCS repo).
- **`dirty`** — Boolean: true if the working tree has uncommitted changes.
- **`source_tree_hash`** — A deterministic hash of the source files sent to the worker (see Canonical Source Manifest below).
- **`untracked_included`** — Boolean: whether untracked files were included in the hash and sync.
- **`lockfiles`** — Optional array of `{ path, sha256 }` for recognized dependency lockfiles (e.g., `Package.resolved`, `Podfile.lock`, `Cartfile.resolved`) when present in the staged source.

The source tree hash is a critical input to `run_id` computation (see Job Identity).

#### Post-stage Source Integrity Check (Recommended; Normative when enabled)

Profiles MAY enable a post-stage integrity check:

```toml
[profiles.ci.source]
verify_after_stage = true
```

When `verify_after_stage = true`:
- The worker harness MUST compute `observed_source_tree_hash` from the staged tree using the same Canonical Source Manifest rules.
- If `observed_source_tree_hash` does not equal the host-provided `source_tree_hash`, the harness MUST fail the job with stable error code `source_hash_mismatch` (retryable: true).
- `attestation.json` MUST record: `source.expected_tree_hash`, `source.observed_tree_hash`, and `source.verified` (true if check passed, false if skipped).

This check catches rsync drift, disk corruption, and staging misconfiguration before the build begins.

#### Post-run Source Integrity Check (Optional; Normative when enabled)

Profiles MAY enable a post-run integrity check:

```toml
[profiles.ci.source]
verify_after_run = true
```

When `verify_after_run = true`:
- The worker harness MUST compute `observed_source_tree_hash_after_run` using the same Canonical Source Manifest rules.
- If the post-run hash differs from the expected `source_tree_hash`, the job MUST fail with stable error code `source_tree_modified`.
- `attestation.json` MUST record: `source.verify_after_run` (boolean), and when enabled: `source.observed_tree_hash_after_run`.

This detects if build scripts or plugins silently modified the staged source tree during execution.

#### Canonical Source Manifest (Normative)

The lane MUST produce a canonical manifest of the staged source and MUST use it to compute `source_tree_hash`. This enables reproducibility verification and debugging.

**Artifact:** `source_manifest.json` (required)

**Required top-level fields:** `kind`, `schema_version`, `lane_version`, `entries`

Each entry in `entries` MUST be an object with:
- `path` (string) — Normalized path: POSIX `/` separators, no leading `./`, UTF-8, sorted lexicographically (byte order).
- `type` (string) — `"file"` or `"symlink"`.
- `mode` (string) — Stable file mode string (e.g., `"100644"` for regular file, `"100755"` for executable).
- `sha256` (string) — Lowercase hex SHA-256 of file contents. For symlinks, hash the link target path bytes (not the target file contents).
- `bytes` (integer) — File size in bytes (symlink: length of target path).
- `link_target` (string|null) — REQUIRED when `type="symlink"`; MUST be the symlink target path as stored in the filesystem. Null for regular files.

**Hash computation:**

```
source_tree_hash = SHA-256( "rch-xcode-lane/source_tree_hash/v1\n" || JCS(source_manifest.entries) )
```

Where `JCS` is the JSON Canonicalization Scheme (RFC 8785) and the result is lowercase hex. The domain prefix prevents accidental digest reuse.

**Normalization rules:**

- Paths MUST use `/` as separator (convert from platform-native).
- Paths MUST NOT have leading `./` or trailing `/`.
- Paths MUST be sorted lexicographically by UTF-8 byte values.
- Symlinks MUST be represented as `type: "symlink"` with `link_target` recorded and `sha256` computed over `link_target` bytes; the lane MUST NOT follow symlinks.
- Binary files are hashed as-is (no newline normalization).
- Default excludes: `.git/`, `.rch/`, `*.xcresult/`, `DerivedData/`. Additional excludes via `[profiles.<name>.source].excludes`.

**VCS mode mode-bit source (Normative):**

When `source.mode = "vcs"` and the repository is a Git working copy, the lane MUST derive `entries[].mode` from VCS metadata (Git index) rather than filesystem permissions:
- Use Git's tracked file mode (e.g., `100644` vs `100755`) as the canonical `mode`.
- If VCS metadata is unavailable (non-git repo), fallback to filesystem mode is permitted.

Rationale: prevents umask/checkout drift from perturbing `source_tree_hash` and `run_id`.

#### Excludes + Submodules

Under `[profiles.<name>.source]`, the following fields MAY be used:
- `excludes` (array of strings) — Additional path globs to exclude from hashing/staging (applied after defaults).
- `submodules` (string) — `"forbid"` (default) | `"include"`. If `"include"`, submodules MUST be staged at their recorded commit and represented in the manifest as normal files.

### Source Policy

Source snapshot behavior is configured under `[profiles.<name>.source]`:

- **`mode`** — `"vcs"` (default): hash and sync only VCS-tracked files. `"working_tree"`: hash and sync all files (respecting excludes).
- **`require_clean`** — If true, lane MUST refuse to run when the working tree is dirty. Default: false for `local` profiles, true for `ci` profiles.
- **`include_untracked`** — If true, untracked files are included in hash and sync. Default: false.
- **`symlinks`** — `"forbid"` | `"allow_safe"` | `"allow_all"`. Default: `"forbid"` for untrusted posture, `"allow_safe"` otherwise.
  - `forbid`: Source tree MUST NOT contain symlinks; if any are found, refuse with `symlinks_disallowed`.
  - `allow_safe`: Allow symlinks only if target is relative, does not start with `/`, and does not contain `..` path segments. Unsafe symlinks cause `unsafe_symlink_target`.
  - `allow_all`: Allow all symlinks (use with caution).
- **`hardlinks`** — `"forbid"` | `"allow"`. Default: `"forbid"`.
  - `forbid`: If any hardlinked files (same inode, multiple paths) are detected in the staged tree, refuse with `hardlinks_disallowed`.
  - `allow`: Allow hardlinks (may cause surprising manifest semantics).
- **`metadata`** — `"strip"` | `"preserve"`. Default: `"strip"`.
  - `strip`: Staging MUST NOT preserve xattrs or ACLs. This improves reproducibility by eliminating macOS-specific metadata that can affect execution (quarantine flags, permissions) and cause drift between workers.
  - `preserve`: Preserve xattrs and ACLs during staging (use with caution; may reduce reproducibility).
- **`max_bytes`** — integer|null. When set, the lane MUST refuse (before staging) if the computed source snapshot size exceeds this limit.
- **`max_file_bytes`** — integer|null. When set, the lane MUST refuse if any single file in the source snapshot exceeds this limit.

### Provenance (Optional, Strongly Recommended)

For builds that require a verifiable chain of custody, the lane MAY emit a `provenance/` directory alongside the standard artifacts:

```
~/.local/share/rch/artifacts/<job_id>/
├── ...standard artifacts...
└── provenance/
    ├── attestation.sig        # Detached signature over attestation.json
    ├── manifest.sig           # Detached signature over manifest.json
    └── verify.json            # Public key reference + algorithm + instructions
```

**Requirements (when provenance is enabled):**

- Signatures MUST use Ed25519 or ECDSA P-256.
- `verify.json` MUST include: `algorithm`, `public_key` (or `public_key_url`), and `signed_files` (list of filename + hash pairs).
- Signing key MUST NOT be stored on the worker; host signs after collecting artifacts.
- Provenance is opt-in via `[profiles.<name>.provenance]` with `enabled = true` and `key_path`.

---

## Required Artifacts

Artifacts are written under a per-job directory on the host, scoped by `repo_key`. The directory is **append-only during execution**, except for `status.json`, which is updated in-place using atomic replacement. The host SHOULD also maintain a stable run index keyed by `run_id` for deduplication and agent/CI ergonomics.

`repo_key` is a stable host-side namespace derived from VCS identity when available (e.g., normalized origin URL hash), otherwise from a workspace identity hash. It is recorded in `attestation.json` and prevents cross-repo run index collisions.

```
~/.local/share/rch/artifacts/repos/<repo_key>/jobs/<job_id>/
├── probe.json             # Captured worker harness probe output used for selection/verification
├── summary.json           # Machine-readable outcome + timings
├── job_request.json       # Exact JSON request sent to harness `run` (MUST NOT contain secret values)
├── effective_config.json  # Fully-resolved config used for the job
├── attestation.json       # Toolchain + environment fingerprint
├── source_manifest.json   # Canonical file list + per-entry hashes used to compute source_tree_hash
├── manifest.json          # Artifact index + SHA-256 hashes + byte sizes
├── decision.json          # Classification + routing decision record
├── policy.json            # Captured allowlist/policy bundle (required)
├── environment.json       # Captured worker environment snapshot
├── timing.json            # Durations for stage/run/collect phases
├── staging.json           # Staging report (method, excludes, bytes/files, duration, tool versions)
├── stage_receipt.json     # Stage receipt consumed by the harness (copied into job dir for audit)
├── stage_verification.json# Optional: post-stage/post-run integrity verification (when enabled)
├── metrics.json           # Resource + transfer metrics + queue stats
├── events.ndjson          # Streaming event log (newline-delimited JSON)
├── status.json            # Current job status (updated in-place during execution)
├── build.log              # Captured harness stderr (human logs + backend output). Stdout is reserved for NDJSON events.
├── backend_invocation.json# Structured backend invocation record (safe; no secret values)
├── redaction_report.json  # Optional: redaction/truncation report (no secret values; emitted when redaction enabled)
├── result.xcresult/       # When tests run and xcresult is produced (or result.xcresult.tar.zst when xcresult_format="tar.zst")
├── artifact_trees/        # Optional: canonical tree manifests for directory artifacts (e.g. result.xcresult/)
└── provenance/            # Optional: signatures + verification report
```

### Run Index (Recommended)

The host SHOULD create:
`~/.local/share/rch/artifacts/repos/<repo_key>/runs/<run_id>/attempt-<attempt>/`
as a symlink or pointer to `jobs/<job_id>/` for that attempt.

This enables:
- Easy deduplication and retention decisions ("keep last N runs")
- Straightforward retrieval of "latest attempt for run_id"
- Cleaner correlation with caches (run_id/config hashes)

### `manifest.json` Requirements

- MUST include SHA-256 hashes for all material artifacts.
- MUST include byte sizes.
- SHOULD include a logical artifact type for each entry (log/json/xcresult/etc.).
- SHOULD include `kind`/`schema_version` for JSON artifacts (or inferable mapping) to aid verifiers.

Each manifest entry MUST include:
- `path` (string) — Relative path within the artifact directory.
- `sha256` (string) — Lowercase hex SHA-256 hash.
- `bytes` (integer) — Size in bytes.

Each manifest entry SHOULD include:
- `artifact_type` (string) — e.g. `"json"` | `"log"` | `"xcresult"` | `"provenance"` | `"other"`.
- `content_type` (string) — e.g. `"application/json"`, `"text/plain"`, `"application/vnd.apple.xcresult"`.
- `encoding` (string|null) — e.g. `"utf-8"` for text files.

Each manifest entry MAY include:
- `storage` (string) — `"host"` | `"worker"` | `"object_store"` (where the artifact is stored).
- `uri` (string|null) — For remote storage, a stable URI for retrieval.
- `etag` (string|null) — Best-effort remote integrity hint (when provided by storage backend).
- `encryption` (string|null) — `"server_side"` | `"client_side"` | `"none"` (best-effort, when applicable).
- `compression` (string) — `"none"` | `"zstd"` (compression applied to the stored artifact).
- `compression_level` (integer|null) — Optional: numeric level used when `compression != "none"` (recording improves reproducibility + CAS effectiveness).
- `logical_name` (string|null) — Stable name for the logical artifact (e.g. `"result.xcresult"` even if stored as `result.xcresult.tar.zst`).
- `sensitivity` (string|null) — `"public"` | `"internal"` | `"sensitive"`. Recommended for policy-aware retention/upload decisions. Enables CI to decide what to upload/retain based on sensitivity classification.

#### Directory / Tree Artifacts (Normative)

Some artifacts are directories (e.g., `result.xcresult/`). For these, `manifest.json` MUST represent them with `artifact_type="tree"` and `sha256` MUST be a *tree hash* computed as:

```
tree_sha256 = SHA-256( "rch-xcode-lane/tree_hash/v1\n" || JCS(tree_entries) )
```

(lowercase hex), where `tree_entries` is an array of `{ path, bytes, sha256 }` for every file under the directory, with:
- `path` normalized (POSIX `/`, relative to artifact root), sorted lexicographically by UTF-8 byte order
- `sha256` = SHA-256(file bytes) for each file (lowercase hex)
- `bytes` = file size

Implementations SHOULD also emit an optional tree manifest:
`artifact_trees/<logical_name>.tree.json` (kind `"artifact_tree"`) containing `tree_entries` and `tree_sha256` to aid portability and validation.

**Remote storage + xcresult (Normative):**

If `store != "host"`, profiles SHOULD set `xcresult_format = "tar.zst"`.
If `xcresult_format = "directory"` and `store != "host"`, the lane MUST refuse with a stable error code `directory_artifact_remote_disallowed`
unless it implements **deterministic packing** (see below).

**Deterministic packing (Normative when used):**
- The lane MUST pack directory artifacts (including `result.xcresult/`) into a byte-stable archive such that identical directory content produces identical archive bytes.
- The packing algorithm MUST:
  - Sort entries lexicographically by UTF-8 byte order of their relative paths
  - Normalize paths to POSIX `/` separators
  - Set uid/gid to 0 and omit uname/gname
  - Set all mtimes to a constant (e.g., Unix epoch `0`)
  - Strip xattrs/ACLs and omit OS-specific metadata
- When compression is applied (e.g., `.tar.zst`), the compressor settings MUST be deterministic and MUST be recorded in `manifest.json`
  (`compression` plus an optional `compression_level` field).

### Artifact Storage & Compression (Normative)

Artifact storage and compression are controlled via `[profiles.<name>.artifacts]`.

#### Storage Modes

- `store = "host"` (default): Host collects all configured artifacts into `~/.local/share/rch/artifacts/<job_id>/`. All artifacts are stored locally.
- `store = "worker"`: Worker retains large artifacts (xcresult, logs); host stores JSON metadata artifacts + manifest with pointers. `rch xcode fetch <job_id>` retrieves on demand.
- `store = "object_store"`: Host uploads artifacts after collection; manifest MUST include stable URIs for retrieval. Requires additional `[profiles.<name>.artifacts.object_store]` configuration (endpoint, bucket, credentials).
  - When configured, object-store uploads SHOULD use TLS and SHOULD enable encryption at rest.
  - `manifest.json` entries for remote artifacts SHOULD include remote metadata when available (e.g., `etag`).
  - Uploads MUST happen only after redaction policy has been applied.

#### Object Store Options (Recommended)

```toml
[profiles.ci.artifacts.object_store]
endpoint = "https://s3.example.com"
bucket = "rch-artifacts"
encryption = "server_side"     # REQUIRED: "server_side" | "client_side" | "none"
kms_key_id = "..."             # optional when supported
```

**Object-store encryption enforcement (Normative):**
- If `store = "object_store"`, the profile MUST specify `artifacts.object_store.encryption`.
- If `trust.posture = "untrusted"` and `encryption = "none"`, the lane MUST refuse with error code `object_store_encryption_required`.

#### Compression Options

- `xcresult_format = "directory"` (default): xcresult stored as directory tree.
- `xcresult_format = "tar.zst"`: xcresult compressed as `result.xcresult.tar.zst` for transfer and storage.
- `compression = "none"` (default): No compression for other artifacts.
- `compression = "zstd"`: Apply zstd compression to large text artifacts (logs) during transfer.

When compression is enabled, the `manifest.json` entry MUST indicate the compression type so consumers can decompress correctly.

#### Fetch Semantics

When `store` is `"worker"` or `"object_store"`, the `rch xcode fetch <job_id>` command MUST:
1. Read `manifest.json` from the local artifact directory.
2. For each entry with `storage` != `"host"`, retrieve the artifact from the indicated location.
3. Verify SHA-256 hash matches the manifest entry.
4. Decompress if `compression` indicates compressed format.
5. Update `manifest.json` to reflect local storage after successful fetch.

### Host CAS Store (Optional; Recommended)

To reduce duplicated storage across retries and identical artifacts, the host MAY enable a content-addressed store (CAS):

- CAS root: `~/.local/share/rch/artifacts/cas/sha256/<aa>/<hash>` (where `<aa>` is the first two hex characters)
- Job directories reference CAS objects via hardlinks or symlinks (implementation choice)

Requirements when CAS is enabled:
- `manifest.json` remains authoritative for `sha256` + `bytes` (CAS does not change validation semantics)
- `rch xcode validate` MUST validate referenced CAS objects equivalently (follow links, hash bytes)
- `rch xcode gc` MUST account for CAS reachability from live job dirs + run index before deleting CAS objects

Rationale: `xcresult` can be very large, and agent loops produce many repeated attempts. CAS makes the lane cheaper to operate (disk + time) and improves the "reuse run" experience because artifacts are already present by hash.

### `backend_invocation.json` Requirements (Normative)

A small, structured record of what the harness actually invoked. MUST be safe to store remotely.

**Required fields:**
- `kind`: `"backend_invocation"`
- `schema_version`, `lane_version`
- `job_id`, `run_id`, `attempt`
- `backend` (object): `{ "preferred": "...", "actual": "..." }`

**Backend-specific fields:**

For `backend.actual = "xcodebuild"`:
- `argv` (array of strings) — The full argument vector passed to xcodebuild.
- `cwd` (string) — The working directory.

For `backend.actual = "xcodebuildmcp"`:
- `mcp_request` (object) — A structured, redacted/safe representation of the MCP request sent.

**Additional required fields:**
- `paths` (object) — The effective confined paths used (`dd`, `result`, `spm`) for auditability and reproducibility.
- `env_names` (array of strings) — Allowlisted environment variable names passed (values MUST NOT be included).

**Prohibited:**
- MUST NOT include secret values (env values, tokens, credentials).
- Environment MUST be represented as names/allowlist only.

### `environment.json` Requirements

- MUST include Xcode path + version + build number.
- MUST include macOS version.
- SHOULD include available simulator runtimes.
- SHOULD include Node.js version (if XcodeBuildMCP used).

### `timing.json` Requirements

- MUST include durations (seconds) for: `staging`, `running`, `collecting`.
- SHOULD include total wall-clock time.

### `staging.json` Requirements (Normative)

`staging.json` provides detailed information about the source staging phase, enabling debugging and performance tuning.

**Required fields:**
- `kind`: `"staging"`
- `schema_version`, `lane_version`, `job_id`, `run_id`, `attempt`
- `method` — `"rsync"` | `"git_snapshot"` | `"bundle"` (future)
- `excludes` — the effective exclude globs applied
- `bytes_sent` — total bytes transferred
- `files_total` — total files considered
- `files_changed` — files actually transferred (changed or new)
- `duration_seconds` — staging phase duration

**Optional fields:**
- `atomic_swap` (boolean) — true if staging used a temp dir + atomic rename into `src/`
- `tooling` — best-effort versions: `rsync`, `zstd` (when used)
- `compression` — whether compression was used during transfer
- `errors` — array of non-fatal staging errors/warnings
- `metadata` (object) — staging metadata handling:
  - `xattrs_stripped` (boolean) — Whether xattrs were stripped during staging
  - `acls_stripped` (boolean) — Whether ACLs were stripped during staging

### `metrics.json` Requirements

`metrics.json` captures resource and transfer metrics for each job run.

**Required fields (when emitted):**
- `staging_bytes_sent` — Total bytes transferred to worker during staging.
- `artifact_bytes_received` — Total bytes transferred back from worker.
- `queue_wait_seconds` — Time spent waiting for a worker lease (mirrors `status.json`).

**Optional fields:**
- `peak_memory_bytes` — Peak resident memory of the `xcodebuild` process on the worker.
- `cpu_seconds` — Total CPU time consumed by the build/test.
- `disk_usage_bytes` — Workspace disk usage at completion.
- `cache_hit` — Object with `derived_data` (boolean) and `spm` (boolean) cache hit/miss status.
- `cache_keys` — Object with `config_hash`, `derived_data_key`, `spm_key` (strings) when available.

### `events.ndjson` Requirements

The event stream is a newline-delimited JSON file where each line is a self-contained event object. Events are appended in real time during job execution.

**Append safety (Normative):**
- Writers MUST append complete, newline-terminated JSON objects only.
- Writers MUST NOT emit partial JSON lines.
- Writers MUST ensure each event line is <= `max_event_line_bytes` when configured; if the next event would exceed the cap, the harness MUST terminate the job with a final `complete` event using error code `event_line_too_long`.
- When `max_events_bytes` is configured, if emitting the next event would exceed the cap, the harness MUST terminate the job with a final `complete` event using error code `events_quota_exceeded`.
- Readers MUST tolerate the file growing during reads, and SHOULD ignore a non-terminal final line only if it is not newline-terminated (defensive tail behavior).

**Required fields per event:**

- `type` — Event type string (e.g., `"build_started"`, `"test_case_passed"`, `"phase_completed"`, `"error"`, `"complete"`).
- `timestamp` — ISO 8601 timestamp.
- `sequence` — Monotonically increasing integer (1-based).
- `job_id` — Job identifier (MUST match artifacts).
- `run_id` — Content-derived identifier.
- `attempt` — Attempt number for this run_id.

**Optional event hash chain (Normative when enabled):**

When `[profiles.<name>.integrity].event_hash_chain = true`:
- Every non-`complete` event MUST include:
  - `prev_event_sha256` (string) — lowercase hex SHA-256 of the previous non-`complete` event's JSON line bytes
  - `event_sha256` (string) — lowercase hex SHA-256 of this event, computed as:
    `SHA-256( "rch-xcode-lane/event_chain/v1\n" || prev_event_sha256 || "\n" || JCS(event_without_chain_fields) )`
  - Where `event_without_chain_fields` is the full event object with `prev_event_sha256` and `event_sha256` removed.
- The first event (`hello`) MUST use `prev_event_sha256` = 64 zero hex characters (`"0000...0000"`).
- The terminal `complete` event MUST include `event_chain_head_sha256` equal to the last non-`complete` event's `event_sha256`.

This enables streaming consumers to verify event integrity incrementally without waiting for the final `events_sha256` digest.

**Sequence integrity (Normative):**

- `sequence` MUST start at 1 and MUST increase by exactly 1 for each subsequent event (no gaps).

**Heartbeat (Recommended):**

- The harness SHOULD emit `{"type":"heartbeat",...}` at least every 10 seconds while running.

**Error model reuse (Normative):**
- Events with `type = "error"` MUST include an `error` object that conforms to the lane Error Object schema (`code`, `message`, `retryable`, `hint`, `detail`).
- Events with `type = "warning"` SHOULD include a `warning` object with the same shape (retryable is typically `false`).
- This enables streaming consumers to react to stable codes without waiting for the terminal `complete` event.

**Standard event types:**

| Type | Emitted when |
|------|-------------|
| `hello` | First event; echoes protocol_version, lane_version, job identity |
| `queued` | Job is waiting for a lease slot (periodic, at least every 10s) |
| `lease_acquired` | Lease slot acquired; job may begin backend execution |
| `job_started` | Job execution begins on worker |
| `phase_started` | A build phase begins (compile, link, etc.) |
| `phase_completed` | A build phase ends (includes duration) |
| `test_suite_started` | A test suite begins execution |
| `test_case_passed` | A single test case passes |
| `test_case_failed` | A single test case fails (includes failure message) |
| `test_suite_completed` | A test suite finishes (includes pass/fail counts) |
| `warning` | Non-fatal issue detected (SHOULD include Error Object) |
| `diagnostic` | Structured diagnostic (file/line/message) suitable for agent consumption |
| `artifact_ready` | A named artifact became available (e.g., build.log, result bundle) |
| `error` | Fatal error during execution (MUST include Error Object) |
| `heartbeat` | Periodic liveness signal (at least every 10s) |
| `complete` | Final event; includes terminal state + error model + `exit_code` |

**`diagnostic` event payload (Recommended):**

When emitting `diagnostic` events, the harness SHOULD include:
- `severity` — `"error"` | `"warning"` | `"note"`
- `message` (string) — The diagnostic message.
- `file` (string|null) — Source file path (relative to staged source root).
- `line` (integer|null) — Line number (1-based).
- `column` (integer|null) — Column number (1-based).
- `category` (string|null) — Diagnostic category, e.g. `"compiler"`, `"linker"`, `"test"`.
- `code` (string|null) — Diagnostic code if available.

This provides a structured middle ground between parsing `xcresult` (heavy) and scraping logs (fragile), enabling real-time "file:line + message" feedback for agents.

**`artifact_ready` event payload (Recommended):**

When emitting `artifact_ready` events, the harness SHOULD include:
- `artifact` (string) — logical name (e.g., `"build.log"`, `"result.xcresult"`)
- `path` (string) — relative-to-job-root path (never absolute)
- `bytes` (integer|null) — size when known, else null

This enables streaming consumers (e.g., `rch xcode watch`) to know when artifacts become available for progressive fetching.

**Optional stream digest (Recommended):**

- The `complete` event MAY include `events_sha256` computed as:
  `SHA-256( "rch-xcode-lane/events_stream/v1\n" || <exact UTF-8 bytes of events.ndjson excluding the complete line> )`
- When present, `summary.json` SHOULD copy the same `events_sha256` for easy validation.

Consumers MUST tolerate unknown event types (forward compatibility).

### `status.json` Requirements

`status.json` is a mutable file updated in-place throughout job execution. It provides a polling-friendly snapshot of current job state.

**Atomic update (Normative):**
- Writers MUST update `status.json` by writing a complete JSON document to a temporary path (e.g., `status.json.tmp`) and then atomically renaming it over `status.json`.
- Readers MUST tolerate missing/empty `status.json` during very early job creation.

**Required fields:**

- `job_id`, `run_id`, `attempt` — Job identity fields.
- `state` — Current lifecycle state (`created`, `staging`, `queued`, `running`, `collecting`, `uploading`, `succeeded`, `failed`, `canceled`, `timed_out`).
- `updated_at` — ISO 8601 timestamp of last update.
- `queued_at` — ISO 8601 timestamp of when the job entered the queue (null if never queued).
- `started_at` — ISO 8601 timestamp of when execution began on the worker (null if not yet started).
- `queue_wait_seconds` — Elapsed seconds between `queued_at` and `started_at` (null if not applicable).

**Optional fields:**

- `progress` — Free-form string (e.g., `"Compiling 42/128 files"`).
- `worker` — Name of the assigned worker.
- `bytes` — Optional transfer snapshot:
  - `staging_sent` (integer|null) — Bytes sent during staging.
  - `artifacts_received` (integer|null) — Bytes received during collection.
  - `uploaded` (integer|null) — Bytes uploaded to object store.

---

## Optional Artifacts

The following artifacts are not required but SHOULD be emitted when the information is available.

### `repro/` (Recommended)

A small, standardized reproduction bundle intended to make failed runs easy to re-run.

When emitted, the lane SHOULD include:
- `repro/inputs.json` — Exact copy of `effective_config.inputs` (enables re-running with identical hashable inputs).
- `repro/attestation_excerpt.json` — Minimal toolchain + destination identifiers needed to reason about reproducibility.
- `repro/README.txt` — Short instructions (no secrets) describing how to re-run with `rch xcode plan`/`test`.
- OPTIONAL: `repro/source.tar.zst` — Exact staged source snapshot (opt-in; may be disallowed in CI/object_store).

Profiles MAY control this via:

```toml
[profiles.ci.repro]
enabled = true
include_source_bundle = false
```

The repro bundle is especially valuable for failed runs, enabling developers to immediately reproduce the failure with identical inputs.

### `junit.xml`

Standard JUnit XML test report for integration with CI systems (Jenkins, GitHub Actions, etc.). Emitted when the job action is `test` and results are available.

- MUST conform to the JUnit XML schema (testsuite/testcase elements).
- SHOULD be generated from xcresult data or event stream test events.

### `test_summary.json`

Machine-readable test summary for agent consumption. Emitted when the job action is `test`.

**Required fields (when emitted):**

- `total` — Total test count.
- `passed` — Number of passed tests.
- `failed` — Number of failed tests.
- `skipped` — Number of skipped tests.
- `duration_seconds` — Total test execution time.
- `failures` — Array of `{ suite, test_case, message, file, line }` objects.

### `sarif.json` (Recommended for CI)

Standard SARIF output for build diagnostics and (optionally) test failures:
- SHOULD be derived from `xcresult` when available (preferred over log scraping).
- MUST be stable and schema-valid SARIF so CI systems can ingest it without custom tooling.
- Enables GitHub code scanning, PR annotations, and other SARIF-compatible integrations.

### `annotations.json` (Recommended for agent workflows)

A small, lane-defined diagnostic summary intended for UIs and agents:
- `{ kind, schema_version, lane_version, items: [...] }`
- Each item SHOULD include `{ severity, message, file, line, code|null, category }`
- Provides "file:line + message" format for agent consumption without bespoke xcresult parsing.

---

## Commands

| Command | Purpose |
|---------|---------|
| `rch xcode doctor` | Validate host setup (daemon, config, SSH tooling) |
| `rch xcode workers [--refresh]` | Enumerate/probe workers; show capability summaries |
| `rch xcode verify [--profile <name>]` | Probe worker + validate config against capabilities |
| `rch xcode plan --profile <name>` | Deterministically resolve effective config, worker selection, destination resolution; optionally compute hashes and show reuse candidates (no staging/run) |
| `rch xcode build [--profile <name>]` | Remote build gate |
| `rch xcode test [--profile <name>]` | Remote test gate |
| `rch xcode fetch <job_id>` | Pull artifacts (if stored remotely) |
| `rch xcode validate <job_id\|path>` | Verify artifacts: schema validation + manifest hashes + event stream integrity (+ provenance if enabled) |
| `rch xcode watch <job_id>` | Stream events + follow logs for a running job (supports resume) |
| `rch xcode status <job_id>` | Query best-effort remote status (queued/running/terminal) + latest sequence (supports resume) |
| `rch xcode cancel <job_id>` | Best-effort cancel (preserve partial artifacts) |
| `rch xcode explain <job_id\|run_id\|path>` | Explain selection/verification/refusal decisions (human + `--json`) |
| `rch xcode retry <job_id>` | Retry a failed job with incremented attempt (preserves run_id when unchanged) |
| `rch xcode reuse <run_id>` | Locate a succeeded attempt for run_id and return its artifact pointer (optionally validates) |
| `rch xcode gc` | Garbage-collect old runs + worker workspaces |
| `rch xcode warm [--profile <name>]` | Ask workers to prewarm simulator runtimes/caches for a profile |

**Machine-readable CLI output (Normative):**

Commands SHOULD accept `--json` to emit a single JSON object to stdout instead of human-readable text.

JSON outputs MUST use a standard envelope with:
- `kind` (string) — Command-specific result type, e.g. `"doctor_result"`, `"verify_result"`, `"plan_result"`, `"validate_result"`
- `schema_version` (string) — Schema version for this result type
- `lane_version` (string) — Lane implementation version
- `ok` (boolean) — Whether the command succeeded
- `error_code` (string|null) — Stable error code on failure (null on success)
- `errors` (array) — Array of error objects from the Error Model

This enables agent/CI integration without log scraping and provides consistent error handling across all commands.

### `plan` Output (Normative when `--json`)

`rch xcode plan --json` MUST emit a single envelope object:
- Standard envelope fields: `kind`, `schema_version`, `lane_version`, `ok`, `error_code`, `errors`
- `kind`: `"plan_result"`
- `profile` (string) — The profile name used
- `worker_selected` (object|null) — Selected worker info, or null if no eligible worker
- `effective_config` (object) — The resolved config envelope
- `hashes` (object|null):
  - `source_tree_hash` (string) — SHA-256 of the source manifest
  - `config_hash` (string) — SHA-256 of `JCS(effective_config.inputs)`
  - `run_id` (string) — Content-derived run identifier
- `reuse_candidate` (object|null): `{ job_id, attempt, path }` — An existing succeeded attempt for this run_id, if found

The command MUST support `--no-hash` to omit `hashes` and `reuse_candidate`, avoiding hashing large trees when speed matters.

### `watch` Resume Semantics (Normative)

`rch xcode watch` MUST be resilient to transport drops:
- While attached to a live `run` session, it SHOULD stream events directly from stdout and logs from stderr.
- If the session is interrupted, it MUST:
  1) Poll `rch-xcode-worker status` to determine whether the job is still running/queued/terminal, and
  2) Resume display by reading durable artifacts (`events.ndjson`, `build.log`) via the configured fetch mechanism.
- Event display MUST be deduplicated by `(job_id, sequence)` and MUST NOT re-emit an event with a sequence number <= the last displayed sequence.
- If `latest_sequence` regresses or gaps are detected, `watch` MUST emit a warning indicating possible corruption or partial file state.

### `explain` Output (Normative when `--json`)

`rch xcode explain --json` MUST emit a single envelope object:
- `kind`: `"explain_result"`
- `schema_version`, `lane_version`, `ok`, `error_code`, `errors` (standard envelope fields)
- `job` (object|null): `{ job_id, run_id, attempt }`
- `decision` (object|null): parsed/normalized `decision.json`
- `selection` (object|null): worker candidates + reasons (when available)
- `pins` (object|null): resolved Xcode build, runtime/device IDs, backend actual, and whether host key was pinned

This provides a one-command path for agents to understand worker selection, pinning decisions, and refusal reasons without parsing multiple artifacts.

### `reuse` Output (Normative when `--json`)

`rch xcode reuse --json` MUST emit a single envelope object:
- Standard envelope fields: `kind`, `schema_version`, `lane_version`, `ok`, `error_code`, `errors`
- `kind`: `"reuse_result"`
- `run_id` (string) — The run_id that was queried.
- `selected` (object|null): `{ job_id, attempt, path }` — The selected succeeded attempt, or null if none found.
- `validated` (boolean) — true if validation was performed and passed.

This enables agents to find and reuse existing succeeded runs without re-executing identical jobs.

### `doctor` Checks (Host)

- RCH daemon running
- SSH tooling available
- Config parseable
- Workers reachable

### `verify` Checks (Worker)

- Worker reachable via SSH
- Xcode installed at expected path (or discoverable)
- Requested destination available (simulator runtime + device)
- XcodeBuildMCP available (if configured as backend)
- Node.js version compatible

### `validate` Checks (Artifacts)

The `validate` command verifies artifact integrity and consistency for a completed job.

**Required validations:**

- All files listed in `manifest.json` exist and have matching SHA-256 hashes.
- All JSON artifacts parse successfully and include required `kind`, `schema_version`, `lane_version` fields.
- `job_request.json` parses successfully and its `job_id/run_id/attempt` match `summary.json`.
- `stage_receipt.json` parses successfully and its `job_id/run_id/attempt/source_tree_hash` match `job_request.json`.
- `job_request.json.config_inputs` is byte-for-byte equal to `effective_config.json.inputs` (after JCS decoding).
- If `job_request_sha256` is present in `hello`/`complete` events, recompute and verify it matches `SHA-256("rch-xcode-lane/job_request_sha256/v1\n" || JCS(job_request_without_digest))`.
- `probe.json` parses successfully and includes required fields (`kind`, `schema_version`, `protocol_versions`, `harness_version`, `lane_version`, `roots`, `backends`, `verbs`).
- JSON artifacts validate against their schemas (when schemas are available).
- Recompute `source_tree_hash` from `source_manifest.entries` using the Normative rules and verify it matches `attestation.json.source.source_tree_hash`.
- Verify `job_request.json.source_tree_hash` equals the recomputed `source_tree_hash` (and equals `stage_receipt.json.source_tree_hash`).
- Recompute `run_id` as `SHA-256( "rch-xcode-lane/run_id/v1\n" || JCS(effective_config.inputs) || "\n" || source_tree_hash_hex )` and verify it matches `summary.json.run_id` (and the `run_id` echoed in `events.ndjson`).
- `events.ndjson` parses as valid NDJSON, has contiguous `sequence` (no gaps), and ends with a terminal `complete` event.
- If `events_sha256` is present in the `complete` event, recompute and verify it matches.
- If hash chain fields are present, verify the chain (`prev_event_sha256`/`event_sha256`) and that `event_chain_head_sha256` matches the final non-`complete` event.
- `job_id`, `run_id`, `attempt` are consistent across artifacts.
- `summary.json` terminal state is consistent with `events.ndjson` terminal `complete` event (exit_code, error_code).
- If a run index (`runs/<run_id>/attempt-<n>`) exists, it MUST point to the matching `jobs/<job_id>` directory.

**Optional validations (when applicable):**

- Provenance signatures verify against `verify.json` public key.
- `source_manifest.json` entries hash to the recorded `source_tree_hash`.
- For directory artifacts (e.g., `result.xcresult/`), verify the tree hash matches the `manifest.json` entry using the normative tree hash algorithm.
- If `artifact_trees/<name>.tree.json` exists, verify its `tree_sha256` matches the manifest entry and the tree entries are consistent.

**Exit codes:**

- `0` — All validations passed.
- `1` — One or more validations failed (details in stdout as JSON).
- `2` — Artifact directory not found or unreadable.

---

## Performance Design

### Incremental Staging

- Use rsync with excludes (`.git`, `DerivedData`, `*.xcresult`, etc.).
- Optional: git archive/clone strategy for pristine working copies.

### Cache Buckets

| Cache | Key Components |
|-------|----------------|
| DerivedData | cache_namespace + config_hash |
| SwiftPM | Xcode build + resolved dependencies hash (derived from lockfiles such as `Package.resolved`) (+ optional toolchain constraints) |

### Cache Policy (Normative)

Cache usage MUST be controlled via `[profiles.<name>.cache]` and MUST be reflected in `effective_config.json`. The lane MUST record cache decisions and hit/miss stats in `metrics.json` (at minimum: derived data + SPM).

#### Cache Write Policy (Normative)

To prevent cache poisoning from untrusted PRs/forks, profiles MUST declare cache write intent:

```toml
[profiles.ci.cache]
derived_data = true
spm = true
mode = "read_only"          # "off" | "read_only" | "read_write"
```

- `off`: Do not read or write caches.
- `read_only` (recommended for CI with untrusted code): May read existing caches but MUST NOT write/update them. Prevents poisoning.
- `read_write` (default for trusted repos): May read and write caches.

The worker MUST enforce the cache mode:
- When `mode = "off"`, the worker MUST NOT use cached DerivedData or SPM packages.
- When `mode = "read_only"`, the worker MUST use cached data if available but MUST NOT write new cache entries.
- When `mode = "read_write"`, the worker MAY write cache entries.

`metrics.json` MUST record:
- `cache_mode` — The effective cache mode for the job.
- `cache_writable` — Boolean: whether the cache was writable for this job (false for `off` and `read_only`).

#### Atomic Cache Promotion (Normative)

To prevent cache corruption and poisoning:
- The worker MUST NOT write directly into the shared cache namespace during a job.
- The worker MUST use per-job cache work directories (e.g., under `jobs/<job_id>/cache-work/` or `cache_root/tmp/<job_id>/`).
- When `cache.mode = "read_write"`:
  - The worker MAY promote updated cache entries ONLY if the job terminal state is `succeeded`, unless explicitly overridden.
  - Promotion MUST be atomic (e.g., write to a new directory then `rename(2)` swap) so readers never observe partial state.
- When `cache.mode = "read_only"`:
  - The worker MUST ensure shared caches are not modified (best-effort via permissions and/or copy-on-write cloning).
  - Any attempted modification detected by the worker SHOULD emit a warning event and MUST NOT mutate the shared cache namespace.

Profiles MAY opt into "promote on failure" for specialized workflows:

```toml
[profiles.<name>.cache]
promote_on_failure = false   # default: false
```

**Performance note (Non-normative):** On macOS, `clonefile(2)` can provide fast copy-on-write snapshots for DerivedData.

#### Cache Isolation (Normative)

To reduce cross-run contamination and cache poisoning, profiles MAY define cache trust boundaries:

```toml
[profiles.ci.cache]
derived_data = true
spm = true
trust_domain = "per_profile"           # "shared" | "per_repo" | "per_profile" | "per_repo_and_posture"
allow_cross_posture_reads = true       # default true for trusted posture; see untrusted posture rules
```

- `trust_domain = "shared"`: All jobs share the same cache namespace. Use only for fully trusted repos.
- `trust_domain = "per_repo"` (default for CI): Caches are segregated by repository identity (e.g., repo URL hash).
- `trust_domain = "per_profile"`: Caches are segregated by profile name within a repo.
- `trust_domain = "per_repo_and_posture"`: Caches are segregated by repo identity AND effective trust posture (`trusted` vs `untrusted`), preventing cross-posture cache reuse by default.

The worker MUST segregate caches by the effective `trust_domain` boundary. At minimum, DerivedData and SPM caches MUST be isolated.

`metrics.json` SHOULD record:
- `cache_namespace` — The computed cache namespace key.
- `cache_writable` — Boolean indicating if the cache was writable for this job.
- `cache_posture_partitioned` — Boolean indicating if cache namespace included trust posture separation.

### Simulator Prewarm

- Boot simulator once, reuse across runs.
- Collect runtime info in `environment.json`.
- Avoid cold-start tax on every job.

### Simulator Hygiene (Normative)

To prevent test pollution and flaky behavior from shared simulator state, profiles MAY define simulator lifecycle policies:

```toml
[profiles.ci.simulator]
strategy = "shared_prebooted"          # "shared_prebooted" | "per_job"
erase_on_start = false                 # Erase simulator data before test run
shutdown_on_end = true                 # Shutdown simulator after job completion
device_set = "shared"                  # "shared" | "per_job" (Recommended for parallel CI)
```

- `strategy = "shared_prebooted"` (default): Reuse prebooted simulators across jobs. Faster but may accumulate state.
- `strategy = "per_job"`: Create a dedicated simulator device for the job, delete on completion (best-effort). Maximum isolation.

When `strategy = "per_job"`:
- The worker MUST create a new simulator device matching the destination spec.
- The worker SHOULD name the device deterministically for cleanup and debugging (e.g., `rch-<job_id>` or `rch-<run_id>-<attempt>`).
- The worker MUST record the created UDID in `effective_config.json`.
- The worker MUST delete the simulator device on job completion (best-effort cleanup).

When `erase_on_start = true`:
- The worker MUST erase the simulator device data before the test run begins.
- This is slower but ensures a clean state for each job.

`effective_config.json` MUST record the actual `simulator_strategy` and `simulator_udid` used.

**Device set isolation (Recommended; Normative when enabled or in untrusted posture):**

When `simulator.device_set = "per_job"` (or when forced by untrusted posture):
- The harness MUST set `SIMULATOR_DEVICE_SET_PATH` to a per-job path under the confined workspace (e.g., `worker_paths.work/device-set/`).
- All `simctl` operations and simulator launches for the job MUST use that device set.
- The harness MUST record `effective_config.resolved.simulator_device_set_path`.

This reduces cross-job CoreSimulator interference and improves parallelism reliability by isolating the device database per job. It also prevents state leakage between trusted and untrusted builds.

### Backend Path Discipline (Normative)

The harness MUST ensure backend outputs are confined to the per-job workspace by construction:

- When invoking `xcodebuild`, the harness MUST pass:
  - `-derivedDataPath` = `worker_paths.dd`
  - `-resultBundlePath` = `worker_paths.result/result.xcresult` (or equivalent under `result/`)
  - When supported and when `cache.spm` is enabled: `-clonedSourcePackagesDirPath` = `worker_paths.spm`
- When invoking XcodeBuildMCP, the harness MUST set equivalent fields in the MCP request so the same confinement holds.

The harness MUST record the effective paths in `effective_config.resolved.paths` and echo them in the `hello.worker_paths`.

### Concurrency Control

- Avoid thrashing by limiting concurrent jobs per worker.
- Queue jobs if worker is busy.
- Report queue position in job status.

#### Worker Leases (Normative)

To prevent resource exhaustion and ensure crash recovery, the lane uses a lease-based concurrency model:

- **Lease acquisition**: Before backend execution begins, the **harness MUST acquire a lease slot locally**. The harness is the source of truth for concurrency/queueing because it has perfect local visibility. The host does not maintain separate lease state; it observes harness `queued`/`lease_acquired` events and reflects them into `status.json`.
- **Lease TTL**: Every lease MUST have a time-to-live (TTL), defaulting to `timeout_seconds + 300` (5-minute grace). TTL is a hard upper bound to prevent runaway jobs.
- **Liveness**: Lease liveness MUST be tied to the active `rch-xcode-worker run` session (SSH connection + harness process). If the session is lost unexpectedly, the worker MUST treat the job as abandoned.
- **Crash recovery**: If liveness is lost (session drop) or TTL is exceeded:
  - Worker MUST terminate the associated job process (SIGTERM, then SIGKILL after 10s).
  - Worker MUST mark the job workspace for cleanup.
  - Host MUST transition the job to `failed` with reason `"lease_expired"` (retryable) when it cannot observe a clean terminal completion.
- **Concurrency limit**: Each worker advertises a maximum concurrent job count (default: 1). If the worker is at capacity, the harness MUST queue the job and emit periodic `queued` events (at least every 10s) until a lease slot is acquired.
- **`status.json` integration**: `queued_at`, `started_at`, and `queue_wait_seconds` fields (see status.json Requirements) MUST reflect actual lease acquisition timing.

**Hello event (lease fields, Normative):**
- The harness `hello` event MUST include `lease_id` and `lease_ttl_seconds` so the host can correlate execution with concurrency state and debug lease failures.

**Lease acquisition protocol (Normative):**

Before starting backend execution, the harness MUST acquire a lease slot consistent with `limits.max_concurrent_jobs`.

- If no slot is available, the harness MUST emit periodic `queued` events (at least every 10s) including:
  - `queue_position` (integer|null) — best-effort position in queue
  - `queue_wait_seconds` (number) — elapsed time queued
- Once a slot is acquired, the harness MUST emit `lease_acquired` including:
  - `lease_id` (string)
  - `lease_ttl_seconds` (integer)
  - `queue_wait_seconds` (number)
- Backend execution MUST NOT begin until after `lease_acquired` is emitted.

### Retry Policy (Normative)

To handle transient infrastructure failures gracefully, profiles MAY define automatic retry behavior:

```toml
[profiles.ci.retry]
max_attempts = 3                       # Maximum total attempts (including first try)
retry_on = ["lease_expired", "worker_unreachable", "source_staging_failed", "artifact_collection_failed"]
```

- `max_attempts` (integer, default: 1) — Maximum number of attempts. Value of 1 means no retries.
- `retry_on` (array of strings) — Error codes that trigger automatic retry. Empty means no automatic retries.

**Retry semantics:**

- Automatic retries MUST keep the same `run_id` (content-derived, unchanged).
- Each retry MUST increment `attempt` (1, 2, 3, ...).
- Each retry MUST allocate a new `job_id` (unique per attempt).
- Retries MUST respect `max_attempts`; after exhausting retries, the job fails with the last error.

**Event recording:**

- Each attempt MUST emit `attempt_started` and `attempt_complete` events in `events.ndjson`.
- `summary.json` MUST include `attempt` field and `attempts` array with per-attempt summaries when retries occur.

**Manual retry:**

`rch xcode retry <job_id>` SHOULD be provided to manually retry a failed job with `attempt` incremented. The new attempt inherits the original `run_id` if source and config are unchanged.

---

## Milestones

- **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
- **M1**: `doctor` + worker capability probe + config validation (`verify`)
- **M2**: Classifier + policy: allowlist routing, refuse-on-uncertain, explain decisions
- **M3**: Workspace sync + remote runner MVP (fallback `xcodebuild`) with log streaming
- **M4**: XcodeBuildMCP backend with structured events (build phases, test events)
- **M5**: Determinism outputs: summary.json, effective_config.json, attestation.json, manifest.json, environment.json, timing.json
- **M5.1**: Decision artifact (`decision.json`) + event stream (`events.ndjson`) + live status (`status.json`)
- **M5.2**: Source snapshot in attestation + source policy enforcement + `source_manifest.json`
- **M5.3**: Artifact schema versioning (`kind`, `schema_version`, `lane_version`) + metrics.json
- **M5.4**: Error model (stable error codes in summary.json, decision.json)
- **M6**: Caching + performance: DerivedData/SPM caches, incremental sync, simulator prewarm, concurrency control
- **M6.1**: Worker leases + crash recovery + queue-wait metrics
- **M6.2**: Cache isolation (trust_domain boundaries)
- **M6.3**: Simulator hygiene (per-job strategy, erase/shutdown policies)
- **M7**: Ops hardening: timeouts, cancellation, worker harness (`rch-xcode-worker`), partial artifact preservation
- **M7.1**: Optional provenance (signed attestation + manifest)
- **M7.2**: Optional CI reports (`junit.xml`, `test_summary.json`)
- **M7.3**: Garbage collection (`rch xcode gc`) + worker workspace retention
- **M7.4**: Retry policy (automatic retries on transient errors)
- **M7.5**: Artifact storage + compression (worker/object_store modes, xcresult compression)
- **M7.6**: `validate` command (artifact integrity verification)
- **M7.7**: Transport trust enforcement (`require_pinned_host_key`)
- **M8**: Compatibility matrix + fixtures: golden configs, sample repos, reproducible failure cases

---

## Conformance & Fixtures (Recommended; Normative for the repo implementing this spec)

To prevent contract drift between host and harness:
- The repository implementing the lane SHOULD include a `fixtures/` directory with:
  - `fixtures/probe/` — probe JSON examples (multiple protocol versions if supported)
  - `fixtures/events/` — NDJSON event streams covering success/failure/cancel/timeout and corruption cases
  - `fixtures/jobs/` — minimal artifact directories used to exercise `rch xcode validate`
- CI SHOULD run a conformance job that:
  - Validates all JSON artifacts against schemas
  - Recomputes and verifies `run_id`, `config_hash`, and `source_tree_hash`
  - Runs `validate` against `fixtures/jobs/*` and asserts stable error codes for known-bad fixtures

### Hashing Test Vectors (Recommended)

To prevent cross-language JCS/run_id drift, the repo SHOULD include `fixtures/hashing/` with:
- `inputs.json` — sample `effective_config.inputs`
- `inputs.jcs` — expected canonical bytes after JCS
- `config_hash.sha256` — expected lowercase hex SHA-256 of `"rch-xcode-lane/config_hash/v1\n" || inputs.jcs`
- `source_manifest.entries.json` — sample entries array
- `source_tree_hash.sha256` — expected lowercase hex hash with domain prefix `"rch-xcode-lane/source_tree_hash/v1\n"`
- `run_id.sha256` — expected lowercase hex run_id with domain prefix `"rch-xcode-lane/run_id/v1\n"`
- `tree_entries.json` — sample tree entries for directory artifact
- `tree_hash.sha256` — expected lowercase hex hash with domain prefix `"rch-xcode-lane/tree_hash/v1\n"`

CI SHOULD recompute and assert exact matches to catch canonicalization drift (Unicode normalization, integer/float encoding, array ordering) and domain prefix changes.

---

## Policies Summary

| Policy | Default | Override |
|--------|---------|----------|
| Code signing | `CODE_SIGNING_ALLOWED=NO` | `code_signing_allowed = true` |
| Mutating commands | Disallowed | `allow_mutating = true` |
| Uncertain classification | Refuse (false negative) | N/A |
| Floating destination (CI) | Disallowed | `allow_floating_destination = true` |
| Duplicate destinations | Error | `destination.on_multiple = "select"` |
| Dirty working tree (CI) | Disallowed | `require_clean = false` |
| Worker user | Dedicated account | Operator responsibility |
| Provenance signing | Disabled | `[profiles.<name>.provenance] enabled = true` |
| Flag passthrough | Disallowed | N/A (must model as config fields) |
| SSH host key pinning | Optional (recorded) | `ssh_host_key_fingerprint` in workers.toml |
| CI requires pinning | Disabled | `[profiles.<name>.trust] require_pinned_host_key = true` |
| Cache write policy | `read_write` | `[profiles.<name>.cache] mode = "read_only"` |
| Environment passthrough | Disallowed | `[profiles.<name>.env] allow = [...]` |
| Log redaction | Disabled | `[profiles.<name>.redaction] enabled = true` |
| Backend preference | `xcodebuildmcp` | `[profiles.<name>.backend] preferred = "xcodebuild"` |
| Trust posture | `auto` | `[profiles.<name>.trust] posture = "untrusted"` |
| Event hash chain | Disabled | `[profiles.<name>.integrity] event_hash_chain = true` |
| Cache promote on failure | Disabled | `[profiles.<name>.cache] promote_on_failure = true` |
| Symlinks in source | `forbid` (untrusted) / `allow_safe` (trusted) | `[profiles.<name>.source] symlinks = "allow_all"` |
| Post-run source verification | Disabled | `[profiles.<name>.source] verify_after_run = true` |
| Worker selection | `least_busy` | `[profiles.<name>.worker] selection = "warm_cache"` |

---

## Next Steps

1. Bring Mac mini worker online
2. Implement `rch xcode doctor` and `rch xcode verify`
3. Add classifier + routing + refusal/explanation paths + decision artifact
4. Implement workspace sync + remote runner + log streaming + worker harness
5. Add XcodeBuildMCP backend + event stream
6. Emit determinism artifacts + source snapshot + schema versioning
7. Add caching + performance optimizations + worker leases
8. Add optional provenance + CI reports
9. Implement garbage collection + retention policies
