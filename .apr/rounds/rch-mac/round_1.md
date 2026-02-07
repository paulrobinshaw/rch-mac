Below are the highest-leverage revisions I’d make to **tighten the architecture**, **reduce ambiguity**, **improve determinism**, **harden safety**, and **make the lane more “agent/CI-compelling.”**
For each change: rationale + a *git-diff style* patch against the README/PLAN you provided.

---

## 1) Make the worker harness the *single* execution surface (backend is an implementation detail)

### Why this makes it better

* Right now the docs imply the host may “fallback to `xcodebuild`.” That creates two execution surfaces (host-runner vs harness-runner) and makes your protocol / artifact / event guarantees harder to keep consistent.
* If **all build/test execution always happens via `rch-xcode-worker run`**, you get one stable NDJSON protocol, one place to enforce resource limits/timeouts, one place to normalize logs/events, and you can still support a “backend fallback” *inside* the harness.
* It also simplifies auditing: the host is *only* a planner/attester/collector; the worker is *only* an executor.

### Patch

````diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## What it is
 
-**RCH Xcode Lane** is a *remote build/test gate* for Apple-platform projects. It extends **Remote Compilation Helper (RCH)** to route safe, allowlisted Xcode build/test commands to a remote **macOS worker** (e.g., a Mac mini) via **XcodeBuildMCP** (preferred) or a fallback `xcodebuild` runner.
+**RCH Xcode Lane** is a *remote build/test gate* for Apple-platform projects. It extends **Remote Compilation Helper (RCH)** to route safe, allowlisted Xcode build/test jobs to a remote **macOS worker** (e.g., a Mac mini) via a **stable worker harness**: `rch-xcode-worker`.
+
+The harness provides a consistent NDJSON event protocol and executes using a configured **worker backend**:
+- **XcodeBuildMCP** (preferred; richer structured events)
+- **`xcodebuild` backend** (compatibility; must be explicitly allowed via config)
@@
 3. **Run** build/test remotely (via XcodeBuildMCP backend; `xcodebuild` fallback allowed).
+3. **Run** build/test remotely by invoking `rch-xcode-worker run` over SSH. The harness selects the configured backend (XcodeBuildMCP preferred; `xcodebuild` backend only if allowed).
@@
 ### macOS worker
 - Xcode installed (pinned version; lane records Xcode build number)
 - SSH access (key-based)
 - `rsync` + `zstd` (fast sync + compression)
-- Node.js + XcodeBuildMCP (recommended backend)
-- `rch-xcode-worker` harness (recommended): stable remote probe/run/collect interface
+- `rch-xcode-worker` harness (**required for build/test**): stable remote probe/run interface (NDJSON events)
+- Node.js + XcodeBuildMCP (**recommended**): richer structured events than raw logs
@@
 ## Minimal `.rch/xcode.toml`
 ```toml
 [profiles.ci]
 action = "test"
 workspace = "MyApp.xcworkspace"
 scheme = "MyApp"
 configuration = "Debug"
 timeout_seconds = 1800
 
+[profiles.ci.backend]
+preferred = "mcp"      # "mcp" | "xcodebuild"
+allow_fallback = false # CI SHOULD be strict; local profiles may allow fallback
+
 [profiles.ci.destination]
 platform = "iOS Simulator"
 name = "iPhone 16"
 os = "18.2"  # CI SHOULD pin; floating "latest" is opt-in (see PLAN.md)
 
 [profiles.ci.safety]
 allow_mutating = false
 code_signing_allowed = false
````

diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@

## Core Architecture

### Host (RCH daemon)

* Classifies commands (strict allowlist)
* Selects an eligible worker (tags: `macos,xcode`)
* Syncs workspace snapshot (rsync + excludes)
* * Executes via backend (XcodeBuildMCP preferred, `rch-xcode-worker` harness recommended)

- - Executes remotely by invoking the worker harness (`rch-xcode-worker run`) over SSH
  - Streams logs; assembles artifacts; emits manifest + attestation

### Worker (macOS)

* Provides a stable Xcode + simulator environment
* Maintains caches (DerivedData, SPM) keyed by effective config
* * Returns xcresult + logs + tool metadata

- - Runs builds/tests via the worker harness, using the configured backend
- * Returns xcresult + logs + tool metadata

-### Worker Harness (Normative, Recommended)
-The recommended way to execute jobs on a worker is via the `rch-xcode-worker` harness — a lightweight executable invoked over SSH that accepts a job request on stdin and emits structured results on stdout.
+### Worker Harness (Normative, Required for build/test)
+Remote execution for `build` and `test` MUST be performed via the `rch-xcode-worker` harness — a lightweight executable invoked over SSH that accepts a job request on stdin and emits structured results on stdout.
+
+#### Backend Model (Normative)
+The harness MUST support at least one backend:
+- `xcodebuild` (required baseline backend)
+The harness SHOULD support:
+- `mcp` (XcodeBuildMCP backend) for richer structured events.
+
+Backend selection MUST be driven by `effective_config.json` and recorded there. If the preferred backend is unavailable:
+- If `backend.allow_fallback = false`, the lane MUST refuse to run.
+- If `backend.allow_fallback = true`, the lane MAY fall back and MUST emit a warning event and record the selected backend in `effective_config.json`.
@@

### Repo Config: `.rch/xcode.toml`

@@

```toml
# Example: .rch/xcode.toml
[profiles.ci]
action = "test"                        # "build" | "test"
workspace = "MyApp.xcworkspace"        # or project = "MyApp.xcodeproj"
scheme = "MyApp"
configuration = "Debug"
timeout_seconds = 1800

