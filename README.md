# RCH Xcode Lane

> **Normative spec:** `PLAN.md` is the source of truth for the lane's contract, artifacts, and safety rules.
> This README is intentionally **non-normative**: mental model + quickstart.

## What it is

**RCH Xcode Lane** is a *remote build/test gate* for Apple-platform projects.
It extends **Remote Compilation Helper (RCH)** to route safe, allowlisted Xcode build/test commands
to a remote **macOS worker** (e.g., a Mac mini) via a **stable worker harness**: `rch-xcode-worker`.

The host **never shells out to `xcodebuild` directly**. It speaks one protocol (`probe` / `run`) and always receives structured NDJSON events. The harness may use **XcodeBuildMCP** (preferred) or a fallback `xcodebuild` backend *internally*.

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

1. **Select worker** (tagged `macos,xcode`) and probe capabilities (**protocol + contract versions**, supported `verbs`, stable `capabilities_sha256`, Xcode, runtimes, backends).
2. **Snapshot + stage source** to the worker (rsync working tree, or git snapshot depending on profile policy).
3. **Run** build/test remotely by invoking `rch-xcode-worker run` (harness selects allowed backend; emits NDJSON events).
   - Production setups SHOULD use the harness in **forced-command mode** so SSH cannot run arbitrary commands.
4. **Collect artifacts** (logs, `xcresult`, structured JSON).
5. **Attest** toolchain + environment; emit machine-readable outputs for CI/agents.

## Versioning at a Glance (Non-normative)

There are four distinct version streams:
- **protocol_version**: host↔harness wire protocol for `probe/run/cancel/...` (negotiated).
- **contract_version**: meaning of `effective_config.inputs` (hashed); changing semantics MUST bump this.
- **lane_version**: implementation/spec version (may change without affecting hashes).
- **schema_version**: per-artifact JSON schema version (validators key off this).

If `protocol_version` or `contract_version` don't overlap between host and harness, the lane refuses deterministically.

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
  - Use separate, restricted data-plane keys (recommended):
    - **Stage key**: write-only, confined to the `stage_root/` (push source snapshot)
    - **Fetch key**: read-only, confined to the `jobs_root/` (pull artifacts back)
- Keep `allow_mutating = false` unless you explicitly need `clean`/`archive`-like behavior
- Prefer a **read-only staged source root** on the worker so project scripts/plugins cannot silently rewrite inputs
- Be explicit about symlink handling: in untrusted posture the lane SHOULD forbid symlinks (see `PLAN.md` `source.symlinks`)
- Pin worker SSH host keys (or at minimum record host key fingerprints in attestation)
- CI profiles SHOULD require pinning (lane can refuse if worker has no configured fingerprint)

See `PLAN.md` § Safety Rules for the full threat model.

Tip: For CI that tests fork PRs or otherwise untrusted sources, enable "untrusted posture" (`trust.posture = "untrusted"`) so the lane automatically disables signing, forces read-only caches, tightens simulator hygiene, and requires log redaction for remote artifact storage.

## Requirements

### macOS worker

- Xcode installed (pinned version; lane records Xcode build number)
- SSH access (key-based)
- `rsync` + `zstd` (fast sync + compression)
- Optional but recommended: `rrsync` (or equivalent) to confine stage/fetch keys to lane roots
- Node.js + XcodeBuildMCP (recommended backend)
- `rch-xcode-worker` harness (**required**): stable remote probe/run/collect interface
  - Strongly recommended: forced-command SSH key (`authorized_keys command=...`) running `rch-xcode-worker --forced`
  - MUST advertise compatible `protocol_versions` **and** `contract_versions` (`rch xcode verify` enforces)

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
| `rch xcode fetch <job_id>` | Pull remote artifacts (worker/object store), verify hashes, materialize locally |
| `rch xcode validate <job_id\|path>` | Verify artifacts: schema validation + manifest hashes + event stream integrity |
| `rch xcode watch <job_id>` | Stream structured events + follow logs for a running job |
| `rch xcode cancel <job_id>` | Best-effort cancel (preserves partial artifacts + terminal summary) |
| `rch xcode explain <job_id\|run_id\|path>` | Explain worker selection + pinning + refusal reasons (human + `--json`) |
| `rch xcode retry <job_id>` | Retry a failed job (increments attempt; keeps run_id if inputs/source unchanged) |
| `rch xcode reuse <run_id>` | Reuse an existing succeeded attempt for a run_id (optionally validates first) |
| `rch xcode gc` | Garbage-collect old job dirs + worker workspaces (retention policy) |
| `rch xcode warm [--profile <name>]` | Ask workers to prewarm simulator runtimes/caches for a profile |

