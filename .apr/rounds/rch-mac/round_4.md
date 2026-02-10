Below are the revisions I’d make to **improve determinism, safety clarity, protocol rigor, caching correctness, and operator UX**, while keeping the spirit of your spec (“gate/lane, not IDE; safe-by-default; agent-friendly artifacts”).

I’m treating **`PLAN.md` as normative** and **`README.md` as operator-facing**.

---

## 1) README: add a “Quickstart” and explicitly state the trust boundary (build scripts run)

### Why this makes it better

Right now the README explains *what* happens, but not the “3 commands you’ll actually run” and not the most important safety nuance: **Xcode builds can execute repo-defined scripts** (Run Script phases, build tool plugins, etc.). If you’re positioning this as “safe-by-default”, you want to be crystal-clear what is and isn’t being prevented (you’re preventing *arbitrary host-side invocation flags / workflows*, not preventing the project itself from executing build-time code on the worker).

### Diff (README.md)

````diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Setup
 1. Add Mac mini to `~/.config/rch/workers.toml` with tags `macos,xcode`
 2. Add `.rch/xcode.toml` to your repo
 3. Start daemon: `rch daemon start`
 4. Run: `rch xcode verify`
+
+## Quickstart
+Most common flows:
+
+```bash
+# Validate setup without executing anything
+rch xcode verify --dry-run
+
+# Explain why a command will/won't be intercepted
+rch xcode explain -- xcodebuild test -workspace MyApp.xcworkspace -scheme MyApp
+
+# Run the repo-defined verify lane (usually build+test)
+rch xcode verify
+```
+
+## Trust boundary (important)
+`rch xcode` is **not** a sandbox. If your Xcode project contains Run Script build phases,
+SwiftPM build tool plugins, or other build-time code execution, that code will run on the worker
+under the `rch` user.
+Treat the worker like CI: dedicated account, minimal secrets, and no personal keychains.
````

---

## 2) PLAN: fix identifier ambiguity (run_id/job_id) and correct artifact schema rules (run-scoped vs job-scoped)

### Why this makes it better

Two issues today:

* `run_id` is described as “stable” but it’s unclear whether it’s deterministic or merely “doesn’t change once assigned.”
* “All JSON artifacts MUST include job_id/job_key” conflicts with the existence of **run-level** artifacts like `run_summary.json` and `worker_selection.json`.

This change makes your contract internally consistent and easier to implement without weird exceptions.

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Terminology
 - **Host**: the machine running `rch` (may be Linux/macOS).
 - **Worker**: macOS machine that executes the job.
 - **Run**: a top-level verification attempt (e.g. `rch xcode verify`) that may include multiple jobs.
-- **Run ID** (`run_id`): stable identifier for the run artifact directory.
+- **Run ID** (`run_id`): unique opaque identifier for the run artifact directory (stable once chosen; not required to be deterministic).
 - **Invocation**: user-provided command line (e.g. `xcodebuild test ...`).
 - **Classifier**: deny-by-default gate that accepts/rejects invocations.
 - **JobSpec** (`job.json`): deterministic, fully-resolved step job description.
+- **Job ID** (`job_id`): unique identifier for a single step job within a run (used for cancellation/log streaming and artifact paths).
 - **Job key** (`job_key`): stable hash used for caching and attestation.
 - **Artifact set**: schema-versioned outputs written under `<job_id>/`.
@@
 ## Artifact schemas + versioning
-All JSON artifacts MUST include:
-- `schema_version`
-- `created_at`
-- `job_id` and `job_key`
+All JSON artifacts MUST include:
+- `schema_version`
+- `created_at`
+
+Run-scoped artifacts MUST include:
+- `run_id`
+
+Job-scoped artifacts MUST include:
+- `run_id`
+- `job_id`
+- `job_key`
```

---

## 3) PLAN: formalize a minimal RPC envelope + error model (maps cleanly to SSH forced-command)

### Why this makes it better

You already say “behave as if there is a versioned protocol.” Make it explicit with a tiny envelope:

* easier to validate/parse
* easier to stream logs with cursors
* easier to implement as a **single forced-command entrypoint** (`rch-worker xcode rpc`) using stdin/stdout JSON

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Host↔Worker protocol (normative)
 The system MUST behave as if there is a versioned protocol even if implemented over SSH.
 
 ### Versioning
 - Host and worker MUST each report `rch_xcode_lane_version` and `protocol_version`.
 - If `protocol_version` is incompatible, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE`.
