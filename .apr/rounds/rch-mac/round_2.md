Below is a **patch series** of proposed improvements. Each patch includes: (1) what it fixes / adds, (2) why it improves robustness / UX / safety / performance, and (3) a **git-diff style** change. Apply in order to get a coherent updated spec set.

---

## Quick review (what’s already strong)

* **Good split**: README non-normative; PLAN normative.
* **Right core pillars**: deny-by-default classifier, deterministic JobSpec/job_key, schema’d artifacts.
* **Correct scope constraint**: explicitly out-of-scope signing/notarization/export.

Main gaps I’d address:

* The **host↔worker protocol** is underspecified (versioning, cancellation, idempotency, log streaming).
* “Determinism” needs concrete rules for **source bundling canonicalization** and **invocation sanitization**.
* **Worker selection** needs a deterministic algorithm (and how “pinned” interacts with capacity).
* **Exit codes**, **artifact schemas**, and **machine-readable summaries** want more structure.
* Caching needs **locks + eviction/GC + correctness proofs** spelled out.
* Security model can be tightened with **forced-command patterns** + **artifact signing** (optional but high value).

---

# Patch 1 — README: clarify scope, add docs map, improve quickstart + examples

### Why this helps

* README currently implies “uses XcodeBuildMCP” but PLAN supports two backends; clarify to avoid operator confusion.
* Adds a small “mental model” + troubleshooting pointers without turning README into the contract.

```diff
diff --git a/README.md b/README.md
index 1111111..2222222 100644
--- a/README.md
+++ b/README.md
@@ -1,23 +1,45 @@
 # RCH Xcode Lane
 
 > **Normative spec:** `PLAN.md` is the contract.  
 > This README is **non-normative**: quickstart, operator notes, and examples.
 
+## Docs map
+- `PLAN.md` — **normative** contract (classifier, JobSpec, protocol, artifacts, caching, security)
+- `.rch/xcode.toml` — repo-scoped configuration (checked in)
+- `~/.config/rch/workers.toml` — host-scoped worker inventory + credentials
+
 ## What it is
-An extension to Remote Compilation Helper (RCH) that offloads Xcode build/test to a remote Mac mini using XcodeBuildMCP.
+An extension to Remote Compilation Helper (RCH) that offloads Xcode build/test to a remote macOS worker (e.g. a Mac mini).
+Execution can use either:
+- **Backend `xcodebuild` (MVP)**, or
+- **Backend `mcp` (preferred)** via XcodeBuildMCP for richer diagnostics/orchestration.
 
 ## Why
 Agents running on Linux or busy Macs can still validate iOS/macOS projects under pinned Xcode conditions without local Xcode installs.
 
 ## Requirements
 **macOS worker**
 - Xcode installed
 - SSH access
 - rsync + zstd
-- Node.js + XcodeBuildMCP (recommended)
+- Node.js + XcodeBuildMCP (recommended for `backend="mcp"`)
+- (Recommended) dedicated `rch` user + constrained SSH key/forced-command
 
 **Host**
 - RCH client + daemon
 - SSH access to worker
 
 ## Setup
 1. Add Mac mini to `~/.config/rch/workers.toml` with tags `macos,xcode`
 2. Add `.rch/xcode.toml` to your repo
 3. Start daemon: `rch daemon start`
 4. Run: `rch xcode verify`
+
+## Mental model (operator view)
+- You run `rch xcode verify` locally (even on Linux).
+- RCH classifies/sanitizes the invocation, builds a deterministic `job.json`, bundles inputs, and ships to macOS.
+- Worker executes and returns schema-versioned artifacts (`summary.json`, logs, `xcresult`, etc.).
 
 ## Repo config (`.rch/xcode.toml`)
 Example:
@@ -55,6 +77,19 @@ Example:
 Useful commands:
 - `rch xcode explain -- <command...>`  (why it will/won't be intercepted)
 - `rch xcode verify --dry-run`         (prints resolved plan + selected worker)
+- `rch workers list --tag macos,xcode` (show matching workers)
+- `rch workers probe <name>`           (fetch capabilities snapshot)
+
+## Common pitfalls
+- **Wrong Xcode selected**: ensure worker `DEVELOPER_DIR` is stable/pinned.
+- **Simulator mismatch**: pinned destination must exist on the worker (see `capabilities.json`).
+- **Long first build**: warm SPM + DerivedData caches (see `cache.*` modes in config).
 
 ## Outputs
 Artifacts are written to:
 `~/.local/share/rch/artifacts/<job_id>/`
```

