Below are the highest-leverage revisions I’d make to tighten determinism, reduce security risk, and make the lane more “agent/CI compelling” (better introspection, provenance, and ergonomics). For each change: rationale + a focused git-diff against the README/PLAN you pasted.

1) Make “pinning” explicit and automatic (selectors → resolved pins)
Why this makes it better

Right now the config allows os = "latest" and destination by human name. That’s great ergonomics, but it’s not deterministic unless you clearly define when/where “latest” resolves and how “iPhone 16” becomes a stable identifier.

Best practice: treat the TOML as selectors; during verify, resolve them into pinned values (runtime version/build + simulator UDID + Xcode build number) and write those into effective_config.json. Then the run consumes only the resolved pins.

This gives:

Determinism without forcing users to hand-pin everything.

A clean mental model: selectors in config → pins in effective_config.

Better caching keys (pins are stable).

Diff (README.md + PLAN.md)
diff
Copy code
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@
 ## How it works
  1. **Select worker** (tagged `macos,xcode`) and probe capabilities (Xcode, runtimes, XcodeBuildMCP).
  2. **Stage source** to the worker (rsync/zstd).
- 3. **Run** build/test remotely (via XcodeBuildMCP backend; `xcodebuild` fallback allowed).
+ 3. **Resolve + pin** selectors (Xcode identity, simulator runtime, destination → UDID) into `effective_config.json`.
+ 4. **Run** build/test remotely (via XcodeBuildMCP backend; `xcodebuild` fallback allowed) using the pinned effective config.
- 4. **Collect artifacts** (logs, `xcresult`, structured JSON).
- 5. **Attest** toolchain + environment; emit machine-readable outputs for CI/agents.
+ 5. **Collect artifacts** (logs, `xcresult`, structured JSON).
+ 6. **Attest** toolchain + environment; emit m
