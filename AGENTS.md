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
- The job lifecycle is: created → staging → queued → running → collecting → uploading → terminal
- All JSON artifacts use RFC 8785 (JCS) for canonicalization
- `run_id = SHA-256("rch-xcode-lane/run_id/v1\n" || JCS(effective_config.inputs) || "\n" || source_tree_hash_hex)` — note: only `inputs` is hashed, not `resolved`. All SHA-256 computations use domain prefixes to prevent digest reuse.
- Worker communication should go through the harness protocol when available

## Current State

- **Spec phase**: PLAN.md is being refined via APR (automated iterative review with GPT)
- **No implementation yet**: Code has not been written; focus is on specification convergence
- **Target**: 75% stability score across APR rounds before moving to implementation

## File Organization

- `/docs` — Operational documentation
- `/scripts` — Utility scripts (CDP recovery, etc.)
- `/.apr` — APR workflow state (rounds, logs, config)

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd sync
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds

## Keep Moving (Between Beads)

When you finish a bead, DO NOT STOP. Immediately:

1. Reread AGENTS.md so it is fresh in your mind
2. Use `bv --robot-triage` or `bv --robot-next` to find the most impactful bead to work on next
3. Mark the bead as in-progress with `bd start <bead-id>`
4. Start coding on it immediately
5. Communicate what you are working on to fellow agents via Agent Mail
6. When done, do a self-review pass: carefully read all new code and modified code, fix anything you find
7. Commit, push, mark bead closed, then GO BACK TO STEP 1

Do NOT get stuck in "communication purgatory" where nothing gets done. Be proactive. If you are not sure what to do next, use bv to prioritise and pick the next bead you can usefully work on.

After context compaction: reread AGENTS.md immediately so your tools and workflow knowledge is restored.