---

# Patch 2 — PLAN: add Terminology, Non-goals, and CLI surface

### Why this helps

* Makes the spec easier to implement consistently across host/worker code.
* CLI surface is part of the contract for agent workflows; define it explicitly.

```diff
diff --git a/PLAN.md b/PLAN.md
index 3333333..4444444 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -1,18 +1,63 @@
 # PLAN — RCH Xcode Lane
 
 ## Vision
 Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker (Mac mini) using XcodeBuildMCP, producing deterministic, machine-readable results.
 
 ## Goals
 - Remote Xcode build/test via macOS workers only
 - Deterministic configuration and attestation
 - Agent-friendly JSON outputs and artifacts
 - Safe-by-default interception (false negatives preferred)
+
+## Non-goals
+- Code signing, provisioning, notarization, export, TestFlight upload, or publishing
+- Arbitrary script execution (“setup steps”), mutable environment bootstrap, or interactive shells
+- Replacing Xcode IDE workflows (this is a gate/lane, not an IDE)
+
+## Terminology
+- **Host**: the machine running `rch` (may be Linux/macOS).
+- **Worker**: macOS machine that executes the job.
+- **Invocation**: user-provided command line (e.g. `xcodebuild test ...`).
+- **Classifier**: deny-by-default gate that accepts/rejects invocations.
+- **JobSpec** (`job.json`): deterministic, fully-resolved job description.
+- **Job key** (`job_key`): stable hash used for caching and attestation.
+- **Artifact set**: schema-versioned outputs written under `<job_id>/`.
+
+## CLI surface (contract)
+The lane MUST provide these user-facing entry points:
+- `rch xcode verify` — run repo-defined `verify` actions (`build`, `test`).
+- `rch xcode explain -- <cmd...>` — classifier explanation and effective constraints.
+- `rch xcode verify --dry-run` — print resolved JobSpec + chosen worker, no execution.
+Optional but recommended:
+- `rch workers list --tag macos,xcode`
+- `rch workers probe <worker>` — capture `capabilities.json` snapshot
+- `rch xcode fetch <job_id>` — materialize remote artifacts locally if stored remotely
```

---

# Patch 3 — PLAN: specify host↔worker protocol + job lifecycle (idempotency, cancellation, log streaming)

### Why this helps

* Prevents “SSH + rsync spaghetti” from becoming a de facto protocol.
* Makes retries safe, enables cancellation, and allows streaming logs (huge for agent UX).

```diff
diff --git a/PLAN.md b/PLAN.md
index 4444444..5555555 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -18,18 +18,74 @@ Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker (Mac mini) using XcodeBuildMCP, producing deterministic, machine-readable results.
 ## Architecture (high level)
 Pipeline stages:
 1. **Classifier**: detects safe, supported Xcode build/test invocations (deny-by-default).
 2. **JobSpec builder**: produces a fully specified, deterministic job description (no ambient defaults).
 3. **Transport**: bundles inputs + sends to worker (integrity checked).
 4. **Executor**: runs the job on macOS via a selected backend (**xcodebuild** or **XcodeBuildMCP**).
 5. **Artifacts**: writes a schema-versioned artifact set + attestation.
+
+## Host↔Worker protocol (normative)
+The system MUST behave as if there is a versioned protocol even if implemented over SSH.
+
+### Versioning
+- Host and worker MUST each report `rch_xcode_lane_version` and `protocol_version`.
+- If `protocol_version` is incompatible, the host MUST fail with `failure_kind=WORKER_INCOMPATIBLE`.
+
+### Job lifecycle + idempotency
+Worker MUST treat `(job_id, job_key)` as an idempotency key:
+- If a job with the same `(job_id, job_key)` is already COMPLETE, worker MAY return the existing artifacts.
+- If a job with the same `(job_id, job_key)` is RUNNING, worker MUST report status and allow log tailing.
+- If `(job_id, job_key)` mismatches (same job_id, different key), worker MUST reject to prevent artifact confusion.
+
+### Cancellation
+- Host MUST be able to request cancellation.
+- Worker MUST attempt a best-effort cancel (terminate backend process tree) and write artifacts with `status=failed`
+  and `failure_kind=CANCELLED`.
+
+### Log streaming (recommended)
+- Worker SHOULD support a “tail” mode so host can stream logs while running.
+- If not supported, host MUST still periodically fetch/append logs to avoid silent hangs.
```

