Below are the revisions I'd make to make this **more deterministic, safer, easier to operate, and more compelling** as the "agent-friendly remote Xcode gate" lane inside RCH.

I'm going to do two things:

1. **Propose changes (architecture/features)** with rationale
2. For each set, show a **git-diff style patch** against your original `README.md` and `PLAN.md`

---

## Proposed revisions (with rationale)

### 1) Make the contract explicit: "lane = remote gate", not "remote IDE"

**Why:** Prevents scope creep, clarifies expectations (false negatives > false positives), and makes your lane "trustable" in agentic/CI workflows.

Key additions:

* A **Non-goals** section
* A **Threat model / safety posture** section

### 2) Determinism: formalize "Pinned Environment" and "Effective Config"

**Why:** Determinism isn't just "remote mac + json outputs". You need a stable *environment contract*:

* Xcode version + build number
* macOS version
* toolchain versions (xcodebuild, swift, clang)
* destination resolution (simulator model/OS/runtime identifiers)
* "effective_config.json" emitted for every run

This is what allows agents to compare runs over time.

### 3) Worker trust & provenance: attest *what ran* and *where*

**Why:** If Linux agents rely on results produced remotely, you want **verifiable provenance**:

* Record worker identity + host keys (TOFU vs pinned)
* Record git state (commit SHA, dirty state)
* Record exact invoked command + normalized arguments
* Hash artifacts and include a manifest

You already list `attestation.json`/`manifest.json`; I'd make them first-class and add `effective_config.json`.

### 4) Safety-by-default routing: define an explicit "Classifier + Allowlist" policy

**Why:** The most likely failure mode is accidentally intercepting commands that mutate signing, provisioning, or project state. Your lane needs:

* A classifier with **strict allowlist**
* A "refuse and explain" behavior (return local / no-op / fail closed depending on mode)

### 5) Performance: define caching and sync strategy up front

**Why:** Remote Xcode work lives/dies on:

* incremental sync (rsync + excludes)
* remote DerivedData & SPM caches keyed by effective config + repo hash
* log streaming (tail -f or MCP streaming) so agents don't time out

### 6) Operational ergonomics: queueing, concurrency, timeouts, cancellation

**Why:** "Remote Mac mini" becomes a shared resource quickly.

* worker-side queue (or RCH daemon queue)
* concurrency limits per worker
* hard timeouts and cancellation
* "lease" concept (job claims a worker)

### 7) Compatibility: call out tricky cases explicitly

**Why:** Xcode builds vary wildly. Being explicit makes this lane credible:

* signing (should be off by default; use `CODE_SIGNING_ALLOWED=NO` unless explicitly enabled)
* tests requiring UI / permissions
* xcresult handling
* simulator boot behavior

---

## Patch: README.md

```diff
diff --git a/README.md b/README.md
index 0000000..1111111 100644
--- a/README.md
+++ b/README.md
@@
-# RCH Xcode Lane
-
-## What it is
-An extension to Remote Compilation Helper (RCH) that offloads Xcode build/test to a remote Mac mini using XcodeBuildMCP.
-
-## Why
-Agents running on Linux or busy Macs can still validate iOS/macOS projects under pinned Xcode conditions without local Xcode installs.
-
-## Requirements
-**macOS worker**
-- Xcode installed
-- SSH access
-- rsync + zstd
-- Node.js + XcodeBuildMCP (recommended)
-
-**Host**
-- RCH client + daemon
-- SSH access to worker
-
-## Setup
-1. Add Mac mini to `~/.config/rch/workers.toml` with tags `macos,xcode`
-2. Add `.rch/xcode.toml` to your repo
-3. Start daemon: `rch daemon start`
-4. Run: `rch xcode verify`
-
-## Outputs
-Artifacts are written to:
-`~/.local/share/rch/artifacts/<job_id>/`
-
-Includes:
-- summary.json
-- attestation.json
-- manifest.json
-- build.log
-- result.xcresult/
-
-## Notes
-- Designed as a build/test gate, not a full IDE replacement
-- Safe-by-default: avoids intercepting setup or mutating commands
+# RCH Xcode Lane
+
+## What it is
+**RCH Xcode Lane** is a *remote build/test gate* for Apple-platform projects.
+It extends **Remote Compilation Helper (RCH)** to route safe, allowlisted Xcode build/test commands
+to a remote **macOS worker** (e.g. a Mac mini) via **XcodeBuildMCP** (preferred) or a fallback `xcodebuild` runner.
+
+## Why
+Agents running on Linux (or saturated Macs) can still verify iOS/macOS projects under a **pinned Xcode + Simulator**
+configuration—without installing Xcode locally—while receiving **machine-readable, auditable outputs**.
+
+## Non-goals
+- Not a remote IDE and not a general "run anything on the Mac" executor
+- Not a provisioning/signing manager (signing is **off by default**)
+- Not a replacement for full CI; this is a deterministic *gate* optimized for agent workflows
+
+## Requirements
+### macOS worker
+- Xcode installed (pinned version; lane records Xcode build number)
+- SSH access (key-based)
+- `rsync` + `zstd` (fast sync + compression)
+- Node.js + XcodeBuildMCP (recommended backend)
+
+### host
+- RCH client + daemon
+- SSH access to the worker
+
+## Determinism & safety model
+This lane is **safe-by-default**:
+- It only intercepts a strict allowlist of build/test invocations.
+- If classification is uncertain, it **refuses to route** (false negatives preferred).
+- It records an **effective configuration** (resolved Xcode path, destination, derived data policy, etc.)
+  so repeated runs can be compared.
+
+## Setup
+1. Register the Mac mini in `~/.config/rch/workers.toml` with tags `macos,xcode`
+2. Add repo config at `.rch/xcode.toml`
+3. Start the daemon: `rch daemon start`
+4. Run a gate: `rch xcode verify`
+
+## Outputs
+Artifacts are written to:
+`~/.local/share/rch/artifacts/<job_id>/`
+
+Includes:
+- `summary.json` (high-level status + timings)
+- `effective_config.json` (resolved/pinned run configuration)
+- `attestation.json` (who/what ran: worker identity, tool versions, repo state)
+- `manifest.json` (artifact listing + hashes)
+- `build.log` (streamed + finalized)
+- `result.xcresult/` (when tests are executed)
+
+## Operational notes
+- Recommended: dedicate a worker user account with minimal privileges
+- Prefer `CODE_SIGNING_ALLOWED=NO` unless explicitly enabled in `.rch/xcode.toml`
+- Use worker concurrency limits to avoid simulator contention
```

