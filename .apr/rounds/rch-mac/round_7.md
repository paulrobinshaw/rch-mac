Below are the highest-leverage spec upgrades I’d make to improve **robustness, determinism, compatibility, and operator sanity**. For each change: rationale + a focused **git-diff** against the text you pasted.

---

## 1) Protocol version negotiation + feature flags (make upgrades survivable)

### Why this makes it better

Right now, “`protocol_version` incompatible ⇒ fail” is *too brittle* for real ops: you’ll want to roll out worker updates gradually, support mixed fleets, and optionally enable ops like `tail`/`events`/`attestation_signing` without forking the protocol.

Adding:

* **`protocol_min`/`protocol_max`** (worker-supported range)
* **`features[]`** (capabilities/optional ops)

…lets the host pick a common protocol version, gracefully degrade (e.g., poll if `tail` absent), and produce clearer “why incompatible” diagnostics.

### Diff

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Host↔Worker protocol (normative)
 The system MUST behave as if there is a versioned protocol even if implemented over SSH.
 
-### Versioning
-- Host and worker MUST each report `rch_xcode_lane_version` and `protocol_version`.
-- If `protocol_version` is incompatible, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE`.
+### Versioning + feature negotiation
+On `probe`, the worker MUST report:
+- `rch_xcode_lane_version` (string)
+- `protocol_min` / `protocol_max` (int; inclusive range supported by the worker)
+- `features` (array of strings; see below)
+
+The host MUST select a single `protocol_version` within the intersection of host- and worker-supported ranges
+and use that value for every subsequent request in the run.
+
+If there is no intersection, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE`.
+If the worker lacks a host-required feature, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE`
+and `failure_subkind=FEATURE_MISSING`.
+
+`features` is a capability list used for forward-compatible optional behavior. Example feature strings:
+`tail`, `fetch`, `events`, `has_source`, `upload_source`, `attestation_signing`.
@@
 ### RPC envelope (normative, recommended)
 All worker operations SHOULD accept a single JSON request on stdin and emit a single JSON response on stdout.
 This maps directly to an SSH forced-command entrypoint.
 
 Request:
-- `protocol_version` (int)
+- `protocol_version` (int; selected by host after probe)
 - `op` (string: probe|submit|status|tail|cancel|fetch|has_source|upload_source)
 - `request_id` (string, caller-chosen)
 - `payload` (object)
@@
 ### Worker RPC surface (recommended, maps cleanly to SSH forced-command)
 Worker SHOULD implement these operations with JSON request/response payloads:
-- `probe` → returns `capabilities.json`
+- `probe` → returns protocol range/features + `capabilities.json`
 - `submit` → accepts `job.json` (+ optional bundle upload reference), returns ACK and initial status
 - `status` → returns current job status and pointers to logs/artifacts
 - `tail` → returns the next chunk of logs/events given a cursor (repeatable; host loops to "stream")
 - `cancel` → requests best-effort cancellation
 - `fetch` → returns artifacts (or a signed manifest + download hints)
 - `has_source` → returns `{exists: bool}` for a given `source_sha256`
 - `upload_source` → accepts a source bundle upload for a given `source_sha256`
```

---

## 2) Specify ID formats + filesystem safety (avoid path weirdness + id collisions)

### Why this makes it better

`run_id`/`job_id`/`request_id` are used everywhere (paths, cache keys, RPC idempotency). If you don’t **normatively constrain** them, someone will eventually generate IDs containing `/`, whitespace, or `..` and you’ll get:

* path traversal hazards
* hard-to-debug cross-platform path issues
* accidental `job_id` collisions that break worker idempotency guarantees

### Diff

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Terminology
@@
 - **Job ID** (`job_id`): unique identifier for a single step job within a run (used for cancellation/log streaming and artifact paths).
@@
 - **Artifact set**: schema-versioned outputs written under `<job_id>/`.