Tip: Most commands support `--json` mode for agents/CI (see PLAN.md).

**Performance tip:** If you enable `worker.selection = "warm_cache"`, the host can optionally query workers for cache presence keyed by `config_hash` and route the job to the warmest eligible worker (see `PLAN.md` Worker Selection and Cache Query verb).

## Setup

1. Register the Mac mini in `~/.config/rch/workers.toml` with tags `macos,xcode`
2. Pin the worker SSH host key fingerprint (CI profiles SHOULD require this)
3. Install `rch-xcode-worker` on the worker and configure the **run key** as forced-command
4. Configure restricted data-plane keys (recommended):
   - **Stage key** (rrsync write-only) confined to `stage_root/`
   - **Fetch key** (rrsync read-only) confined to `jobs_root/`
5. Add repo config at `.rch/xcode.toml` (see example below)
6. Start the daemon: `rch daemon start`
7. Check setup: `rch xcode doctor`
8. Validate config: `rch xcode verify --profile ci`
9. Run a gate: `rch xcode test --profile ci`

## Minimal `.rch/xcode.toml`

```toml
[profiles.ci]
action = "test"
workspace = "MyApp.xcworkspace"
scheme = "MyApp"
configuration = "Debug"
timeout_seconds = 1800

# Larger repos MAY define a base profile and have others extend it (see PLAN.md).

[profiles.ci.destination]
platform = "iOS Simulator"
name = "iPhone 16"
os = "18.2"  # CI SHOULD pin; floating "latest" is opt-in (see PLAN.md)

# Strongly recommended for CI (more stable than human-friendly names):
# device_type_id = "com.apple.CoreSimulator.SimDeviceType.iPhone-16"
# runtime_id = "com.apple.CoreSimulator.SimRuntime.iOS-18-2"

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

**Event correlation:** `events.ndjson` lines include `job_id`, `run_id`, and `attempt` so tooling can safely correlate
streaming output to the run index. Deployments MAY also surface a `trace_id` for cross-system correlation
(CI logs ↔ host logs ↔ worker logs).

```
<job_id>/
├── probe.json             # Captured harness probe output (versioned: kind/schema_version) used for selection/verification
├── summary.json           # High-level status + timings (includes job_id, run_id, attempt, error_code)
├── job_request.json       # EXACT JSON sent to `rch-xcode-worker run` (sanitized; no secrets)
├── effective_config.json  # Resolved/pinned run configuration
├── decision.json          # Interception/classification decision + refusal reasons (stable error codes)
├── attestation.json       # Worker identity, tool versions, repo state
├── source_manifest.json   # Canonical file list + per-entry hashes used to compute source_tree_hash
├── manifest.json          # Artifact listing + SHA-256 hashes
├── environment.json       # Worker environment snapshot
├── timing.json            # Phase durations (staging/running/collecting)
├── staging.json           # Staging method + excludes + transfer stats (bytes/files/duration)
├── metrics.json           # Resource + transfer metrics (cpu/mem/disk/bytes, queue stats)
├── status.json            # Latest job state snapshot (atomic updates while running)
├── events.ndjson          # Structured event stream (append-only; optional hash chain for tamper-evident verification)
├── backend_invocation.json# Structured backend invocation record (safe, no secret values)
├── build.log              # Captured harness stderr (human logs + backend output)
├── redaction_report.json  # Optional: what redaction/truncation was applied (no secret values)
├── result.xcresult/       # When tests are executed (or `result.xcresult.tar.zst` when compression enabled)
└── provenance/            # Optional: signatures + verification report
```

## Operational Notes

- Recommended: dedicate a worker user account with minimal privileges
- Prefer `CODE_SIGNING_ALLOWED=NO` unless explicitly enabled in config
- Use worker concurrency limits + leases to avoid simulator contention
  - The harness is the source of truth for queueing (`queued`) and lease acquisition (`lease_acquired`)
  - `workers --refresh` can surface `probe.load` so selection strategies like `least_busy` are grounded
- Prefer per-job simulator hygiene for flaky UI tests (erase/create policies)
- CI profiles SHOULD pin destination runtime/device; floating resolution is opt-in
- Deterministic IDs: `run_id` is content-derived from `effective_config.inputs` (including `contract_version`) + `source_tree_hash`; `job_id` is per-attempt. Timestamps live in `summary.json`, not config.
- Failure modes are first-class: timeouts/cancellation preserve partial artifacts
- Transient errors (lease_expired, worker_unreachable) can be auto-retried via retry policy

## Next

- Read the contract: `PLAN.md`
- Add a minimal `.rch/xcode.toml` and run `rch xcode verify --profile ci`
