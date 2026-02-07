I’m basing this review on the README.md + PLAN.md you pasted in your message. Below are the highest-leverage revisions I’d make to improve robustness, determinism, security, and “agent/CI usefulness”.

I’m presenting these as separate, focused diffs (some touch the same sections, so you’ll merge them when you apply).

1) Make the contract “unmissable”: schema_version + decision + events + snapshot artifacts
Why this makes it better

Right now, you have great artifacts, but you’re missing two things that make agent/CI integration dramatically easier:

A schema-versioned contract: consumers can validate and evolve safely without brittle parsing.

“Why” artifacts: a lane that fails closed needs to explain why it refused/intercepted/failed in a machine-readable way.

A single normalized event stream: lets you build better summaries, progress UI, and failure triage (and makes MCP vs fallback interchangeable).

Diff
diff
Copy code
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## Outputs
 Artifacts are written to: `~/.local/share/rch/artifacts/<job_id>/`
 
 ``` <job_id>/
 ├── summary.json           # High-level status + timings
+├── decision.json          # Classifier + policy decisions (what/why)
 ├── effective_config.json  # Resolved/pinned run configuration
 ├── attestation.json       # Worker identity, tool versions, repo state
 ├── manifest.json          # Artifact listing + SHA-256 hashes
 ├── environment.json       # Worker environment snapshot
 ├── timing.json            # Phase durations (staging/running/collecting)
 ├── build.log              # Streamed + finalized stdout/stderr
+├── events.ndjson          # Normalized build/test event stream (backend-agnostic)
+├── source_snapshot.json   # Repo snapshot + content hash used for staging
+├── lease.json             # Worker lease + queue metadata (when applicable)
+├── cache_report.json      # Cache hits/misses + sizes (when caching enabled)
 └── result.xcresult/       # When tests are executed


+> Tip: Each JSON artifact includes schema_version and is validated against docs/schema/ as defined in PLAN.md.
diff --git a/PLAN.md b/PLAN.md
--- a/PLAN.md
+++ b/PLAN.md
@@

Determinism Contract

Every run MUST emit:

effective_config.json — Fully-resolved configuration used for the job (Xcode path, destination, build settings, cache policy).

attestation.json — Environment fingerprint: macOS version
