# IMPLEMENTATION — RCH Xcode Lane

## Overview
Introduce a new JobKind::Xcode into RCH to support remote Xcode build/test execution on macOS workers.

## Key components
- **Classifier**: Detect xcodebuild / xcodebuildmcp commands
- **Scheduler**: Route only to workers tagged `macos,xcode`
- **Executor**:
  - Primary: XcodeBuildMCP (CLI over SSH)
  - Fallback: xcodebuild
- **Artifacts**: xcresult, logs, summary, attestation

## Config
Project-level config in `.rch/xcode.toml`:
- scheme, workspace/project
- destination
- pinned Xcode (DEVELOPER_DIR)
- engine selection

## Remote execution
- Sync repo to stable remote workspace
- Preserve DerivedData remotely
- Stream logs back
- Sync artifacts only

## Error handling
- Dedicated error codes (RCH-E600 range)
- Fail-open only when local Xcode is available

## Testing
- Classifier unit tests
- Integration tests with mock SSH
- Optional true E2E on real Mac mini