+
+### Identifier formats (normative)
+`run_id`, `job_id`, and `request_id` MUST be treated as opaque strings, but MUST be filesystem-safe:
+- MUST NOT contain `/`, `\`, whitespace, control characters, or `..`
+- MUST match: `^[A-Za-z0-9][A-Za-z0-9_-]{9,63}$`
+- MUST be safe to embed in file paths without additional escaping
+
+Generation guidance:
+- Host SHOULD generate `run_id` and `job_id` using a sortable unique ID (ULID or UUIDv7 recommended).
+- `job_id` MUST be globally unique per host over time (host MUST NOT reuse a `job_id` across runs).
+- `request_id` MUST be unique per RPC request (per host process) to allow reliable correlation and retries.
```

---

## 3) Worker selection: explicit `mode` (deterministic vs adaptive) so “deterministic” actually means something

### Why this makes it better

Your spec currently says “host MUST choose deterministically” but also allows “lowest load / highest free disk” as a tie-breaker. That’s *implicitly time-varying*, which will surprise users (“same inputs, different worker today”).

Solution: make the policy explicit and auditable:

* `worker_selection.mode = "deterministic"` (default): only stable fields can affect sorting.
* `worker_selection.mode = "adaptive"`: allow dynamic metrics *but record them* as selection evidence.

### Diff

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Worker selection (normative)
 Given a set of eligible workers (tag match + reachable), host MUST choose deterministically
 unless user explicitly requests randomness.
 
+Selection mode (normative):
+- Default: `worker_selection.mode = "deterministic"`
+- Optional: `worker_selection.mode = "adaptive"`
+
+In `deterministic` mode, dynamic metrics (e.g. current load, free disk) MUST NOT affect ordering/choice.
+In `adaptive` mode, dynamic metrics MAY be used as tie-breakers, but the host MUST record the exact
+metric values used in `worker_selection.json`.
+
 Selection inputs:
 - required tags: `macos,xcode` (and any repo-required tags)
 - constraints: Xcode version/build, platform (iOS/macOS), destination availability
-- preference: lowest load / highest free disk MAY be used only as a tie-breaker
+- preference: only in `adaptive` mode, lowest load / highest free disk MAY be used as a tie-breaker
 
 Selection algorithm (default):
 1. Filter by required tags.
 2. Probe or load cached `capabilities.json` snapshots (bounded staleness).
 3. Filter by constraints (destination exists, required Xcode available).
 4. Sort deterministically by:
    - explicit worker priority (host config)
    - then stable worker name
 5. Choose first.
@@
 The host MUST write:
 - `worker_selection.json` (inputs, filtered set, chosen worker, reasons)
 - `capabilities.json` snapshot as used for the decision
