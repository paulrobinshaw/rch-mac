Below are the best revisions I’d make to **improve robustness, determinism, performance, and operator/agent UX**—while staying aligned with your “gate/lane, not IDE” and “deny-by-default” posture.

I’m treating the `README.md` + `PLAN.md` you pasted as the current baseline and proposing changes against that.

---

## Change 1 — Make `verify` a first-class *run* (multi-step pipeline), not “just a job”

### Why this makes it better

Right now the mental model oscillates between “verify runs build+test” and “a run is driven by a JobSpec (`job.json`)”. That becomes awkward when `verify = ["build","test"]` because:

* agents want **one stable handle** for “the whole verification”
* operators want **one folder** + an aggregated summary
* performance wants **one worker selection** and **one working directory** (to reuse DerivedData between steps safely)
* caching/attestation becomes clearer when you separate *run identity* vs *step job identity*

### Patch (README + PLAN)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Mental model (operator view)
 - You run `rch xcode verify` locally (even on Linux).
 - RCH classifies/sanitizes the invocation, builds a deterministic `job.json`, bundles inputs, and ships to macOS.
 - Worker executes and returns schema-versioned artifacts (`summary.json`, logs, `xcresult`, etc.).
+-
+- `rch xcode verify` is a **run** that may contain multiple **step jobs** (e.g. `build` then `test`).
+- The run produces a **run summary** that links to each step job’s artifact set.

@@
 ## Outputs
 Artifacts are written to:
-`~/.local/share/rch/artifacts/<job_id>/`
+`~/.local/share/rch/artifacts/xcode/<run_id>/`
+
+Layout (example):
+- `run_summary.json`
+- `worker_selection.json`
+- `capabilities.json`
+- `steps/build/<job_id>/...`
+- `steps/test/<job_id>/...`

 Includes:
 - summary.json
 - attestation.json
 - manifest.json
 - effective_config.json
 - job.json
 - invocation.json
 - toolchain.json
 - metrics.json
 - source_manifest.json
 - worker_selection.json
+- run_summary.json
 - test_summary.json (recommended)
 - build_summary.json (recommended)
 - build.log
 - result.xcresult/
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Terminology
 - **Host**: the machine running `rch` (may be Linux/macOS).
 - **Worker**: macOS machine that executes the job.
+- **Run**: a top-level verification attempt (e.g. `rch xcode verify`) that may include multiple jobs.
+- **Run ID** (`run_id`): stable identifier for the run artifact directory.
 - **Invocation**: user-provided command line (e.g. `xcodebuild test ...`).
 - **Classifier**: deny-by-default gate that accepts/rejects invocations.
 - **JobSpec** (`job.json`): deterministic, fully-resolved job description.
 - **Job key** (`job_key`): stable hash used for caching and attestation.
@@
 ## CLI surface (contract)
 The lane MUST provide these user-facing entry points:
 - `rch xcode verify` — run repo-defined `verify` actions (`build`, `test`).
+- `rch xcode run --action <build|test>` — run a single action as a one-step run (recommended).
 - `rch xcode explain -- <cmd...>` — classifier explanation and effective constraints.
 - `rch xcode verify --dry-run` — print resolved JobSpec + chosen worker, no execution.
@@
 ## Architecture (high level)
 Pipeline stages:
 1. **Classifier**: detects safe, supported Xcode build/test invocations (deny-by-default).
-2. **JobSpec builder**: produces a fully specified, deterministic job description (no ambient defaults).
+2. **Run builder**: resolves repo `verify` actions into an ordered run plan and chooses a worker once.
+3. **JobSpec builder**: produces a fully specified, deterministic step job description (no ambient defaults).
 3. **Transport**: bundles inputs + sends to worker (integrity checked).
 4. **Executor**: runs the job on macOS via a selected backend (**xcodebuild** or **XcodeBuildMCP**).
 5. **Artifacts**: writes a schema-versioned artifact set + attestation.
@@
 ## Artifacts
+- run_summary.json
 - summary.json
 - attestation.json
 - manifest.json
 - effective_config.json
 - job.json
 - invocation.json
 - toolchain.json
 - metrics.json
 - source_manifest.json
 - worker_selection.json
