# APR Round 10 — rch-mac

**Reviewer:** Claude (spec reviewer)
**Date:** 2026-02-10
**Focus:** Underspecified artifact schemas, resumption semantics, operational gaps, cleanup contracts

---

## Change 1: Run resumption semantics

### Rationale
PLAN.md states the host "MUST be able to resume by reading `run_plan.json` and reusing the same `job_id`s" but never defines HOW resumption works: which jobs to skip, which to retry, how to handle partially-written artifacts from a crashed job. Without this, implementors will diverge on resumption behavior, defeating the determinism goal.

### Patch (PLAN.md)
After the `run_plan.json` paragraph in "Run plan artifact (normative)", append:

```
### Run resumption (normative)
On restart, the host MUST:
1. Read `run_plan.json` to recover the step list and `job_id`s.
2. For each step job, check whether `job_index.json` exists (the commit marker):
   - If present: treat the job as COMPLETE (success or failure per its `summary.json`).
   - If absent: query the worker via `status` using the original `job_id`.
     - If worker reports RUNNING: resume tailing/waiting.
     - If worker reports COMPLETE: fetch artifacts normally.
     - If worker reports unknown `job_id` or is unreachable: re-submit the job with the same `job_id`.
3. The host MUST NOT skip a failed job on resumption unless the run is already CANCELLED.
4. `run_state.json` MUST record `resumed_at` (RFC 3339 UTC) when a run is resumed.
```

---

## Change 2: Ephemeral simulator cleanup contract

### Rationale
PLAN.md defines `destination.provisioning="ephemeral"` where the worker "provisions a clean simulator per job" but never specifies cleanup. Leaked simulators accumulate and exhaust worker disk/resources. This is an operational time bomb.

### Patch (PLAN.md)
After the destination provisioning bullet in Architecture, append:

```
   - **Ephemeral cleanup (normative)**: when `provisioning="ephemeral"`, the worker MUST delete the provisioned simulator after artifact collection completes (or after cancellation). If cleanup fails, the worker MUST log a warning in `build.log` and record `ephemeral_cleanup_failed: true` in `summary.json`. Workers SHOULD implement a startup sweep that deletes any orphaned ephemeral simulators from previous crashed runs (identifiable by a naming convention, e.g. `rch-ephemeral-<job_id>`).
```

---

## Change 3: run_summary.json schema definition

### Rationale
`run_summary.json` is listed in Artifacts, referenced in "Run-level exit code", and pointed to by `run_index.json`, but its schema is never defined. This is a normative gap — implementors must guess its contents.

### Patch (PLAN.md)
Add a new section after "Run-level exit code (normative)":

```
### run_summary.json schema (normative)
`run_summary.json` MUST include at least:
| Field            | Type   | Description                                         |
|------------------|--------|-----------------------------------------------------|
| `schema_version` | int    | Always `1` for this version                         |
| `schema_id`      | string | `rch-xcode/run_summary@1`                           |
| `run_id`         | string | Run identifier                                      |
| `created_at`     | string | RFC 3339 UTC                                        |
| `status`         | string | `success` | `failed` | `cancelled`                  |
| `exit_code`      | int    | Aggregated per run-level exit code rules             |
| `step_count`     | int    | Total steps in the run                               |
| `steps_succeeded`| int    | Count of steps with `status=success`                 |
| `steps_failed`   | int    | Count of steps with `status=failed`                  |
| `steps_cancelled`| int    | Count of steps with `status=cancelled`               |
| `duration_ms`    | int    | Wall-clock duration of the entire run                |
| `human_summary`  | string | One-line human-readable summary                      |
```

---

## Change 4: metrics.json schema definition

### Rationale
`metrics.json` is referenced in Caching ("includes cache hit/miss + sizes + timing") and Cache directory layout ("record resolved cache paths") but has no schema. This is the only artifact referenced normatively with zero field definitions.

### Patch (PLAN.md)
Add after the "Locking + isolation" section:

```
### metrics.json schema (recommended)
`metrics.json` SHOULD include at least:
| Field               | Type   | Description                                          |
|---------------------|--------|------------------------------------------------------|
| `schema_version`    | int    | Always `1`                                           |
| `schema_id`         | string | `rch-xcode/metrics@1`                                |
| `run_id`            | string | Parent run identifier                                |
| `job_id`            | string | Job identifier                                       |
| `job_key`           | string | Job key                                              |
| `created_at`        | string | RFC 3339 UTC                                         |
| `timings`           | object | `{ bundle_ms, upload_ms, queue_ms, execute_ms, fetch_ms, total_ms }` |
| `cache`             | object | `{ derived_data_hit: bool, spm_hit: bool, result_cache_hit: bool }` |
| `cache_paths`       | object | Resolved cache directory paths used                  |
| `sizes`             | object | `{ source_bundle_bytes, artifact_bytes, xcresult_bytes }` |
| `cache_key_components` | object | Concrete key components used (job_key, xcode_build, etc.) |
```

---

## Change 5: destination.json and toolchain.json schemas

### Rationale
Both `destination.json` and `toolchain.json` appear in the Artifacts list and are referenced throughout (toolchain resolution records into `toolchain.json`; destination resolver records resolved destination). Neither has a schema definition. Cross-reference gap.

### Patch (PLAN.md)
Add after "Artifact profiles (recommended)":