+ [profiles.ci.backend]
+ preferred = "mcp"                      # "mcp" | "xcodebuild"
+ allow_fallback = false
+
[profiles.ci.destination]
platform = "iOS Simulator"
name = "iPhone 16"
os = "18.2"                            # pinned version; required for CI unless allow_floating_destination = true
@@
### Resolution Rules
@@
- Effective config MUST be resolved deterministically (profile defaults + CLI overrides).
+ - Backend MUST be resolved deterministically (`backend.preferred`, `backend.allow_fallback`) and recorded in `effective_config.json`.
@@
### `verify` Checks (Worker)
- Worker reachable via SSH
+ - `rch-xcode-worker` present and compatible protocol_version (for build/test)
- Xcode installed at expected path (or discoverable)
- Requested destination available (simulator runtime + device)
- - XcodeBuildMCP available (if configured as backend)
+ - Preferred backend available (MCP if configured; otherwise baseline `xcodebuild`)
- Node.js version compatible
```

---

## 2) Add “preflight planning” + “warmup” commands (huge UX + performance win)

### Why this makes it better

* Agents/CI want: “show me exactly what you will do” *before* they pay the cost or risk.
* `plan` gives deterministic transparency: resolved worker, destination UDID, backend, computed `run_id`, source policy verdicts (dirty tree refusal), etc.
* `warm` turns your existing “Simulator Prewarm + cache buckets” into a first-class primitive (and lets CI shave minutes on cold starts).

### Patch

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Commands
 
 | Command | Purpose |
 |---------|---------|
 | `rch xcode doctor` | Validate host setup (daemon, config, SSH tooling) |
+| `rch xcode plan [--profile <name>]` | Resolve + pin effective config, destination, backend; compute `run_id` (no build/test) |
 | `rch xcode verify [--profile <name>]` | Probe worker + validate config against capabilities |
+| `rch xcode warm [--profile <name>]` | Prewarm simulator + caches on the worker (optional) |
 | `rch xcode build [--profile <name>]` | Remote build gate |
 | `rch xcode test [--profile <name>]` | Remote test gate |
@@
 ## Setup
@@
-5. Validate config: `rch xcode verify --profile ci`
+5. Preflight: `rch xcode plan --profile ci`
+6. Validate config: `rch xcode verify --profile ci`
-6. Run a gate: `rch xcode test --profile ci`
+7. (Optional) Warm worker: `rch xcode warm --profile ci`
+8. Run a gate: `rch xcode test --profile ci`
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Commands
 | Command | Purpose |
 |---------|---------|
 | `rch xcode doctor` | Validate host setup (daemon, config, SSH tooling) |
+| `rch xcode plan [--profile <name>]` | Produce resolved `effective_config.json` + destination pin + computed `run_id` (no execution) |
 | `rch xcode verify [--profile <name>]` | Probe worker + validate config against capabilities |
+| `rch xcode warm [--profile <name>]` | Prewarm simulator + caches (no build/test output; best-effort) |
 | `rch xcode build [--profile <name>]` | Remote build gate |
 | `rch xcode test [--profile <name>]` | Remote test gate |
@@
 ### `verify` Checks (Worker)
@@
   - Node.js version compatible
+
+### `plan` Semantics (Normative)
+`rch xcode plan` MUST:
+- Resolve `effective_config.json` deterministically (including destination and backend selection).
+- Compute `source_tree_hash` and `run_id` using the normative algorithms.
+- Emit `effective_config.json` and a `plan.json` artifact containing: chosen worker, destination resolution, backend decision, and refusal reasons (if any).
+- Perform no build/test execution and MUST NOT mutate the source tree or worker workspace.
+
+### `warm` Semantics (Non-mutating, Best-effort)
+`rch xcode warm` SHOULD:
+- Boot the resolved simulator destination (or validate it is bootable).
+- Optionally prime caches (SPM/DerivedData) in a way that does not require code signing and does not produce a gate result.
+- Record timing/metrics and emit a terminal summary artifact (state `succeeded`/`failed`) for observability.
```