---

# Patch 4 — PLAN: make “deterministic bundling” concrete (canonicalization + source manifest)

### Why this helps

Deterministic job keys are only as good as the rules for what goes into `source_sha256`. This patch prevents “same repo, different tarball order / mtimes / ignored files” from causing cache misses and irreproducible attestation.

```diff
diff --git a/PLAN.md b/PLAN.md
index 5555555..6666666 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -34,16 +34,55 @@ Pipeline stages:
 ## Deterministic JobSpec + Job Key
 Each remote run is driven by a `job.json` (JobSpec) generated on the host.
 The host computes:
 - `source_sha256` — SHA-256 of the sent source bundle (after canonicalization)
 - `job_key` — SHA-256 over: `source_sha256 + effective_config + sanitized_invocation + toolchain`
 Artifacts include both values, enabling reproducible reruns and cache keys.
+
+## Source bundling canonicalization (normative)
+The host MUST create a canonical source bundle such that identical inputs yield identical `source_sha256`.
+
+Rules:
+- Use a deterministic archive format (e.g. `tar`) with:
+  - sorted file paths (lexicographic, UTF-8)
+  - normalized mtimes (e.g. 0) and uid/gid (0)
+  - stable file modes (preserve executable bit; normalize others)
+- Exclude by default:
+  - `.git/`, `.DS_Store`, `DerivedData/`, `.build/`, `**/*.xcresult/`, `**/.swiftpm/` (build artifacts)
+  - any host-local RCH artifact directories
+- Include repo config `.rch/xcode.toml` in the bundle (so worker always has the same constraints)
+- The host MUST emit `source_manifest.json` listing:
+  - `path`, `size`, `sha256` per file
+  - manifest `schema_version`
+
+If the bundler cannot apply canonicalization, the job MUST be rejected (`failure_kind=BUNDLER`).
```

---

# Patch 5 — PLAN: classifier hardening (allow/deny matrix + sanitized invocation spec)

### Why this helps

* “Reject unknown flags” is good but insufficiently actionable for implementers.
* A concrete allow/deny matrix prevents security regressions and makes `explain` trustworthy.

```diff
diff --git a/PLAN.md b/PLAN.md
index 6666666..7777777 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -41,17 +41,71 @@ Artifacts include both values, enabling reproducible reruns and cache keys.
 ## Classifier (safety gate)
 The classifier MUST:
 - match only supported forms of `xcodebuild` invocations
 - reject unknown flags / actions by default
 - enforce repo config constraints (workspace/project, scheme, destination)
 - emit a machine-readable explanation when rejecting (`summary.json` includes `rejection_reason`)
+
+### Supported actions (initial contract)
+Allowed (when fully constrained by repo config):
+- `build`
+- `test`
+Explicitly denied:
+- `archive`, `-exportArchive`, `-exportNotarizedApp`, notarization/signing flows
+- `-resultBundlePath` to arbitrary locations (worker controls output paths)
+- `-derivedDataPath` to arbitrary locations (worker controls paths per cache mode)
+
+### Supported flags (initial contract)
+The classifier MAY allow a minimal safe subset (example):
+- `-workspace` OR `-project` (must match repo config)
+- `-scheme` (must match repo config)
+- `-destination` (must match resolved/pinned destination)
+- `-configuration` (optional; if allowed must be pinned or whitelisted)
+All other flags MUST be rejected unless explicitly added to the allowlist.
+
+### Sanitized invocation (normative)
+If accepted, the host MUST emit `invocation.json` containing:
+- `original_argv` (as received)
+- `sanitized_argv` (canonical ordering, normalized quoting)
+- `accepted_action` (`build`|`test`)
+- `rejected_flags` (if any; for dry-run/explain)
+`sanitized_argv` MUST NOT contain:
+- output path overrides
+- script hooks
+- unconstrained destinations
```