```

---

## 4) Strengthen `job_key_inputs` to prevent “false cache hits” after macOS/simulator changes + add simulator provisioning mode

### Why this makes it better

Two big correctness risks:

1. **macOS version/build matters** even with pinned Xcode (system libraries, simulator services, underlying toolchain plumbing). If the worker OS updates, reusing results keyed only by Xcode build can become “mysteriously wrong.”

2. Simulator tests are often flaky because state leaks between runs. A first-class **`destination.provisioning = existing|ephemeral`** lets you opt into clean per-job simulators when you need reliability.

This also makes your “deterministic destination resolution” more *truly* deterministic: you should record not just `OS=latest` → `OS=19.2`, but also the **runtime identifier/build**.

### Diff

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### Job key computation (normative)
 `job_key_inputs` MUST be an object containing the fully-resolved, output-affecting inputs for the job, and MUST include at least:
 - `source_sha256` (digest of the canonical source bundle)
 - `sanitized_argv` (canonical xcodebuild arguments; no output-path overrides)
-- `toolchain` (at minimum: Xcode build number + `developer_dir`)
-- `destination` (resolved destination details; may also be present inside `sanitized_argv`)
+- `toolchain` (resolved toolchain identity; see below)
+- `destination` (fully-resolved destination identity; see below)
 
 `job_key` is the SHA-256 hex digest of the RFC 8785 JSON Canonicalization (JCS) of `job_key_inputs`.
 
+Toolchain identity (normative):
+`job_key_inputs.toolchain` MUST include:
+- `xcode_build` (e.g. `"16C5032a"`)
+- `developer_dir` (absolute path on worker)
+- `macos_version` (e.g. `"15.3.1"`)
+- `macos_build` (e.g. `"24D60"`)
+- `arch` (e.g. `"arm64"`)
+
+Destination identity (normative):
+`job_key_inputs.destination` MUST include enough detail to prevent cross-runtime confusion.
+At minimum it MUST include:
+- `platform` (e.g. `"iOS Simulator"` or `"macOS"`)
+- `name` (device name when applicable)
+- `os_version` (resolved concrete version; MUST NOT be `"latest"`)
+- `provisioning` (`"existing"` | `"ephemeral"`)
+
+For iOS/tvOS/watchOS Simulator destinations, it MUST ALSO include:
+- `sim_runtime_identifier` (e.g. `com.apple.CoreSimulator.SimRuntime.iOS-19-2`)
+- `sim_runtime_build` (runtime build string if available from `simctl`)
+- `device_type_identifier` (e.g. `com.apple.CoreSimulator.SimDeviceType.iPhone-16`)
+
 `job_key_inputs` MUST NOT include host-only operational settings that should not invalidate correctness-preserving caches,
 including (non-exhaustive): timeouts, worker inventory/SSH details, cache toggles, worker selection metadata, backend selection.
@@
 ## Architecture (high level)
 Pipeline stages:
@@
-3. **Destination resolver**: resolves any destination constraints (e.g. `OS=latest`) using the chosen worker's `capabilities.json` snapshot and records the resolved destination.
+3. **Destination resolver**: resolves any destination constraints (e.g. `OS=latest`) using the chosen worker's `capabilities.json` snapshot and records the resolved destination (including simulator runtime identifiers/builds).
+3.1 **Destination provisioning (optional, recommended)**: if `destination.provisioning="ephemeral"` and the destination is a Simulator, the worker provisions a clean simulator per job and records the created UDID in artifacts (UDID MUST NOT affect `job_key`).
@@
 ## Worker capabilities
 Worker reports a `capabilities.json` including:
@@
 - macOS version + architecture
-- available runtimes/devices (simctl)
+- available runtimes/devices (simctl), including runtime identifiers and runtime build strings when available
 - installed tooling versions (rch-worker, XcodeBuildMCP)
 - capacity (max concurrent jobs, disk free)
@@
 ## Artifacts
@@
 - invocation.json
 - toolchain.json
+- destination.json (recommended; resolved destination + provisioning details)
 - metrics.json
 - source_manifest.json
 - worker_selection.json
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 [destination]
 mode = "constraints"  # pinned | constraints
 value = "platform=iOS Simulator,name=iPhone 16,OS=latest"
 # In constraints mode, the host resolves "latest" using the selected worker's capabilities snapshot
 # and records the resolved destination into job.json for determinism.
+provisioning = "existing" # existing | ephemeral
+# If ephemeral, the worker provisions a clean Simulator per job (recommended for flaky test suites).
```

---

## 5) Artifact profiles + profile-aware result cache (avoid “cached but missing the stuff I asked for”)

### Why this makes it better

You explicitly allow caching keyed by `job_key` *without* including `backend`. That’s fine for correctness, but it can become a UX footgun:

* A user asks for the **MCP backend** (richer diagnostics),
* the worker satisfies it from an **xcodebuild-cached** result,
* and you’re missing “rich” artifacts (events, summaries, JUnit, etc.).

Solution: add an **artifact profile** requested by the JobSpec, and make the worker’s result cache only reuse entries that satisfy the requested profile.

### Diff

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### JobSpec schema outline (v1)
 The canonical JobSpec (`job.json`) MUST include at least these fields:
@@
 | `effective_config`  | object   | Merged repo + host config snapshot               |
 | `created_at`        | string   | RFC 3339 UTC timestamp                           |
+
+Optional but recommended fields (v1, backward-compatible):
+| `artifact_profile`  | string   | `minimal` or `rich` (defaults to `minimal`)      |
@@
 ## Backend contract (normative)
 Regardless of backend, the worker MUST:
@@
 Minimum artifact expectations (normative):
 - `build.log` MUST be present for all jobs
 - `result.xcresult/` MUST be present for `test` jobs (backend may generate via `-resultBundlePath`)
 - `summary.json` MUST include backend identity (`backend=...`) and a stable exit_code mapping