---

## 3) Tighten determinism: canonical source hashing + *identifier-grade* destination pinning

### Why this makes it better

* Your `run_id` depends on `source_tree_hash`, but the hashing algorithm is currently underspecified (path normalization, symlinks, executable bits, etc.).
* Simulator selection by `(platform, name, os)` is better than floating, but still prone to drift/duplication across workers. You want **stable identifiers** (runtime identifier + device type identifier) as an optional “CI-grade pin.”

### Patch

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### Destination Disambiguation (Config)
 Under `[profiles.<name>.destination]`, the following fields MAY be used:
 - `on_multiple` — `"error"` (default) | `"select"`
 - `selector` — `"highest_udid"` | `"lowest_udid"` (future: more selectors)
 - `udid` — optional explicit simulator UDID (generally host/worker-specific; best for local-only profiles)
+- `runtime_identifier` — optional stable runtime id (e.g. `com.apple.CoreSimulator.SimRuntime.iOS-18-2`)
+- `device_type_identifier` — optional stable device type id (e.g. `com.apple.CoreSimulator.SimDeviceType.iPhone-16`)
@@
 ### Destination Resolution Algorithm
  1. Read `destination.platform`, `destination.name`, `destination.os` from effective config.
+ 1.1 If `destination.runtime_identifier` or `destination.device_type_identifier` is set, prefer identifier matching over name/os matching.
  2. If `os` is `"latest"` and `allow_floating_destination` is false, reject with error.
- 3. Query worker for available simulators matching platform + name + os.
+ 3. Query worker for available simulators (identifier + UDID + runtime version + device type), and match either:
+    - (runtime_identifier + device_type_identifier), or
+    - (platform + name + os)
  4. If exactly one match, use its UDID.
@@
  7. Record resolved UDID, runtime version, and device type in `effective_config.json`.
+ 7.1 If identifier matching was used, also record `runtime_identifier` and `device_type_identifier` in `effective_config.json`.
@@
 ### Source Snapshot (Normative)
@@
- - **`source_tree_hash`** — A deterministic hash of the source files sent to the worker (SHA-256 of sorted file paths + contents, excluding `.rch/`, `.git/`, and configured excludes).
+ - **`source_tree_hash`** — A deterministic hash of the source files sent to the worker, computed via the normative algorithm below.
@@
 ### Source Policy
 Source snapshot behavior is configured under `[profiles.<name>.source]`:
@@
   - **`include_untracked`** — If true, untracked files are included in hash and sync. Default: false.
