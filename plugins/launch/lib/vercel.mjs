// Vercel CLI calls: login, environment variables and the production deploy.

import { existsSync } from 'node:fs';
import fs from 'node:fs/promises';
import path from 'node:path';
import { run, spawnInteractive, baseEnv, tailLines } from './exec.mjs';
import { ensureVercelignore } from './github.mjs';
import { LOCAL_ONLY_KEYS, extractDeviceCode, extractDeviceUrl, isLocalOnlyValue, parseDeployUrl, parseEnvFile, parseInspectAlias, stripAnsi } from './parse.mjs';

function vercelEnv(extra = {}) {
  return baseEnv({ CI: '', ...extra });
}

/** `{ loggedIn, username }` from `vercel whoami`. Never throws. */
export async function vercelWhoami(vercelBin, cwd) {
  const result = await run(vercelBin, ['whoami'], { cwd, env: vercelEnv(), timeoutMs: 15_000 });
  if (result.code !== 0) return { loggedIn: false, username: null };
  const name = result.stdout.trim().split('\n').pop()?.trim();
  return { loggedIn: Boolean(name), username: name || null };
}

/**
 * Runs `vercel login` non-interactively. Vercel's login is normally an interactive prompt (pick a
 * provider, or a device code for some flows); without a TTY it either prints a device code — same
 * handling as `gh auth login --web` — or it can't proceed at all. We give it `detectTimeoutMs` to
 * show a code, and if nothing usable appears we kill it and ask the caller to fall back to a
 * terminal. Resolves `{ ok, needsFallback, username }`.
 */
export async function vercelLogin(vercelBin, cwd, { onCode, detectTimeoutMs = 15_000, timeoutMs = 10 * 60_000 } = {}) {
  let code = null;
  let url = null;
  let announced = false;
  let sawPrompt = false;
  const proc = spawnInteractive(vercelBin, ['login'], {
    cwd,
    env: vercelEnv(),
    timeoutMs,
    onData: (_chunk, buffer) => {
      code = code ?? extractDeviceCode(buffer);
      url = url ?? extractDeviceUrl(buffer);
      if (code && url && !announced) {
        announced = true;
        onCode?.(code, url);
      }
      if (/press enter/i.test(buffer.slice(-400))) proc.write('\n');
      if (/use arrows to move|continue with|enter your email/i.test(buffer)) sawPrompt = true;
    },
  });
  const detector = new Promise((resolve) => {
    const timer = setTimeout(() => resolve('timeout'), detectTimeoutMs);
    const check = setInterval(() => {
      if (announced) {
        clearInterval(check);
        clearTimeout(timer);
        resolve('code');
      } else if (sawPrompt) {
        clearInterval(check);
        clearTimeout(timer);
        resolve('prompt');
      }
    }, 200);
  });
  const outcome = await Promise.race([proc.wait.then(() => 'exited'), detector]);
  if (outcome === 'prompt' || (outcome === 'timeout' && !announced)) {
    proc.kill();
    return { ok: false, needsFallback: true, username: null };
  }
  const { code: exitCode } = await proc.wait;
  if (exitCode !== 0) return { ok: false, needsFallback: !announced, username: null };
  const status = await vercelWhoami(vercelBin, cwd);
  return { ok: status.loggedIn, needsFallback: false, username: status.username };
}

const ENV_FILES = ['.env', '.env.local', '.env.production'];

/** Env files present at `root`. `{ files: string[], vars: { KEY: { value, isLocal, files } } }` */
export async function discoverEnvVars(root) {
  const present = [];
  for (const name of ENV_FILES) {
    if (existsSync(path.join(root, name))) present.push(name);
  }
  const vars = {};
  for (const name of present) {
    let content = '';
    try {
      content = await fs.readFile(path.join(root, name), 'utf8');
    } catch {
      continue;
    }
    for (const [key, value] of Object.entries(parseEnvFile(content))) {
      vars[key] = vars[key] ?? { value, files: [] };
      vars[key].value = value; // later files (more specific) win
      vars[key].files.push(name);
    }
  }
  for (const [key, entry] of Object.entries(vars)) entry.isLocal = LOCAL_ONLY_KEYS.includes(key) || isLocalOnlyValue(entry.value);
  return { files: present, vars };
}

/** `vercel env add KEY production`, piping the value on stdin. Treats "already exists" as success. */
export async function addEnvVar(vercelBin, cwd, key, value) {
  const result = await run(vercelBin, ['env', 'add', key, 'production', '--yes'], { cwd, env: vercelEnv(), timeoutMs: 30_000, input: `${value}\n` });
  if (result.code === 0) return { added: true };
  if (/already exists/i.test(result.stderr + result.stdout)) return { added: false, alreadyExists: true };
  throw new Error(`could not add ${key}: ${result.stderr || result.stdout}`);
}

/**
 * `vercel deploy --prod --yes`, streaming output to `onLine` (last ~12 lines) as it runs.
 * Resolves `{ ok, url, output }`.
 */
export async function deployProduction(vercelBin, cwd, { onLine } = {}) {
  await ensureVercelignore(cwd);
  const proc = spawnInteractive(vercelBin, ['deploy', '--prod', '--yes'], {
    cwd,
    env: vercelEnv(),
    timeoutMs: 10 * 60_000,
    onData: (_chunk, buffer) => onLine?.(tailLines(buffer, 12)),
  });
  const { code, output } = await proc.wait;
  let url = parseDeployUrl(output);
  if (code !== 0) throw new Error(`vercel deploy failed:\n${tailLines(output, 20).join('\n')}`);
  if (!url) throw new Error(`deploy finished but no URL was found in the output:\n${tailLines(output, 20).join('\n')}`);
  // No alias line: ask Vercel for the project's public domain instead of the protected deployment URL.
  if (!/Aliased/i.test(stripAnsi(output))) {
    const inspect = await run(vercelBin, ['inspect', url], { cwd, env: vercelEnv(), timeoutMs: 30_000 });
    url = parseInspectAlias(inspect.stdout + inspect.stderr) ?? url;
  }
  return { ok: true, url, output };
}

/** Best-effort: connects the Vercel project to the git repo so future pushes auto-deploy. */
export async function connectGit(vercelBin, cwd) {
  const result = await run(vercelBin, ['git', 'connect', '--yes'], { cwd, env: vercelEnv(), timeoutMs: 30_000 });
  return result.code === 0;
}
