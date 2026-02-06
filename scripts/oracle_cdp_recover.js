#!/usr/bin/env node
/**
 * oracle_cdp_recover.js — CDP-based recovery of full GPT responses.
 *
 * Connects to Chrome via Chrome DevTools Protocol and extracts the last
 * assistant message from a ChatGPT tab. Designed to recover responses
 * that Oracle's 5.5s verification window truncated.
 *
 * Usage:
 *   node oracle_cdp_recover.js [--timeout 120] [--min-length 200] [--stable-polls 10]
 *
 * Prerequisites:
 *   - Chrome running with remote debugging (ORACLE_BROWSER_PORT=9222)
 *   - npm install -g chrome-remote-interface
 *
 * Output: assistant response markdown on stdout, diagnostics on stderr.
 * Exit: 0 on success, 1 on error.
 */

'use strict';

const fs = require('fs');
const path = require('path');
const os = require('os');

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

function parseArgs() {
  const args = process.argv.slice(2);
  const opts = {
    timeout: 120,
    minLength: 200,
    stablePolls: 10,
    pollInterval: 1000,
  };

  for (let i = 0; i < args.length; i++) {
    switch (args[i]) {
      case '--timeout':
        opts.timeout = parseInt(args[++i], 10);
        break;
      case '--min-length':
        opts.minLength = parseInt(args[++i], 10);
        break;
      case '--stable-polls':
        opts.stablePolls = parseInt(args[++i], 10);
        break;
      case '--poll-interval':
        opts.pollInterval = parseInt(args[++i], 10);
        break;
      case '--help':
        console.error('Usage: node oracle_cdp_recover.js [--timeout 120] [--min-length 200] [--stable-polls 10]');
        process.exit(0);
        break;
      default:
        console.error(`Unknown argument: ${args[i]}`);
        process.exit(1);
    }
  }

  return opts;
}

// ---------------------------------------------------------------------------
// Chrome discovery
// ---------------------------------------------------------------------------

function readCdpPort() {
  const profileDir = path.join(os.homedir(), '.oracle', 'browser-profile');
  const portFile = path.join(profileDir, 'DevToolsActivePort');

  if (!fs.existsSync(portFile)) {
    throw new Error(`CDP port file not found: ${portFile}`);
  }

  const content = fs.readFileSync(portFile, 'utf-8').trim();
  const lines = content.split('\n');
  const port = parseInt(lines[0], 10);

  if (isNaN(port) || port <= 0) {
    throw new Error(`Invalid CDP port in ${portFile}: ${lines[0]}`);
  }

  return port;
}

function verifyChromePid() {
  const pidFile = path.join(os.homedir(), '.oracle', 'browser-profile', 'chrome.pid');

  if (!fs.existsSync(pidFile)) {
    console.error('[cdp-recover] Warning: chrome.pid not found, skipping PID check');
    return;
  }

  const pid = parseInt(fs.readFileSync(pidFile, 'utf-8').trim(), 10);
  if (isNaN(pid)) {
    console.error('[cdp-recover] Warning: invalid PID in chrome.pid');
    return;
  }

  try {
    process.kill(pid, 0); // signal 0 = check existence
  } catch (err) {
    if (err.code === 'ESRCH') {
      throw new Error(`Chrome process ${pid} is not running`);
    }
    // EPERM means it exists but we can't signal it — fine
  }

  console.error(`[cdp-recover] Chrome PID ${pid} is alive`);
}

// ---------------------------------------------------------------------------
// DOM selectors (matching Oracle's constants.js)
// ---------------------------------------------------------------------------

const SELECTORS = {
  STOP_BUTTON: '[data-testid="stop-button"]',
  COPY_BUTTON: 'button[data-testid="copy-turn-action-button"]',
  THUMBS_UP: 'button[data-testid="good-response-turn-action-button"]',
  ASSISTANT_TURN: '[data-message-author-role="assistant"]',
};

// ---------------------------------------------------------------------------
// CDP interaction
// ---------------------------------------------------------------------------

async function findChatGptTab(CDP, port) {
  const targets = await CDP.List({ port });
  const tab = targets.find(
    (t) => t.type === 'page' && t.url && t.url.includes('chatgpt.com')
  );

  if (!tab) {
    const urls = targets
      .filter((t) => t.type === 'page')
      .map((t) => t.url);
    throw new Error(
      `No ChatGPT tab found. Open tabs: ${urls.join(', ') || '(none)'}`
    );
  }

  console.error(`[cdp-recover] Found ChatGPT tab: ${tab.url}`);
  return tab;
}