```
### toolchain.json schema (normative)
`toolchain.json` MUST be emitted per job and MUST include:
| Field             | Type   | Description                                        |
|-------------------|--------|----------------------------------------------------|
| `schema_version`  | int    | Always `1`                                         |
| `schema_id`       | string | `rch-xcode/toolchain@1`                            |
| `run_id`          | string | Parent run identifier                              |
| `job_id`          | string | Job identifier                                     |
| `job_key`         | string | Job key                                            |
| `created_at`      | string | RFC 3339 UTC                                       |
| `xcode_version`   | string | e.g. `"16.2"`                                      |
| `xcode_build`     | string | e.g. `"16C5032a"`                                  |
| `developer_dir`   | string | Absolute path on worker                            |
| `macos_version`   | string | e.g. `"15.3.1"`                                    |
| `macos_build`     | string | e.g. `"24D60"`                                     |
| `arch`            | string | e.g. `"arm64"`                                     |

### destination.json schema (normative)
`destination.json` MUST be emitted per job and MUST include:
| Field             | Type   | Description                                        |
|-------------------|--------|----------------------------------------------------|
| `schema_version`  | int    | Always `1`                                         |
| `schema_id`       | string | `rch-xcode/destination@1`                          |
| `run_id`          | string | Parent run identifier                              |
| `job_id`          | string | Job identifier                                     |
| `job_key`         | string | Job key                                            |
| `created_at`      | string | RFC 3339 UTC                                       |
| `platform`        | string | e.g. `"iOS Simulator"`                             |
| `name`            | string | Device name when applicable                        |
| `os_version`      | string | Resolved concrete version                          |
| `provisioning`    | string | `"existing"` or `"ephemeral"`                      |
| `original_constraint` | string | The raw destination string from config          |
| `sim_runtime_identifier` | string | (Simulator only) Runtime identifier         |
| `sim_runtime_build` | string | (Simulator only) Runtime build string           |
| `device_type_identifier` | string | (Simulator only) Device type identifier     |
| `udid`            | string | (Optional) Actual device/simulator UDID used       |
```

---

## Change 6: Host-side artifact retention policy

### Rationale
Worker eviction is specified (size-based or age-based, normative). Host artifacts under `~/.local/share/rch/artifacts/xcode/` grow unbounded with no eviction contract. For long-running agents, this is an operational gap that will eventually fill the host disk.

### Patch (PLAN.md)
Add after "Eviction / garbage collection (normative)":

```
### Host artifact retention (recommended)
The host SHOULD implement artifact retention to bound disk usage under the local artifact root:
- Retention policy SHOULD support age-based and/or count-based limits (e.g. keep last N runs or last N days).
- The host MUST NOT delete artifacts for RUNNING runs.
- `rch xcode gc` (optional CLI) MAY trigger manual garbage collection.
- The host SHOULD log when artifacts are evicted and record the policy in `effective_config.json`.
```

### Patch (README.md)
In the "Useful commands" list, add:

```
- `rch xcode gc`                       (optional: clean old local artifacts)
```

---

## Change 7: Graceful shutdown and signal handling

### Rationale
No specification exists for what happens when the host process receives SIGTERM/SIGINT mid-run. Without this, implementations may leave runs in an indeterminate state with no artifacts, orphan workers, or corrupt state files.

### Patch (PLAN.md)
Add after "Cancellation":

```
### Host signal handling (normative)
On receiving SIGINT or SIGTERM, the host MUST:
1. Send `cancel` for all RUNNING jobs in the current run.
2. Wait up to a bounded grace period (recommended: 10 seconds) for cancellation acknowledgement.
3. Persist `run_state.json` with `state=CANCELLED` and `run_summary.json` with available results.
4. Exit with code 80 (`CANCELLED`).

On receiving a second SIGINT (double-interrupt), the host MAY exit immediately without waiting for cancellation acknowledgement, but MUST still persist `run_state.json`.
```

---

## Change 8: Probe failure handling during worker selection

### Rationale
Worker selection says "Probe or load cached `capabilities.json` snapshots" but doesn't specify what happens if a probe fails (timeout, SSH error, protocol mismatch) for some but not all workers. Without this, a transient network issue could silently exclude the preferred worker.

### Patch (PLAN.md)
In the "Selection algorithm (default)" section, after step 2, add:

```
   - If a probe fails (timeout, connection error, or protocol error), the host MUST exclude that worker
     from the candidate set for this run and record `{ worker, probe_error, probe_duration_ms }` in
     `worker_selection.json` under a `probe_failures[]` array.
   - If ALL probes fail, the host MUST fail with `failure_kind=SSH` and `human_summary` listing the
     unreachable workers.
   - If a cached snapshot exists but is stale (older than TTL), and the fresh probe fails, the host
     MUST NOT fall back to the stale snapshot (to prevent using outdated capability data).
```

---

## Summary of changes

| # | Title | Files | Type |
|---|-------|-------|------|
| 1 | Run resumption semantics | PLAN.md | normative |
| 2 | Ephemeral simulator cleanup | PLAN.md | normative |
| 3 | run_summary.json schema | PLAN.md | normative |
| 4 | metrics.json schema | PLAN.md | recommended |
| 5 | destination.json + toolchain.json schemas | PLAN.md | normative |
| 6 | Host artifact retention | PLAN.md + README.md | recommended |
| 7 | Signal handling | PLAN.md | normative |
| 8 | Probe failure handling | PLAN.md | normative |
