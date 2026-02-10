Below are the strongest revisions I’d make to turn this from a “nice idea” into a **trustworthy, deterministic remote build/test gate** that agents and CI can lean on. I’m aiming for: **clear contract, safer interception, reproducible runs, better artifacts, pluggable backend, strong worker capability model, and real performance wins (caching + reuse).**

---

## 1) Make the contract explicit: README is non-normative; PLAN is the source of truth

### Why this improves things

Right now the README reads like *the* spec (even though you have a `PLAN.md`). Making the contract explicit prevents drift and gives contributors a single normative target. It also lets README become “operator docs” (setup, troubleshooting, examples) without risking spec divergence.

### Diff (README.md)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 # RCH Xcode Lane
 
+> **Normative spec:** `PLAN.md` is the contract.  
+> This README is **non-normative**: quickstart, operator notes, and examples.
+
 ## What it is
 An extension to Remote Compilation Helper (RCH) that offloads Xcode build/test to a remote Mac mini using XcodeBuildMCP.
 
 ## Why
 Agents running on Linux or busy Macs can still validate iOS/macOS projects under pinned Xcode conditions without local Xcode installs.
```

---

## 2) Define an explicit architecture: Classifier → JobSpec → Transport → Executor (backend) → Artifacts

### Why this improves things

You’ll avoid “one big command runner” entropy. This separation also unlocks:

* multiple execution backends (`xcodebuild` MVP vs XcodeBuildMCP later)
* deterministic JobSpec hashing
* clearer safety boundaries (classifier is the gatekeeper)
* easier testing (each stage is unit-testable)

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Vision
 Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker (Mac mini) using XcodeBuildMCP, producing deterministic, machine-readable results.
 
+## Architecture (high level)
+Pipeline stages:
+1. **Classifier**: detects safe, supported Xcode build/test invocations (deny-by-default).
+2. **JobSpec builder**: produces a fully specified, deterministic job description (no ambient defaults).
+3. **Transport**: bundles inputs + sends to worker (integrity checked).
+4. **Executor**: runs the job on macOS via a selected backend (**xcodebuild** or **XcodeBuildMCP**).
+5. **Artifacts**: writes a schema-versioned artifact set + attestation.
+
+## Backends
+- **Backend: xcodebuild (MVP)** — minimal dependencies, fastest path to correctness.
+- **Backend: XcodeBuildMCP (preferred)** — richer structure, better diagnostics, multi-step orchestration.
```

---

## 3) Introduce a deterministic JobSpec + stable Job ID (content-addressed runs)

### Why this improves things

A “job_id” that’s just a random UUID is fine for storage, but **a stable, content-addressed ID** is gold for:

* caching (DerivedData/SPM) keyed by job inputs
* deduplication and re-runs
* forensic reproducibility (“run this exact job again”)

The key idea: compute a `job_key` hash from **(repo tree / source bundle hash + config + toolchain versions + sanitized invocation + selected destination)**, then derive a job_id from that.

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Goals
 - Remote Xcode build/test via macOS workers only
 - Deterministic configuration and attestation
 - Agent-friendly JSON outputs and artifacts
 - Safe-by-default interception (false negatives preferred)
+
+## Deterministic JobSpec + Job Key
+Each remote run is driven by a `job.json` (JobSpec) generated on the host.
+The host computes:
+- `source_sha256` — SHA-256 of the sent source bundle (after canonicalization)
+- `job_key` — SHA-256 over: `source_sha256 + effective_config + sanitized_invocation + toolchain`
+Artifacts include both values, enabling reproducible reruns and cache keys.
```

### Diff (README.md) – mention the model

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Notes
 - Designed as a build/test gate, not a full IDE replacement
 - Safe-by-default: avoids intercepting setup or mutating commands
+ - Deterministic: runs produce a JobSpec (`job.json`) and stable `job_key` used for caching and attestation
```

---

## 4) Expand `.rch/xcode.toml` into a real config contract (and document it)

### Why this improves things

Right now config is implied but not defined. Agents/CI need:

* explicit **actions** (`build`, `test`) and **schemes**
* explicit **destination policy** (pinned / resolved / allowed list)
* explicit **timeouts**
* explicit **backend selection**
* explicit **cache policy**
* explicit **allow/deny** lists for build settings overrides

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
+## Repo config (`.rch/xcode.toml`)
+Example:
+```toml
+schema_version = 1
+backend = "xcodebuild" # or "mcp"
+
+[project]
+workspace = "MyApp.xcworkspace" # or project = "MyApp.xcodeproj"
+scheme = "MyApp"
+
+[actions]
+verify = ["build", "test"]
+
+[destination]
+mode = "pinned"
+value = "platform=iOS Simulator,name=iPhone 16,OS=latest"
+
+[timeouts]
+overall_seconds = 1800
+idle_log_seconds = 300
+
+[cache]
+derived_data = "shared"   # off | per_job | shared
+spm = "shared"            # off | shared
+```
````

### Diff (PLAN.md) – define config scope

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Next steps
 1. Bring Mac mini worker online
 2. Implement `rch xcode verify`
 3. Add classifier + routing
 4. Add XcodeBuildMCP backend
+
+## Configuration
+- Repo-scoped config: `.rch/xcode.toml` (checked in)
+- Host/user config: `~/.config/rch/*` (workers, credentials, defaults)
+`effective_config.json` MUST be emitted per job (post-merge, fully resolved).
```

---

## 5) Add a worker capability handshake (Xcode versions, simulators, MCP version, capacity)

### Why this improves things

You’ll avoid “it worked on that Mac yesterday” drift. Capability info lets the host:

* pick a compatible worker deterministically
* fail early with a clear reason
* decide destination resolution safely
* record toolchain facts in the attestation

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Milestones
 - **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
 - **M1**: Classifier detects Xcode build/test safely
 - **M2**: MVP remote execution with `xcodebuild`
 - **M3**: Switch to XcodeBuildMCP backend
 - **M4**: Emit summary.json, attestation.json, manifest.json
 - **M5**: Remote caching (DerivedData, SPM) and performance tuning
+ - **M6**: Worker capability handshake + deterministic worker selection
+
+## Worker capabilities
+Worker reports a `capabilities.json` including:
+- Xcode version(s) + build number, `DEVELOPER_DIR`
+- available runtimes/devices (simctl)
+- installed tooling versions (rch-worker, XcodeBuildMCP)
+- capacity (max concurrent jobs, disk free)
+Host stores the chosen worker capability snapshot in artifacts.
```

---

## 6) Strengthen “safe-by-default” into a concrete allowlist + `explain` / `dry-run`

### Why this improves things

“Safe-by-default” is great, but unless it’s defined, you’ll get accidental interception of commands you didn’t intend (or agents won’t trust it). Add:

* explicit allowlist of recognized invocations
* explicit deny rules (e.g., signing/export/upload)
* `rch xcode explain` that says “would intercept because … / would not because …”
* `--dry-run` for CI debug and agent transparency

### Diff (README.md)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Setup
@@
 4. Run: `rch xcode verify`
+
+## Safety model
+Intercept is **deny-by-default**:
+- Allowed: `xcodebuild build`, `xcodebuild test` (within configured workspace/project + scheme)
+- Denied: archive/export, notarization, signing/export workflows, arbitrary scripts, “mutating setup”
+
+Useful commands:
+- `rch xcode explain -- <command...>`  (why it will/won’t be intercepted)
+- `rch xcode verify --dry-run`         (prints resolved plan + selected worker)
```

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Goals
@@
 - Safe-by-default interception (false negatives preferred)
+
+## Classifier (safety gate)
+The classifier MUST:
+- match only supported forms of `xcodebuild` invocations
+- reject unknown flags / actions by default
+- enforce repo config constraints (workspace/project, scheme, destination)
+- emit a machine-readable explanation when rejecting (`summary.json` includes `rejection_reason`)
```

---

## 7) Upgrade artifacts: schema versioning + “effective_config” + invocation + toolchain + metrics

### Why this improves things

Your current artifact set is a good start, but you’ll quickly want:

* `effective_config.json` (what actually ran)
* `job.json` (the JobSpec)
* `invocation.json` (sanitized command + derived settings)
* `toolchain.json` (Xcode/MCP versions, sim runtimes)
* `metrics.json` (durations, cache hits, transfer sizes)
* schema versioning so tools can evolve safely

### Diff (README.md)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Outputs
 Artifacts are written to:
 `~/.local/share/rch/artifacts/<job_id>/`
 
 Includes:
 - summary.json
 - attestation.json
 - manifest.json
+- effective_config.json
+- job.json
+- invocation.json
+- toolchain.json
+- metrics.json
 - build.log
 - result.xcresult/
