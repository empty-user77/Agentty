// Project detection, git plumbing and `gh` (GitHub CLI) calls: the "Project check" and
// "Save to GitHub" steps.

import { existsSync } from 'node:fs';
import fs from 'node:fs/promises';
import path from 'node:path';
import { run, spawnInteractive, baseEnv } from './exec.mjs';
import { detectFramework, envFilesAtRisk, mergeGitignore, sanitizeRepoName, extractDeviceCode, extractDeviceUrl } from './parse.mjs';

/** Walks up from `startDir` to the nearest `.git` or `package.json`, else returns `startDir` itself. */
export async function findProjectRoot(startDir) {
  let dir = startDir;
  for (let i = 0; i < 40; i++) {
    if (existsSync(path.join(dir, '.git')) || existsSync(path.join(dir, 'package.json'))) return dir;
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return startDir;
}

/** Reads and parses `package.json` at `root`, or `null` if there isn't one / it doesn't parse. */
export async function readPackageJson(root) {
  try {
    return JSON.parse(await fs.readFile(path.join(root, 'package.json'), 'utf8'));
  } catch {
    return null;
  }
}

/** `{ root, framework, pkgName, isGitRepo, hasPackageJson, hasIndexHtml }` for the panel's first step. */
export async function inspectProject(root) {
  const pkg = await readPackageJson(root);
  const hasIndexHtml = existsSync(path.join(root, 'index.html'));
  return {
    root,
    pkg,
    hasPackageJson: pkg !== null,
    hasIndexHtml,
    isGitRepo: existsSync(path.join(root, '.git')),
    framework: detectFramework({ pkg, hasIndexHtml }),
    pkgName: pkg?.name ?? null,
  };
}

async function git(cwd, args, opts) {
  return run('git', args, { cwd, timeoutMs: 30_000, env: baseEnv(), ...opts });
}

export async function isGitRepo(cwd) {
  const result = await git(cwd, ['rev-parse', '--is-inside-work-tree']);
  return result.code === 0;
}

export async function ensureGitRepo(cwd) {
  if (await isGitRepo(cwd)) return;
  const result = await git(cwd, ['init', '-b', 'main']);
  if (result.code !== 0) throw new Error(`git init failed: ${result.stderr || result.stdout}`);
}

/** Adds any missing lines from `parse.mjs`'s `REQUIRED_GITIGNORE_LINES`; leaves the rest alone. */
export async function ensureGitignore(root) {
  const file = path.join(root, '.gitignore');
  let existing = '';
  try {
    existing = await fs.readFile(file, 'utf8');
  } catch {
    // No .gitignore yet.
  }
  const merged = mergeGitignore(existing);
  if (merged !== existing) await fs.writeFile(file, merged);
}

/** Every `.env*` file (besides `.env.example`/`.env.sample`) already tracked or staged, if any. */
export async function envFilesToRefuse(cwd) {
  const tracked = await git(cwd, ['ls-files', '--', '.env*']);
  const staged = await git(cwd, ['diff', '--cached', '--name-only', '--', '.env*']);
  const others = await git(cwd, ['status', '--porcelain', '--', '.env*']);
  const fromStatus = others.stdout
    .split('\n')
    .map((l) => l.slice(3).trim())
    .filter(Boolean);
  const paths = new Set([...tracked.stdout.split('\n'), ...staged.stdout.split('\n'), ...fromStatus].map((l) => l.trim()).filter(Boolean));
  return envFilesAtRisk([...paths]);
}

export async function setLocalGitUserIfMissing(cwd, { name, email }) {
  const current = await git(cwd, ['config', 'user.name']);
  if (!current.stdout.trim()) await git(cwd, ['config', 'user.name', name]);
  const currentEmail = await git(cwd, ['config', 'user.email']);
  if (!currentEmail.stdout.trim()) await git(cwd, ['config', 'user.email', email]);
}

/** `git add -A` then a commit, unless the tree is already clean. Returns whether it committed. */
export async function commitAll(cwd, message) {
  await git(cwd, ['add', '-A']);
  const status = await git(cwd, ['status', '--porcelain']);
  if (!status.stdout.trim()) return false;
  const result = await git(cwd, ['commit', '-m', message]);
  if (result.code !== 0) throw new Error(`git commit failed: ${result.stderr || result.stdout}`);
  return true;
}

export async function hasOrigin(cwd) {
  const result = await git(cwd, ['remote', 'get-url', 'origin']);
  return result.code === 0 ? result.stdout.trim() : null;
}

export async function pushOrigin(cwd) {
  const result = await git(cwd, ['push', '-u', 'origin', 'HEAD']);
  if (result.code !== 0) throw new Error(`git push failed: ${result.stderr || result.stdout}`);
}

// -- gh CLI -----------------------------------------------------------------------------------

function ghEnv(extra = {}) {
  return baseEnv({ GH_PROMPT_DISABLED: '1', ...extra });
}

/** `{ loggedIn, username }` from `gh auth status`. Never throws. */
export async function ghAuthStatus(ghBin, cwd) {
  const result = await run(ghBin, ['auth', 'status', '-h', 'github.com'], { cwd, env: ghEnv(), timeoutMs: 15_000 });
  if (result.code !== 0) return { loggedIn: false, username: null };
  const whoami = await run(ghBin, ['api', 'user', '--jq', '.login'], { cwd, env: ghEnv(), timeoutMs: 15_000 });
  return { loggedIn: true, username: whoami.code === 0 ? whoami.stdout.trim() : null };
}

/** `{ login, id }` of the signed-in user, for the noreply commit email Launch sets locally. */
export async function ghUserIdentity(ghBin, cwd) {
  const result = await run(ghBin, ['api', 'user', '--jq', '{login: .login, id: .id}'], { cwd, env: ghEnv(), timeoutMs: 15_000 });
  if (result.code !== 0) return null;
  try {
    return JSON.parse(result.stdout.trim());
  } catch {
    return null;
  }
}

/**
 * Runs `gh auth login --web` non-interactively. Calls `onCode(code, url)` as soon as both are
 * seen in its output, and resolves `{ ok, username }` once the browser flow completes or times out.
 */
export async function ghLogin(ghBin, cwd, { onCode, timeoutMs = 10 * 60_000 } = {}) {
  let code = null;
  let url = null;
  let announced = false;
  const proc = spawnInteractive(ghBin, ['auth', 'login', '--web', '-h', 'github.com', '-p', 'https', '--skip-ssh-key'], {
    cwd,
    env: baseEnv(), // GH_PROMPT_DISABLED unset here — it breaks the web login flow.
    timeoutMs,
    onData: (_chunk, buffer) => {
      code = code ?? extractDeviceCode(buffer);
      url = url ?? extractDeviceUrl(buffer);
      if (code && url && !announced) {
        announced = true;
        onCode?.(code, url);
      }
      // gh asks to "Press Enter to open github.com in your browser" — we open it ourselves via
      // `onCode`, but still need to unblock the prompt.
      if (/press enter/i.test(buffer.slice(-400))) proc.write('\n');
    },
  });
  const { code: exitCode } = await proc.wait;
  if (exitCode !== 0) return { ok: false, username: null };
  await run(ghBin, ['auth', 'setup-git'], { cwd, env: ghEnv(), timeoutMs: 15_000 });
  const status = await ghAuthStatus(ghBin, cwd);
  return { ok: status.loggedIn, username: status.username };
}

/** Creates a private (or public) GitHub repo from `cwd`, retrying with a numeric suffix if the name is taken. */
export async function createAndPushRepo(ghBin, cwd, { name, isPublic = false, maxAttempts = 8 } = {}) {
  const base = sanitizeRepoName(name);
  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    const candidate = attempt === 1 ? base : `${base}-${attempt}`;
    const args = ['repo', 'create', candidate, isPublic ? '--public' : '--private', '--source', '.', '--remote', 'origin', '--push'];
    const result = await run(ghBin, args, { cwd, env: ghEnv(), timeoutMs: 60_000 });
    if (result.code === 0) return { name: candidate };
    if (!/already exists|name already taken/i.test(result.stderr + result.stdout)) {
      throw new Error(`gh repo create failed: ${result.stderr || result.stdout}`);
    }
  }
  throw new Error(`could not find a free repository name starting from "${base}"`);
}

/** The repo's web URL, from `gh` if possible, else derived from the `origin` remote. */
export async function repoUrl(ghBin, cwd) {
  const viaGh = await run(ghBin, ['repo', 'view', '--json', 'url', '-q', '.url'], { cwd, env: ghEnv(), timeoutMs: 15_000 });
  if (viaGh.code === 0 && viaGh.stdout.trim()) return viaGh.stdout.trim();
  const origin = await hasOrigin(cwd);
  if (!origin) return null;
  return origin
    .replace(/^git@github\.com:/, 'https://github.com/')
    .replace(/^https:\/\/[^@]+@github\.com\//, 'https://github.com/')
    .replace(/\.git$/, '');
}