```

---

## Change 2 — Add a content-addressed **source bundle store** (skip transfers when possible)

### Why this makes it better

Your canonical bundling rules are excellent, but you’ll still pay the “ship repo over SSH” cost repeatedly. Since you already define `source_sha256`, you can make transfers far cheaper:

* first run: upload bundle once
* subsequent runs with same source bundle: **zero upload**, just submit `job.json`
* enables future optimizations like “bundle de-dupe across jobs” and “remote GC by LRU”

This is a **big real-world speed win** for agents iterating rapidly.

### Patch (PLAN)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Deterministic JobSpec + Job Key
 Each remote run is driven by a `job.json` (JobSpec) generated on the host.
 The host computes:
 - `source_sha256` — SHA-256 of the sent source bundle (after canonicalization)
 - `job_key` — SHA-256 over: `source_sha256 + effective_config + sanitized_invocation + toolchain`
 Artifacts include both values, enabling reproducible reruns and cache keys.
+
+## Source bundle store (performance, correctness-preserving)
+Workers SHOULD maintain a content-addressed store keyed by `source_sha256`.
+
+Protocol expectations:
+- Host MAY query whether the worker already has `source_sha256`.
+- If present, host SHOULD skip re-uploading the bundle and submit only `job.json`.
+- If absent, host uploads the canonical bundle once; worker stores it under `source_sha256`.
+
+GC expectations:
+- Bundle GC MUST NOT remove bundles referenced by RUNNING jobs.
+- Bundle GC policy MAY align with cache eviction policy (age/size based).
```

---

## Change 3 — Make bundling policy explicit: `.rchignore`, modes, and symlink safety

### Why this makes it better

Determinism isn’t just archive ordering—it’s also defining *what counts as inputs*.
Today you specify excludes, but you don’t define:

* whether to include **untracked files**
* how to treat **symlinks** (danger: path escape)
* how repos can add **extra excludes** without changing tooling

Adding a tiny, explicit bundling policy makes agent behavior consistent and reduces “why did this build differ?” incidents.

### Patch (README + PLAN)

````diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Repo config (`.rch/xcode.toml`)
 Example:
 ```toml
 schema_version = 1
 backend = "xcodebuild" # or "mcp"
@@
 [timeouts]
 overall_seconds = 1800
 idle_log_seconds = 300
+
+[bundle]
+mode = "worktree"       # worktree | git_index
+ignore_file = ".rchignore"
+max_bytes = 0           # 0 = unlimited (host may still enforce sane caps)

 [cache]
 derived_data = "shared"   # off | per_job | shared
 spm = "shared"            # off | shared
````

diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@

## Source bundling canonicalization (normative)

The host MUST create a canonical source bundle such that identical inputs yield identical `source_sha256`.
@@

* Include repo config `.rch/xcode.toml` in the bundle (so worker always has the same constraints)
  +- The host MUST support a repo ignore file (recommended: `.rchignore`) for additional excludes.
  +- Symlink handling MUST be safe:

- - symlinks that escape the repo root MUST be rejected (`failure_kind=BUNDLER`)
- * host MUST choose either “preserve symlink” or “dereference within root” deterministically per config
-

+### Bundle modes (recommended)
+- `worktree`: include tracked + untracked files (except excluded patterns).
+- `git_index`: include only git-index tracked files (plus `.rch/xcode.toml` and ignore file).
+
If the bundler cannot apply canonicalization, the job MUST be rejected (`failure_kind=BUNDLER`).

````

---

## Change 4 — Make the “protocol over SSH” concrete: a tiny worker RPC surface
### Why this makes it better
You already say “MUST behave as if there is a versioned protocol even if implemented over SSH”—good. But implementers (and operators) need a **small, explicit command surface** to support:
- forced-command SSH keys (no shell)
- probing, submit, status, tail, cancel, fetch
- consistent machine-readable responses

This also sharply reduces security risk because your SSH account never needs a general-purpose shell.

### Patch (PLAN + README operator note)
```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Requirements
 **macOS worker**
@@
 - (Recommended) dedicated `rch` user + constrained SSH key/forced-command
+  - Recommended: forced-command runs a single `rch-worker xcode ...` entrypoint (no shell)

diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Host↔Worker protocol (normative)
 The system MUST behave as if there is a versioned protocol even if implemented over SSH.
+
+### Worker RPC surface (recommended, maps cleanly to SSH forced-command)
+Worker SHOULD implement these operations with JSON request/response payloads:
+- `probe` → returns `capabilities.json`
+- `submit` → accepts `job.json` (+ optional bundle upload reference), returns ACK and initial status
+- `status` → returns current job status and pointers to logs/artifacts
+- `tail` → streams logs/events with a cursor
+- `cancel` → requests best-effort cancellation
+- `fetch` → returns artifacts (or a signed manifest + download hints)
````

---

## Change 5 — Add a structured **event stream** (`events.jsonl`) for progress + watchdog correctness

### Why this makes it better

