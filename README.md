# RCH Xcode Lane

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

## Outputs
Artifacts are written to:
`~/.local/share/rch/artifacts/<job_id>/`

Includes:
- summary.json
- attestation.json
- manifest.json
- build.log
- result.xcresult/

## Notes
- Designed as a build/test gate, not a full IDE replacement
- Safe-by-default: avoids intercepting setup or mutating commands