+
+### RPC envelope (normative, recommended)
+All worker operations SHOULD accept a single JSON request on stdin and emit a single JSON response on stdout.
+This maps directly to an SSH forced-command entrypoint.
+
+Request:
+- `protocol_version` (int)
+- `op` (string: probe|submit|status|tail|cancel|fetch)
+- `request_id` (string, caller-chosen)
+- `payload` (object)
+
+Response:
+- `protocol_version` (int)
+- `request_id` (string)
+- `ok` (bool)
+- `payload` (object, when ok=true)
+- `error` (object, when ok=false) containing: `code`, `message`, optional `data`
```

---

## 4) PLAN+README: make destination resolution explicitly deterministic (resolve from capability snapshot, record resolved value)

### Why this makes it better

Your README uses `OS=latest` with `mode="pinned"`, which is a determinism footgun unless you define **what “latest” means** and **where it’s resolved**.

The contract should be:

* resolve destination **on the host** using a specific `capabilities.json` snapshot
* record the **resolved destination string** into `job.json`
* worker uses exactly what the host resolved (or rejects if it doesn’t exist)

### Diff (README.md)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 [destination]
-mode = "pinned"
-value = "platform=iOS Simulator,name=iPhone 16,OS=latest"
+mode = "constraints"  # pinned | constraints
+value = "platform=iOS Simulator,name=iPhone 16,OS=latest"
+# In constraints mode, the host resolves "latest" using the selected worker's capabilities snapshot
+# and records the resolved destination into job.json for determinism.
```

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Architecture (high level)
 Pipeline stages:
 1. **Classifier**: detects safe, supported Xcode build/test invocations (deny-by-default).
 2. **Run builder**: resolves repo `verify` actions into an ordered run plan and chooses a worker once.
-3. **JobSpec builder**: produces a fully specified, deterministic step job description (no ambient defaults).
+3. **Destination resolver**: resolves any destination constraints (e.g. `OS=latest`) using the chosen worker’s `capabilities.json` snapshot and records the resolved destination.
+4. **JobSpec builder**: produces a fully specified, deterministic step job description (no ambient defaults).
 4. **Transport**: bundles inputs + sends to worker (integrity checked).
 5. **Executor**: runs the job on macOS via a selected backend (**xcodebuild** or **XcodeBuildMCP**).
 6. **Artifacts**: writes a schema-versioned artifact set + attestation.
```

---

## 5) PLAN: separate **idempotency** (job_id) from **caching/dedup** (job_key) to avoid collisions and enable reuse

### Why this makes it better

Today: `(job_id, job_key)` as idempotency key prevents clean reuse when you generate a new job_id for a rerun.

Better:

* `job_id` = idempotency for “this submission attempt”
* `job_key` = correctness-preserving cache key (can be reused across attempts)
* worker MAY serve a cached result for a new job_id by materializing artifacts and recording provenance

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### Job lifecycle + idempotency
-Worker MUST treat `(job_id, job_key)` as an idempotency key:
-- If a job with the same `(job_id, job_key)` is already COMPLETE, worker MAY return the existing artifacts.
-- If a job with the same `(job_id, job_key)` is RUNNING, worker MUST report status and allow log tailing.
-- If `(job_id, job_key)` mismatches (same job_id, different key), worker MUST reject to prevent artifact confusion.
+Worker MUST treat `job_id` as the idempotency key:
+- If a job with the same `job_id` is already COMPLETE, worker MAY return the existing artifacts.
+- If a job with the same `job_id` is RUNNING, worker MUST report status and allow log tailing.
+- If `job_id` already exists but the submitted `job_key` differs, worker MUST reject to prevent artifact confusion.
+
+Worker MAY additionally maintain a correctness-preserving *result cache* keyed by `job_key`:
+- On submit of a new `job_id` with a previously completed `job_key`, worker MAY materialize artifacts
+  from the cached result into the new `<job_id>/` artifact directory and record `cached_from_job_id`.
```

---

## 6) PLAN: tighten canonical bundling spec (tar format details + reproducibility tests) and explicitly define compression

### Why this makes it better

You’re close, but “deterministic archive format” needs enough specificity to avoid cross-platform tar differences. Also, you mention `rsync + zstd` in README requirements; the plan should acknowledge **zstd compression** as a transport detail while keeping the **canonical tar** as the hash basis.

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Source bundling canonicalization (normative)
 The host MUST create a canonical source bundle such that identical inputs yield identical `source_sha256`.
 
 Rules:
