# APR Round 18 — rch-mac

## Issues found: 2

### Issue 1: `continue_on_failure` description in run_plan.json is inverted/ambiguous
**Severity:** Bug (implementer could invert the logic)
**Location:** PLAN.md → Run plan artifact (normative) → `continue_on_failure` field description

The parenthetical reads: "whether failed steps should skip remaining steps" — this describes the *opposite* of what `true` means. When `continue_on_failure = true`, failed steps do NOT skip remaining steps. An implementer reading only the field table would get the logic backwards.

**Fix:** Change description to: "if true, execute remaining steps even after a failure (default false: abort on first failure)"

### Issue 2: README lists `rsync` as a worker requirement but protocol doesn't use it
**Severity:** Interop / deployment confusion
**Location:** README.md → Requirements → macOS worker

The README requires `rsync + zstd` on the worker. However, PLAN.md specifies source transfer via `upload_source` using binary-framed tar (optionally zstd-compressed) over SSH stdin — no rsync involved. Listing rsync as a requirement will confuse operators about actual dependencies and may block deployments that lack rsync unnecessarily.

**Fix:** Change `rsync + zstd` to `zstd (for source bundle compression)` and drop rsync.
