# RCH Xcode Lane

> **Normative spec:** `PLAN.md` is the source of truth for the lane's contract, artifacts, and safety rules.
> This README is intentionally **non-normative**: mental model + quickstart.

## What it is

**RCH Xcode Lane** is a *remote build/test gate* for Apple-platform projects.
It extends **Remote Compilation Helper (RCH)** to route safe, allowlisted Xcode build/test commands
to a remote **macOS worker** (e.g., a Mac mini) via a **stable worker harness**: `rch-xcode-worker`.
The harness may use **XcodeBuildMCP** (preferred) or a fallback `xcodebuild` backend, but the host always speaks one protocol and always receives structured NDJSON events.

## Why

Agents running on Linux (or saturated Macs) can still verify iOS/macOS projects under a **pinned Xcode + Simulator**
configuration—without installing Xcode locally—while receiving **machine-readable, auditable outputs**.

## How it works

```
┌──────────────┐      SSH (+ pinned host key)      ┌───────────────────┐
│ Host (RCH)   │ ──────────────────────────────▶   │ macOS Worker      │
│ - classify   │                                   │ - Xcode + Sims    │
│ - resolve    │   stage (rsync/git snapshot)      │ - caches          │
│ - attest     │ ◀──────────────────────────────   │ - rch-xcode-worker│
│ - assemble   │   NDJSON events + artifacts       │   (probe/run)     │
└──────────────┘                                   └───────────────────┘
```

1. **Select worker** (tagged `macos,xcode`) and probe capabilities (Xcode, runtimes, XcodeBuildMCP).
2. **Snapshot + stage source** to the worker (rsync working tree, or git snapshot depending on profile policy).
3. **Run** build/test remotely by invoking `rch-xcode-worker run` (harness selects allowed backend; emits NDJSON events).
4. **Collect artifacts** (logs, `xcresult`, structured JSON).
5. **Attest** toolchain + environment; emit machine-readable outputs for CI/agents.

## Non-goals

- Not a remote IDE and not a general "run anything on the Mac" executor
- Not a provisioning/signing manager (signing is **off by default**)
- Not a replacement for full CI; this is a deterministic *gate* optimized for agent workflows

## Safety / Security Model

Xcode builds can execute project-defined scripts and plugins. Treat the worker as executing **potentially untrusted code** from the repository under test.

Recommended:
- Dedicated macOS user account for RCH runs
- Dedicated machine (or at least dedicated environment) for lane execution
- Harden SSH on the worker:
  - Prefer a forced-command key for `rch-xcode-worker --forced`
    (harness reads `SSH_ORIGINAL_COMMAND` and only allows `probe`/`run`;
    no-pty, no-agent-forwarding, no-port-forwarding)
  - Use a separate, restricted rsync key confined to the workspace/staging root (e.g., `rrsync`)
- Keep `allow_mutating = false` unless you explicitly need `clean`/`archive`-like behavior
- Pin worker SSH host keys (or at minimum record host key fingerprints in attestation)
- CI profiles SHOULD require pinning (lane can refuse if worker has no configured fingerprint)

See `PLAN.md` § Safety Rules for the full threat model.

## Requirements

### macOS worker

- Xcode installed (pinned version; lane records Xcode build number)
- SSH access (key-based)
- `rsync` + `zstd` (fast sync + compression)
- Node.js + XcodeBuildMCP (recommended backend)
- `rch-xcode-worker` harness (recommended): stable remote probe/run/collect interface

### Host

- RCH client + daemon
- SSH access to the worker

## Commands