-- Use a deterministic archive format (e.g. `tar`) with:
+- Use a deterministic `tar` archive (PAX recommended) with:
   - sorted file paths (lexicographic, UTF-8)
   - normalized mtimes (e.g. 0) and uid/gid (0)
   - stable file modes (preserve executable bit; normalize others)
+  - fixed pax headers where applicable (avoid host-dependent extended attributes)
 - Exclude by default:
@@
 - The host MUST emit `source_manifest.json` listing:
   - `path`, `size`, `sha256` per file
   - manifest `schema_version`
+
+Transport note (non-normative but recommended):
+- The canonical tar MAY be compressed with zstd for transfer, but `source_sha256` MUST be computed
+  over the canonical (pre-compression) tar bytes.
+
+Compliance (recommended):
+- Provide a fixture-based reproducibility test: identical repo inputs on Linux/macOS produce identical `source_sha256`.
```

---

## 7) PLAN: make cache keying explicitly include Xcode build + platform, and add a “result cache” as a first-class concept

### Why this makes it better

Shared caches without explicit keying inevitably rot. You *already* have `toolchain.json`; use it:

* DerivedData key should include `job_key` (already) **and** Xcode build number (prevents cross-Xcode corruption)
* SPM shared cache should include `Package.resolved` hash **and** toolchain identity

Also: you’ll get a huge UX win by letting the worker reuse prior job results via a result cache.

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Caching
 Caching MUST be correctness-preserving:
 - Cache keys derive from `job_key` (or documented sub-keys).
@@
 ### Locking + isolation (normative)
 - `per_job` DerivedData MUST be written under a directory derived from `job_key`.
 - `shared` caches MUST use a lock to prevent concurrent writers corrupting state.
@@
 - Worker MUST execute each job in an isolated working directory (unique per job_id).
+
+### Cache keying details (normative)
+- Any cache directory that can be reused across jobs MUST be additionally keyed by toolchain identity
+  (at minimum: Xcode build number and macOS major version) to prevent cross-toolchain corruption.
+- `metrics.json` SHOULD record the concrete cache key components used (job_key, xcode_build, macos_version, etc.).
+
+### Result cache (recommended)
+Worker SHOULD maintain an optional result cache keyed by `job_key`:
+- If present and complete, a submit MAY be satisfied by materializing artifacts from the cached result.
+- The worker MUST still emit a correct `attestation.json` for the new `job_id` referencing the same `job_key`.
```

---

## 8) PLAN+README: strengthen “security posture” language with concrete mitigations (secrets, keychain, env allowlist)

### Why this makes it better

Your current posture is good (forced-command, dedicated user, no publishing workflows), but it reads like “we’re safe because we deny archive/export.” In reality, the biggest risks are:

* secrets present on worker (keychain, env vars, tokens)
* project build scripts (expected, but should be acknowledged)
* SSH exposure (you’ve already started addressing)

Add explicit mitigations and optional knobs for hardened environments.

### Diff (README.md)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Notes
 - Designed as a build/test gate, not a full IDE replacement
 - Safe-by-default: avoids intercepting setup or mutating commands
 - Deterministic: runs produce a JobSpec (`job.json`) and stable `job_key` used for caching and attestation
 - Security posture: prefer a dedicated `rch` user; optionally use SSH forced-command; avoid signing/publishing workflows
 - Integrity: host verifies `manifest.json` digests; attestation binds worker identity + artifact set
+
+### Hardening recommendations
+- Keep the worker "CI-clean": no personal Apple ID sessions, no developer keychains, minimal credentials.
+- Prefer an env allowlist for the executor (only pass through known-safe vars), and redact secrets from logs.
+- Consider running the worker user with reduced permissions (no admin), and keep artifacts + caches on a dedicated volume.
```

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Threat model / security notes
 - Remote execution is limited to configured Xcode build/test actions.
 - Worker SHOULD run under a dedicated user with constrained permissions.
 - Prefer an implementation that does not require arbitrary interactive shell access.
 - Not in scope: code signing, notarization, exporting archives, publishing.
+
+Clarification (normative):
+- The lane does not attempt to sandbox Xcode builds. Repo-defined build phases/plugins may execute on the worker.
+  Operators MUST treat the worker as a CI machine and scope secrets accordingly.
+
+Recommended mitigations:
+- Executor SHOULD use an environment-variable allowlist and redact obvious secrets in logs/artifacts.
+- Worker SHOULD avoid unlocking or accessing user keychains during execution.
```

