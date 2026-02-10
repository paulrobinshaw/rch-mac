# APR Operations Reference

## Infrastructure

| Component | Location | Access |
|---|---|---|
| VPS | `161.97.185.227` | `ssh -i ~/.ssh/acfs_ed25519 ubuntu@161.97.185.227` |
| Mac mini | `100.122.100.99` (Tailscale) | `ssh -o IdentitiesOnly=yes -i ~/.ssh/acfs_ed25519 paul@100.122.100.99` |
| Oracle serve | Mac mini, port 9333 | PID managed by launchd/supervisor (auto-restarts on kill) |
| apr_auto.py | VPS, tmux session `rch-mac` | `tmux send-keys -t rch-mac ...` |
| VPS repo | `/data/projects/rch-mac/` | |
| VPS env vars | `~/.zshenv` | `ORACLE_REMOTE_HOST`, `ORACLE_REMOTE_TOKEN`, `APR_NO_ORACLE_PATCH` |

## Key Paths

### VPS
- `apr_auto.py` — `/data/projects/rch-mac/apr_auto.py`
- Rounds output — `/data/projects/rch-mac/.apr/rounds/rch-mac/`
- Auto logs — `/data/projects/rch-mac/.apr/auto-logs/`
- Status — `/data/projects/rch-mac/.apr/auto-logs/status.json`
- Lock files — `/data/projects/rch-mac/.apr/.locks/`
- Workflow config — `/data/projects/rch-mac/.apr/workflows/rch-mac.yaml`
- apr binary — `/home/ubuntu/.local/bin/apr`
- Oracle binary — `/home/ubuntu/.bun/bin/oracle`

### Mac mini
- Oracle binary — `/opt/homebrew/bin/oracle`
- Oracle dist — `/opt/homebrew/lib/node_modules/@steipete/oracle/dist/`
- Chrome profile — `/Users/paul/.oracle/browser-profile`
- CDP recovery script — `~/dev/rch-mac/scripts/oracle_cdp_recover.js`

## Oracle TDZ Bug (Issue #85, PR #90)

### Problem
`buildMarkdownFallbackExtractor` in Oracle's browser actions had a JavaScript temporal dead zone bug. When called with `'MIN_TURN_INDEX'`, it generated:
```js
const MIN_TURN_INDEX = (MIN_TURN_INDEX >= 0 ? MIN_TURN_INDEX : null);
```
The inner `const` shadows the outer scope → `ReferenceError`. This crashed the DOM observer silently, forcing every response through `recoverAssistantResponse` which grabbed ~600 chars with no stability checks.

### Fix
Rename inner variable to `__minTurn`. Applied manually on Mac mini:
```
File: /opt/homebrew/lib/node_modules/@steipete/oracle/dist/src/browser/actions/assistantResponse.js
Line 689: const __minTurn = ${turnIndexValue};
Lines 731, 734: references changed from MIN_TURN_INDEX to __minTurn
```
Outer scope references (lines 409, 425, 466, 469, 474, 475) left unchanged — they're correct.

### Permanence
This is a manual patch to `dist/`. Will be overwritten on `npm update` / `brew upgrade`. Need Oracle >= 0.8.6 for permanent fix. PR #90 is merged on `main` but no release yet (latest is 0.8.5 from Jan 19).

### VPS patch
Same fix applied at `/home/ubuntu/.bun/lib/node_modules/@steipete/oracle/dist/src/browser/actions/assistantResponse.js` but this copy doesn't matter — VPS is just the remote client. The browser automation runs on the Mac mini.

## apr_auto.py Configuration

| Setting | Value | Notes |
|---|---|---|
| Integration timeout | 3600s (1 hour) | Was 300s, then 900s. `apr integrate` prompt is heavy. |
| Round timeout | 3600s | Per-round Oracle timeout |
| Stability target | 75% | |
| Max rounds | 50 | |
| Cooldown | 10s | Between rounds |
| Settle/stable MS | 45000 | Passed as `APR_ORACLE_MIN_STABLE_MS` / `APR_ORACLE_SETTLE_WINDOW_MS` |

## Env Var Naming (Critical)

`apr_auto.py` reads from env:
- `ORACLE_REMOTE_HOST`
- `ORACLE_REMOTE_TOKEN`

But must pass to `apr` subprocess as:
- `APR_ORACLE_REMOTE_HOST`
- `APR_ORACLE_REMOTE_TOKEN`

The `APR_` prefix is what `apr` actually reads. Mismatch caused Oracle to never be contacted remotely in earlier versions.

## Common Operations

### Check status
```bash
ssh -i ~/.ssh/acfs_ed25519 ubuntu@161.97.185.227 'cat /data/projects/rch-mac/.apr/auto-logs/status.json; tail -20 $(ls -t /data/projects/rch-mac/.apr/auto-logs/run_*.log 2>/dev/null | head -1)'
```

### Kill apr_auto
```bash
ssh -i ~/.ssh/acfs_ed25519 ubuntu@161.97.185.227 'pkill -9 -f apr_auto'
```

### Clean locks + stale sessions
```bash
ssh -i ~/.ssh/acfs_ed25519 ubuntu@161.97.185.227 'rm -f /data/projects/rch-mac/.apr/.locks/*.lock; oracle session --clear --all'
```

