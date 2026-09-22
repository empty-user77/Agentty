// The account side of Vercel: who is signed in, which scopes they have, and every project in one
// of them with the repository it deploys from.
//
// The CLI has no command that shows how a project is wired to a repository, so this asks Vercel's
// API directly, with the session the CLI already holds — `vercel whoami` runs first so an expired
// session is refreshed by the CLI itself before it is read. The session never leaves this module:
// it goes into one Authorization header, is never written anywhere, never logged, never put on a
// command line and never shown in an error. When it cannot be read at all, `projectsViaCli` still
// lists the projects by name from `vercel project ls --json`.

import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { baseEnv, run } from './exec.mjs';
import { isHostname, isNameSegment } from './parse.mjs';

const API = 'https://api.vercel.com';
const CLI_DIR = 'com.vercel.cli';

/** Where the Vercel CLI keeps its session, most current layout first. */
export function sessionFileCandidates(env = process.env, home = os.homedir(), platform = process.platform) {
  const dirs = [];
  if (platform === 'darwin') dirs.push(path.join(home, 'Library', 'Application Support', CLI_DIR));
  else if (platform === 'win32') {
    for (const base of [env.APPDATA, env.LOCALAPPDATA]) if (base) dirs.push(path.join(base, CLI_DIR));
  } else dirs.push(path.join(env.XDG_DATA_HOME || path.join(home, '.local', 'share'), CLI_DIR));
  dirs.push(path.join(home, '.vercel'), path.join(home, '.now'));
  return dirs.map((dir) => path.join(dir, 'auth.json'));
}

/** Same folders, for `config.json` (which scope the CLI is pointed at — no credentials in it). */
export function configFileCandidates(env = process.env, home = os.homedir(), platform = process.platform) {
  return sessionFileCandidates(env, home, platform).map((file) => path.join(path.dirname(file), 'config.json'));
}

async function readJson(file) {
  try {
    return JSON.parse(await fs.readFile(file, 'utf8'));
  } catch {
    return null;
  }
}

/**
 * The session to call the API with: `VERCEL_TOKEN` when the user set one, else the one the CLI
 * holds. Returns `null` when there is none — never an error carrying the value.
 */
export async function vercelSession({ env = process.env, home = os.homedir(), platform = process.platform } = {}) {
  const fromEnv = env.VERCEL_TOKEN || env.NOW_TOKEN;
  if (typeof fromEnv === 'string' && fromEnv.trim()) return fromEnv.trim();
  for (const file of sessionFileCandidates(env, home, platform)) {
    const json = await readJson(file);
    if (json && typeof json.token === 'string' && json.token.trim()) return json.token.trim();
  }
  return null;
}

/** The team the CLI is currently pointed at, or `null` for the personal account. */
export async function currentTeamId({ env = process.env, home = os.homedir(), platform = process.platform } = {}) {
  for (const file of configFileCandidates(env, home, platform)) {
    const json = await readJson(file);
    if (json && typeof json.currentTeam === 'string' && json.currentTeam) return json.currentTeam;
  }
  return null;
}

/** One API call. `{ ok, status, body }`; never throws, and never repeats the session anywhere. */
export async function vercelApi(session, apiPath, { timeoutMs = 20_000 } = {}) {
  if (!session) return { ok: false, status: 0, body: null };
  try {
    const response = await fetch(`${API}${apiPath}`, {
      headers: { Authorization: `Bearer ${session}`, Accept: 'application/json' },
      signal: AbortSignal.timeout(timeoutMs),
    });
    let body = null;
    try {
      body = await response.json();
    } catch {
      body = null;
    }
    return { ok: response.ok, status: response.status, body };
  } catch {
    return { ok: false, status: 0, body: null };
  }
}

/** `{ username, name, version, defaultTeamId }` of the signed-in account, or null. */
export async function account(session) {
  const { ok, body } = await vercelApi(session, '/v2/user');
  const user = body?.user ?? body;
  if (!ok || !user || typeof user.username !== 'string') return null;
  return {
    username: user.username,
    name: typeof user.name === 'string' ? user.name : null,
    version: typeof user.version === 'string' ? user.version : null,
    defaultTeamId: typeof user.defaultTeamId === 'string' ? user.defaultTeamId : null,
  };
}

/**
 * Everywhere this account keeps projects: `[{ id, slug, name, personal }]`, `id` being `null` for
 * the personal account. Newer Vercel accounts ("northstar") no longer have one — the personal
 * account was turned into a team, and asking without a team id answers with that same team's
 * projects — so for those only the teams are listed, and the same projects are not offered twice.
 */
export function scopeList(user, teams) {
  const named = (Array.isArray(teams) ? teams : [])
    .filter((team) => team && typeof team.id === 'string')
    .map((team) => ({ id: team.id, slug: typeof team.slug === 'string' ? team.slug : team.id, name: team.name || team.slug || team.id, personal: false }));
  const converted = (user?.version === 'northstar' || Boolean(user?.defaultTeamId)) && named.length > 0;
  if (converted) return named;
  return [{ id: null, slug: user?.username ?? 'me', name: user?.name || user?.username || 'Personal', personal: true }, ...named];
}

export async function scopes(session, user) {
  const { ok, body } = await vercelApi(session, '/v2/teams?limit=50');
  return scopeList(user, ok && Array.isArray(body?.teams) ? body.teams : []);
}

/** The raw `/v9/projects` answer for one scope; `normalizeVercelProjects` turns it into the panel's shape. */
export async function projectsPayload(session, { teamId = null, limit = 100 } = {}) {
  const query = new URLSearchParams({ limit: String(Math.min(Math.max(limit, 1), 100)) });
  if (teamId) query.set('teamId', teamId);
  return vercelApi(session, `/v9/projects?${query}`);
}

/**
 * What the CLI alone can tell us when the API is out of reach: names, their production address and
 * when they last changed — no repository links. `{ scope, projects }`.
 */
export async function projectsViaCli(vercelBin, cwd, { limit = 50 } = {}) {
  if (!vercelBin) return { scope: null, projects: [] };
  const result = await run(vercelBin, ['project', 'ls', '--json', '--limit', String(Math.min(Math.max(limit, 1), 100))], {
    cwd,
    env: baseEnv({ CI: '' }),
    timeoutMs: 45_000,
  });
  if (result.code !== 0) return { scope: null, projects: [] };
  let payload = null;
  try {
    payload = JSON.parse(result.stdout.slice(result.stdout.indexOf('{')));
  } catch {
    return { scope: null, projects: [] };
  }
  const scope = typeof payload?.contextName === 'string' ? payload.contextName : null;
  const list = Array.isArray(payload?.projects) ? payload.projects : [];
  const projects = list
    .filter((p) => p && typeof p.name === 'string')
    .map((p) => {
      const host = String(p.latestProductionUrl ?? '').replace(/^https?:\/\//, '').replace(/\/.*$/, '');
      return {
        id: typeof p.id === 'string' ? p.id : p.name,
        name: p.name,
        framework: null,
        repo: null,
        // The CLI knows the address but not how the last build went, so the state stays unknown.
        production: isHostname(host) ? { state: 'unknown', domain: host, domains: [host], url: `https://${host}`, createdAt: null, ref: null, sha: null, message: null } : null,
        updatedAt: Number.isFinite(p.updatedAt) ? p.updatedAt : null,
        inspectUrl: isNameSegment(scope) && isNameSegment(p.name) ? `https://vercel.com/${scope}/${p.name}` : null,
      };
    })
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0));
  return { scope, projects };
}