---

# Patch 6 — PLAN: deterministic worker selection + capabilities snapshot rules

### Why this helps

Worker selection becomes a frequent source of “it worked yesterday” flakiness. This makes selection reproducible and debuggable, and ties selection into attestation.

```diff
diff --git a/PLAN.md b/PLAN.md
index 7777777..8888888 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -63,20 +63,66 @@ On failure, artifacts MUST still include logs + diagnostics if available.
 ## Worker capabilities
 Worker reports a `capabilities.json` including:
 - Xcode version(s) + build number, `DEVELOPER_DIR`
 - available runtimes/devices (simctl)
 - installed tooling versions (rch-worker, XcodeBuildMCP)
 - capacity (max concurrent jobs, disk free)
 Host stores the chosen worker capability snapshot in artifacts.
+
+## Worker selection (normative)
+Given a set of eligible workers (tag match + reachable), host MUST choose deterministically
+unless user explicitly requests randomness.
+
+Selection inputs:
+- required tags: `macos,xcode` (and any repo-required tags)
+- constraints: Xcode version/build, platform (iOS/macOS), destination availability
+- preference: lowest load / highest free disk MAY be used only as a tie-breaker
+
+Selection algorithm (default):
+1. Filter by required tags.
+2. Probe or load cached `capabilities.json` snapshots (bounded staleness).
+3. Filter by constraints (destination exists, required Xcode available).
+4. Sort deterministically by:
+   - explicit worker priority (host config)
+   - then stable worker name
+5. Choose first.
+
+The host MUST write:
+- `worker_selection.json` (inputs, filtered set, chosen worker, reasons)
+- `capabilities.json` snapshot as used for the decision
```

---

# Patch 7 — PLAN+README: artifacts + exit codes + richer agent-friendly summaries

### Why this helps

* Agents want structured summaries (test counts, failures, build errors) without parsing logs/xcresult.
* Stable exit codes make it automatable in CI and agent pipelines.

```diff
diff --git a/PLAN.md b/PLAN.md
index 8888888..9999999 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -48,20 +48,55 @@ Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker (Mac mini) using XcodeBuildMCP, producing deterministic, machine-readable results.
 ## Failure taxonomy
 `summary.json` MUST include:
 - `status`: success | failed | rejected
 - `failure_kind`: CLASSIFIER_REJECTED | SSH | TRANSFER | EXECUTOR | XCODEBUILD | MCP | ARTIFACTS
 - `exit_code`: stable integer for scripting
 - `human_summary`: short string for console output
+
+### Stable exit codes (normative)
+Define a small stable range (example):
+- 0: SUCCESS
+- 10: CLASSIFIER_REJECTED
+- 20: SSH/CONNECT
+- 30: TRANSFER
+- 40: EXECUTOR
+- 50: XCODEBUILD_FAILED
+- 60: MCP_FAILED
+- 70: ARTIFACTS_FAILED
+- 80: CANCELLED
+
+## Agent-friendly summaries (recommended)
+In addition to `summary.json`, the worker SHOULD emit:
+- `test_summary.json` (counts, failing tests, duration, top failures)
+- `build_summary.json` (targets, warnings/errors counts, first error location if available)
+These MUST be derived from authoritative sources (`xcresult` when present; logs as fallback).
@@ -96,15 +131,19 @@ ## Artifacts
 - summary.json
 - attestation.json
 - manifest.json
 - effective_config.json
 - job.json
 - invocation.json
 - toolchain.json
 - metrics.json
+- source_manifest.json
+- worker_selection.json
+- test_summary.json (recommended)
+- build_summary.json (recommended)
 - build.log
 - result.xcresult/
diff --git a/README.md b/README.md
index 2222222..3333333 100644
--- a/README.md
+++ b/README.md
@@ -77,6 +77,10 @@ Includes:
 - invocation.json
 - toolchain.json
 - metrics.json
+- source_manifest.json
+- worker_selection.json
+- test_summary.json (recommended)
+- build_summary.json (recommended)
 - build.log
 - result.xcresult/
```

---

