Below are the strongest revisions I'd make to your **README + PLAN** to improve (a) contract clarity, (b) determinism/attestation depth, (c) reliability, (d) performance, and (e) operator safety—while keeping the "build/test gate" vibe.

I'm treating the pasted `README.md` + `PLAN.md` as the baseline.

---

## Change 1 — Make the lane "contract-first" (README becomes quickstart; add a normative spec)

### Why this makes it better

Right now, there's no single **authoritative contract** for: configuration semantics, artifact meanings, job lifecycle states, or what "safe-by-default" *precisely* means. A normative spec prevents drift, enables schema-first tooling, and makes agent/CI integration predictable.

### Patch (adds `docs/XCODE_LANE.md`, updates README/PLAN to point to it)

```diff
diff --git a/README.md b/README.md
index 1111111..2222222 100644
--- a/README.md
+++ b/README.md
@@ -1,29 +1,55 @@
 # RCH Xcode Lane

+> **Normative spec:** `docs/XCODE_LANE.md` is the source of truth for the lane's contract, artifacts, and safety rules.
+> This README is intentionally **non-normative**: mental model + quickstart.
+
 ## What it is
 An extension to Remote Compilation Helper (RCH) that offloads Xcode build/test to a remote Mac mini using XcodeBuildMCP.

 ## Why
 Agents running on Linux or busy Macs can still validate iOS/macOS projects under pinned Xcode conditions without local Xcode installs.

+## How it works (high level)
+1. **Select worker** (tagged `macos,xcode`) and probe capabilities (Xcode, runtimes, XcodeBuildMCP).
+2. **Stage source** to the worker (rsync/zstd).
+3. **Run** build/test remotely (via XcodeBuildMCP backend; `xcodebuild` fallback allowed).
+4. **Collect artifacts** (logs, `xcresult`, structured JSON).
+5. **Attest** toolchain + environment; emit machine-readable outputs for CI/agents.
+
 ## Requirements
 **macOS worker**
 - Xcode installed
 - SSH access
 - rsync + zstd
 - Node.js + XcodeBuildMCP (recommended)
@@ -23,7 +49,7 @@
 ## Setup
 1. Add Mac mini to `~/.config/rch/workers.toml` with tags `macos,xcode`
 2. Add `.rch/xcode.toml` to your repo
 3. Start daemon: `rch daemon start`
 4. Run: `rch xcode verify`
@@ -31,15 +57,19 @@
 ## Outputs
 Artifacts are written to: `~/.local/share/rch/artifacts/<job_id>/`
 Includes:
 - summary.json
 - attestation.json
 - manifest.json
 - build.log
 - result.xcresult/

 ## Notes
 - Designed as a build/test gate, not a full IDE replacement
 - Safe-by-default: avoids intercepting setup or mutating commands
+
+## Next
+- Read the contract: `docs/XCODE_LANE.md`
+- Add a minimal `.rch/xcode.toml` and run `rch xcode verify`
diff --git a/PLAN.md b/PLAN.md
index 3333333..4444444 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -1,23 +1,32 @@
 # PLAN — RCH Xcode Lane

 ## Vision
 Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker (Mac mini) using XcodeBuildMCP, producing deterministic, machine-readable results.

 ## Goals
 - Remote Xcode build/test via macOS workers only
 - Deterministic configuration and attestation
 - Agent-friendly JSON outputs and artifacts
 - Safe-by-default interception (false negatives preferred)

+## Contract-first deliverables
+- **Normative spec:** `docs/XCODE_LANE.md` (job lifecycle, config semantics, artifacts, safety rules)
+- **Schemas (later):** `docs/schema/xcode-lane/*.schema.json` for outputs/config (optional milestone)
+
 ## Milestones
 - **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
 - **M1**: Classifier detects Xcode build/test safely
 - **M2**: MVP remote execution with `xcodebuild`
 - **M3**: Switch to XcodeBuildMCP backend
 - **M4**: Emit summary.json, attestation.json, manifest.json
 - **M5**: Remote caching (DerivedData, SPM) and performance tuning
@@ -19,3 +28,74 @@
 ## Next steps
 1. Bring Mac mini worker online
 2. Implement `rch xcode verify`
 3. Add classifier + routing
 4. Add XcodeBuildMCP backend
diff --git a/docs/XCODE_LANE.md b/docs/XCODE_LANE.md
new file mode 100644
index 0000000..5555555
--- /dev/null
+++ b/docs/XCODE_LANE.md
@@ -0,0 +1,74 @@
+# RCH Xcode Lane — Normative Specification (v0)
+
+This document defines the **contract** for the RCH Xcode Lane: configuration semantics, job lifecycle, artifacts,
+and safety behavior. If anything conflicts with README/PLAN, this document wins.
+
+## Terms
+- **Host**: machine running `rch` client + daemon.
+- **Worker**: macOS machine running jobs over SSH.
+- **Job**: one remote build/test execution with stable, addressable artifacts.
+- **Profile**: named configuration block (e.g. `ci`, `local`, `release`).
+
+## Job lifecycle (minimum)
+1. **created** → 2. **staging** → 3. **running** → 4. terminal:
+   - **succeeded** | **failed** | **canceled** | **timed_out**
+
+Host MUST preserve partial artifacts for non-success terminals.
+
+## Safety rules (minimum)
+- Lane MUST NOT run mutating commands by implicit interception (e.g. `clean`, `archive`) unless explicitly requested.
+- When classification is uncertain, lane MUST prefer **not** to intercept (false negatives are acceptable).
+
+## Required artifacts (minimum)
+Artifacts are written under an immutable job directory on the host:
+- `summary.json` (machine-readable outcome)
+- `attestation.json` (toolchain + environment fingerprint)
+- `manifest.json` (artifact index + hashes)
+- `build.log` (stdout/stderr capture)
+- `result.xcresult/` (when tests run and `xcresult` is produced)
+
+## Determinism requirements (minimum)
+- Lane MUST record Xcode identity (path + version) and macOS version in `attestation.json`.
+- Lane MUST record effective configuration used for the run (inline or referenced).
```

---

## Change 2 — Make `.rch/xcode.toml` profile-based + explicitly "pin" toolchain/destination

### Why this makes it better

A single flat config tends to sprawl and makes CI vs local runs ambiguous. Profiles keep runs intentional, and pinning destination/toolchain is how you get repeatability (and meaningful attestation).

### Patch (adds an example config and describes expected fields)

````diff
diff --git a/README.md b/README.md
index 2222222..2222223 100644
--- a/README.md
+++ b/README.md
@@ -20,10 +20,44 @@
 ## Setup
 1. Add Mac mini to `~/.config/rch/workers.toml` with tags `macos,xcode`
 2. Add `.rch/xcode.toml` to your repo
 3. Start daemon: `rch daemon start`
 4. Run: `rch xcode verify`
+
+## Minimal `.rch/xcode.toml` (example)
+```toml
+# Choose a profile with: `rch xcode verify --profile ci`
+[profiles.ci]
+action = "test"                        # "build" | "test"
+workspace = "MyApp.xcworkspace"        # or `project = "MyApp.xcodeproj"`
+scheme = "MyApp"
+configuration = "Debug"
+
+# Pin destination (determinism)
+destination.platform = "iOS Simulator"
+destination.name = "iPhone 16"
+destination.os = "latest"              # or "18.2" / "18.3" etc
+
+# Pin toolchain (determinism)
+xcode.path = "/Applications/Xcode.app" # optional if worker default is acceptable
+
+# Safety defaults
+allow_mutating = false                 # disallow implicit clean/archive
+timeout_seconds = 1800
+```
diff --git a/PLAN.md b/PLAN.md
index 4444444..4444445 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -6,6 +6,16 @@
 ## Goals
 - Remote Xcode build/test via macOS workers only
 - Deterministic configuration and attestation
 - Agent-friendly JSON outputs and artifacts
 - Safe-by-default interception (false negatives preferred)
+
+## Configuration model (v0)
+- Repo config: `.rch/xcode.toml` with **named profiles** (`profiles.<name>`)
+- Host config: `~/.config/rch/workers.toml` with worker tags + SSH
+- Effective config MUST be resolved deterministically (profile defaults + overrides)
+- Destination + Xcode identity SHOULD be pinned for CI profiles
+
diff --git a/.rch/xcode.toml.example b/.rch/xcode.toml.example
new file mode 100644
index 0000000..6666666
--- /dev/null
+++ b/.rch/xcode.toml.example
@@ -0,0 +1,40 @@
+[profiles.ci]
+action = "test"
+workspace = "MyApp.xcworkspace"
+scheme = "MyApp"
+configuration = "Debug"
+timeout_seconds = 1800
+
+[profiles.ci.destination]
+platform = "iOS Simulator"
+name = "iPhone 16"
+os = "latest"
+
+[profiles.ci.xcode]
+path = "/Applications/Xcode.app"
+
+[profiles.ci.cache]
+derived_data = true
+spm = true
+
+[profiles.ci.safety]
+allow_mutating = false
````

---

## Change 3 — Expand artifacts into a "debuggable + verifiable" bundle (effective config + environment + timings)

### Why this makes it better

`summary.json` + `attestation.json` + `manifest.json` is a great start, but when something flakes you'll want:

* **effective_config.json** (what actually ran)
* **environment.json** (Xcode, runtimes, sim list snapshot, host/worker info)
* **timing.json** (stage/run/collect durations)
  This makes failures diagnosable by agents without "reading logs like a human".

### Patch (updates README outputs + normative spec)

````diff
diff --git a/README.md b/README.md
index 2222223..2222224 100644
--- a/README.md
+++ b/README.md
@@ -48,12 +48,25 @@
 ## Outputs
 Artifacts are written to: `~/.local/share/rch/artifacts/<job_id>/`
 Includes:
 - summary.json
 - attestation.json
 - manifest.json
+- effective_config.json
+- environment.json
+- timing.json
 - build.log
 - result.xcresult/
+
+Recommended structure:
+```text
+<job_id>/
+  summary.json
+  attestation.json
+  manifest.json
+  effective_config.json
+  environment.json
+  timing.json
+  build.log
+  result.xcresult/   (optional)
+```
diff --git a/docs/XCODE_LANE.md b/docs/XCODE_LANE.md
index 5555555..5555556 100644
--- a/docs/XCODE_LANE.md
+++ b/docs/XCODE_LANE.md
@@ -23,16 +23,34 @@
 ## Required artifacts (minimum)
 Artifacts are written under an immutable job directory on the host:
 - `summary.json` (machine-readable outcome)
 - `attestation.json` (toolchain + environment fingerprint)
 - `manifest.json` (artifact index + hashes)
+- `effective_config.json` (fully-resolved config used for the job)
+- `environment.json` (captured worker environment snapshot relevant to determinism)
+- `timing.json` (durations for stage/run/collect)
 - `build.log` (stdout/stderr capture)
 - `result.xcresult/` (when tests run and `xcresult` is produced)
+
+### `manifest.json` requirements (v0)
+- MUST include SHA-256 hashes for all material artifacts
+- MUST include byte sizes
+- SHOULD include a logical artifact type for each entry (log/json/xcresult/etc.)
````

---

## Change 4 — Add a "doctor" + capability probing step (turn "verify" into something operators trust)

### Why this makes it better

`rch xcode verify` is great, but in practice you'll need:

* `doctor`: checks **host** dependencies, auth, config parsing, daemon status
* capability probe: checks **worker** (Xcode version/path, sim runtimes, Node, XcodeBuildMCP availability)
  This dramatically reduces "why is it failing?" cycles.

### Patch (README command surface + PLAN milestone tweak)

```diff
diff --git a/README.md b/README.md
index 2222224..2222225 100644
--- a/README.md
+++ b/README.md
@@ -1,6 +1,6 @@
 # RCH Xcode Lane

 > **Normative spec:** `docs/XCODE_LANE.md` is the source of truth for the lane's contract, artifacts, and safety rules.
 > This README is intentionally **non-normative**: mental model + quickstart.
@@ -15,6 +15,20 @@
 4. **Collect artifacts** (logs, `xcresult`, structured JSON).
 5. **Attest** toolchain + environment; emit machine-readable outputs for CI/agents.

+## Commands
+- `rch xcode doctor` — validate host setup (daemon, config, ssh tooling)
+- `rch xcode verify [--profile <name>]` — probe worker + validate config against capabilities
+- `rch xcode build [--profile <name>]` — remote build gate
+- `rch xcode test  [--profile <name>]` — remote test gate
+- `rch xcode fetch <job_id>` — pull artifacts (if stored remotely)
+
 ## Requirements
 **macOS worker**
 - Xcode installed
@@ -59,6 +73,9 @@
 ## Notes
 - Designed as a build/test gate, not a full IDE replacement
 - Safe-by-default: avoids intercepting setup or mutating commands
diff --git a/PLAN.md b/PLAN.md
index 4444445..4444446 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -13,10 +13,12 @@
 ## Milestones
 - **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
-- **M1**: Classifier detects Xcode build/test safely
+- **M1**: `doctor` + worker capability probe + config validation (`verify`)
 - **M2**: MVP remote execution with `xcodebuild`
 - **M3**: Switch to XcodeBuildMCP backend
 - **M4**: Emit summary.json, attestation.json, manifest.json
 - **M5**: Remote caching (DerivedData, SPM) and performance tuning
```

---

## Change 5 — Specify job lifecycle + cancellation/timeouts/retries (make failure modes deliberate)

### Why this makes it better

Remote builds fail in messy ways (SSH drops, sim hangs, Xcodebuild deadlocks). If the lane defines a **job state machine** + timeouts + cancellation semantics, you can build reliable automation and avoid "half-written artifact dirs".

### Patch (tightens lifecycle + mentions timeouts/cancel)

```diff
diff --git a/docs/XCODE_LANE.md b/docs/XCODE_LANE.md
index 5555556..5555557 100644
--- a/docs/XCODE_LANE.md
+++ b/docs/XCODE_LANE.md
@@ -12,11 +12,25 @@
 ## Job lifecycle (minimum)
 1. **created** → 2. **staging** → 3. **running** → 4. terminal:
    - **succeeded** | **failed** | **canceled** | **timed_out**

 Host MUST preserve partial artifacts for non-success terminals.
+
+### Timeouts + cancellation (v0)
+- Lane MUST support a per-job timeout (`timeout_seconds`).
+- If a timeout triggers, terminal state MUST be `timed_out`.
+- Lane SHOULD support user cancellation (best-effort SIGINT/remote termination).
+- A canceled job MUST still emit `summary.json` + `manifest.json` referencing available artifacts.
diff --git a/README.md b/README.md
index 2222225..2222226 100644
--- a/README.md
+++ b/README.md
@@ -74,6 +74,9 @@
 ## Notes
 - Designed as a build/test gate, not a full IDE replacement
 - Safe-by-default: avoids intercepting setup or mutating commands
+- Failure modes are first-class: timeouts/cancellation preserve partial artifacts
```

---

## Change 6 — Make performance a first-class design (incremental sync + cache keys + simulator prewarm)

### Why this makes it better

"M5: caching" is good, but you'll want a clearer plan for:

* **incremental staging** (rsync filters, exclude DerivedData, optionally use git-based staging)
* **cache keys** (Xcode identity + SDK/runtime + resolved deps hash)
* **simulator prewarm** (boot once, reuse, avoid cold-start tax)

### Patch (PLAN becomes more actionable on performance)

```diff
diff --git a/PLAN.md b/PLAN.md
index 4444446..4444447 100644
--- a/PLAN.md
+++ b/PLAN.md
@@ -16,7 +16,20 @@
 - **M2**: MVP remote execution with `xcodebuild`
 - **M3**: Switch to XcodeBuildMCP backend
 - **M4**: Emit summary.json, attestation.json, manifest.json
-- **M5**: Remote caching (DerivedData, SPM) and performance tuning
+- **M5**: Performance + caching
+  - Incremental staging (rsync with excludes; optional git archive/clone strategy)
+  - Cache buckets:
+    - DerivedData (keyed by Xcode identity + destination + repo hash)
+    - SwiftPM (keyed by Xcode identity + resolved deps)
+  - Simulator prewarm (boot + reuse; collect runtime info in `environment.json`)
+  - Concurrency control on worker (avoid thrashing; queue jobs if needed)
```

---

## Change 7 — Clarify the security model and "safe-by-default" behavior (so operators aren't surprised)

### Why this makes it better

Remote Xcode builds execute arbitrary project scripts (build phases, SPM plugins). Even in "gate mode", this is effectively "run untrusted code on the worker." Making this explicit prevents bad assumptions and encourages safe deployment patterns (dedicated user, isolated machine).

### Patch (README + normative spec warnings + explicit opt-in for mutating ops)

```diff
diff --git a/README.md b/README.md
index 2222226..2222227 100644
--- a/README.md
+++ b/README.md
@@ -7,6 +7,14 @@
 ## What it is
 An extension to Remote Compilation Helper (RCH) that offloads Xcode build/test to a remote Mac mini using XcodeBuildMCP.

+## Safety / security model (read this)
+Xcode builds can execute project-defined scripts and plugins. Treat the worker as executing **potentially untrusted code**
+from the repository under test. Recommended:
+- dedicated macOS user account for RCH runs
+- dedicated machine (or at least dedicated environment) for lane execution
+- keep `allow_mutating = false` unless you explicitly need `clean/archive`-like behavior
+
diff --git a/docs/XCODE_LANE.md b/docs/XCODE_LANE.md
index 5555557..5555558 100644
--- a/docs/XCODE_LANE.md
+++ b/docs/XCODE_LANE.md
@@ -18,6 +18,15 @@
 ## Safety rules (minimum)
 - Lane MUST NOT run mutating commands by implicit interception (e.g. `clean`, `archive`) unless explicitly requested.
 - When classification is uncertain, lane MUST prefer **not** to intercept (false negatives are acceptable).
+
+### Threat model (v0)
+- Repositories may contain build scripts/plugins that execute code during build/test.
+- Lane does not attempt to sandbox Xcode beyond best-effort OS/user isolation.
+- Operators SHOULD deploy workers as dedicated, non-sensitive machines/accounts.
```

---

### Net effect

After these changes, your lane reads like a **real, reliable subsystem**:

* contract-first (spec)
* profile-based deterministic config
* richer artifacts for agent/CI automation
* preflight + capability probing
* explicit lifecycle semantics
* performance plan that's implementable
* clear safety model

If you want, I can also propose **one consolidated "final combined diff"** that applies all changes at once (instead of these staged patches), but the above is already structured as incremental commits.
