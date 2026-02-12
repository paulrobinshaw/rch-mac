# RCH Xcode Lane

> **Normative spec:** `PLAN.md` is the contract.
> This README is **non-normative**: quickstart, operator notes, and examples.

## Status

**Active implementation.** The core pipeline is functional with ~27k lines of Rust across 63 source files.

**Implemented:**
- M0: Worker inventory, SSH connectivity, `rch workers list/probe`
- M1: Classifier with deny-by-default gate, `rch explain`
- M1.5: Mock worker with protocol conformance (probe/submit/status/tail/cancel/has_source/upload_source)
- M2 (partial): Source bundling, run pipeline, artifact collection, state machine
- CLI: `explain`, `verify`, `run`, `tail`, `cancel`, `artifacts`, `doctor`, `fetch`, `workers`

**In progress:** M2 completion (remote execution with xcodebuild backend)

Build and test:
```bash
cargo build
cargo test   # 100+ tests passing
```

## Docs map
- `IMPLEMENTATION.md` — **practical build plan** (MVP scope, component map, milestone order)
- `PLAN.md` — **normative** contract (classifier, JobSpec, protocol, artifacts, caching, security)
- `.rch/xcode.toml` — repo-scoped configuration (checked in)
- `~/.config/rch/workers.toml` — host-scoped worker inventory + credentials
- `docs/worker-ssh-setup.md` — SSH hardening guide for workers

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

## How it fits into RCH (lane boundaries)
RCH is a multi-lane system. Each lane owns a specific kind of remote work. The **Xcode lane** owns:
- Remote `xcodebuild build` and `xcodebuild test` on macOS workers.
- Classifier, JobSpec, transport, artifact collection for those actions.

It does **not** own: code signing, publishing, non-Xcode builds, or arbitrary remote commands. Other lanes (future) may handle those.

## CLI commands

```bash
# Explain why a command will/won't be intercepted
rch-xcode explain -- xcodebuild build -workspace MyApp.xcworkspace -scheme MyApp
rch-xcode explain --human -- xcodebuild test -scheme MyApp

# List available workers
rch-xcode workers list
rch-xcode workers list --tag macos,xcode

# Probe a worker for capabilities
rch-xcode workers probe macmini-01
rch-xcode workers probe macmini-01 --json --save

# Validate configuration and connectivity
rch-xcode doctor --worker macmini-01

# Run the repo-defined verify lane (build+test)
rch-xcode run
rch-xcode run --dry-run
rch-xcode run --action test

# Stream logs from a running job
rch-xcode tail <run_id|job_id>

# Cancel a running job
rch-xcode cancel <run_id|job_id>

# Show artifact paths
rch-xcode artifacts <run_id|job_id>

# Fetch remote artifacts
rch-xcode fetch <job_id> --worker macmini-01
```

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
- zstd (for source bundle compression)
- Node.js + XcodeBuildMCP (recommended for `backend="mcp"`)
- (Recommended) dedicated `rch` user + constrained SSH key/forced-command
  - Recommended: forced-command runs a single `rch-worker xcode ...` entrypoint (no shell)

**Host**
- Rust toolchain (to build)
- SSH access to worker

## Setup
1. Build: `cargo build --release`
2. Add Mac mini to `~/.config/rch/workers.toml` with tags `macos,xcode`
3. Add `.rch/xcode.toml` to your repo
4. Run: `./target/release/rch-xcode run`

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
workspace = "MyApp.xcworkspace"
schemes = ["MyApp"]
configurations = ["Debug", "Release"]

[[verify]]
action = "build"
configuration = "Debug"

[[verify]]
action = "test"
```

## Safety model
Intercept is **deny-by-default**:
- Allowed: `xcodebuild build`, `xcodebuild test` (within configured workspace/project + scheme)
- Denied: archive/export, notarization, signing/export workflows, arbitrary scripts, "mutating setup"

Flags:
- **Allowed:** `-workspace`, `-project`, `-scheme`, `-destination`, `-configuration`, `-sdk`, `-quiet`
- **Denied:** `-exportArchive`, `-exportNotarizedApp`, `-resultBundlePath`, `-derivedDataPath`, `-archivePath`, `-exportPath`, `-exportOptionsPlist`

## Trust boundary (important)
`rch xcode` is **not** a sandbox. If your Xcode project contains Run Script build phases,
SwiftPM build tool plugins, or other build-time code execution, that code will run on the worker
under the `rch` user.
Treat the worker like CI: dedicated account, minimal secrets, and no personal keychains.

## Architecture

```
Host (Linux/macOS)                          Worker (macOS)
+-----------------------+                   +-----------------------+
| rch-xcode CLI         |                   | rch-worker xcode rpc  |
|   +- Classifier       |   SSH + JSON RPC  |   +- Executor         |
|   +- Run builder      | <---------------> |   +- Artifact writer  |
|   +- Source bundler   |                   |   +- Cache manager    |
|   +- Artifact store   |                   +-----------------------+
+-----------------------+
```

## Source structure

```
src/
  artifact/       # Artifact loading and schema validation
  bundle/         # Deterministic source bundling
  cancel/         # Cancellation handling
  classifier/     # Deny-by-default command classifier
  config/         # Configuration loading and merging
  conformance/    # Protocol conformance tests
  destination/    # Destination resolution
  host/           # Host-side RPC client
  inventory/      # Worker inventory management
  job/            # Job creation and state
  mock/           # Mock worker for testing
  protocol/       # RPC envelope and errors
  run/            # Run pipeline and resumption
  selection/      # Worker selection logic
  signal/         # Signal handling
  state/          # Run/job state machines
  summary/        # Summary artifact generation
  toolchain/      # Toolchain resolution
  worker/         # Worker capabilities and probe
  lib.rs          # Library exports
  main.rs         # CLI entry point
  pipeline.rs     # Main execution pipeline
  timeout.rs      # Timeout handling
```

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

## Common pitfalls
- **Wrong Xcode selected**: ensure worker `DEVELOPER_DIR` is stable/pinned.
- **Silent Xcode update**: prefer pinning by Xcode build number in worker capabilities + selection constraints.
- **Simulator mismatch**: pinned destination must exist on the worker (see `capabilities.json`).
- **Long first build**: warm SPM + DerivedData caches (see `cache.*` modes in config).

## Hardening recommendations
- Keep the worker "CI-clean": no personal Apple ID sessions, no developer keychains, minimal credentials.
- Prefer an env allowlist for the executor (only pass through known-safe vars), and redact secrets from logs.
- Consider running the worker user with reduced permissions (no admin), and keep artifacts + caches on a dedicated volume.
- Use `authorized_keys` options to restrict the SSH key:
  ```
  command="/usr/local/bin/rch-worker xcode rpc",no-port-forwarding,no-agent-forwarding,no-pty ssh-ed25519 AAAA... rch@host
  ```

See `docs/worker-ssh-setup.md` for detailed SSH hardening instructions.