+
+## Artifact profiles (recommended)
+Jobs MAY request an `artifact_profile`:
+- `minimal` (default): only the minimum artifact expectations are required.
+- `rich`: in addition to `minimal`, the worker MUST emit:
+  - `events.jsonl`
+  - `build_summary.json` (for build jobs)
+  - `test_summary.json` + `junit.xml` (for test jobs)
+
+`summary.json` SHOULD record the `artifact_profile` actually produced.
@@
 ### Result cache (recommended)
 Worker SHOULD maintain an optional result cache keyed by `job_key`:
 - If present and complete, a submit MAY be satisfied by materializing artifacts from the cached result.
 - The worker MUST still emit a correct `attestation.json` for the new `job_id` referencing the same `job_key`.
+
+Profile-aware reuse (recommended, prevents surprise omissions):
+- Each cached entry SHOULD record `artifact_profile` it satisfies.
+- A submit MAY be satisfied from cache ONLY if cached `artifact_profile` is >= requested `artifact_profile`.
+- Otherwise, the worker MUST execute the job to produce the missing richer artifacts.
```

---

## 6) Align cache namespace naming (`cache.namespace`) across docs + make it explicitly filesystem-safe

### Why this makes it better

You currently have:

* README config: `[cache] namespace = "myapp"`
* PLAN text: `cache_namespace` (different name)

That inconsistency will leak into implementations, schemas, and operator configs. Also, namespace is used in directory naming → needs filesystem safety.

### Diff

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### Cache namespace (recommended)
-Repo config SHOULD provide a stable `cache_namespace` used as part of shared cache directory names,
+Repo config SHOULD provide a stable `cache.namespace` used as part of shared cache directory names,
 to prevent collisions across unrelated repos on the same worker.
+
+`cache.namespace` MUST be filesystem-safe:
+- MUST match: `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`
+- MUST NOT contain `/`, `\`, whitespace, or `..`
```

---

## 7) Schema compatibility policy + `schema_id` (make third-party tooling reliable)

### Why this makes it better

You’re investing in JSON Schemas and deterministic artifacts. To make that ecosystem reliable, you want:

* A stable **`schema_id`** that tools can match against (even if file paths move)
* A clear **compatibility rule** so adding fields doesn’t panic consumers

This is especially useful for: CI validators, artifact browsers, future “BV”/dashboards, and external integrators.

### Diff

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Artifact schemas + versioning
 All JSON artifacts MUST include:
 - `schema_version`
+- `schema_id`
 - `created_at`
@@
 Job-scoped artifacts MUST include:
 - `run_id`
 - `job_id`
 - `job_key`
+
+### Schema compatibility rules (normative)
+- Adding new optional fields is a backward-compatible change and MUST NOT require bumping `schema_version`.
+- Removing fields, changing meanings/types, or tightening constraints in a way that breaks old producers/consumers
+  MUST bump `schema_version`.
+- `schema_id` MUST be a stable string identifier for the artifact schema, e.g.:
+  - `rch-xcode/job@1`
+  - `rch-xcode/summary@1`
+  - `rch-xcode/run_plan@1`
@@
 ## JSON Schemas (recommended)
 The project SHOULD ship machine-readable JSON Schemas for all normative artifacts under `schemas/rch-xcode/`.
 Schema files SHOULD follow the naming convention `<artifact_name>.schema.json` (e.g. `schemas/rch-xcode/job.schema.json`).
 Schemas enable automated validation in CI and by third-party tooling.
+
+Schema authoring recommendations:
+- Each schema SHOULD include a JSON Schema `$id` that corresponds to `schema_id`.
+- Use a single JSON Schema draft consistently across the project (2020-12 recommended).
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Docs map
@@
-- `schemas/rch-xcode/*` — machine-readable JSON Schemas for normative artifacts
+- `schemas/rch-xcode/*` — machine-readable JSON Schemas for normative artifacts (see `schema_id` in each JSON)
```

---

If you want one more “big swing” after these: I’d add a **small normative section that defines canonical ordering of allowed `xcodebuild` flags** (so `sanitized_argv` canonicalization is unambiguous), plus a strict rule that **exactly one destination** is allowed per job (multiple destinations can be modeled as multiple step jobs). That tends to eliminate a lot of “why did the cache miss/hit?” confusion.