```

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Artifacts
 - summary.json
 - attestation.json
 - manifest.json
+- effective_config.json
+- job.json
+- invocation.json
+- toolchain.json
+- metrics.json
 - build.log
 - result.xcresult/
+
+## Artifact schemas + versioning
+All JSON artifacts MUST include:
+- `schema_version`
+- `created_at`
+- `job_id` and `job_key`
```

---

## 8) Add reliability primitives: failure taxonomy, retries, timeouts, partial salvage

### Why this improves things

Agents need to distinguish:

* “classifier rejected” (expected / safe)
* “transport failed”
* “worker unavailable”
* “xcodebuild failed tests”
* “backend crashed”
  …and handle them differently. Also, when something fails, you still want the logs + partial artifacts.

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Goals
@@
 - Agent-friendly JSON outputs and artifacts
@@
+## Failure taxonomy
+`summary.json` MUST include:
+- `status`: success | failed | rejected
+- `failure_kind`: CLASSIFIER_REJECTED | SSH | TRANSFER | EXECUTOR | XCODEBUILD | MCP | ARTIFACTS
+- `exit_code`: stable integer for scripting
+- `human_summary`: short string for console output
+
+## Timeouts + retries
+- SSH/connect retries with backoff
+- Transfer retries (idempotent)
+- Executor timeout (overall + idle-log watchdog)
+On failure, artifacts MUST still include logs + diagnostics if available.
```

---

## 9) Treat caching as a first-class design (keys, safety, concurrency) — not just “performance tuning later”

### Why this improves things

Caching is where remote execution becomes *compelling* (not just “move the pain elsewhere”). But caching can also break correctness if keyed poorly. Tie it directly to the `job_key` / `source_sha256`, and document:

* DerivedData strategy (`per_job` vs `shared`)
* SPM cache strategy
* concurrency locks (avoid stampedes)
* cache report in `metrics.json`

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Milestones
@@
 - **M5**: Remote caching (DerivedData, SPM) and performance tuning
@@
+## Caching
+Caching MUST be correctness-preserving:
+- Cache keys derive from `job_key` (or documented sub-keys).
+- DerivedData modes: `off` | `per_job` | `shared` (shared requires safe keying + locking).
+- SPM cache mode: `off` | `shared` (shared keyed by resolved Package.resolved + toolchain).
+`metrics.json` includes cache hit/miss + sizes + timing.
```

---

## 10) Tighten the worker execution/security model (least privilege, forced command option, no ambient secrets)

### Why this improves things

Remote “build/test gate” is a security boundary. You’ll want a crisp posture:

* dedicated `rch` user on the Mac
* optional SSH `authorized_keys` forced command
* no arbitrary remote shell required (transport + a single runner entrypoint)
* sandbox-ish run directories, cleanup policy
* explicit statement: not intended for signing/publishing

### Diff (README.md)

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Requirements
 **macOS worker**
 - Xcode installed
 - SSH access
 - rsync + zstd
 - Node.js + XcodeBuildMCP (recommended)
@@
 ## Notes
 - Designed as a build/test gate, not a full IDE replacement
 - Safe-by-default: avoids intercepting setup or mutating commands
+ - Security posture: prefer a dedicated `rch` user; optionally use SSH forced-command; avoid signing/publishing workflows
```

### Diff (PLAN.md)

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Vision
@@
 producing deterministic, machine-readable results.
+
+## Threat model / security notes
+- Remote execution is limited to configured Xcode build/test actions.
+- Worker SHOULD run under a dedicated user with constrained permissions.
+- Prefer an implementation that does not require arbitrary interactive shell access.
+- Not in scope: code signing, notarization, exporting archives, publishing.
```

---

# A consolidated “better plan” outcome (what you end up with)

After these changes, the project reads like a real remote gate:

* **Predictable**: deterministic JobSpec + stable `job_key`
* **Trustworthy**: explicit classifier rules + explain/dry-run
* **Extensible**: backend interface (xcodebuild now, MCP later)
* **Operable**: capabilities handshake + clear failure kinds + metrics
* **Fast**: cache strategy designed in, keyed correctly
* **Safe**: least privilege guidance + non-goals around signing/publishing

If you want one more “big leverage” addition: define JSON Schemas (even minimal) for `summary.json`, `attestation.json`, and `job.json`—that’s the difference between “nice logs” and a platform agents can build on.