Console logs alone are unreliable for “is it stuck?” detection—especially with MCP where output can be buffered.
A structured event stream gives you:

* clean “stage transitions” (`TRANSFER`, `EXECUTOR`, `TESTING`, `FINALIZING`)
* robust idle watchdog keyed off **events**, not just stdout
* better agent consumption (no log scraping)

### Patch (README + PLAN)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 Includes:
@@
 - metrics.json
 - source_manifest.json
 - worker_selection.json
+- events.jsonl (recommended)
 - test_summary.json (recommended)
 - build_summary.json (recommended)

diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Log streaming (recommended)
 - Worker SHOULD support a "tail" mode so host can stream logs while running.
 - If not supported, host MUST still periodically fetch/append logs to avoid silent hangs.
+
+## Structured events (recommended)
+Worker SHOULD emit `events.jsonl` (JSON Lines) for machine-readable progress.
+Each event SHOULD include: `ts`, `stage`, `kind`, and optional `data`.
+Idle-log watchdog SHOULD treat either new log bytes OR new events as “activity”.
```

---

## Change 6 — Expand worker capabilities + pinning: OS/Xcode/runtimes + signing key identity

### Why this makes it better

Worker selection is only as good as the capability snapshot. Tightening the contract helps:

* deterministic selection (“destination exists”, “Xcode build number matches”)
* meaningful attestation identity (“this exact worker + toolchain produced these results”)
* fewer operator surprises (“Xcode updated silently”)

### Patch (PLAN + README pitfalls)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Common pitfalls
 - **Wrong Xcode selected**: ensure worker `DEVELOPER_DIR` is stable/pinned.
+- **Silent Xcode update**: prefer pinning by Xcode build number in worker capabilities + selection constraints.
 - **Simulator mismatch**: pinned destination must exist on the worker (see `capabilities.json`).
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Worker capabilities
 Worker reports a `capabilities.json` including:
 - Xcode version(s) + build number, `DEVELOPER_DIR`
+- macOS version + architecture
 - available runtimes/devices (simctl)
 - installed tooling versions (rch-worker, XcodeBuildMCP)
 - capacity (max concurrent jobs, disk free)
+Optional but recommended:
+- worker identity material (SSH host key fingerprint and/or attestation public key fingerprint)
 Host stores the chosen worker capability snapshot in artifacts.
```

---

## Change 7 — Strengthen artifact integrity: manifest digests + attestation covers the artifact set

### Why this makes it better

You already have `manifest.json` and `attestation.json`, but the spec doesn’t explicitly require:

* per-artifact SHA-256 in the manifest
* a single root digest for the artifact set
* host-side verification steps (especially important if artifacts move through storage)

This is low complexity and **high trust**.

### Patch (PLAN + README note)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Notes
@@
 - Deterministic: runs produce a JobSpec (`job.json`) and stable `job_key` used for caching and attestation
-- Security posture: prefer a dedicated `rch` user; optionally use SSH forced-command; avoid signing/publishing workflows
+- Security posture: prefer a dedicated `rch` user; optionally use SSH forced-command; avoid signing/publishing workflows
+- Integrity: host verifies `manifest.json` digests; attestation binds worker identity + artifact set

diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Artifact attestation (normative + optional signing)
 Artifacts MUST include `attestation.json` with:
 - `job_id`, `job_key`, `source_sha256`
 - worker identity (name, stable fingerprint) + `capabilities.json` digest
 - backend identity (xcodebuild/XcodeBuildMCP version)
+ - `manifest_sha256` (digest of `manifest.json`)
+
+## Artifact manifest (normative)
+`manifest.json` MUST enumerate produced artifacts with at least:
+- `path` (relative), `size`, `sha256`
+`manifest.json` SHOULD also include `artifact_root_sha256` (digest over ordered entries) to bind the set.
```

---

## Change 8 — Add explicit “busy/backpressure” semantics + a stable exit code

### Why this makes it better

You already require `max_concurrent_jobs` and a structured “busy” state, but it isn’t wired into:

* failure taxonomy
* stable exit codes
* host behavior (“retry-after”, “try another worker”)

This matters a lot under multi-agent load.

### Patch (PLAN)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Failure taxonomy
 `summary.json` MUST include:
 - `status`: success | failed | rejected
-- `failure_kind`: CLASSIFIER_REJECTED | SSH | TRANSFER | EXECUTOR | XCODEBUILD | MCP | ARTIFACTS | CANCELLED | WORKER_INCOMPATIBLE | BUNDLER | ATTESTATION
+- `failure_kind`: CLASSIFIER_REJECTED | SSH | TRANSFER | EXECUTOR | XCODEBUILD | MCP | ARTIFACTS | CANCELLED | WORKER_INCOMPATIBLE | BUNDLER | ATTESTATION | WORKER_BUSY
 - `exit_code`: stable integer for scripting
@@
 ### Stable exit codes (normative)
@@
 - 80: CANCELLED
+- 90: WORKER_BUSY
@@
 ### Concurrency + capacity (normative)
 - Worker MUST enforce `max_concurrent_jobs`.
-- If capacity exceeded, worker MUST respond with a structured "busy" state so host can retry/backoff.
+- If capacity exceeded, worker MUST respond with a structured "busy" state so host can retry/backoff.
+  - Response SHOULD include `retry_after_seconds`.
```

