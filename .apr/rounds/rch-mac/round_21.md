# APR Round 21 — Reconciliation: Spec ↔ Implementation Alignment

## Premise
PLAN.md grew from 31→1,117 lines over 20 rounds without reference to IMPLEMENTATION.md. This round reconciles all three documents.

## Finding 1: IMPLEMENTATION.md is dangerously stale
IMPLEMENTATION.md is ~40 lines written before any spec maturation. It contradicts PLAN.md in several places and omits nearly everything the spec defines. An implementer following IMPLEMENTATION.md alone would build the wrong thing.

## Finding 2: Direct contradictions

| IMPLEMENTATION.md says | PLAN.md says | Resolution |
|---|---|---|
| "RCH-E600 range" error codes | Named `failure_kind` + stable exit codes 0-93 | Use PLAN.md's taxonomy. Drop E600. |
| "Fail-open only when local Xcode is available" | Deny-by-default, no local fallback concept | Drop fail-open. RCH is a remote gate; local Xcode fallback is out of scope. |
| XcodeBuildMCP primary, xcodebuild "fallback" | Two separate backends, no fallback relationship | Use PLAN.md framing: xcodebuild is MVP backend, MCP is preferred/richer. No auto-fallback. |
| "Sync repo to stable remote workspace" | Canonical source bundling with deterministic tar + SHA-256 | Use PLAN.md's source bundling. "Sync" is too vague. |
| "engine selection" | "backend" | Use "backend" consistently. |
| "Preserve DerivedData remotely" | Cache modes: off / per_job / shared with keying | Use PLAN.md's cache model. |

## Finding 3: PLAN.md over-engineering for MVP

These sections are well-designed but gratuitously complex for a first working version:

1. **Worker leases** (reserve/release/renew) — MVP has one worker; leases add complexity with no benefit until multi-worker contention exists.
2. **Resumable uploads** (`upload_resumable` feature) — premature optimization; bundles are typically <100MB.
3. **Attestation signing + key pinning** — the unsigned attestation is sufficient for MVP. Signing can be added later without breaking changes.
4. **Run resumption protocol** (§Run resumption) — 6-step recovery protocol is important but can be simplified to "re-run" for MVP.
5. **Concurrent host runs** — MVP: one run at a time is fine.
6. **Artifact profiles** (minimal/rich) — MVP should just produce all artifacts it can; profiling is premature.
7. **Result cache with profile-aware reuse** — result caching is a performance optimization, not MVP.
8. **Ephemeral simulator provisioning** — "existing" simulators are fine for MVP.

**Recommendation:** Keep these in PLAN.md (they're well-specified for future milestones) but mark MVP vs post-MVP clearly. IMPLEMENTATION.md should only reference what's actually being built first.

## Finding 4: Things in IMPLEMENTATION.md not in spec
- "RCH-E600 range" — no error code numbering scheme in PLAN.md (and shouldn't be; the stable exit codes are better)
- "Fail-open when local Xcode is available" — interesting concept but contradicts the lane's purpose as a remote gate

## Finding 5: Spec is solid but needs MVP scoping
PLAN.md reads like a complete V1.0 spec, but there's no indication of what's MVP vs what's future. The milestones (M0-M7) help but don't map to spec sections. Adding a "MVP scope" annotation would help implementers enormously.

## Patches applied

### PLAN.md
- Added MVP scope markers to milestones section
- Added a "Scope annotations" note clarifying normative vs post-MVP sections

### README.md
- Added status note about implementation alignment
- Minor: noted that IMPLEMENTATION.md exists for practical build plan

### IMPLEMENTATION.md
- **Complete rewrite** to align with mature PLAN.md while staying practical
- Organized by MVP milestones (M0-M4) vs post-MVP (M5-M7)
- Replaced contradictions (E600, fail-open, engine→backend, sync→bundle)
- Added concrete implementation notes per component
- Kept it concise (~150 lines) as the "what to build" complement to PLAN.md's "what it must do"
