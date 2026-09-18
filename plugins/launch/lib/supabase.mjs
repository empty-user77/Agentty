// Supabase CLI calls: login, picking or creating the hosted project, the public keys for
// `.env.local`, and applying `supabase/migrations`. Launch never reads the CLI's access token and
// never asks for the service_role key — only the key a browser may hold leaves this module.

import crypto from 'node:crypto';
import { existsSync } from 'node:fs';
import fs from 'node:fs/promises';
import path from 'node:path';
import { run, spawnInteractive, baseEnv, tailLines } from './exec.mjs';
import {
  detectSupabaseUse,
  extractSupabaseLoginUrl,
  isLocalOnlyValue,
  mergeEnvFile,
  normalizeSupabaseOrgs,
  normalizeSupabaseProjects,
  parseEnvFile,
  parseLooseJson,
  pickSupabasePublicKey,
  sanitizeRepoName,
  supabaseEnvNames,
  supabaseErrorKind,
} from './parse.mjs';

// The CLI answers in a JSON "agent mode" when it detects an AI-agent environment. Launch parses the
// plain output, so the mode is pinned.
const PLAIN = ['--agent', 'no'];

const DB_PASSWORD_KEY = 'SUPABASE_DB_PASSWORD';

/** An error whose `kind` (`login`, `free-limit`, `db-password`, `no-org`, `starting`) picks a plain-language message. */
export class SupabaseError extends Error {
  constructor(message, kind = null) {
    super(message);
    this.kind = kind;
  }
}

function fail(what, result) {
  const output = `${result.stderr}\n${result.stdout}`.trim();
  return new SupabaseError(`${what}:\n${tailLines(output, 20).join('\n')}`, supabaseErrorKind(output));
}

async function sb(bin, cwd, args, opts = {}) {
  return run(bin, [...args, ...PLAIN], { cwd, env: baseEnv(opts.env), timeoutMs: opts.timeoutMs ?? 60_000 });
}

/** `{ loggedIn, projects }` from `supabase projects list`. Never throws: not being logged in is a state, not an error. */
export async function supabaseProjects(bin, cwd) {
  const result = await sb(bin, cwd, ['projects', 'list', '-o', 'json']);
  if (result.code !== 0) return { loggedIn: false, projects: [] };
  return { loggedIn: true, projects: normalizeSupabaseProjects(parseLooseJson(result.stdout)) };
}

export async function supabaseOrgs(bin, cwd) {
  const result = await sb(bin, cwd, ['orgs', 'list', '-o', 'json']);
  if (result.code !== 0) throw fail('could not list your Supabase organizations', result);
  return normalizeSupabaseOrgs(parseLooseJson(result.stdout));
}

/**
 * Starts `supabase login`. Unlike `gh` and `vercel`, the browser shows a verification code that has
 * to be typed back into the CLI, and the CLI only runs this flow on a terminal — so it runs under
 * `script` (a pseudo-terminal) and the code the user enters in the panel is written to its stdin.
 * `onLink(url)` fires once the login link is known. Returns `{ submitCode, cancel, done }`.
 *
 * `script` refuses a socket as stdin (Node's pipes are socket pairs: "tcgetattr/ioctl: Operation not
 * supported on socket"), so `cat` puts a real pipe in between. Process substitution rather than
 * `cat | script`: the shell then doesn't wait for `cat`, and the CLI's exit ends the whole thing.
 */
export function supabaseLogin(bin, cwd, { onLink, timeoutMs = 10 * 60_000 } = {}) {
  let url = null;
  const underPty = '/usr/bin/script -q /dev/null "$0" "$@" < <(cat 2>/dev/null)';
  const proc = spawnInteractive('/bin/bash', ['-c', underPty, bin, 'login', '--no-browser', ...PLAIN, '--output-format', 'text'], {
    cwd,
    env: baseEnv(),
    timeoutMs,
    onData: (_chunk, buffer) => {
      if (url) return;
      url = extractSupabaseLoginUrl(buffer);
      if (url) onLink?.(url);
    },
  });
  return {
    submitCode: (code) => proc.write(`${String(code).trim()}\n`),
    cancel: () => proc.kill(),
    done: proc.wait.then(({ code }) => ({ ok: code === 0 })),
  };
}

/** A database password nobody has to remember: URL-safe, so it needs no escaping in a connection string. */
export function generateDbPassword() {
  return crypto.randomBytes(24).toString('base64url');
}

/** Creates the hosted project and resolves its `ref`. The password goes to `.env.local` only (see `saveDbPassword`). */
export async function createSupabaseProject(bin, cwd, { name, orgId, region, dbPassword }) {
  const args = ['projects', 'create', sanitizeRepoName(name), '--org-id', orgId, '--region', region, '--db-password', dbPassword, '-o', 'json'];
  const result = await sb(bin, cwd, args, { timeoutMs: 2 * 60_000 });
  if (result.code !== 0) throw fail('could not create the Supabase project', result);
  const [created] = normalizeSupabaseProjects([parseLooseJson(result.stdout)].flat());
  if (created) return created.ref;
  // The CLI printed something else than JSON: find the project by name instead.
  const { projects } = await supabaseProjects(bin, cwd);
  const match = projects.find((p) => p.name === sanitizeRepoName(name));
  if (!match) throw new SupabaseError(`the project was created but could not be found:\n${tailLines(result.stdout, 10).join('\n')}`);
  return match.ref;
}