+  - **`excludes`** — Optional array of glob-like patterns (repo-relative) excluded from hashing and staging (in addition to built-ins).
+  - **`follow_symlinks`** — Default false. If true, symlink targets are hashed as file contents; otherwise symlinks are hashed as link metadata.
+
+### Source Tree Hash Algorithm (Normative)
+To ensure `source_tree_hash` is stable across platforms/implementations:
+1. Enumerate files according to `source.mode` and `include_untracked`, excluding:
+   - Built-ins: `.git/`, `.rch/`, `DerivedData/`, `*.xcresult/`
+   - Any configured `source.excludes`
+2. Normalize each path:
+   - Repo-relative, UTF-8
+   - Use `/` as separator
+   - No leading `./`
+3. Sort paths by bytewise UTF-8 order.
+4. For each entry, append to the hash input:
+   - `path` + `\n`
+   - `type` (`file` | `symlink`) + `\n`
+   - For regular files: raw file bytes, then `\n`
+   - For symlinks:
+     - if `follow_symlinks=false`: link target UTF-8 bytes, then `\n`
+     - else: treat as file and hash the target file bytes
+5. Compute `SHA-256` over the concatenated bytes; hex-encode lowercase.
```

---

## 4) Make artifacts *auditable in practice*: atomic writes + portable bundle + directory hashing

### Why this makes it better

* You already have `manifest.json`, but large directory artifacts (like `result.xcresult/`) are painful to verify or move as a unit.
* Atomic write rules prevent half-written JSON from confusing agents.
* A `bundle.tar.zst` (optional) makes “fetch/copy/archive” reliable and enables resumable transfers without inventing a new transport.

### Patch

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
  <job_id>/
@@
  ├── build.log              # Streamed + finalized stdout/stderr
  ├── result.xcresult/       # When tests are executed
+ ├── bundle.tar.zst         # Optional: portable bundle of all artifacts (for storage/transfer)
  └── provenance/            # Optional: signatures + verification report
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ## Required Artifacts
@@
  ├── build.log              # stdout/stderr capture
  └── result.xcresult/       # When tests run and xcresult is produced
+
+### Artifact Assembly Rules (Normative)
+- JSON artifacts MUST be written atomically (write to `*.tmp`, fsync, then rename).
+- `status.json` updates MUST be atomic (replace via rename) to support polling consumers.
+- `manifest.json` MUST be written last, after all other artifacts are in place, and MUST reflect the final set.
+- Once `summary.json.state` is terminal, artifacts in the job directory SHOULD be treated as immutable.
@@
 ### `manifest.json` Requirements
  - MUST include SHA-256 hashes for all material artifacts.
  - MUST include byte sizes.
  - SHOULD include a logical artifact type for each entry (log/json/xcresult/etc.).
  - SHOULD include `kind`/`schema_version` for JSON artifacts (or inferable mapping) to aid verifiers.
+ - For directory artifacts (e.g., `result.xcresult/`), manifest SHOULD include a `tree_hash` computed using the same path normalization rules as `source_tree_hash` (hash over relative paths + file bytes).
+
+### Optional Portable Bundle (`bundle.tar.zst`)
+When enabled, the lane MAY emit `bundle.tar.zst` containing the complete job directory contents (excluding itself).
+If emitted:
+- `manifest.json` MUST include an entry for `bundle.tar.zst` (hash + size).
+- The bundle format MUST be deterministic (sorted paths, normalized metadata) so its hash is stable.
```

---

## 5) Harden security where it actually breaks in practice: env/agent forwarding + secret redaction + network posture

### Why this makes it better

* The biggest real-world footguns aren’t just “host key pinning” — they’re **accidental secret exposure**:

  * SSH agent forwarding into an untrusted build.
  * Dumping full environment variables into `environment.json`.
  * Allowing outbound network without acknowledging it in attestation.
* Making these *explicit defaults* dramatically reduces operator mistakes.

### Patch

```diff
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Safety / Security Model
@@
 Recommended:
 - Dedicated macOS user account for RCH runs
 - Dedicated machine (or at least dedicated environment) for lane execution
 - Keep `allow_mutating = false` unless you explicitly need `clean`/`archive`-like behavior
 - Pin worker SSH host keys (or at minimum record host key fingerprints in attestation)
+- Disable SSH agent forwarding (treat the repo as untrusted code)
+- Do not capture or emit full process environments; prefer curated environment snapshots + redaction
+- Consider restricting outbound network on the worker (at minimum, record network posture in attestation)
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### Transport Trust (Normative)
@@
 `attestation.json` MUST record the observed SSH host key fingerprint used for the session (even if not pinned), so audits can detect worker identity drift.
+
+#### SSH Session Hygiene (Normative)
+- The host MUST disable SSH agent forwarding for all lane sessions (equivalent to `ForwardAgent=no`).
+- The host MUST NOT forward arbitrary environment variables to the worker harness by default.
+- Any explicitly forwarded environment variables MUST be allowlisted in config and recorded in `effective_config.json` and `attestation.json`.
@@
 ## Safety Rules
@@
 ### Threat Model
@@
  - Operators SHOULD deploy workers as dedicated, non-sensitive machines/accounts.
  - Operators SHOULD use a dedicated macOS user account with minimal privileges for RCH runs.
+
+### Environment + Redaction (Normative)
+- `environment.json` MUST be a curated snapshot (OS/Xcode/simulator runtimes/tool versions) and MUST NOT include full process environment variables by default.
+- If redaction is applied to logs or artifacts, the lane MUST record that redaction policy in `attestation.json` (e.g., `redaction.enabled=true`, `redaction.rules_version`).
+
+### Network Posture (Recommended, Recorded)
+The lane SHOULD record whether the worker run had normal outbound network access or was restricted (best-effort) in `attestation.json` as `network.mode` (`"default"` | `"restricted"`).
```