### Restart apr_auto
```bash
ssh -i ~/.ssh/acfs_ed25519 ubuntu@161.97.185.227 'tmux send-keys -t rch-mac "python3 apr_auto.py" Enter'
```

### Check Oracle on Mac mini
```bash
ssh -o IdentitiesOnly=yes -i ~/.ssh/acfs_ed25519 paul@100.122.100.99 'ps aux | grep "oracle serve" | grep -v grep'
```

### Restart Oracle on Mac mini
```bash
ssh -o IdentitiesOnly=yes -i ~/.ssh/acfs_ed25519 paul@100.122.100.99 'kill -9 $(pgrep -f "oracle serve")'
# Auto-restarts via supervisor
```

### Archive rounds and fresh start
```bash
ssh -i ~/.ssh/acfs_ed25519 ubuntu@161.97.185.227 'cd /data/projects/rch-mac && mkdir -p .apr/rounds/rch-mac/archive && mv .apr/rounds/rch-mac/round_*.md .apr/rounds/rch-mac/archive/'
```

### Full cleanup and restart (copy-paste one-liner)
Kill process, clean locks, clear Oracle sessions, restart in tmux:
```bash
ssh -i ~/.ssh/acfs_ed25519 ubuntu@161.97.185.227 'pkill -f apr_auto 2>/dev/null; sleep 1; rm -f /data/projects/rch-mac/.apr/.locks/*.lock 2>/dev/null; oracle session --clear --all 2>&1; tmux send-keys -t rch-mac "cd /data/projects/rch-mac && python3 apr_auto.py" Enter'
```

### Clean auto-logs (keep last 5)
```bash
ssh -i ~/.ssh/acfs_ed25519 ubuntu@161.97.185.227 'cd /data/projects/rch-mac/.apr/auto-logs && ls -t run_*.log | tail -n +6 | xargs rm -f 2>/dev/null; ls -t integrate_claude_round_*.log | tail -n +6 | xargs rm -f 2>/dev/null'
```

### Check integration debug output
After integration, check what Claude actually did:
```bash
ssh -i ~/.ssh/acfs_ed25519 ubuntu@161.97.185.227 'cat /data/projects/rch-mac/.apr/auto-logs/integrate_claude_round_*.log 2>/dev/null | head -100'
```

### Deploy apr_auto.py changes
After editing locally, push to VPS:
```bash
# Local: commit and push
git add apr_auto.py && git commit -m "apr-auto: <description>" && git push
# VPS: pull
ssh -i ~/.ssh/acfs_ed25519 ubuntu@161.97.185.227 'cd /data/projects/rch-mac && git pull --ff-only'
```

## Cleanup Checklist

When things go wrong, clean up in this order:

1. **Kill `apr_auto`**: `pkill -f apr_auto` — always do this first
2. **Wait**: `sleep 2` — let child processes (Oracle sessions, Claude) terminate
3. **Clear locks**: `rm -f .apr/.locks/*.lock` — VPS zsh globbing may error on no match, use `2>/dev/null`
4. **Clear Oracle sessions**: `oracle session --clear --all` — prevents "stale session" errors on restart
5. **Verify Oracle on Mac mini**: SSH to `paul@100.122.100.99`, check `pgrep -f "oracle serve"` — if dead, `kill -9` will auto-restart
6. **Optionally archive rounds**: move `round_*.md` to `archive/` for a fresh convergence run
7. **Restart**: `tmux send-keys -t rch-mac "cd /data/projects/rch-mac && python3 apr_auto.py" Enter`

**ZSH gotcha**: VPS runs zsh. `rm -f *.lock` fails with "no matches found" if no files exist. Always append `2>/dev/null`.

## Integration Notes

- `apr integrate` generates a heavy prompt: reads AGENTS.md, README.md, "investigation agent mode", "ultrathink", then the round diffs. This is why integration takes 10-60 minutes.
- Failed integrations save the prompt to `.apr/auto-logs/integrate_round_N.md`
- Successful integrations save Claude's output to `.apr/auto-logs/integrate_claude_round_N.log`
- `claude -p` requires `--append-system-prompt` to force tool use — without it, Claude describes changes in text rather than using Edit/Write tools
- Current invocation: `claude -p --allowedTools "Read Edit Write Bash" --permission-mode bypassPermissions --append-system-prompt "You MUST use the Edit or Write tool to apply changes directly to the files. Do not just describe the changes in your output." -`
- Manual integration via spawned Claude Code agents is more reliable for large rounds

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| ~600 chars per round, "completed" | Oracle TDZ bug | Patch `assistantResponse.js` on Mac mini |
| "Another APR process is already running" | Stale lock file | `rm .apr/.locks/*.lock 2>/dev/null` |
| "Stale Oracle session detected" | Duplicate prompt content | `oracle session --clear --all` |
| Integration timeout | `apr integrate` prompt too heavy | Timeout is now 3600s; may still need manual integration for very large rounds |
| Integration exit 0 but PLAN.md unchanged | Claude describing changes instead of editing | Need `--append-system-prompt` forcing tool use |
| `apr_auto.py` exits silently | Oracle process died mid-round | Check Mac mini Oracle serve is running |
| 0% stability score | Too few rounds / truncated outputs | Need 3+ full rounds for meaningful score |
| zsh "no matches found" on cleanup | Glob expansion with no matching files | Append `2>/dev/null` to glob commands |
