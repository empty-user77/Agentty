// Process helpers: run a command to completion, or spawn one and watch its output live (for
// non-interactive login flows that print a device code and wait for Enter).

import { spawn } from 'node:child_process';
import path from 'node:path';
import { stripAnsi } from './parse.mjs';

/** Extra PATH entries plugins should check besides the login-shell PATH Agentty already gives them. */
export const EXTRA_PATH = ['/opt/homebrew/bin', '/usr/local/bin'];

/**
 * PATH for spawned tools. The folder of the Node.js running this plugin goes first: `npm`, `vercel`
 * and `supabase` all start with `#!/usr/bin/env node`, and a Node.js from nvm, fnm or Volta is not
 * on the PATH of an app started from the Dock ("env: node: No such file or directory").
 */
export function pathWithExtras(env = process.env, nodeDir = path.dirname(process.execPath)) {
  const parts = String(env.PATH ?? '').split(':').filter(Boolean);
  if (nodeDir && !parts.includes(nodeDir)) parts.unshift(nodeDir);
  for (const dir of EXTRA_PATH) if (!parts.includes(dir)) parts.push(dir);
  return parts.join(':');
}

/** Common environment for spawned tools: no interactive prompts, no color codes. */
export function baseEnv(extra = {}) {
  return {
    ...process.env,
    PATH: pathWithExtras(process.env),
    GIT_TERMINAL_PROMPT: '0',
    NO_COLOR: '1',
    FORCE_COLOR: '0',
    ...extra,
  };
}

/** Runs a command to completion. Resolves (never rejects) with `{ code, stdout, stderr }`, ANSI-stripped. */
export function run(cmd, args, { cwd, env, timeoutMs = 60_000, input } = {}) {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(cmd, args, { cwd, env: env ?? baseEnv(), stdio: ['pipe', 'pipe', 'pipe'] });
    } catch (err) {
      resolve({ code: -1, stdout: '', stderr: String(err?.message ?? err) });
      return;
    }
    let stdout = '';
    let stderr = '';
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      child.kill('SIGKILL');
      resolve({ code: -1, stdout: stripAnsi(stdout), stderr: `${stripAnsi(stderr)}\n(timed out after ${timeoutMs}ms)` });
    }, timeoutMs);
    child.stdout.on('data', (d) => (stdout += d));
    child.stderr.on('data', (d) => (stderr += d));
    if (input !== undefined) child.stdin.write(input);
    child.stdin.end();
    child.on('close', (code) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({ code, stdout: stripAnsi(stdout), stderr: stripAnsi(stderr) });
    });
    child.on('error', (err) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({ code: -1, stdout: stripAnsi(stdout), stderr: String(err?.message ?? err) });
    });
  });
}

/**
 * Spawns a long-running process (a login flow) and streams its combined stdout+stderr to
 * `onData` as it arrives. Returns a handle to write to stdin and await the exit code.
 */
export function spawnInteractive(cmd, args, { cwd, env, onData, timeoutMs = 10 * 60_000 } = {}) {
  const child = spawn(cmd, args, { cwd, env: env ?? baseEnv(), stdio: ['pipe', 'pipe', 'pipe'] });
  let buffer = '';
  const feed = (chunk) => {
    const text = stripAnsi(chunk.toString());
    buffer += text;
    onData?.(text, buffer);
  };
  child.stdout.on('data', feed);
  child.stderr.on('data', feed);
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    child.kill('SIGKILL');
  }, timeoutMs);
  const wait = new Promise((resolve) => {
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({ code: timedOut ? -1 : code, output: buffer, timedOut });
    });
    child.on('error', () => {
      clearTimeout(timer);
      resolve({ code: -1, output: buffer, timedOut });
    });
  });
  return {
    child,
    wait,
    get output() {
      return buffer;
    },
    write(text) {
      try {
        child.stdin.write(text);
      } catch {
        // Process already exited.
      }
    },
    kill() {
      clearTimeout(timer);
      try {
        child.kill('SIGKILL');
      } catch {
        // Already dead.
      }
    },
  };
}

/** Last `n` non-empty lines of some command output, for a small log view. */
export function tailLines(text, n = 12) {
  return stripAnsi(text)
    .split(/\n/)
    .map((l) => l.trimEnd())
    .filter((l) => l.length > 0)
    .slice(-n);
}
