# APR Round 22 — Reconciliation

## Scope
Cross-document alignment check: PLAN.md ↔ IMPLEMENTATION.md ↔ README.md

## Findings

### 1. MVP milestone alignment (M0–M4): ✅ Aligned
IMPLEMENTATION.md milestones match PLAN.md definitions. M0–M4 scope is consistent.

### 2. Missing PLAN.md requirements in IMPLEMENTATION.md: Two minor gaps

**Gap A — Destination resolver not in component map or M2 scope.**
PLAN.md defines "Destination resolver" as pipeline stage 3 (resolves `OS=latest` against
worker capabilities, records simulator runtime identifiers). IMPLEMENTATION.md M2 mentions
worker selection constraints but doesn't explicitly cover destination resolution as a step.
This is critical for JobSpec determinism (job_key_inputs.destination must be fully resolved).

*Fix:* Add destination resolver to M2 scope and component map.

**Gap B — State artifacts (run_state.json, job_state.json) not scoped to a milestone.**
These are normative in PLAN.md (§Run + Job state machine) but IMPLEMENTATION.md doesn't
explicitly assign them to M2 or M4. They're implied by signal handling and artifact writer
but should be explicit.

*Fix:* Add state artifact emission to M2 (needed for signal handling and resumption).

### 3. README accuracy: ✅ Accurate
README correctly reflects pre-implementation status, doc hierarchy, config precedence,
CLI surface, and safety model. No contradictions found.

### 4. Other contradictions/gaps: None found
- Post-MVP items (M5–M7) correctly deferred
- Error handling and failure taxonomy consistent
- Testing strategy aligns with PLAN.md conformance requirements

### 5. Realism: ✅ Reasonable
MVP scope (M0–M4) is achievable. The heaviest milestone (M2) bundles transport, bundling,
run builder, and execution but the component decomposition is clear.

## Patches applied
- IMPLEMENTATION.md: Added destination resolver to M2 scope + component map; added state artifacts to M2.
