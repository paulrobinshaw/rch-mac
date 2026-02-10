# RCH Xcode Lane

> **Normative spec:** `PLAN.md` is the contract.  
> This README is **non-normative**: quickstart, operator notes, and examples.

## Docs map
- `PLAN.md` — **normative** contract (classifier, JobSpec, protocol, artifacts, caching, security)
- `.rch/xcode.toml` — repo-scoped configuration (checked in)
- `~/.config/rch/workers.toml` — host-scoped worker inventory + credentials
- `schemas/rch-xcode/*` — machine-readable JSON Schemas for normative artifacts (see `schema_id` in each JSON)

## At a glance
- **Gate, not IDE:** this lane validates build/test under pinned Xcode; it is not a signing/export/publish pipeline.
- **Deny-by-default:** false negatives are preferred; interception is intentionally conservative.
- **Deterministic + auditable:** every job emits schema-versioned artifacts + a stable `job_key` for caching/attestation.
- **Not a sandbox:** treat the worker like CI; build phases/plugins execute as the worker user.

## What it is
An extension to Remote Compilation Helper (RCH) that offloads Xcode build/test to a remote macOS worker (e.g. a Mac mini).
Execution can use either:
- **Backend `xcodebuild` (MVP)**, or
- **Backend `mcp` (preferred)** via XcodeBuildMCP for richer diagnostics/orchestration.
Both backends MUST emit the same **minimum artifact contract** (see `PLAN.md` → "Backend contract").

## Status
**Pre-implementation / design phase.** The `PLAN.md` spec is being refined through iterative review. No runnable code yet.

## How it fits into RCH (lane boundaries)
RCH is a multi-lane system. Each lane owns a specific kind of remote work. The **Xcode lane** owns:
- Remote `xcodebuild build` and `xcodebuild test` on macOS workers.
- Classifier, JobSpec, transport, artifact collection for those actions.

It does **not** own: code signing, publishing, non-Xcode builds, or arbitrary remote commands. Other lanes (future) may handle those.

## Config precedence
Configuration is merged in this order (last wins):
1. Built-in defaults (hardcoded in the RCH client)
2. Host/user config (`~/.config/rch/`)
3. Repo config (`.rch/xcode.toml`)
4. CLI flags (e.g. `--action`, `--worker`)

Config precedence + merge semantics are **normative** (see `PLAN.md` → "Configuration merge").  
`effective_config.json` is emitted per job showing the final merged result (with secrets redacted).

## Why
Agents running on Linux or busy Macs can still validate iOS/macOS projects under pinned Xcode conditions without local Xcode installs.

## Requirements
**macOS worker**
- Xcode installed
- SSH access
- rsync + zstd
- Node.js + XcodeBuildMCP (recommended for `backend="mcp"`)
- (Recommended) dedicated `rch` user + constrained SSH key/forced-command
  - Recommended: forced-command runs a single `rch-worker xcode ...` entrypoint (no shell)

**Host**
- RCH client + daemon
- SSH access to worker

## Setup
1. Add Mac mini to `~/.config/rch/workers.toml` with tags `macos,xcode`
2. Add `.rch/xcode.toml` to your repo
3. Start daemon: `rch daemon start`
4. Run: `rch xcode verify`

## Quickstart
Most common flows:

```bash
# Validate setup without executing anything
rch xcode verify --dry-run

# Explain why a command will/won't be intercepted
rch xcode explain -- xcodebuild test -workspace MyApp.xcworkspace -scheme MyApp

# Run the repo-defined verify lane (usually build+test)
rch xcode verify

# Run a single action directly
rch xcode run --action test
```

## Trust boundary (important)
`rch xcode` is **not** a sandbox. If your Xcode project contains Run Script build phases,
SwiftPM build tool plugins, or other build-time code execution, that code will run on the worker
under the `rch` user.
Treat the worker like CI: dedicated account, minimal secrets, and no personal keychains.

## Mental model (operator view)
- You run `rch xcode verify` locally (even on Linux).
- RCH classifies/sanitizes the invocation, builds a deterministic `job.json`, bundles inputs, and ships to macOS.
- The host selects a worker once and (if supported) acquires a time-bounded **lease** to avoid cross-host contention.
- Worker executes and returns schema-versioned artifacts (`summary.json`, logs, `xcresult`, etc.).
- `rch xcode verify` is a **run** that may contain multiple **step jobs** (e.g. `build` then `test`), executed sequentially. A failed step skips the rest (unless `continue_on_failure` is set).
- The host persists a `run_plan.json` up front (including the selected worker and `continue_on_failure` flag) so runs can be resumed after interruption with identical semantics.
- The run produces a **run summary** that links to each step job's artifact set.

## Worker inventory example (`~/.config/rch/workers.toml`)
```toml
schema_version = 1

[[worker]]
name = "macmini-01"
host = "macmini.local"
user = "rch"
port = 22
tags = ["macos","xcode"]
known_host_fingerprint = "SHA256:..."
attestation_pubkey_fingerprint = "SHA256:..." # optional pin for signed attestation
ssh_key_path = "~/.ssh/rch_macmini"
priority = 10
```