async function evalInTab(Runtime, expression) {
  const result = await Runtime.evaluate({
    expression,
    returnByValue: true,
    awaitPromise: false,
  });

  if (result.exceptionDetails) {
    const msg = result.exceptionDetails.text ||
      (result.exceptionDetails.exception && result.exceptionDetails.exception.description) ||
      'Unknown eval error';
    throw new Error(`JS eval error: ${msg}`);
  }

  return result.result.value;
}

async function waitForStableResponse(Runtime, opts) {
  const deadline = Date.now() + opts.timeout * 1000;
  let stableCount = 0;
  let lastText = null;
  let lastLength = 0;

  console.error(`[cdp-recover] Polling for stable response (timeout=${opts.timeout}s, need ${opts.stablePolls} stable polls)...`);

  while (Date.now() < deadline) {
    // Check if still generating (stop button visible)
    const stopVisible = await evalInTab(Runtime, `
      !!document.querySelector('${SELECTORS.STOP_BUTTON}')
    `);

    if (stopVisible) {
      console.error('[cdp-recover] Stop button visible — response still generating');
      stableCount = 0;
      lastText = null;
      await sleep(opts.pollInterval);
      continue;
    }

    // Check for finished action buttons
    const actionsVisible = await evalInTab(Runtime, `
      !!(document.querySelector('${SELECTORS.COPY_BUTTON}') ||
         document.querySelector('${SELECTORS.THUMBS_UP}'))
    `);

    // Extract last assistant message text
    const currentText = await evalInTab(Runtime, `
      (() => {
        const turns = document.querySelectorAll('${SELECTORS.ASSISTANT_TURN}');
        if (turns.length === 0) return null;
        const last = turns[turns.length - 1];
        return last.innerText || null;
      })()
    `);

    if (!currentText) {
      console.error('[cdp-recover] No assistant message found in DOM');
      stableCount = 0;
      lastText = null;
      await sleep(opts.pollInterval);
      continue;
    }

    const currentLength = currentText.length;

    if (currentText === lastText && actionsVisible) {
      stableCount++;
      if (stableCount % 3 === 0 || stableCount >= opts.stablePolls) {
        console.error(`[cdp-recover] Stable: ${stableCount}/${opts.stablePolls} (${currentLength} chars, actions=${actionsVisible})`);
      }
    } else {
      if (lastText !== null && currentLength !== lastLength) {
        console.error(`[cdp-recover] Text changed: ${lastLength} -> ${currentLength} chars`);
      }
      stableCount = actionsVisible ? 1 : 0;
    }

    lastText = currentText;
    lastLength = currentLength;

    if (stableCount >= opts.stablePolls) {
      console.error(`[cdp-recover] Response stable after ${stableCount} polls (${currentLength} chars)`);
      return currentText;
    }

    await sleep(opts.pollInterval);
  }

  // Timeout — return what we have if it meets minimum
  if (lastText && lastText.length >= opts.minLength) {
    console.error(`[cdp-recover] Timeout reached but returning ${lastText.length} chars (meets minimum)`);
    return lastText;
  }

  throw new Error(
    `Timeout after ${opts.timeout}s. Last length: ${lastLength}, stable: ${stableCount}/${opts.stablePolls}`
  );
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  const opts = parseArgs();

  // Discover Chrome
  const port = readCdpPort();
  console.error(`[cdp-recover] CDP port: ${port}`);
  verifyChromePid();

  // Load chrome-remote-interface
  let CDP;
  try {
    CDP = require('chrome-remote-interface');
  } catch (err) {
    console.error('[cdp-recover] ERROR: chrome-remote-interface not installed');
    console.error('[cdp-recover] Run: npm install -g chrome-remote-interface');
    process.exit(1);
  }

  // Find ChatGPT tab
  const tab = await findChatGptTab(CDP, port);

  // Connect to tab
  let client;
  try {
    client = await CDP({ target: tab, port });
  } catch (err) {
    throw new Error(`Failed to connect to tab: ${err.message}`);
  }

  try {
    const { Runtime } = client;
    await Runtime.enable();

    // Wait for stable response
    const text = await waitForStableResponse(Runtime, opts);

    // Validate
    if (!text || text.length < opts.minLength) {
      throw new Error(
        `Response too short: ${text ? text.length : 0} chars (minimum: ${opts.minLength})`
      );
    }

    // Output to stdout
    process.stdout.write(text);
    console.error(`[cdp-recover] Success: ${text.length} chars extracted`);
  } finally {
    await client.close();
  }
}

main().catch((err) => {
  console.error(`[cdp-recover] FATAL: ${err.message}`);
  process.exit(1);
});