# Patch 8 — PLAN: caching correctness rules, locking, eviction/GC, concurrency limits

### Why this helps

Caching is where reliability goes to die unless you specify:

* what’s keyed by what,
* how locks work,
* how to avoid cross-job corruption,
* and how to garbage collect safely.

```diff
diff --git a/PLAN.md b/PLAN.md
index 9999999..aaaaaaa 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -56,18 +56,61 @@ On failure, artifacts MUST still include logs + diagnostics if available.
 ## Caching
 Caching MUST be correctness-preserving:
 - Cache keys derive from `job_key` (or documented sub-keys).
 - DerivedData modes: `off` | `per_job` | `shared` (shared requires safe keying + locking).
 - SPM cache mode: `off` | `shared` (shared keyed by resolved Package.resolved + toolchain).
 `metrics.json` includes cache hit/miss + sizes + timing.
+
+### Locking + isolation (normative)
+- `per_job` DerivedData MUST be written under a directory derived from `job_key`.
+- `shared` caches MUST use a lock to prevent concurrent writers corrupting state.
+  - Lock MUST have a timeout and emit diagnostics if contention occurs.
+- Worker MUST execute each job in an isolated working directory (unique per job_id).
+
+### Eviction / garbage collection (normative)
+Worker MUST implement at least one:
+- size-based eviction (e.g. keep under N GB)
+- age-based eviction (e.g. delete items unused for N days)
+Eviction MUST NOT delete caches that are currently locked/in use.
+
+### Concurrency + capacity (normative)
+- Worker MUST enforce `max_concurrent_jobs`.
+- If capacity exceeded, worker MUST respond with a structured “busy” state so host can retry/backoff.
```

---

# Patch 9 — PLAN: security tightening + optional artifact signing (practical, high-value)

### Why this helps

You already have “deny-by-default invocation,” but the biggest real-world risk tends to be:

* drift into running more than you intended (shell access / arbitrary commands),
* and lack of confidence that artifacts came from the worker/toolchain you think.

This patch adds practical controls without forcing heavyweight infrastructure.

```diff
diff --git a/PLAN.md b/PLAN.md
index aaaaaaa..bbbbbbb 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -68,17 +68,64 @@ Worker reports a `capabilities.json` including:
 ## Threat model / security notes
 - Remote execution is limited to configured Xcode build/test actions.
 - Worker SHOULD run under a dedicated user with constrained permissions.
 - Prefer an implementation that does not require arbitrary interactive shell access.
 - Not in scope: code signing, notarization, exporting archives, publishing.
+
+## SSH hardening (recommended)
+- Use a dedicated `rch` user on the worker.
+- Prefer SSH keys restricted with:
+  - forced-command that only runs the worker entrypoint (no shell)
+  - disable agent forwarding, no-pty, restrictive source addresses where possible
+- Host SHOULD pin worker host keys (no TOFU surprises).
+
+## Artifact attestation (normative + optional signing)
+Artifacts MUST include `attestation.json` with:
+- `job_id`, `job_key`, `source_sha256`
+- worker identity (name, stable fingerprint) + `capabilities.json` digest
+- backend identity (xcodebuild/XcodeBuildMCP version)
+
+Optional but recommended:
+- Worker signs `attestation.json` with a worker-held key (e.g. Ed25519).
+- Host verifies signature and records `attestation_verification.json`.
+If signature verification fails, host MUST mark the run as failed (`failure_kind=ATTESTATION`).
```

---

## A few “next-level” features I’d consider (not patched above, but worth tracking)

* **`build-for-testing` / `test-without-building`** support as an opt-in action set (can massively speed CI loops).
* **Result extraction pipeline**: a small, versioned “xcresult → JSON” transformer so agents never parse Apple’s formats ad hoc.
* **Remote artifact store** option (S3/minio/etc.) keyed by `job_key` with local materialization.
* **Warm pool**: keep simulators booted / preheated for consistent latency (careful: determinism vs convenience).
* **Hermetic toolchain pinning**: explicit `DEVELOPER_DIR` selection + verification and fail-fast on drift.

If you want, I can also produce a **single consolidated diff** that rolls all nine patches into one “final state” (easier to apply), but the patch series above is designed to be readable and reviewable commit-by-commit.
