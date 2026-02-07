# AGENTS.md — RCH Xcode Lane

## Project Overview

RCH Xcode Lane is a remote build/test gate for Apple-platform projects. It routes safe, allowlisted Xcode commands to a macOS worker via XcodeBuildMCP (or xcodebuild fallback), producing deterministic, machine-readable artifacts.

## Document Hierarchy

| Document | Role |
|---|---|
| **PLAN.md** | Normative specification — the source of truth |
| **README.md** | Non-normative — mental model + quickstart |
| **IMPLEMENTATION.md** | Implementation notes and component breakdown |
| **AGENTS.md** | This file — agent guidelines |

If README.md conflicts with PLAN.md, PLAN.md wins.

## Architecture Summary

- **Host**: RCH daemon — classifies commands, selects workers, stages source, collects artifacts
- **Worker**: macOS machine (Mac mini) — Xcode + simulator environment, runs builds/tests
- **Worker harness** (`rch-xcode-worker`): JSON stdin/stdout protocol over SSH for probe/run operations
- **Backends**: XcodeBuildMCP (preferred), xcodebuild (fallback) — both emit normalized event streams

## Key Design Principles

1. **Determinism first** — pinned destinations, schema-versioned artifacts, source snapshot digests
2. **Safe by default** — false negatives preferred over false positives, signing off by default, refuse on uncertainty
3. **Machine consumers** — every artifact is JSON with `kind`, `schema_version`, `lane_version`
4. **Fail explicitly** — decision.json explains every interception/refusal, partial artifacts preserved on failure

## Agent Guidelines

### When modifying PLAN.md
- It is normative — changes here define the contract
- Every JSON artifact must include `schema_version`
- New config fields need defaults and must be backwards-compatible
- New artifacts must be added to the Required Artifacts listing AND manifest.json
- `effective_config.json` separates `inputs` (hashed for run_id) from `resolved` (execution-time details like UDIDs, paths)

### When modifying README.md
- Keep it non-normative — quickstart and mental model only
- Artifact listing should mirror PLAN.md but with brief comments
- Config examples should use pinned values (not `latest`)

### When implementing
- Start from PLAN.md, not README.md
- The job lifecycle is: created → queued → staging → running → terminal
- All JSON artifacts use RFC 8785 (JCS) for canonicalization
- `run_id = SHA-256(JCS(effective_config.inputs) || \n || source_tree_hash_hex)` — note: only `inputs` is hashed, not `resolved`
- Worker communication should go through the harness protocol when available

## Current State

- **Spec phase**: PLAN.md is being refined via APR (automated iterative review with GPT)
- **No implementation yet**: Code has not been written; focus is on specification convergence
- **Target**: 75% stability score across APR rounds before moving to implementation

## File Organization

- `/docs` — Operational documentation
- `/scripts` — Utility scripts (CDP recovery, etc.)
- `/.apr` — APR workflow state (rounds, logs, config)