## Repo config (`.rch/xcode.toml`)
Example:
```toml
schema_version = 1
backend = "xcodebuild" # or "mcp"

[project]
workspace = "MyApp.xcworkspace" # or project = "MyApp.xcodeproj"
scheme = "MyApp"

[actions]
verify = ["build", "test"]

[destination]
mode = "constraints"  # pinned | constraints
value = "platform=iOS Simulator,name=iPhone 16,OS=latest"
# In constraints mode, the host resolves "latest" using the selected worker's capabilities snapshot
# and records the resolved destination into job.json for determinism.
provisioning = "existing" # existing | ephemeral
# If ephemeral, the worker provisions a clean Simulator per job (recommended for flaky test suites).

[toolchain]
# Pin the Xcode build number for reproducible builds.
# The host matches this against the worker's installed Xcodes.
xcode_build = "16C5032a"     # preferred: exact build number
# xcode_version = "16.2"    # alternative: version string (less precise)

[timeouts]
overall_seconds = 1800
idle_log_seconds = 300

[bundle]
mode = "worktree"       # worktree | git_index
ignore_file = ".rchignore"
max_bytes = 0           # 0 = unlimited (host may still enforce sane caps)

[cache]
namespace = "myapp"      # recommended: stable per-repo namespace to avoid collisions
derived_data = "shared"   # off | per_job | shared
spm = "shared"            # off | shared
```

## Safety model
Intercept is **deny-by-default**:
- Allowed: `xcodebuild build`, `xcodebuild test` (within configured workspace/project + scheme)
- Denied: archive/export, notarization, signing/export workflows, arbitrary scripts, "mutating setup"

Useful commands:
- `rch xcode explain -- <command...>`  (why it will/won't be intercepted)
- `rch xcode verify --dry-run`         (prints resolved plan + selected worker)
- `rch xcode run --action <build|test>`(run a single action as a one-step run)
- `rch xcode tail <run_id|job_id>`     (stream logs/events while running)
- `rch xcode cancel <run_id|job_id>`   (best-effort cancellation)
- `rch xcode artifacts <run_id|job_id>`(print artifact locations + key files)
- `rch xcode gc`                       (optional: clean old local artifacts)
- `rch workers list --tag macos,xcode` (show matching workers)
- `rch workers probe <name>`           (fetch capabilities snapshot)
- `rch xcode doctor`                   (validate config, SSH, Xcode, destination)

## Common pitfalls
- **Wrong Xcode selected**: ensure worker `DEVELOPER_DIR` is stable/pinned.
- **Silent Xcode update**: prefer pinning by Xcode build number in worker capabilities + selection constraints.
- **Simulator mismatch**: pinned destination must exist on the worker (see `capabilities.json`).
- **Long first build**: warm SPM + DerivedData caches (see `cache.*` modes in config).

## Outputs
Artifacts are written to:
`~/.local/share/rch/artifacts/xcode/<run_id>/`

Layout (example):
- `run_index.json`
- `run_summary.json`
- `run_plan.json`
- `run_state.json`
- `worker_selection.json`
- `capabilities.json`
- `steps/build/<job_id>/job_index.json`
- `steps/build/<job_id>/...`
- `steps/test/<job_id>/job_index.json`
- `steps/test/<job_id>/...`

All JSON artifacts follow normative schemas defined in `PLAN.md` (including `summary.json` which has a full schema table).

Includes:
- run_index.json
- run_summary.json
- run_plan.json
- run_state.json
- summary.json
- attestation.json
- manifest.json
- effective_config.json
- job_key_inputs.json
- job.json
- job_state.json
- invocation.json
- toolchain.json
- destination.json
- metrics.json
- source_manifest.json (run-scoped)
- worker_selection.json
- job_index.json (per job)
- events.jsonl (recommended)
- test_summary.json (recommended)
- build_summary.json (recommended)
- junit.xml (recommended, test jobs)
- build.log
- result.xcresult/

## Notes
- Designed as a build/test gate, not a full IDE replacement
- Safe-by-default: avoids intercepting setup or mutating commands
- Deterministic: runs produce a JobSpec (`job.json`) plus the exact `job_key_inputs.json` used to compute a stable `job_key` for caching/attestation
- Security posture: prefer a dedicated `rch` user; optionally use SSH forced-command; avoid signing/publishing workflows
- Integrity: host verifies `manifest.json` digests; attestation binds worker identity + artifact set
- Forward-compatible: consumers ignore unknown fields in artifacts with matching `schema_id` major version

### Hardening recommendations
- Keep the worker "CI-clean": no personal Apple ID sessions, no developer keychains, minimal credentials.
- Prefer an env allowlist for the executor (only pass through known-safe vars), and redact secrets from logs.
- Consider running the worker user with reduced permissions (no admin), and keep artifacts + caches on a dedicated volume.
- Use `authorized_keys` options to restrict the SSH key:
  ```
  command="/usr/local/bin/rch-worker xcode rpc",no-port-forwarding,no-agent-forwarding,no-pty ssh-ed25519 AAAA... rch@host
  ```
- Forced-command example for `rch-worker xcode rpc`:
  ```bash
  # /usr/local/bin/rch-worker-gate (set as forced command in authorized_keys)
  #!/bin/sh
  # Only allow the rch-worker xcode rpc entrypoint
  if [ "$SSH_ORIGINAL_COMMAND" != "rch-worker xcode rpc" ]; then
    echo '{"ok":false,"error":{"code":"FORBIDDEN","message":"only rch-worker xcode rpc allowed"}}'
    exit 1
  fi
  exec /usr/local/bin/rch-worker xcode rpc
  ```