---

## 6) Add worker capability *constraints* to make multi-worker fleets deterministic (and avoid “works on my Mac mini”)

### Why this makes it better

* Tags alone (`macos,xcode`) aren’t enough for fleets: you want “Xcode build number must match,” “must have iOS 18.2 runtime,” “must support MCP backend,” etc.
* This prevents silent drift and makes selection explainable in artifacts.

### Patch

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### Configuration Model
@@
 ### Repo Config: `.rch/xcode.toml`
@@
  [profiles.ci.xcode]
  path = "/Applications/Xcode.app"       # optional; uses worker default if omitted
+
+ [profiles.ci.worker]
+ require_xcode_build = "16C5032a"       # optional exact build number constraint
+ require_sim_runtime = "iOS 18.2"       # optional minimum/required runtime presence
+ require_backend = "mcp"               # optional: "mcp" | "xcodebuild"
@@
 ### Resolution Rules
@@
  - Destination + Xcode identity MUST be pinned for CI profiles unless `allow_floating_destination = true`.
+ - Worker selection MUST filter eligible workers against any configured `[profiles.<name>.worker]` constraints, and record the selection rationale in `decision.json`.
@@
 ### `verify` Checks (Worker)
@@
   - Requested destination available (simulator runtime + device)
+  - Worker satisfies any configured `[profiles.<name>.worker]` constraints (Xcode build, runtimes, backend)
@@
 ### Decision Artifact (Normative)
@@
  **Required fields:**
@@
  - `worker_selected` — Worker name, or null if refused.
+ - `worker_rejected` — Optional array of `{ name, reason }` for workers considered but rejected.
  - `timestamp` — ISO 8601 timestamp of the decision.
```

---

## 7) Make failures “machine-friendly”: reason codes + stable CLI exit codes + consistent event identity

### Why this makes it better

* Agents need to branch on outcomes without log scraping: “refused due to floating destination” vs “worker unreachable” vs “tests failed.”
* You already have rich artifacts; adding **reason codes** and **exit codes** makes the lane *programmable*.
* Also: your event requirements are slightly inconsistent across sections (some say `job_id` required per event, later tables omit it). Make identity fields consistent everywhere.

### Patch

```diff
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@
 ### `events.ndjson` Requirements
@@
  **Required fields per event:**
  - `type` — Event type string (e.g., `"build_started"`, `"test_case_passed"`, `"phase_completed"`, `"error"`, `"complete"`).
  - `timestamp` — ISO 8601 timestamp.
  - `sequence` — Monotonically increasing integer (1-based).
+ - `job_id` — Job identifier.
+ - `run_id` — Run identifier.
+ - `attempt` — Attempt number.
@@
  Consumers MUST tolerate unknown event types (forward compatibility).
+
+## Outcome Taxonomy (Normative)
+`summary.json` MUST include:
+- `state` (`succeeded` | `failed` | `canceled` | `timed_out` | `refused`)
+- `reason_code` — stable machine-readable reason (see below)
+
+### Standard `reason_code` values
+- `refused_uncertain_classification`
+- `refused_policy_mutating_disallowed`
+- `refused_policy_floating_destination_disallowed`
+- `refused_dirty_working_tree`
+- `refused_backend_unavailable`
+- `worker_unreachable`
+- `destination_not_found`
+- `lease_expired`
+- `timeout`
+- `canceled`
+- `xcodebuild_failed`
+
+## CLI Exit Codes (Normative)
+The `rch xcode` subcommands MUST use stable exit codes:
+- `0` success
+- `10` refused (policy / uncertainty)
+- `11` configuration error (invalid/missing profile, parse failures)
+- `12` worker/probe error (unreachable, missing harness)
+- `20` build/test failed (execution completed but failed)
+- `21` timed out
+- `22` canceled
```

---

If you want one additional “big banger” after these: I’d add a **`schemas/` contract check** to `verify` (host validates *its own emitted artifacts* against JSON Schemas after the run). That catches schema drift immediately and makes the lane feel “CI-grade.”