| Command | Purpose |
|---------|---------|
| `rch xcode doctor` | Validate host setup (daemon, config, SSH tooling) |
| `rch xcode workers [--refresh]` | List/probe eligible macOS workers and summarize capabilities |
| `rch xcode verify [--profile <name>]` | Probe worker + validate config against capabilities |
| `rch xcode plan --profile <name>` | Resolve effective config + select worker + resolve destination (no staging/run) |
| `rch xcode build [--profile <name>]` | Remote build gate |
| `rch xcode test [--profile <name>]` | Remote test gate |
| `rch xcode fetch <job_id>` | Pull artifacts (if stored remotely) |
| `rch xcode validate <job_id\|path>` | Verify artifacts: schema validation + manifest hashes + event stream integrity |
| `rch xcode watch <job_id>` | Stream structured events + follow logs for a running job |
| `rch xcode cancel <job_id>` | Best-effort cancel (preserves partial artifacts + terminal summary) |
| `rch xcode gc` | Garbage-collect old job dirs + worker workspaces (retention policy) |

## Setup

1. Register the Mac mini in `~/.config/rch/workers.toml` with tags `macos,xcode`
2. Add repo config at `.rch/xcode.toml` (see example below)
3. Start the daemon: `rch daemon start`
4. Check setup: `rch xcode doctor`
5. Validate config: `rch xcode verify --profile ci`
6. Run a gate: `rch xcode test --profile ci`

## Minimal `.rch/xcode.toml`

```toml
[profiles.ci]
action = "test"
workspace = "MyApp.xcworkspace"
scheme = "MyApp"
configuration = "Debug"
timeout_seconds = 1800

[profiles.ci.destination]
platform = "iOS Simulator"
name = "iPhone 16"
os = "18.2"  # CI SHOULD pin; floating "latest" is opt-in (see PLAN.md)

[profiles.ci.safety]
allow_mutating = false
code_signing_allowed = false

[profiles.ci.trust]
require_pinned_host_key = true
```

## Outputs

Artifacts are written to:
- Canonical per-attempt dir: `~/.local/share/rch/artifacts/jobs/<job_id>/`
- Stable run index: `~/.local/share/rch/artifacts/runs/<run_id>/attempt-<n>/` (links/pointers to job dirs)

All JSON artifacts are **versioned** (`schema_version`) and self-describing (`kind`, `lane_version`).
`summary.json` and `decision.json` include stable `error_code`/`errors[]` fields so CI/agents can react without log scraping.
Consumers SHOULD validate against schemas in `schemas/rch-xcode-lane/` (recommended for CI/agents).

```
<job_id>/
├── summary.json           # High-level status + timings (includes job_id, run_id, attempt, error_code)
├── effective_config.json  # Resolved/pinned run configuration
├── decision.json          # Interception/classification decision + refusal reasons (stable error codes)
├── attestation.json       # Worker identity, tool versions, repo state
├── source_manifest.json   # Canonical file list + per-entry hashes used to compute source_tree_hash
├── manifest.json          # Artifact listing + SHA-256 hashes
├── environment.json       # Worker environment snapshot
├── timing.json            # Phase durations (staging/running/collecting)
├── metrics.json           # Resource + transfer metrics (cpu/mem/disk/bytes, queue stats)
├── status.json            # Latest job state snapshot (atomic updates while running)
├── events.ndjson          # Structured event stream (append-only)
├── build.log              # Captured harness stderr (human logs + backend output)
├── result.xcresult/       # When tests are executed (or `result.xcresult.tar.zst` when compression enabled)
└── provenance/            # Optional: signatures + verification report
```

## Operational Notes

- Recommended: dedicate a worker user account with minimal privileges
- Prefer `CODE_SIGNING_ALLOWED=NO` unless explicitly enabled in config
- Use worker concurrency limits + leases to avoid simulator contention
- Prefer per-job simulator hygiene for flaky UI tests (erase/create policies)
- CI profiles SHOULD pin destination runtime/device; floating resolution is opt-in
- Deterministic IDs: `run_id` is content-derived; `job_id` is per-attempt. Timestamps live in `summary.json`, not config.
- Failure modes are first-class: timeouts/cancellation preserve partial artifacts
- Transient errors (lease_expired, worker_unreachable) can be auto-retried via retry policy

## Next

- Read the contract: `PLAN.md`
- Add a minimal `.rch/xcode.toml` and run `rch xcode verify --profile ci`
