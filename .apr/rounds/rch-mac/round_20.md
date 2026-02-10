# APR Round 20 — FINAL (rch-mac)

**Reviewer:** Claude (spec reviewer)
**Date:** 2026-02-10
**Status:** ✅ CLEAN — no issues found

## Sweep summary

Final review of PLAN.md (1117 lines) and README.md after 19 rounds of iterative refinement. Round 19 found zero issues. This round confirms the spec is ready for implementation.

**No bugs, ambiguities, or contradictions found.**

The spec is internally consistent across all normative sections:
- State machines, exit codes, and status values align throughout
- Schema tables match their prose descriptions
- Protocol versioning (probe bootstrap with v0, negotiation, resumption verification) is coherent
- Binary framing discriminator, fetch envelope bypass, and upload atomicity are well-specified
- Classifier rejection semantics (null job_key, no job_state.json, summary.json emitted) are consistent
- Run-level exit code aggregation priority (rejected > cancelled > failed > success) is unambiguous
- Manifest inclusion/exclusion rules and verification steps are complete
- Cache materialization rewrite rules and profile-aware reuse are clear
- README accurately reflects PLAN.md without introducing normative claims

## Convergence summary

| Metric | Value |
|--------|-------|
| PLAN.md start (round 0) | 31 lines |
| PLAN.md final (round 20) | 1117 lines |
| Growth factor | ~36x |
| Total rounds | 20 (19 with changes + 2 clean passes) |
| Rounds with patches | 18 (rounds 1–18) |
| Clean rounds | 2 (rounds 19, 20) |

### Major areas covered

1. **Terminology & identifiers** — run/job/key semantics, ID formats, timestamps, clock skew
2. **CLI surface** — verify, run, explain, dry-run, tail, cancel, artifacts, doctor, gc
3. **Pipeline architecture** — classifier → run builder → destination resolver → JobSpec → transport → executor → artifacts
4. **Dual backends** — xcodebuild (MVP) + XcodeBuildMCP (preferred), unified artifact contract
5. **Host↔Worker protocol** — versioned RPC envelope, probe bootstrap, binary framing, error codes, feature negotiation
6. **Worker RPC ops** — probe, reserve, release, submit, status, tail, cancel, fetch, has_source, upload_source
7. **Job lifecycle** — idempotency, state machines (run + job), cancellation, resumption, orphan recovery
8. **Source bundling** — canonical tar, deterministic sha256, bundle modes, size enforcement, TOCTOU handling
9. **Artifact system** — 20+ artifact types, schemas, indices, manifest verification, atomicity (two-phase commit)
10. **Caching** — DerivedData modes, SPM cache, result cache, namespace isolation, keying, locking, GC
11. **Security** — classifier deny-by-default, env allowlist, SSH hardening, attestation signing
12. **Configuration** — 4-layer merge, effective_config audit, schema versioning/compatibility
13. **Failure taxonomy** — 13 exit codes, failure_kind/subkind, run-level aggregation
14. **Conformance testing** — classifier, determinism, schema validation, protocol replay, mock worker
15. **Operability** — worker selection, leases, concurrent runs, signal handling, artifact retention

### Verdict

**This spec is ready for implementation.** It is comprehensive, internally consistent, and has stabilized across two consecutive clean review passes. The normative sections provide clear contracts for implementers; the recommended sections offer sensible guidance without over-constraining.