---

## 9) PLAN: make failure taxonomy implementable (add `failure_subkind`, add missing stable exit codes)

### Why this makes it better

You list failure kinds like `WORKER_INCOMPATIBLE`, `BUNDLER`, `ATTESTATION`—but don’t give stable exit codes for them. Also, timeouts are discussed but not represented cleanly.

Keep `failure_kind` stable and add `failure_subkind` for timeouts/details.

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Failure taxonomy
 `summary.json` MUST include:
 - `status`: success | failed | rejected
 - `failure_kind`: CLASSIFIER_REJECTED | SSH | TRANSFER | EXECUTOR | XCODEBUILD | MCP | ARTIFACTS | CANCELLED | WORKER_INCOMPATIBLE | BUNDLER | ATTESTATION | WORKER_BUSY
+- `failure_subkind`: optional string for details (e.g. TIMEOUT_OVERALL | TIMEOUT_IDLE | PROTOCOL_ERROR)
 - `exit_code`: stable integer for scripting
 - `human_summary`: short string for console output
@@
 ### Stable exit codes (normative)
 - 0: SUCCESS
 - 10: CLASSIFIER_REJECTED
 - 20: SSH/CONNECT
 - 30: TRANSFER
 - 40: EXECUTOR
 - 50: XCODEBUILD_FAILED
 - 60: MCP_FAILED
 - 70: ARTIFACTS_FAILED
 - 80: CANCELLED
 - 90: WORKER_BUSY
+- 91: WORKER_INCOMPATIBLE
+- 92: BUNDLER
+- 93: ATTESTATION
```

---

## 10) CLI/UX: add first-class “tail/logs/cancel/artifacts” commands and align README “useful commands” to the contract

### Why this makes it better

Your protocol already implies “tail/status/cancel/fetch,” but the CLI surface doesn’t expose it explicitly. For agents and operators, the difference between a usable lane and a frustrating one is:

* streaming logs/events during execution
* deterministic access to artifacts (`rch xcode artifacts <run_id>`)
* reliable cancellation

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## CLI surface (contract)
 The lane MUST provide these user-facing entry points:
 - `rch xcode verify` — run repo-defined `verify` actions (`build`, `test`).
 - `rch xcode run --action <build|test>` — run a single action as a one-step run.
 - `rch xcode explain -- <cmd...>` — classifier explanation and effective constraints.
 - `rch xcode verify --dry-run` — print resolved JobSpec + chosen worker, no execution.
+ - `rch xcode tail <run_id|job_id>` — stream logs/events with a cursor (falls back to polling if worker lacks tail).
+ - `rch xcode cancel <run_id|job_id>` — request best-effort cancellation and persist cancellation artifacts.
+ - `rch xcode artifacts <run_id|job_id>` — print the local artifact path(s) and key files (summary/xcresult/log).
 Optional but recommended:
 - `rch workers list --tag macos,xcode`
 - `rch workers probe <worker>` — capture `capabilities.json` snapshot
 - `rch xcode fetch <job_id>` — materialize remote artifacts locally if stored remotely
 - `rch xcode doctor` — validate worker reachability, protocol, Xcode pinning, and destination availability
```

### Diff (README.md)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 Useful commands:
 - `rch xcode explain -- <command...>`  (why it will/won't be intercepted)
 - `rch xcode verify --dry-run`         (prints resolved plan + selected worker)
+- `rch xcode tail <run_id|job_id>`     (stream logs/events while running)
+- `rch xcode cancel <run_id|job_id>`   (best-effort cancellation)
+- `rch xcode artifacts <run_id|job_id>`(print artifact locations + key files)
 - `rch workers list --tag macos,xcode` (show matching workers)
 - `rch workers probe <name>`           (fetch capabilities snapshot)
 - `rch xcode doctor`                   (validate config, SSH, Xcode, destination)
```

---

If you want one extra “high leverage” follow-on change after these: I’d add a **tiny “Schemas” appendix** (still in `PLAN.md` or a new `SCHEMAS.md`) that pins the exact fields for `job.json`, `run_summary.json`, and `capabilities.json`—because once agents depend on these, schema drift becomes the #1 source of breakage.
