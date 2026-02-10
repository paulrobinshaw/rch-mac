# RCH Xcode Lane

> **Normative spec:** `PLAN.md` is the contract.  
> This README is **non-normative**: quickstart, operator notes, and examples.

## What it is
An extension to Remote Compilation Helper (RCH) that offloads Xcode build/test to a remote Mac mini using XcodeBuildMCP.

## Why
Agents running on Linux or busy Macs can still validate iOS/macOS projects under pinned Xcode conditions without local Xcode installs.

## Requirements
**macOS worker**
- Xcode installed
- SSH access
- rsync + zstd
- Node.js + XcodeBuildMCP (recommended)

**Host**
- RCH client + daemon
- SSH access to worker

## Setup
1. Add Mac mini to `~/.config/rch/workers.toml` with tags `macos,xcode`
2. Add `.rch/xcode.toml` to your repo
3. Start daemon: `rch daemon start`
4. Run: `rch xcode verify`

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
mode = "pinned"
value = "platform=iOS Simulator,name=iPhone 16,OS=latest"

[timeouts]
overall_seconds = 1800
idle_log_seconds = 300

[cache]
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

## Outputs
Artifacts are written to:
`~/.local/share/rch/artifacts/<job_id>/`

Includes:
- summary.json
- attestation.json
- manifest.json
- effective_config.json
- job.json
- invocation.json
- toolchain.json
- metrics.json
- build.log
- result.xcresult/

## Notes
- Designed as a build/test gate, not a full IDE replacement
- Safe-by-default: avoids intercepting setup or mutating commands
- Deterministic: runs produce a JobSpec (`job.json`) and stable `job_key` used for caching and attestation
- Security posture: prefer a dedicated `rch` user; optionally use SSH forced-command; avoid signing/publishing workflows