---

## Patch: PLAN.md

```diff
diff --git a/PLAN.md b/PLAN.md
index 0000000..2222222 100644
--- a/PLAN.md
+++ b/PLAN.md
@@
 # PLAN — RCH Xcode Lane

 ## Vision
 Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker (Mac mini) using XcodeBuildMCP, producing deterministic, machine-readable results.

 ## Goals
 - Remote Xcode build/test via macOS workers only
 - Deterministic configuration and attestation
 - Agent-friendly JSON outputs and artifacts
 - Safe-by-default interception (false negatives preferred)

+## Core architecture
+**Host (RCH daemon)**
+- Classifies commands (strict allowlist)
+- Selects an eligible worker (tags: `macos,xcode`)
+- Syncs workspace snapshot (rsync + excludes)
+- Executes via backend (XcodeBuildMCP preferred)
+- Streams logs; assembles artifacts; emits manifest + attestation
+
+**Worker (macOS)**
+- Provides a stable Xcode + simulator environment
+- Maintains caches (DerivedData, SPM) keyed by effective config
+- Returns xcresult + logs + tool metadata
+
+## Determinism contract
+Every run MUST emit:
+- `effective_config.json` (resolved Xcode path, destination, build settings, cache policy)
+- `attestation.json` (macOS version, Xcode version/build, toolchain versions, worker identity, repo state)
+- `manifest.json` (artifact inventory + hashes)
+
+Determinism inputs SHOULD include:
+- Explicit Xcode selection (path or version constraint)
+- Explicit destination resolution strategy (simulator identifiers over human names)
+- Explicit signing policy (default off)
+
 ## Milestones
 - **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
-- **M1**: Classifier detects Xcode build/test safely
-- **M2**: MVP remote execution with `xcodebuild`
-- **M3**: Switch to XcodeBuildMCP backend
-- **M4**: Emit summary.json, attestation.json, manifest.json
-- **M5**: Remote caching (DerivedData, SPM) and performance tuning
+- **M1**: Classifier + policy: allowlist routing, refuse-on-uncertain, explain decisions
+- **M2**: Workspace sync + remote runner MVP (fallback `xcodebuild`) with log streaming
+- **M3**: XcodeBuildMCP backend with structured events (build phases, test events)
+- **M4**: Determinism outputs: summary.json, effective_config.json, attestation.json, manifest.json
+- **M5**: Caching + performance: DerivedData/SPM caches, incremental sync, artifact hashing
+- **M6**: Ops hardening: timeouts, cancellation, worker leases, concurrency limits, queueing
+- **M7**: Compatibility matrix + fixtures: golden configs, sample repos, reproducible failure cases

 ## Artifacts
 - summary.json
+- effective_config.json
 - attestation.json
 - manifest.json
 - build.log
 - result.xcresult/

+## Policies & safety
+- Default signing policy: `CODE_SIGNING_ALLOWED=NO` unless explicitly enabled
+- Default routing policy: refuse if command classification is uncertain
+- Default worker policy: run under a dedicated user; never require interactive UI
+
 ## Next steps
 1. Bring Mac mini worker online
 2. Implement `rch xcode verify`
-3. Add classifier + routing
-4. Add XcodeBuildMCP backend
+3. Add classifier + routing + refusal/explanation paths
+4. Implement workspace sync + remote runner + log streaming
+5. Add XcodeBuildMCP backend
+6. Emit determinism artifacts + caching
```

---

## The "top 6" changes I'd prioritize first (pragmatic order)

1. **Classifier + refusal behavior** (protects you from unsafe interception)
2. **effective_config.json + attestation.json** (makes results trustworthy)
3. **Workspace sync + excludes + log streaming** (makes it usable day-to-day)
4. **XcodeBuildMCP backend** (structured events + better UX for agents)
5. **Caching** (DerivedData + SPM) keyed by effective config (makes it fast)
6. **Timeout/cancel/leases/concurrency** (makes it operable as shared infra)

If you want, I can also draft a first-pass `.rch/xcode.toml` schema (minimal but real) that bakes in: Xcode selection, destination resolution, signing policy, cache policy, and worker selection tags.
