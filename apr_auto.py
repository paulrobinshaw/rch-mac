#!/usr/bin/env python3
"""
apr-auto: Thin orchestrator for Automated Plan Reviser Pro.

Runs `apr run N --wait` in a loop until convergence, then stops.
Trusts APR's own retry logic, preflight checks, and Oracle management.
This script only adds: the loop, convergence gating, and optional
Claude Code integration + git commit between rounds.

Usage:
    python3 apr_auto.py                     # Run until 75% convergence
    python3 apr_auto.py --target 90         # Run until 90% convergence
    python3 apr_auto.py --max-rounds 30     # Safety cap at 30 rounds
    python3 apr_auto.py --start-round 5     # Override auto-detect
    python3 apr_auto.py --dry-run           # Pre-flight only
    python3 apr_auto.py --no-integrate      # Skip Claude Code integration
    python3 apr_auto.py --no-commit         # Skip git commit/push
    python3 apr_auto.py --no-cdp-recovery   # Disable CDP truncation recovery
    python3 apr_auto.py --cdp-ssh-host HOST # Override CDP recovery SSH host

Auto-detection:
    Scans .apr/rounds/{workflow}/ for existing round files and starts
    from the next one. Use --start-round to override.

    Stopping:
    Primary: `apr stats -w <workflow>` stability score (>= target)
    Secondary: max rounds safety cap (default 50).
    Emergency: 3 consecutive failures.

Environment variables:
    ORACLE_REMOTE_HOST          Tailscale IP:port (e.g. 100.122.100.99:9333)
    ORACLE_REMOTE_TOKEN         Oracle auth token
    APR_ORACLE_MIN_STABLE_MS    Oracle stability timing (default: 45000)
    APR_ORACLE_SETTLE_WINDOW_MS Oracle settle timing (default: 45000)
    APR_MAX_RETRIES             APR's own retry count (forwarded)
    APR_INITIAL_BACKOFF         APR's own backoff (forwarded)
    APR_AUTO_LOG_DIR            Log directory (default: .apr/auto-logs)
    APR_AUTO_NOTIFY_CMD         Command to run on completion (e.g. webhook)
    APR_CDP_RECOVERY_SSH_HOST   SSH host for CDP recovery (default: oracle host)
    APR_CDP_RECOVERY_SCRIPT     Path to CDP recovery script on remote host
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import re
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


# =============================================================================
# Configuration
# =============================================================================

DEFAULT_MAX_ROUNDS = 50
DEFAULT_CONVERGENCE_TARGET = 75  # percent
DEFAULT_WORKFLOW = "default"
DEFAULT_ORACLE_HOST = "100.122.100.99"
DEFAULT_ORACLE_PORT = 9333
DEFAULT_MIN_STABLE_MS = "45000"
DEFAULT_SETTLE_WINDOW_MS = "45000"
DEFAULT_COOLDOWN_SECONDS = 10
ROUND_TIMEOUT_SECONDS = 3600  # 1 hour per round
MAX_CONSECUTIVE_FAILURES = 3
FAILURE_BACKOFF_SECONDS = 10
MAX_TRUNCATION_RETRIES = 3
CDP_RECOVERY_TIMEOUT = 150          # SSH + script total (seconds)
CDP_RECOVERY_SCRIPT = "~/dev/rch-mac/scripts/oracle_cdp_recover.js"

# Truncation heuristics (applied to round output files)
MIN_OUTPUT_CHARS = 200
MIN_OUTPUT_LINES = 5

# Log management
MAX_LOG_FILES = 20


# =============================================================================
# Data Classes
# =============================================================================

@dataclass
class Config:
    """Orchestrator configuration."""
    max_rounds: int = DEFAULT_MAX_ROUNDS
    start_round: Optional[int] = None
    workflow: str = DEFAULT_WORKFLOW
    convergence_target: float = DEFAULT_CONVERGENCE_TARGET
    cooldown: int = DEFAULT_COOLDOWN_SECONDS
    dry_run: bool = False
    integrate: bool = True
    commit: bool = True
    oracle_host: str = ""
    oracle_port: int = DEFAULT_ORACLE_PORT
    oracle_token: str = ""
    log_dir: Path = Path(".apr/auto-logs")
    notify_cmd: Optional[str] = None
    reset_workflow: bool = False
    cdp_recovery_enabled: bool = True
    cdp_recovery_ssh_host: str = ""
    cdp_recovery_script_path: str = ""
    cdp_recovery_timeout: int = CDP_RECOVERY_TIMEOUT

    def __post_init__(self):
        self.log_dir = Path(
            os.environ.get("APR_AUTO_LOG_DIR", str(self.log_dir))
        )

        if not self.oracle_token:
            self.oracle_token = os.environ.get("ORACLE_REMOTE_TOKEN", "")

        if not self.oracle_host:
            host_env = os.environ.get("ORACLE_REMOTE_HOST", "").strip()
            if host_env:
                if ":" in host_env:
                    parts = host_env.rsplit(":", 1)
                    self.oracle_host = parts[0]
                    try:
                        self.oracle_port = int(parts[1])
                    except ValueError:
                        pass
                else:
                    self.oracle_host = host_env
            if not self.oracle_host:
                self.oracle_host = DEFAULT_ORACLE_HOST

        if not self.notify_cmd:
            self.notify_cmd = os.environ.get("APR_AUTO_NOTIFY_CMD")

        if not self.cdp_recovery_ssh_host:
            self.cdp_recovery_ssh_host = os.environ.get(
                "APR_CDP_RECOVERY_SSH_HOST", ""
            ) or f"paul@{self.oracle_host}"

        if not self.cdp_recovery_script_path:
            self.cdp_recovery_script_path = os.environ.get(
                "APR_CDP_RECOVERY_SCRIPT", CDP_RECOVERY_SCRIPT
            )


@dataclass
class RoundResult:
    """Result of a single APR round."""
    round_num: int
    success: bool
    error_msg: Optional[str] = None
    duration_seconds: float = 0.0
    convergence_pct: Optional[float] = None
    round_chars: int = 0
    round_lines: int = 0
    truncated: bool = False
    cdp_recovery_attempted: bool = False
    integrated: bool = False
    committed: bool = False
    commit_sha: Optional[str] = None
    timestamp: str = field(
        default_factory=lambda: datetime.now(timezone.utc).isoformat()
    )


@dataclass
class RunSummary:
    """Summary of the full orchestration run."""
    started_at: str = field(
        default_factory=lambda: datetime.now(timezone.utc).isoformat()
    )
    finished_at: Optional[str] = None
    stopped_reason: Optional[str] = None
    rounds_completed: int = 0
    rounds_failed: int = 0
    results: List[Dict[str, Any]] = field(default_factory=list)


# =============================================================================
# Logging
# =============================================================================


def setup_logging(log_dir: Path) -> logging.Logger:
    """Set up file + console logging with rotation."""
    log_dir.mkdir(parents=True, exist_ok=True)

    # Rotate old logs
    logs = sorted(log_dir.glob("run_*.log"), key=lambda p: p.stat().st_mtime)
    while len(logs) >= MAX_LOG_FILES:
        logs.pop(0).unlink(missing_ok=True)

    ts = datetime.now().strftime("%Y%m%d_%H%M%S")
    log_file = log_dir / f"run_{ts}.log"

    logger = logging.getLogger("apr-auto")
    logger.setLevel(logging.DEBUG)

    if logger.handlers:
        for handler in list(logger.handlers):
            logger.removeHandler(handler)

    fh = logging.FileHandler(log_file, encoding="utf-8")
    fh.setLevel(logging.DEBUG)
    fh.setFormatter(logging.Formatter(
        "%(asctime)s [%(levelname)s] %(message)s"
    ))

    ch = logging.StreamHandler()
    ch.setLevel(logging.INFO)
    ch.setFormatter(logging.Formatter("%(message)s"))

    logger.addHandler(fh)
    logger.addHandler(ch)

    logger.info(f"📝 Log: {log_file}")
    return logger


# =============================================================================
# Filesystem Helpers
# =============================================================================


def _clean_yaml_value(raw: str) -> str:
    value = raw.split("#", 1)[0].strip()
    return value.strip("\"'")


def read_default_workflow(config_path: Path = Path(".apr/config.yaml")) -> Optional[str]:
    if not config_path.exists():
        return None
    try:
        for line in config_path.read_text().splitlines():
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            if stripped.startswith("default_workflow:"):
                _, value = stripped.split(":", 1)
                value = _clean_yaml_value(value)
                return value or None
    except OSError:
        return None
    return None


def read_rounds_output_dir(workflow_file: Path) -> Optional[str]:
    if not workflow_file.exists():
        return None
    try:
        lines = workflow_file.read_text().splitlines()
    except OSError:
        return None

    in_rounds = False
    rounds_indent = 0

    for line in lines:
        raw = line.rstrip("\n")
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue

        indent = len(raw) - len(raw.lstrip(" "))

        if not in_rounds:
            if stripped.startswith("rounds:"):
                in_rounds = True
                rounds_indent = indent
            continue

        if indent <= rounds_indent:
            break

        if stripped.startswith("output_dir:"):
            _, value = stripped.split(":", 1)
            return _clean_yaml_value(value)

    return None


def resolve_workflow_name(requested: str) -> str:
    if requested != DEFAULT_WORKFLOW:
        return requested
    return read_default_workflow() or requested


def resolve_rounds_dir(workflow_name: str) -> Path:
    workflow_file = Path(f".apr/workflows/{workflow_name}.yaml")
    output_dir = read_rounds_output_dir(workflow_file)
    if output_dir:
        return Path(output_dir)
    return Path(f".apr/rounds/{workflow_name}")


def round_file(rounds_dir: Path, round_num: int) -> Path:
    """Path to a specific round output file."""
    return rounds_dir / f"round_{round_num}.md"


def metrics_file(workflow: str) -> Path:
    """Path to the analytics metrics file for a workflow."""
    return Path(f".apr/analytics/{workflow}/metrics.json")


def detect_last_round(rounds_dir: Path) -> int:
    """Find the highest completed round number from filesystem."""
    if not rounds_dir.exists():
        return 0

    highest = 0
    for f in rounds_dir.glob("round_*.md"):
        match = re.search(r"round_(\d+)\.md$", f.name)
        if match:
            n = int(match.group(1))
            if f.stat().st_size > 0:
                highest = max(highest, n)
    return highest


def _extract_stability_score(data: Dict[str, Any]) -> Optional[float]:
    conv = data.get("convergence") or {}
    candidates = [
        conv.get("score"),
        conv.get("confidence"),
        conv.get("stability_score"),
        data.get("convergence_score"),
        data.get("stability_score"),
    ]
    for raw in candidates:
        if raw is None:
            continue
        try:
            val = float(raw)
        except (TypeError, ValueError):
            continue
        if val <= 1.0:
            return val * 100
        if val <= 100.0:
            return val
    return None


def read_stability_score(
    workflow: str,
    config: "Config",
    logger: logging.Logger,
) -> Optional[float]:
    """
    Read stability score from `apr stats --export json`.

    Returns percentage (0-100) or None if unavailable.
    """
    cmd = ["apr", "stats", "--export", "json"]
    if workflow != DEFAULT_WORKFLOW:
        cmd.extend(["-w", workflow])

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=build_env(config),
            timeout=30,
        )
    except (subprocess.TimeoutExpired, OSError) as e:
        logger.debug(f"  Stats error: {e}")
        return None

    if result.returncode != 0 or not result.stdout.strip():
        logger.debug(f"  Stats exit {result.returncode}")
        return None

    try:
        data = json.loads(result.stdout.strip())
    except json.JSONDecodeError:
        logger.debug("  Stats output not valid JSON")
        return None

    return _extract_stability_score(data)


# =============================================================================
# Truncation Detection
# =============================================================================


def check_output_truncation(
    rounds_dir: Path,
    round_num: int,
    output_sizes: List[int],
    logger: logging.Logger,
) -> bool:
    """
    Check if a round output file appears truncated.

    Reads the file directly from disk. Uses:
    - Minimum size/line thresholds
    - Partial-word detection (GPT truncation cuts mid-word)
    - Relative size comparison against rolling average
    """
    rf = round_file(rounds_dir, round_num)
    if not rf.exists():
        logger.warning(f"Round {round_num} output file missing")
        return True

    try:
        content = rf.read_text(encoding="utf-8", errors="replace")
    except OSError as e:
        logger.warning(f"Cannot read round {round_num} output: {e}")
        return True

    chars = len(content)
    lines = content.count("\n")

    if chars < MIN_OUTPUT_CHARS:
        logger.warning(f"Round {round_num} too short: {chars} chars")
        return True

    if lines < MIN_OUTPUT_LINES:
        logger.warning(f"Round {round_num} too few lines: {lines}")
        return True

    # Partial-word detection
    stripped = content.rstrip()
    if stripped:
        tokens = stripped.split()
        if tokens:
            last = tokens[-1]

            # Incomplete code fence
            if stripped.endswith("``") and not stripped.endswith("```"):
                logger.warning(
                    f"Round {round_num} truncated: incomplete code fence"
                )
                return True

            # No-vowel fragment (e.g. "implementt", "specifc")
            if (
                len(last) > 3
                and last[-1].isalpha()
                and not any(c in last.lower() for c in "aeiouy")
            ):
                logger.warning(
                    f"Round {round_num} truncated: "
                    f"no-vowel fragment '{last}'"
                )
                return True

            # Ends on a heading with nothing after it
            last_line = stripped.splitlines()[-1]
            if last_line.startswith("#") and chars < 1000:
                logger.warning(
                    f"Round {round_num} truncated: "
                    f"ends with heading, only {chars} chars"
                )
                return True

    # Relative size: <30% of rolling average is suspicious
    if output_sizes and chars > 0:
        avg = sum(output_sizes) / len(output_sizes)
        if chars < avg * 0.3:
            logger.warning(
                f"Round {round_num}: {chars} chars is <30% of "
                f"avg {avg:.0f} — likely truncated"
            )
            return True

    logger.debug(f"Round {round_num} OK: {chars} chars, {lines} lines")
    return False


# =============================================================================
# Infrastructure Checks
# =============================================================================


def check_tailscale(host: str, port: int) -> bool:
    """Quick TCP check to Oracle."""
    try:
        with socket.create_connection((host, port), timeout=5):
            return True
    except OSError:
        return False


def check_apr_status(logger: logging.Logger) -> bool:
    """
    Verify APR is configured via `apr robot status`.

    This is the one robot mode call we keep — it gives structured
    data about configuration state in a single request.
    """
    env = os.environ.copy()
    env.update({
        "NO_COLOR": "1",
        "APR_CHECK_UPDATES": "0",
        "APR_OUTPUT_FORMAT": "json",
    })

    try:
        result = subprocess.run(
            ["apr", "robot", "status"],
            capture_output=True, text=True, timeout=15,
            env=env,
        )
    except (subprocess.TimeoutExpired, OSError) as e:
        logger.error(f"  ❌ apr robot status failed: {e}")
        return False

    if not result.stdout.strip():
        logger.error("  ❌ apr robot status returned no output")
        return False

    try:
        data = json.loads(result.stdout.strip())
    except json.JSONDecodeError:
        logger.error("  ❌ apr robot status returned invalid JSON")
        return False

    if not data.get("ok"):
        code = data.get("code", "unknown")
        hint = data.get("hint")
        logger.error(f"  ❌ APR status error: {code}")
        if hint:
            logger.error(f"  ❌ Hint: {hint}")
        return False

    inner = data.get("data", {})
    configured = inner.get("configured", False)
    wf_count = inner.get("workflow_count", 0)
    oracle_ok = inner.get("oracle_available", False)

    logger.info(
        f"  ✅ APR: configured={configured}, "
        f"workflows={wf_count}, oracle={oracle_ok}"
    )

    if not configured:
        logger.error(
            "  ❌ APR not configured. Run: apr setup"
        )
        return False

    if wf_count == 0:
        logger.error(
            "  ❌ No workflows. Run: apr setup"
        )
        return False

    if not oracle_ok:
        logger.warning(
            "  ⚠️  Oracle reported unavailable"
        )

    return True


def check_workflow_exists(workflow: str, logger: logging.Logger) -> bool:
    """Verify the target workflow YAML exists."""
    wf_file = Path(f".apr/workflows/{workflow}.yaml")
    if wf_file.exists():
        logger.info(f"  ✅ Workflow '{workflow}'")
        return True

    # Maybe default_workflow in config.yaml resolves differently
    config_file = Path(".apr/config.yaml")
    if workflow == DEFAULT_WORKFLOW and config_file.exists():
        try:
            text = config_file.read_text()
            for line in text.splitlines():
                if line.strip().startswith("default_workflow:"):
                    actual = line.split(":", 1)[1].strip().strip("\"'")
                    actual_file = Path(f".apr/workflows/{actual}.yaml")
                    if actual_file.exists():
                        logger.info(
                            f"  ✅ Workflow '{actual}' (default)"
                        )
                        return True
        except OSError:
            pass

    wf_dir = Path(".apr/workflows")
    if wf_dir.exists():
        available = [f.stem for f in wf_dir.glob("*.yaml")]
        if available:
            logger.error(
                f"  ❌ Workflow '{workflow}' not found. "
                f"Available: {', '.join(available)}"
            )
        else:
            logger.error("  ❌ No workflows. Run: apr setup")
    else:
        logger.error("  ❌ No .apr/workflows/. Run: apr setup")
    return False


# =============================================================================
# APR CLI Wrappers
# =============================================================================


def build_env(config: Config) -> Dict[str, str]:
    """Build environment for APR subprocess calls."""
    env = os.environ.copy()
    if config.oracle_host and config.oracle_port:
        env["ORACLE_REMOTE_HOST"] = (
            f"{config.oracle_host}:{config.oracle_port}"
        )
    if config.oracle_token:
        env["ORACLE_REMOTE_TOKEN"] = config.oracle_token
    env.setdefault("APR_ORACLE_MIN_STABLE_MS", DEFAULT_MIN_STABLE_MS)
    env.setdefault("APR_ORACLE_SETTLE_WINDOW_MS", DEFAULT_SETTLE_WINDOW_MS)
    env.setdefault("APR_CHECK_UPDATES", "0")
    env.setdefault("NO_COLOR", "1")
    return env


def run_apr_round(
    round_num: int,
    config: Config,
    logger: logging.Logger,
) -> Tuple[bool, str]:
    """
    Run a single APR round using `apr run N --wait`.

    Synchronous — APR blocks until Oracle completes, handles its own
    retries (APR_MAX_RETRIES), preflight, and session management.

    Returns (success, error_message).
    """
    cmd = ["apr", "run", str(round_num), "--wait"]
    if config.workflow != DEFAULT_WORKFLOW:
        cmd.extend(["-w", config.workflow])

    logger.debug(f"Running: {' '.join(cmd)}")

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=build_env(config),
            timeout=ROUND_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired:
        return False, f"Timed out after {ROUND_TIMEOUT_SECONDS}s"

    # Log APR's stderr (human-readable progress)
    if result.stderr:
        for line in result.stderr.strip().splitlines():
            logger.debug(f"  [apr] {line.rstrip()}")

    if result.returncode == 0:
        return True, ""

    # Map exit codes
    error_map = {
        2: "Usage error",
        3: "Dependency error (Oracle not found)",
        4: "Config error (missing files or workflow)",
        10: "Network error",
    }
    msg = error_map.get(
        result.returncode,
        f"Exit code {result.returncode}"
    )

    # Append last line of stderr for context
    stderr_lines = result.stderr.strip().splitlines()
    if stderr_lines:
        msg += f" — {stderr_lines[-1][:200]}"

    return False, msg


def attempt_cdp_recovery(
    round_num: int,
    config: Config,
    logger: logging.Logger,
) -> Optional[str]:
    """
    Attempt to recover a full GPT response via CDP from the Mac mini.

    SSH into the recovery host and run the CDP recovery script, which
    connects to Chrome's DevTools Protocol and extracts the last assistant
    message from the still-open ChatGPT tab.

    Returns recovered markdown text or None on any failure.
    """
    if not config.cdp_recovery_enabled:
        return None

    host = config.cdp_recovery_ssh_host
    script = config.cdp_recovery_script_path

    if not host or not script:
        logger.debug("  CDP recovery: no host or script configured")
        return None

    logger.info(f"  🔄 Attempting CDP recovery from {host}...")

    ssh_cmd = [
        "ssh",
        "-o", "BatchMode=yes",
        "-o", "ConnectTimeout=10",
        "-o", "StrictHostKeyChecking=accept-new",
        host,
        f"/usr/local/bin/node {shlex.quote(script)} --timeout 120 --min-length {MIN_OUTPUT_CHARS}",
    ]

    try:
        result = subprocess.run(
            ssh_cmd,
            capture_output=True,
            text=True,
            timeout=config.cdp_recovery_timeout,
        )
    except subprocess.TimeoutExpired:
        logger.warning(
            f"  ⚠️  CDP recovery timed out after {config.cdp_recovery_timeout}s"
        )
        return None
    except OSError as e:
        logger.warning(f"  ⚠️  CDP recovery SSH error: {e}")
        return None

    if result.returncode != 0:
        stderr_tail = result.stderr.strip().splitlines()
        last_line = stderr_tail[-1][:200] if stderr_tail else "(no stderr)"
        logger.warning(
            f"  ⚠️  CDP recovery failed (exit {result.returncode}): {last_line}"
        )
        return None

    recovered = result.stdout
    if not recovered or len(recovered) < MIN_OUTPUT_CHARS:
        logger.warning(
            f"  ⚠️  CDP recovery too short: {len(recovered or '')} chars"
        )
        return None

    logger.info(
        f"  ✅ CDP recovery: {len(recovered)} chars, "
        f"{recovered.count(chr(10))} lines"
    )
    return recovered


def run_backfill(config: Config, logger: logging.Logger, force: bool = False) -> bool:
    """Run `apr backfill` to update analytics metrics."""
    cmd = ["apr", "backfill"]
    if force:
        cmd.append("--force")
    if config.workflow != DEFAULT_WORKFLOW:
        cmd.extend(["-w", config.workflow])

    try:
        result = subprocess.run(
            cmd,
            capture_output=True, text=True,
            env=build_env(config), timeout=60,
        )
        if result.returncode == 0:
            logger.debug("  Backfill complete")
            return True
        logger.debug(f"  Backfill exit {result.returncode}")
    except (subprocess.TimeoutExpired, OSError) as e:
        logger.debug(f"  Backfill error: {e}")
    return False


def run_integrate(
    round_num: int,
    config: Config,
    logger: logging.Logger,
) -> bool:
    """
    Run integration: get prompt from `apr integrate`, pipe to Claude Code.

    Falls back to saving the prompt to a file if Claude Code is unavailable.
    """
    env = build_env(config)
    wf_args = ["-w", config.workflow] if config.workflow != DEFAULT_WORKFLOW else []

    # Get integration prompt
    prompt_cmd = ["apr", "integrate", str(round_num), "--quiet"] + wf_args
    try:
        prompt_result = subprocess.run(
            prompt_cmd, capture_output=True, text=True,
            env=env, timeout=30,
        )
        if prompt_result.returncode != 0 or not prompt_result.stdout.strip():
            logger.warning("  Could not get integration prompt")
            return False
        prompt = prompt_result.stdout.strip()
    except (subprocess.TimeoutExpired, OSError):
        logger.warning("  Integration prompt failed")
        return False

    # Try Claude Code first
    if shutil.which("claude"):
        try:
            claude_result = subprocess.run(
                ["claude", "--print", "-"],
                input=prompt,
                capture_output=True, text=True,
                timeout=300,
            )
            if claude_result.returncode == 0:
                logger.info("  ✅ Claude Code integration complete")
                return True
            logger.warning(
                f"  ⚠️  Claude Code exit {claude_result.returncode}"
            )
        except subprocess.TimeoutExpired:
            logger.warning("  ⚠️  Claude Code timed out")
        except OSError as e:
            logger.warning(f"  ⚠️  Claude Code error: {e}")

    # Fallback: save prompt to file
    prompt_file = config.log_dir / f"integrate_round_{round_num}.md"
    config.log_dir.mkdir(parents=True, exist_ok=True)
    try:
        prompt_file.write_text(prompt)
        logger.info(f"  📄 Integration prompt saved: {prompt_file}")
        return True
    except OSError:
        return False


# =============================================================================
# Git
# =============================================================================


def git_commit_and_push(
    round_num: int,
    conv_info: str,
    logger: logging.Logger,
) -> Optional[str]:
    """Commit and push. Returns short SHA or None."""
    try:
        subprocess.run(
            ["git", "add", "-u"], capture_output=True, timeout=30
        )
        status = subprocess.run(
            ["git", "status", "--porcelain", "-z"],
            capture_output=True, text=True, timeout=10,
        )
        if status.returncode == 0 and status.stdout:
            paths = []
            for entry in status.stdout.split("\0"):
                if not entry.startswith("?? "):
                    continue
                path = entry[3:]
                if path.startswith(".apr/"):
                    continue
                if path:
                    paths.append(path)
            if paths:
                subprocess.run(
                    ["git", "add", "--"] + paths,
                    capture_output=True, timeout=30,
                )

        msg = f"apr-auto: round {round_num}"
        if conv_info:
            msg += f" ({conv_info})"

        result = subprocess.run(
            ["git", "commit", "-m", msg],
            capture_output=True, text=True, timeout=30,
        )
        if result.returncode != 0:
            if "nothing to commit" in (result.stdout + result.stderr):
                return None
            logger.debug(f"  Commit issue: {result.stderr[:200]}")
            return None

        sha_result = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, timeout=10,
        )
        sha = sha_result.stdout.strip() if sha_result.returncode == 0 else ""

        subprocess.run(
            ["git", "pull", "--rebase"],
            capture_output=True, text=True, timeout=60,
        )

        push_result = subprocess.run(
            ["git", "push"],
            capture_output=True, text=True, timeout=60,
        )
        if push_result.returncode == 0:
            logger.info(f"  📦 Committed + pushed ({sha})")
        else:
            logger.warning(f"  ⚠️  Push failed: {push_result.stderr[:200]}")

        return sha

    except (subprocess.TimeoutExpired, OSError) as e:
        logger.warning(f"  ⚠️  Git error: {e}")
        return None


# =============================================================================
# Notification + Status
# =============================================================================


def notify(message: str, config: Config, logger: logging.Logger):
    """Run notification command if configured."""
    if not config.notify_cmd:
        return
    try:
        cmd = shlex.split(config.notify_cmd)
        if not cmd:
            return
        subprocess.run(
            cmd,
            input=message,
            text=True,
            capture_output=True,
            timeout=30,
        )
    except (subprocess.TimeoutExpired, OSError, ValueError) as e:
        logger.debug(f"Notify failed: {e}")


def atomic_write_json(path: Path, payload: Dict[str, Any], indent: int = 2):
    """Write JSON atomically to avoid partial reads."""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=path.parent,
            delete=False,
            prefix=f"{path.name}.",
            suffix=".tmp",
        ) as tmp_file:
            json.dump(payload, tmp_file, indent=indent)
            tmp_file.flush()
            os.fsync(tmp_file.fileno())
            tmp_path = Path(tmp_file.name)
        os.replace(tmp_path, path)
        dir_fd = os.open(path.parent, os.O_DIRECTORY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    except OSError:
        return


def write_status(
    log_dir: Path, current_round: int, completed: int, status: str
):
    """Write machine-readable status for external monitoring."""
    payload = {
        "current_round": current_round,
        "rounds_completed": completed,
        "status": status,
        "updated_at": datetime.now(timezone.utc).isoformat(),
    }
    atomic_write_json(log_dir / "status.json", payload, indent=2)


# =============================================================================
# Orchestrator
# =============================================================================


class Orchestrator:
    def __init__(self, config: Config, logger: logging.Logger):
        self.config = config
        self.logger = logger
        self.summary = RunSummary()
        self._shutting_down = False
        self._output_sizes: List[int] = []
        self._truncation_attempts: Dict[int, int] = {}
        self._workflow_name: Optional[str] = None
        self._rounds_dir: Optional[Path] = None

        signal.signal(signal.SIGINT, self._handle_signal)
        signal.signal(signal.SIGTERM, self._handle_signal)

    def _handle_signal(self, signum, frame):
        self.logger.info("\n⚠️  Shutdown signal received")
        self._shutting_down = True

    def preflight(self) -> bool:
        """Verify everything is ready."""
        self.logger.info("\n🔍 Pre-flight checks")
        ok = True

        if shutil.which("apr"):
            self.logger.info("  ✅ apr found")
        else:
            self.logger.error("  ❌ apr not on PATH")
            ok = False

        host, port = self.config.oracle_host, self.config.oracle_port
        if host:
            if not self.config.oracle_token:
                self.logger.error("  ❌ Oracle token missing (set ORACLE_REMOTE_TOKEN)")
                ok = False
            elif check_tailscale(host, port):
                self.logger.info(f"  ✅ Oracle reachable ({host}:{port})")
            else:
                self.logger.error(f"  ❌ Oracle unreachable ({host}:{port})")
                ok = False
        else:
            self.logger.info("  ℹ️  Oracle remote not configured (skipping reachability check)")

        if not check_apr_status(self.logger):
            ok = False

        if not check_workflow_exists(self.config.workflow, self.logger):
            ok = False

        if self.config.commit:
            if shutil.which("git"):
                self.logger.info("  ✅ Git available")
            else:
                self.logger.warning(
                    "  ⚠️  Git not found, commits disabled"
                )
                self.config.commit = False

        if self.config.integrate and shutil.which("claude"):
            self.logger.info("  ✅ Claude Code available")
        elif self.config.integrate:
            self.logger.info(
                "  ℹ️  Claude Code not found — "
                "prompts saved to files instead"
            )

        # CDP recovery preflight
        if self.config.cdp_recovery_enabled:
            cdp_host = self.config.cdp_recovery_ssh_host
            cdp_script = self.config.cdp_recovery_script_path
            if cdp_host:
                try:
                    probe = subprocess.run(
                        [
                            "ssh",
                            "-o", "BatchMode=yes",
                            "-o", "ConnectTimeout=5",
                            cdp_host,
                            f"test -f {shlex.quote(cdp_script)} && echo ok",
                        ],
                        capture_output=True, text=True, timeout=15,
                    )
                    if probe.returncode == 0 and "ok" in probe.stdout:
                        self.logger.info(
                            f"  ✅ CDP recovery script reachable ({cdp_host})"
                        )
                    else:
                        self.logger.warning(
                            f"  ⚠️  CDP recovery script not found on {cdp_host}: "
                            f"{cdp_script}"
                        )
                except (subprocess.TimeoutExpired, OSError) as e:
                    self.logger.warning(
                        f"  ⚠️  CDP recovery host unreachable ({cdp_host}): {e}"
                    )
            else:
                self.logger.warning(
                    "  ⚠️  CDP recovery enabled but no SSH host configured"
                )

        return ok

    def run(self):
        """Main orchestration loop."""
        config = self.config
        logger = self.logger

        logger.info("")
        logger.info("=" * 55)
        logger.info("  APR Auto-Orchestrator")
        logger.info("=" * 55)
        logger.info(f"  Target:     {config.convergence_target}%")
        logger.info(f"  Max rounds: {config.max_rounds}")
        logger.info(f"  Workflow:   {config.workflow}")
        logger.info(f"  Cooldown:   {config.cooldown}s")
        if config.oracle_host:
            logger.info(
                f"  Oracle:     {config.oracle_host}:{config.oracle_port}"
            )
        else:
            logger.info("  Oracle:     (unset)")
        if config.cdp_recovery_enabled:
            logger.info(
                f"  Recovery:   CDP via {config.cdp_recovery_ssh_host}"
            )
        else:
            logger.info("  Recovery:   disabled")
        logger.info("=" * 55)

        if not self.preflight():
            if config.dry_run:
                logger.info("\n🏁 Dry run — issues found above")
                return
            logger.error("\n❌ Pre-flight failed.")
            self.summary.stopped_reason = "preflight_failed"
            self._save_summary()
            notify("APR auto: pre-flight failed", config, logger)
            sys.exit(1)

        workflow_name = resolve_workflow_name(config.workflow)
        rounds_dir_path = resolve_rounds_dir(workflow_name)
        self._workflow_name = workflow_name
        self._rounds_dir = rounds_dir_path
        logger.info(f"  📁 Rounds dir: {rounds_dir_path}")

        # Detect start
        if config.start_round is not None:
            start = config.start_round
            logger.info(f"\n📍 Starting at round {start} (manual)")
        else:
            last = detect_last_round(rounds_dir_path)
            start = last + 1
            if last > 0:
                logger.info(
                    f"\n📍 {last} existing rounds → "
                    f"starting at round {start}"
                )
                self._seed_output_sizes(rounds_dir_path, last)
            else:
                logger.info("\n📍 Fresh start at round 1")

        end = start + config.max_rounds

        if start > 1:
            logger.info("  📊 Backfilling metrics...")
            run_backfill(config, logger, force=True)

        if config.dry_run:
            logger.info(
                f"\n🏁 Dry run — would run rounds {start}–{end - 1} "
                f"until {config.convergence_target}%"
            )
            return

        logger.info(
            f"\n🎯 Target: {config.convergence_target}% "
            f"(cap: round {end - 1})\n"
        )

        # === The loop ===
        consecutive_failures = 0
        round_num = start

        while round_num < end:
            if self._shutting_down:
                self.summary.stopped_reason = "shutdown"
                break

            logger.info(f"{'─' * 50}")
            logger.info(f"📍 Round {round_num}")
            logger.info(f"{'─' * 50}")

            write_status(
                config.log_dir, round_num,
                self.summary.rounds_completed, "running",
            )

            # --- Execute via apr run --wait ---
            t0 = time.time()
            logger.info("  🚀 apr run --wait...")
            success, error_msg = run_apr_round(round_num, config, logger)
            duration = time.time() - t0

            if not success:
                consecutive_failures += 1
                self.summary.rounds_failed += 1
                logger.error(f"  ❌ {error_msg}")
                self.summary.results.append(asdict(RoundResult(
                    round_num=round_num, success=False,
                    error_msg=error_msg, duration_seconds=duration,
                )))
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES:
                    logger.error(
                        f"\n🛑 {MAX_CONSECUTIVE_FAILURES} consecutive "
                        f"failures."
                    )
                    self.summary.stopped_reason = "consecutive_failures"
                    break
                if FAILURE_BACKOFF_SECONDS > 0:
                    logger.info(f"  ⏳ Backoff {FAILURE_BACKOFF_SECONDS}s before retry")
                    time.sleep(FAILURE_BACKOFF_SECONDS)
                logger.info("")
                continue  # retry same round_num

            # --- Verify output ---
            rf = round_file(rounds_dir_path, round_num)
            chars, line_count = 0, 0
            truncated = False

            if rf.exists():
                content = rf.read_text(encoding="utf-8", errors="replace")
                chars = len(content)
                line_count = content.count("\n")
            truncated = check_output_truncation(
                rounds_dir_path, round_num,
                self._output_sizes, logger,
            )
            if chars > 0 and not truncated:
                self._output_sizes.append(chars)

            if truncated:
                attempts = self._truncation_attempts.get(round_num, 0) + 1
                self._truncation_attempts[round_num] = attempts
                logger.warning(
                    f"  ⚠️  Output may be truncated "
                    f"({chars} chars, {line_count} lines)"
                )

                if rf.exists():
                    backup = rounds_dir_path / (
                        f"round_{round_num}.truncated.{attempts}.md"
                    )
                    try:
                        rf.rename(backup)
                        logger.info(f"  📄 Moved truncated output to {backup}")
                    except OSError as e:
                        logger.warning(f"  ⚠️  Could not move truncated file: {e}")

                # --- CDP recovery attempt (before falling through to retry) ---
                recovered = attempt_cdp_recovery(round_num, config, logger)
                if recovered:
                    # Write recovered content to the round file
                    try:
                        rf.write_text(recovered, encoding="utf-8")
                        logger.info(
                            f"  📝 Wrote CDP-recovered content to {rf}"
                        )
                    except OSError as e:
                        logger.warning(f"  ⚠️  Could not write recovered content: {e}")
                        recovered = None

                if recovered:
                    # Re-check truncation on recovered content
                    chars = len(recovered)
                    line_count = recovered.count("\n")
                    still_truncated = check_output_truncation(
                        rounds_dir_path, round_num,
                        self._output_sizes, logger,
                    )
                    if not still_truncated:
                        logger.info(
                            f"  ✅ CDP recovery accepted "
                            f"({chars} chars, {line_count} lines)"
                        )
                        truncated = False
                        consecutive_failures = 0
                        self._output_sizes.append(chars)
                        # Fall through to the success path below
                    else:
                        logger.warning(
                            "  ⚠️  CDP-recovered content still looks truncated"
                        )
                        # Fall through to retry logic below

                if truncated:
                    # Original retry logic (unchanged)
                    consecutive_failures += 1
                    self.summary.rounds_failed += 1
                    self.summary.results.append(asdict(RoundResult(
                        round_num=round_num, success=False,
                        error_msg="output_truncated", duration_seconds=duration,
                        round_chars=chars, round_lines=line_count,
                        truncated=True,
                        cdp_recovery_attempted=recovered is not None or config.cdp_recovery_enabled,
                    )))

                    if attempts < MAX_TRUNCATION_RETRIES:
                        logger.warning(
                            f"  🔁 Retrying round {round_num} "
                            f"(truncation {attempts}/{MAX_TRUNCATION_RETRIES})"
                        )
                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES:
                            logger.error(
                                f"\n🛑 {MAX_CONSECUTIVE_FAILURES} consecutive "
                                f"failures."
                            )
                            self.summary.stopped_reason = "consecutive_failures"
                            break
                        if FAILURE_BACKOFF_SECONDS > 0:
                            logger.info(f"  ⏳ Backoff {FAILURE_BACKOFF_SECONDS}s before retry")
                            time.sleep(FAILURE_BACKOFF_SECONDS)
                        logger.info("")
                        continue

                    logger.error(
                        f"\n🛑 Stopping: output truncated {attempts} times."
                    )
                    self.summary.stopped_reason = "truncated_output"
                    break

            consecutive_failures = 0
            self.summary.rounds_completed += 1
            logger.info(
                f"  ✅ Done ({chars} chars, {line_count} lines, "
                f"{duration:.0f}s)"
            )

            # --- Backfill + convergence ---
            backfill_ok = run_backfill(config, logger, force=True)
            conv_info = ""
            conv_pct = None
            if backfill_ok:
                conv_pct = read_stability_score(workflow_name, config, logger)
                if conv_pct is not None:
                    conv_info = f"{conv_pct:.1f}%"
                    logger.info(f"  📊 Stability score: {conv_info}")
                else:
                    logger.warning("  ⚠️  Stats unavailable; convergence unknown")
            else:
                logger.warning("  ⚠️  Backfill failed; convergence unknown")

            # --- Integrate + commit ---
            integrated = False
            if config.integrate:
                logger.info(f"  🔧 Integrating round {round_num}...")
                integrated = run_integrate(round_num, config, logger)

            committed, commit_sha = False, None
            if config.commit:
                sha = git_commit_and_push(round_num, conv_info, logger)
                if sha:
                    committed, commit_sha = True, sha

            # --- Record result ---
            self.summary.results.append(asdict(RoundResult(
                round_num=round_num, success=True,
                duration_seconds=duration, convergence_pct=conv_pct,
                round_chars=chars, round_lines=line_count,
                truncated=truncated, integrated=integrated,
                committed=committed, commit_sha=commit_sha,
            )))

            # --- Check convergence ---
            converged = False
            if conv_pct is not None and conv_pct >= config.convergence_target:
                logger.info(
                    f"\n🎯 Converged! {conv_pct:.1f}% >= "
                    f"{config.convergence_target}%"
                )
                converged = True

            if converged:
                self.summary.stopped_reason = "converged"
                break

            if conv_pct is not None:
                gap = config.convergence_target - conv_pct
                logger.info(f"  📈 {gap:.1f}% to go")

            round_num += 1

            if config.cooldown > 0 and round_num < end:
                logger.debug(f"  💤 {config.cooldown}s cooldown...")
                time.sleep(config.cooldown)

            logger.info("")

        # === End of loop ===
        if round_num >= end and not self.summary.stopped_reason:
            self.summary.stopped_reason = "max_rounds_reached"
            logger.warning(
                f"\n⚠️  Cap ({config.max_rounds} rounds) reached "
                f"without {config.convergence_target}% convergence"
            )

        self.summary.finished_at = datetime.now(timezone.utc).isoformat()
        if not self.summary.stopped_reason:
            self.summary.stopped_reason = "completed"

        self._print_summary()
        self._save_summary()
        write_status(
            config.log_dir, round_num,
            self.summary.rounds_completed,
            self.summary.stopped_reason,
        )
        notify(
            f"APR auto: {self.summary.stopped_reason} — "
            f"{self.summary.rounds_completed} rounds",
            config, logger,
        )

    # ----- Helpers -----

    def _seed_output_sizes(self, rounds_dir: Path, up_to: int):
        """Seed rolling average from last few round files."""
        for n in range(max(1, up_to - 4), up_to + 1):
            rf = rounds_dir / f"round_{n}.md"
            if rf.exists():
                try:
                    size = rf.stat().st_size
                    if size > 0:
                        self._output_sizes.append(size)
                except OSError:
                    pass
        if self._output_sizes:
            avg = sum(self._output_sizes) / len(self._output_sizes)
            self.logger.debug(
                f"  Seeded from {len(self._output_sizes)} rounds "
                f"(avg {avg:.0f} chars)"
            )

    def _print_summary(self):
        s = self.summary
        self.logger.info("")
        self.logger.info("=" * 55)
        self.logger.info("  Summary")
        self.logger.info("=" * 55)
        self.logger.info(f"  Status:    {s.stopped_reason}")
        self.logger.info(f"  Completed: {s.rounds_completed}")
        self.logger.info(f"  Failed:    {s.rounds_failed}")
        if s.results:
            last = s.results[-1]
            pct = last.get("convergence_pct")
            if pct is not None:
                self.logger.info(f"  Final:     {pct:.1f}%")
        self.logger.info("=" * 55)

    def _save_summary(self):
        payload = asdict(self.summary)
        atomic_write_json(self.config.log_dir / "last_run.json", payload, indent=2)


# =============================================================================
# CLI
# =============================================================================


def parse_args() -> Config:
    p = argparse.ArgumentParser(
        description="Thin APR orchestrator — loop until convergence."
    )
    p.add_argument(
        "-t", "--target", type=float, default=DEFAULT_CONVERGENCE_TARGET,
        help=f"Convergence target %% (default: {DEFAULT_CONVERGENCE_TARGET})",
    )
    p.add_argument(
        "-n", "--max-rounds", type=int, default=DEFAULT_MAX_ROUNDS,
        help=f"Safety cap (default: {DEFAULT_MAX_ROUNDS})",
    )
    p.add_argument(
        "-s", "--start-round", type=int, default=None,
        help="Override auto-detect start round",
    )
    p.add_argument(
        "-w", "--workflow", type=str, default=DEFAULT_WORKFLOW,
        help=f"Workflow name (default: {DEFAULT_WORKFLOW})",
    )
    p.add_argument(
        "--cooldown", type=int, default=DEFAULT_COOLDOWN_SECONDS,
        help=f"Seconds between rounds (default: {DEFAULT_COOLDOWN_SECONDS})",
    )
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--no-integrate", action="store_true")
    p.add_argument("--no-commit", action="store_true")
    p.add_argument("--log-dir", type=str, default=None)
    p.add_argument(
        "--no-cdp-recovery", action="store_true",
        help="Disable CDP recovery on truncation",
    )
    p.add_argument(
        "--cdp-ssh-host", type=str, default=None,
        help="SSH host for CDP recovery (default: oracle host)",
    )

    args = p.parse_args()
    config = Config(
        max_rounds=args.max_rounds,
        start_round=args.start_round,
        workflow=args.workflow,
        convergence_target=args.target,
        cooldown=args.cooldown,
        dry_run=args.dry_run,
        integrate=not args.no_integrate,
        commit=not args.no_commit,
        cdp_recovery_enabled=not args.no_cdp_recovery,
    )
    if args.log_dir:
        config.log_dir = Path(args.log_dir)
    if args.cdp_ssh_host:
        config.cdp_recovery_ssh_host = args.cdp_ssh_host
    return config


def main():
    config = parse_args()
    logger = setup_logging(config.log_dir)
    Orchestrator(config, logger).run()


if __name__ == "__main__":
    main()