/** A new project takes a minute or two to start. Polls until it reports healthy. */
export async function waitUntilHealthy(bin, cwd, ref, { onTick, timeoutMs = 5 * 60_000, intervalMs = 5_000 } = {}) {
  const started = Date.now();
  for (;;) {
    const { projects } = await supabaseProjects(bin, cwd);
    const status = projects.find((p) => p.ref === ref)?.status ?? '';
    // An empty status means this CLI version doesn't report one; the key request below settles it.
    if (status === 'ACTIVE_HEALTHY' || status === '') return;
    if (Date.now() - started > timeoutMs) throw new SupabaseError(`the Supabase project is still starting (${status})`, 'starting');
    onTick?.(status);
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
}

export function supabaseProjectUrl(ref) {
  return `https://${ref}.supabase.co`;
}

/** The anon / publishable key of a project. Never `--reveal`, never the service_role key. */
export async function supabasePublicKey(bin, cwd, ref) {
  const result = await sb(bin, cwd, ['projects', 'api-keys', '--project-ref', ref, '-o', 'json']);
  if (result.code !== 0) throw fail('could not read the project keys', result);
  const key = pickSupabasePublicKey(parseLooseJson(result.stdout));
  if (!key) throw new SupabaseError('the project has no public (anon) key yet', 'starting');
  return key;
}

async function readEnv(root, files) {
  const vars = {};
  for (const name of files) {
    try {
      Object.assign(vars, parseEnvFile(await fs.readFile(path.join(root, name), 'utf8')));
    } catch {
      // Not there.
    }
  }
  return vars;
}

async function migrationFiles(root) {
  try {
    return (await fs.readdir(path.join(root, 'supabase', 'migrations'))).filter((f) => f.endsWith('.sql')).sort();
  } catch {
    return [];
  }
}

/**
 * Where the project stands with Supabase. `configured` means a hosted project's URL and public key
 * are already in the env files; `envFile` is where Launch would write them — `.env.production`
 * when `.env.local` points at a local Supabase, so local development keeps working.
 */
export async function inspectSupabase(root, { pkg, framework } = {}) {
  const values = await readEnv(root, ['.env', '.env.local', '.env.production']);
  const documented = await readEnv(root, ['.env.example', '.env.sample']);
  const names = [...new Set([...Object.keys(values), ...Object.keys(documented)])];
  const envNames = supabaseEnvNames(framework, names);
  const ref = /^https:\/\/([a-z]{20})\.supabase\.co\/?$/.exec(values[envNames.url] ?? '')?.[1] ?? null;
  const deps = { ...(pkg?.dependencies ?? {}), ...(pkg?.devDependencies ?? {}) };
  return {
    used: detectSupabaseUse({ pkg, hasSupabaseDir: existsSync(path.join(root, 'supabase')), envNames: names }),
    hasClient: Boolean(deps['@supabase/supabase-js']),
    envNames,
    envFile: isLocalOnlyValue(values[envNames.url]) ? '.env.production' : '.env.local',
    configured: Boolean(ref && values[envNames.key]),
    ref,
    migrations: await migrationFiles(root),
    hasDbPassword: Boolean(values[DB_PASSWORD_KEY]),
  };
}

async function setEnvVars(file, vars, { mode } = {}) {
  let existing = '';
  try {
    existing = await fs.readFile(file, 'utf8');
  } catch {
    // New file.
  }
  await fs.writeFile(file, mergeEnvFile(existing, vars), mode ? { mode } : undefined);
  if (mode) await fs.chmod(file, mode).catch(() => {});
}

/**
 * Writes the project URL and public key to the env file (created `0600`), and documents the names —
 * without values — in `.env.example`. The caller makes sure `.env*` is gitignored first.
 */
export async function writeSupabaseEnv(root, { envFile, envNames, ref, publicKey }) {
  await setEnvVars(path.join(root, envFile), { [envNames.url]: supabaseProjectUrl(ref), [envNames.key]: publicKey }, { mode: 0o600 });
  const example = path.join(root, '.env.example');
  const documented = await readEnv(root, ['.env.example']);
  const missing = [envNames.url, envNames.key].filter((name) => !(name in documented));
  if (missing.length > 0) await setEnvVars(example, Object.fromEntries(missing.map((name) => [name, ''])));
}

/** The database password stays on this computer: `.env.local`, which Launch never offers to upload by default. */
export async function saveDbPassword(root, dbPassword) {
  await setEnvVars(path.join(root, '.env.local'), { [DB_PASSWORD_KEY]: dbPassword }, { mode: 0o600 });
}

/** Applies `supabase/migrations` to the hosted database. The password travels in the environment, not on the command line. */
export async function pushMigrations(bin, cwd, ref) {
  const dbPassword = (await readEnv(cwd, ['.env.local']))[DB_PASSWORD_KEY];
  if (!dbPassword) throw new SupabaseError('the database password is not known', 'db-password');
  const env = { [DB_PASSWORD_KEY]: dbPassword };
  const link = await sb(bin, cwd, ['link', '--project-ref', ref], { env, timeoutMs: 2 * 60_000 });
  if (link.code !== 0) throw fail('could not connect to the Supabase database', link);
  const push = await sb(bin, cwd, ['db', 'push', '--yes'], { env, timeoutMs: 5 * 60_000 });
  if (push.code !== 0) throw fail('could not apply the database changes', push);
}