---

## Change 9 — Improve shared cache correctness by adding a `cache_namespace` and more explicit keying

### Why this makes it better

Your cache section is strong, but “shared DerivedData” can become unsafe/corrupting if multiple repos or branches collide.
A small addition—**explicit namespace**—gives operators control and keeps determinism:

* avoids cross-repo collisions
* allows stable cache directories even when source changes
* makes eviction/accounting saner

### Patch (README + PLAN)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 [cache]
+namespace = "myapp"      # recommended: stable per-repo namespace to avoid collisions
 derived_data = "shared"   # off | per_job | shared
 spm = "shared"            # off | shared
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Caching
 Caching MUST be correctness-preserving:
 - Cache keys derive from `job_key` (or documented sub-keys).
 - DerivedData modes: `off` | `per_job` | `shared` (shared requires safe keying + locking).
@@
 ### Locking + isolation (normative)
 - `per_job` DerivedData MUST be written under a directory derived from `job_key`.
 - `shared` caches MUST use a lock to prevent concurrent writers corrupting state.
@@
+### Cache namespace (recommended)
+Repo config SHOULD provide a stable `cache_namespace` used as part of shared cache directory names,
+to prevent collisions across unrelated repos on the same worker.
```

---

## Change 10 — Beef up operator UX: show a concrete `workers.toml` example + “doctor” checks

### Why this makes it better

Your README tells users where `workers.toml` lives, but not what it looks like. Also, in real usage most failures are:

* “wrong host key”
* “wrong DEVELOPER_DIR”
* “destination doesn’t exist”
* “worker missing XcodeBuildMCP”
  A `doctor` command (even if optional) and a config example cut onboarding friction dramatically.

### Patch (README + PLAN optional CLI)

````diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Docs map
 - `PLAN.md` — **normative** contract (classifier, JobSpec, protocol, artifacts, caching, security)
 - `.rch/xcode.toml` — repo-scoped configuration (checked in)
 - `~/.config/rch/workers.toml` — host-scoped worker inventory + credentials
+
+## Worker inventory example (`~/.config/rch/workers.toml`)
+```toml
+schema_version = 1
+
+[[worker]]
+name = "macmini-01"
+host = "macmini.local"
+user = "rch"
+port = 22
+tags = ["macos","xcode"]
+known_host_fingerprint = "SHA256:..."
+ssh_key_path = "~/.ssh/rch_macmini"
+priority = 10
+```
@@
 Useful commands:
@@
 - `rch workers probe <name>`           (fetch capabilities snapshot)
+- `rch xcode doctor`                  (recommended: validate config, SSH, Xcode, destination)

diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 Optional but recommended:
 - `rch workers list --tag macos,xcode`
 - `rch workers probe <worker>` — capture `capabilities.json` snapshot
 - `rch xcode fetch <job_id>` — materialize remote artifacts locally if stored remotely
+- `rch xcode doctor` — validate worker reachability, protocol, Xcode pinning, and destination availability
````

---

# A couple “meta” observations (no patch needed, but worth adopting)

* **MVP backend choice:** if `verify` includes `test`, consider documenting that `test` implies a build; many teams will configure `verify=["test"]` and rely on test’s build step. Keeping both is fine—just call out the tradeoff in README (“build-first gives faster compile-fail feedback; test-only reduces duplicate work”).
* **Classifier scope:** your classifier spec is good; you might also explicitly state that `rch xcode verify` is the preferred mode (config-driven), while `explain -- <cmd>` is for diagnosing whether a *user-supplied* invocation would be accepted.

If you want, I can also propose **concrete JSON schemas** (minimal, versioned) for `run_summary.json`, `summary.json`, `capabilities.json`, and `events.jsonl` in the same style as your other “schema_version + created_at + ids” contract—those will make implementation and agent consumption significantly easier.
