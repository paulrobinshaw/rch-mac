# RCH Xcode Lane — Specification

> **This document is normative.** It defines the contract for the RCH Xcode Lane: configuration semantics, job lifecycle, artifacts, safety rules, and performance requirements. If README.md conflicts with this document, this document wins.

## Vision

Extend Remote Compilation Helper (RCH) so Xcode build/test commands are routed to a remote macOS worker (Mac mini) using XcodeBuildMCP, producing deterministic, machine-readable results.

## Goals

- Remote Xcode build/test via macOS workers only
- Deterministic configuration and attestation
- Agent-friendly JSON outputs and artifacts
- Safe-by-default interception (false negatives preferred)

---

## Terms

- **Host**: Machine running `rch` client + daemon.
- **Worker**: macOS machine running jobs over SSH.
- **Job**: One remote build/test execution with stable, addressable artifacts.
- **Profile**: Named configuration block (e.g., `ci`, `local`, `release`).
- **Lane**: The Xcode-specific subsystem within RCH.

---

## Core Architecture

### Host (RCH daemon)

- Classifies commands (strict allowlist)
- Selects an eligible worker (tags: `macos,xcode`)
- Syncs workspace snapshot (rsync + excludes)
- Executes via backend (XcodeBuildMCP preferred)
- Streams logs; assembles artifacts; emits manifest + attestation

### Worker (macOS)

- Provides a stable Xcode + simulator environment
- Maintains caches (DerivedData, SPM) keyed by effective config
- Returns xcresult + logs + tool metadata

---

## Configuration Model

### Repo Config: `.rch/xcode.toml`

Configuration uses **named profiles** (`[profiles.<name>]`). Select a profile with `--profile <name>`.

```toml
# Example: .rch/xcode.toml
[profiles.ci]
action = "test"                        # "build" | "test"
workspace = "MyApp.xcworkspace"        # or project = "MyApp.xcodeproj"
scheme = "MyApp"
configuration = "Debug"
timeout_seconds = 1800

[profiles.ci.destination]
platform = "iOS Simulator"
name = "iPhone 16"
os = "latest"                          # or pinned: "18.2"

[profiles.ci.xcode]
path = "/Applications/Xcode.app"       # optional; uses worker default if omitted

[profiles.ci.cache]
derived_data = true
spm = true

[profiles.ci.safety]
allow_mutating = false                 # disallow implicit clean/archive
code_signing_allowed = false           # CODE_SIGNING_ALLOWED=NO

[profiles.release]
action = "build"
workspace = "MyApp.xcworkspace"
scheme = "MyApp"
configuration = "Release"
timeout_seconds = 3600

[profiles.release.safety]
allow_mutating = true
code_signing_allowed = true
```

### Host Config: `~/.config/rch/workers.toml`

```toml
[[workers]]
name = "mac-mini-1"
host = "mac-mini-1.local"
tags = ["macos", "xcode"]
ssh_user = "rch"
ssh_key = "~/.ssh/rch_ed25519"
```

### Resolution Rules

- Effective config MUST be resolved deterministically (profile defaults + CLI overrides).
- Destination + Xcode identity SHOULD be pinned for CI profiles.
- If a profile is not specified, lane MUST refuse to run (no implicit defaults).

---

## Job Lifecycle

### States

```
1. created → 2. staging → 3. running → 4. terminal
                                         ├── succeeded
                                         ├── failed
                                         ├── canceled
                                         └── timed_out
```

### Requirements

- Host MUST preserve partial artifacts for non-success terminals.
- Terminal state MUST be recorded in `summary.json`.

### Timeouts + Cancellation

- Lane MUST support a per-job timeout (`timeout_seconds`).
- If a timeout triggers, terminal state MUST be `timed_out`.
- Lane SHOULD support user cancellation (best-effort SIGINT/remote termination).
- A canceled job MUST still emit `summary.json` + `manifest.json` referencing available artifacts.

---

## Safety Rules

### Interception Policy

- Lane MUST NOT run mutating commands by implicit interception (e.g., `clean`, `archive`) unless `allow_mutating = true`.
- When classification is uncertain, lane MUST prefer **not** to intercept (false negatives are acceptable).
- Default signing policy: `CODE_SIGNING_ALLOWED=NO` unless explicitly enabled.

### Threat Model

- Repositories may contain build scripts/plugins that execute arbitrary code during build/test.
- Lane does not attempt to sandbox Xcode beyond best-effort OS/user isolation.
- Operators SHOULD deploy workers as dedicated, non-sensitive machines/accounts.
- Operators SHOULD use a dedicated macOS user account with minimal privileges for RCH runs.

---

## Determinism Contract

Every run MUST emit:

- `effective_config.json` — Fully-resolved configuration used for the job (Xcode path, destination, build settings, cache policy).
- `attestation.json` — Environment fingerprint: macOS version, Xcode version/build, toolchain versions, worker identity, repo state.
- `manifest.json` — Artifact inventory + hashes.

Determinism inputs SHOULD include:

- Explicit Xcode selection (path or version constraint)
- Explicit destination resolution strategy (simulator UDIDs over human names)
- Explicit signing policy (default off)

---

## Required Artifacts

Artifacts are written under an immutable job directory on the host:

```
~/.local/share/rch/artifacts/<job_id>/
├── summary.json           # Machine-readable outcome + timings
├── effective_config.json  # Fully-resolved config used for the job
├── attestation.json       # Toolchain + environment fingerprint
├── manifest.json          # Artifact index + SHA-256 hashes + byte sizes
├── environment.json       # Captured worker environment snapshot
├── timing.json            # Durations for stage/run/collect phases
├── build.log              # stdout/stderr capture
└── result.xcresult/       # When tests run and xcresult is produced
```

### `manifest.json` Requirements

- MUST include SHA-256 hashes for all material artifacts.
- MUST include byte sizes.
- SHOULD include a logical artifact type for each entry (log/json/xcresult/etc.).

### `environment.json` Requirements

- MUST include Xcode path + version + build number.
- MUST include macOS version.
- SHOULD include available simulator runtimes.
- SHOULD include Node.js version (if XcodeBuildMCP used).

### `timing.json` Requirements

- MUST include durations (seconds) for: `staging`, `running`, `collecting`.
- SHOULD include total wall-clock time.

---

## Commands

| Command | Purpose |
|---------|---------|
| `rch xcode doctor` | Validate host setup (daemon, config, SSH tooling) |
| `rch xcode verify [--profile <name>]` | Probe worker + validate config against capabilities |
| `rch xcode build [--profile <name>]` | Remote build gate |
| `rch xcode test [--profile <name>]` | Remote test gate |
| `rch xcode fetch <job_id>` | Pull artifacts (if stored remotely) |

### `doctor` Checks (Host)

- RCH daemon running
- SSH tooling available
- Config parseable
- Workers reachable

### `verify` Checks (Worker)

- Worker reachable via SSH
- Xcode installed at expected path (or discoverable)
- Requested destination available (simulator runtime + device)
- XcodeBuildMCP available (if configured as backend)
- Node.js version compatible

---

## Performance Design

### Incremental Staging

- Use rsync with excludes (`.git`, `DerivedData`, `*.xcresult`, etc.).
- Optional: git archive/clone strategy for pristine working copies.

### Cache Buckets

| Cache | Key Components |
|-------|----------------|
| DerivedData | Xcode identity + destination + repo content hash |
| SwiftPM | Xcode identity + resolved dependencies hash |

### Simulator Prewarm

- Boot simulator once, reuse across runs.
- Collect runtime info in `environment.json`.
- Avoid cold-start tax on every job.

### Concurrency Control

- Avoid thrashing by limiting concurrent jobs per worker.
- Queue jobs if worker is busy.
- Report queue position in job status.

---

## Milestones

- **M0**: macOS worker reachable via SSH, tagged `macos,xcode`
- **M1**: `doctor` + worker capability probe + config validation (`verify`)
- **M2**: Classifier + policy: allowlist routing, refuse-on-uncertain, explain decisions
- **M3**: Workspace sync + remote runner MVP (fallback `xcodebuild`) with log streaming
- **M4**: XcodeBuildMCP backend with structured events (build phases, test events)
- **M5**: Determinism outputs: summary.json, effective_config.json, attestation.json, manifest.json, environment.json, timing.json
- **M6**: Caching + performance: DerivedData/SPM caches, incremental sync, simulator prewarm, concurrency control
- **M7**: Ops hardening: timeouts, cancellation, worker leases, partial artifact preservation
- **M8**: Compatibility matrix + fixtures: golden configs, sample repos, reproducible failure cases

---

## Policies Summary

| Policy | Default | Override |
|--------|---------|----------|
| Code signing | `CODE_SIGNING_ALLOWED=NO` | `code_signing_allowed = true` |
| Mutating commands | Disallowed | `allow_mutating = true` |
| Uncertain classification | Refuse (false negative) | N/A |
| Worker user | Dedicated account | Operator responsibility |

---

## Next Steps

1. Bring Mac mini worker online
2. Implement `rch xcode doctor` and `rch xcode verify`
3. Add classifier + routing + refusal/explanation paths
4. Implement workspace sync + remote runner + log streaming
5. Add XcodeBuildMCP backend
6. Emit determinism artifacts
7. Add caching + performance optimizations
