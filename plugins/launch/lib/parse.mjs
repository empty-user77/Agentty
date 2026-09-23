// Pure text-parsing helpers used by the login flows, deploy step and env-file handling.
// No side effects, no imports besides node:path — easy to unit test.

/** Strips ANSI escape codes (colors, cursor moves) so parsed text and logs stay readable. */
export function stripAnsi(text) {
  return String(text ?? '')
    .replace(/\x1B\[[0-9;?]*[ -/]*[@-~]/g, '')
    .replace(/\x1B\][^\x07\x1B]*(?:\x07|\x1B\\)/g, '')
    .replace(/\r/g, '');
}

const DEVICE_CODE_RE = /\b([A-Z0-9]{4}-[A-Z0-9]{4})\b/;
const URL_RE = /https?:\/\/[^\s'"<>)\]]+/g;

/** The one-time device code (`XXXX-XXXX`) `gh auth login --web` or `vercel login` prints. */
export function extractDeviceCode(text) {
  const match = stripAnsi(text).match(DEVICE_CODE_RE);
  return match ? match[1] : null;
}

/** The URL to open in a browser to enter the device code (github.com/login/device, vercel.com/…). */
export function extractDeviceUrl(text) {
  const clean = stripAnsi(text);
  const urls = clean.match(URL_RE) || [];
  const preferred = urls.find((u) => /login\/device|device-flow|\/device\b|activate/i.test(u));
  return preferred || urls[0] || null;
}

/**
 * The production URL from `vercel deploy --prod` output. Prefers an explicit "Production:" /
 * "Aliased to" line (the real domain), falling back to the last https URL printed.
 */
export function parseDeployUrl(output) {
  const clean = stripAnsi(output);
  // The alias (my-app.vercel.app) is public; the per-deployment URL on the "Production:" line is
  // usually behind Vercel's deployment protection, so it only counts when there is no alias.
  for (const label of [/Aliased(?: to)?/i, /Production/i]) {
    for (const line of clean.split(/\n/)) {
      const match = line.match(new RegExp(`${label.source}\\s*:?\\s*(https:\\/\\/\\S+)`, 'i'));
      if (match) return match[1].replace(/[.,]+$/, '');
    }
  }
  const urls = clean.match(URL_RE) || [];
  return urls.length ? urls[urls.length - 1].replace(/[.,]+$/, '') : null;
}

/** Parses `.env`-style content: `KEY=value`, `export KEY=value`, quotes and `#` comments. */
export function parseEnvFile(content) {
  const vars = {};
  for (const rawLine of String(content ?? '').split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    const withoutExport = line.replace(/^export\s+/, '');
    const match = withoutExport.match(/^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/);
    if (!match) continue;
    let [, key, value] = match;
    value = value.trim();
    const quoted = (value.startsWith('"') && value.endsWith('"') && value.length >= 2) || (value.startsWith("'") && value.endsWith("'") && value.length >= 2);
    if (quoted) {
      value = value.slice(1, -1);
    } else {
      const hashIndex = value.indexOf(' #');
      if (hashIndex !== -1) value = value.slice(0, hashIndex).trim();
    }
    vars[key] = value;
  }
  return vars;
}

/** Whether a value only makes sense on this machine (dev server, loopback address). */
export function isLocalOnlyValue(value) {
  return /^(?:[a-z]+:\/\/)?(?:localhost|127\.0\.0\.1|0\.0\.0\.0)(?::\d+)?(?:\/|$)/i.test(String(value ?? '').trim());
}

/** A safe GitHub repository name from a folder name: lower-case, `a-z0-9._-` only. */
export function sanitizeRepoName(name) {
  const cleaned = String(name ?? '')
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^[-.]+|[-.]+$/g, '');
  return cleaned || 'my-project';
}

/** Lines Launch wants in `.gitignore`. Order matters: negations must follow what they negate. */
export const REQUIRED_GITIGNORE_LINES = [
  'node_modules',
  '.env',
  '.env.local',
  '.env.*.local',
  '.env.*',
  '!.env.example',
  '!.env.sample',
  '.envrc',
  '*.pem',
  '*.p12',
  '*.pfx',
  'id_rsa*',
  'id_ed25519*',
  '.aws/',
  '.ssh/',
  '.claude/settings.local.json',
  '.vercel',
  'supabase/.temp',
  '.agentty-ecc',
  '.next',
  'dist',
  '.DS_Store',
];

/** The owner's private notes from "Build my idea": never part of a public repository. */
export const IDEA_NOTES_GITIGNORE_LINES = ['docs/idea/'];

/**
 * What a deploy must not upload. A site without a framework serves every uploaded file, so the
 * owner's idea notes and attachments, agent settings and working notes would be readable at
 * `https://<site>/docs/idea/IDEA.md`. None of these are needed to build — of `supabase/` only what
 * no app imports (generated types there still build).
 */
export const REQUIRED_VERCELIGNORE_LINES = [
  'docs/idea',
  '.claude',
  '.agentty-ecc',
  'supabase/.temp',
  'supabase/migrations',
  'supabase/seed.sql',
  'PLAN.md',
  'CLAUDE.md',
  'AGENTS.md',
  '.env*',
  '!.env.example',
];

/** Appends whichever of `REQUIRED_GITIGNORE_LINES` are missing, without touching existing lines. */
export function mergeGitignore(existing, required = REQUIRED_GITIGNORE_LINES) {
  const text = String(existing ?? '');
  const have = new Set(
    text
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter(Boolean),
  );
  const missing = required.filter((line) => !have.has(line));
  if (missing.length === 0) return text;
  const needsNewline = text.length > 0 && !text.endsWith('\n');
  const needsBlankLine = text.trim().length > 0;
  return `${text}${needsNewline ? '\n' : ''}${needsBlankLine ? '\n' : ''}# Added by Agentty Launch\n${missing.join('\n')}\n`;
}

/** `.env*` files (tracked or staged) that would ship real values, i.e. not `.env.example`/`.env.sample`. */
export function envFilesAtRisk(paths) {
  return (paths ?? []).filter((p) => {
    const base = String(p).split('/').pop() ?? '';
    if (/^\.envrc$/.test(base)) return true;
    if (!/^\.env(?:\..+)?$/.test(base)) return false;
    return !/^\.env\.(?:example|sample)$/i.test(base);
  });
}

/** Key material and credential files (tracked or staged) that must not be uploaded either. */
export function secretFilesAtRisk(paths) {
  return (paths ?? []).filter((p) => {
    const path = String(p);
    const base = path.split('/').pop() ?? '';
    return /\.(?:pem|p12|pfx)$/i.test(base) || /^id_(?:rsa|dsa|ecdsa|ed25519)$/.test(base) || /(?:^|\/)\.(?:aws|ssh)\//.test(path) || base === '.netrc';
  });
}

/** `host/owner/repo` of a git remote URL, for showing where a push goes. `null` when it is not a URL we know. */
export function describeRemote(url) {
  const text = String(url ?? '').trim();
  const match = text.match(/^(?:https?:\/\/(?:[^@/]+@)?|ssh:\/\/(?:[^@/]+@)?|[^@/\s]+@)([^/:\s]+)[/:](.+?)(?:\.git)?\/?$/);
  return match ? { host: match[1].toLowerCase(), path: match[2], display: `${match[1].toLowerCase()}/${match[2]}` } : null;
}

/** Framework name from a parsed `package.json` (dependencies + devDependencies), or from `index.html` alone. */
export function detectFramework({ pkg, hasIndexHtml } = {}) {
  if (pkg && typeof pkg === 'object') {
    const deps = { ...(pkg.dependencies ?? {}), ...(pkg.devDependencies ?? {}) };
    if (deps.next) return 'next';
    if (deps.nuxt || deps.nuxt3) return 'nuxt';
    if (deps.astro) return 'astro';
    if (deps['@sveltejs/kit']) return 'sveltekit';
    if (deps.svelte) return 'svelte';
    if (deps.vite) return 'vite';
    if (deps.react) return 'react';
    if (deps.vue) return 'vue';
    return 'node';
  }
  if (hasIndexHtml) return 'static';
  return null;
}

// -- Supabase -----------------------------------------------------------------------------------

/** Env vars that only make sense on this computer, whatever their value: never pre-selected for the live site. */
export const LOCAL_ONLY_KEYS = ['SUPABASE_DB_PASSWORD'];

/** The browser link `supabase login --no-browser` prints before it waits for the verification code. */
export function extractSupabaseLoginUrl(text) {
  const urls = stripAnsi(text).match(URL_RE) || [];
  return urls.find((u) => /supabase\.com\/dashboard\/cli\/login/i.test(u)) ?? null;
}

/** JSON printed by a CLI that may put log lines around it. `null` when nothing parses. */
export function parseLooseJson(text) {
  const clean = stripAnsi(text).trim();
  try {
    return JSON.parse(clean);
  } catch {
    // Fall through: look for the first array/object in the text.
  }
  for (const [open, close] of [['[', ']'], ['{', '}']]) {
    const start = clean.indexOf(open);
    const end = clean.lastIndexOf(close);
    if (start === -1 || end <= start) continue;
    try {
      return JSON.parse(clean.slice(start, end + 1));
    } catch {
      // Try the next shape.
    }
  }
  return null;
}

/** A list from CLI JSON that is either the array itself or wrapped (`{ projects: [...] }`, `{ data: [...] }`). */
function looseArray(json) {
  if (Array.isArray(json)) return json;
  if (json && typeof json === 'object') {
    for (const value of Object.values(json)) if (Array.isArray(value)) return value;
  }
  return [];
}

/** `supabase projects list -o json` → `[{ ref, name, region, status, orgId }]`. Older CLIs call the ref `id`. */
export function normalizeSupabaseProjects(json) {
  return looseArray(json)
    .map((p) => ({
      ref: String(p?.ref ?? p?.id ?? ''),
      name: String(p?.name ?? ''),
      region: String(p?.region ?? ''),
      status: String(p?.status ?? ''),
      orgId: String(p?.organization_slug ?? p?.organization_id ?? ''),
    }))
    .filter((p) => /^[a-z]{20}$/.test(p.ref));
}

/** `supabase orgs list -o json` → `[{ id, name }]`. */
export function normalizeSupabaseOrgs(json) {
  return looseArray(json)
    .map((o) => ({ id: String(o?.slug ?? o?.id ?? ''), name: String(o?.name ?? '') }))
    .filter((o) => o.id);
}

/**
 * The key a browser may hold, from `supabase projects api-keys -o json`: the legacy `anon` key or a
 * `publishable` one. `service_role` and `secret` keys bypass row-level security and are never used.
 */
export function pickSupabasePublicKey(json) {
  const keys = looseArray(json).filter((k) => typeof k?.api_key === 'string' && k.api_key);
  const isPrivate = (k) => /service_role|secret/i.test(`${k?.name ?? ''} ${k?.type ?? ''}`) || /^sb_secret_/.test(k.api_key);
  const usable = keys.filter((k) => !isPrivate(k));
  const chosen = usable.find((k) => k.name === 'anon') ?? usable.find((k) => k.type === 'publishable') ?? null;
  return chosen ? chosen.api_key : null;
}

const SUPABASE_URL_NAME = /SUPABASE_URL$/;
const SUPABASE_KEY_NAME = /SUPABASE_(?:ANON_KEY|PUBLISHABLE_KEY|PUBLISHABLE_DEFAULT_KEY|KEY)$/;

/**
 * Names for the project URL and public key. Names the project already uses win (its code reads
 * them); otherwise the framework's convention for variables the browser may see.
 */
export function supabaseEnvNames(framework, existingNames = []) {
  const names = existingNames.filter((n) => !/SERVICE_ROLE|SECRET/.test(n));
  const prefix = { next: 'NEXT_PUBLIC_', vite: 'VITE_', react: 'VITE_', vue: 'VITE_', svelte: 'VITE_', astro: 'PUBLIC_', sveltekit: 'PUBLIC_' }[framework] ?? '';
  return {
    url: names.find((n) => SUPABASE_URL_NAME.test(n)) ?? `${prefix}SUPABASE_URL`,
    key: names.find((n) => SUPABASE_KEY_NAME.test(n)) ?? (framework === 'nuxt' ? 'SUPABASE_KEY' : `${prefix}SUPABASE_ANON_KEY`),
  };
}

/** Whether the project uses Supabase: the client library, a `supabase/` folder, or env names mentioning it. */
export function detectSupabaseUse({ pkg, hasSupabaseDir = false, envNames = [] } = {}) {
  const deps = { ...(pkg?.dependencies ?? {}), ...(pkg?.devDependencies ?? {}) };
  if (Object.keys(deps).some((name) => name.startsWith('@supabase/'))) return true;
  if (hasSupabaseDir) return true;
  return envNames.some((name) => /SUPABASE/.test(name));
}

/** Sets `vars` in `.env`-style content: existing keys are replaced in place, new ones appended, `null` removes a key. */
export function mergeEnvFile(existing, vars) {
  const remaining = new Map(Object.entries(vars));
  const lines = String(existing ?? '')
    .split(/\r?\n/)
    .map((line) => {
      const match = line.match(/^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=/);
      if (!match || !remaining.has(match[1])) return line;
      const value = remaining.get(match[1]);
      remaining.delete(match[1]);
      return value == null ? null : `${match[1]}=${value}`;
    })
    .filter((line) => line !== null);
  while (lines.length && lines[lines.length - 1] === '') lines.pop();
  for (const [key, value] of remaining) if (value != null) lines.push(`${key}=${value}`);
  return lines.length ? `${lines.join('\n')}\n` : '';
}

/** The Supabase region closest to a time zone (`Asia/Seoul` → `ap-northeast-2`), so nobody has to pick one. */
export function supabaseRegionForTimeZone(timeZone) {
  const tz = String(timeZone ?? '');
  const table = [
    [/^Asia\/Seoul$/, 'ap-northeast-2'],
    [/^Asia\/Tokyo$/, 'ap-northeast-1'],
    [/^Asia\/(?:Kolkata|Calcutta|Karachi|Dhaka|Colombo|Dubai)$/, 'ap-south-1'],
    [/^(?:Australia|Pacific)\//, 'ap-southeast-2'],
    [/^Asia\//, 'ap-southeast-1'],
    [/^Europe\/(?:London|Dublin|Lisbon)$/, 'eu-west-2'],
    [/^(?:Europe|Africa)\//, 'eu-central-1'],
    [/^America\/(?:Sao_Paulo|Argentina|Buenos_Aires|Santiago|Bogota|Lima|Montevideo)/, 'sa-east-1'],
    [/^America\/(?:Toronto|Montreal|Halifax|Winnipeg)$/, 'ca-central-1'],
    [/^America\/(?:Los_Angeles|Vancouver|Tijuana|Phoenix|Denver|Anchorage)$/, 'us-west-1'],
  ];
  return table.find(([pattern]) => pattern.test(tz))?.[1] ?? 'us-east-1';
}

/** A plain-language reason for a failed Supabase command, or `null` to show the raw output. */
export function supabaseErrorKind(output) {
  const text = stripAnsi(output);
  if (/access token not provided|unauthorized|invalid access token/i.test(text)) return 'login';
  if (/maximum limits? .*free projects|free projects? limit|limit of \d+ (?:active )?free/i.test(text)) return 'free-limit';
  if (/password authentication failed|failed SASL auth/i.test(text)) return 'db-password';
  return null;
}

/** The shortest `*.vercel.app` alias in `vercel inspect` output (the project's public domain). */
export function parseInspectAlias(output) {
  const urls = (stripAnsi(output).match(URL_RE) || []).map((u) => u.replace(/[.,]+$/, '')).filter((u) => /^https:\/\/[^/]+\.vercel\.app\/?$/.test(u));
  return urls.sort((a, b) => a.length - b.length)[0] ?? null;
}

// -- Hosting already set up outside Launch ------------------------------------------------------

/** Vercel's GitHub app creates the deployments (`vercel[bot]`); its production ones are named "Production" (or "Production – <project>"). */
export function isVercelProductionDeployment(deployment) {
  const creator = String(deployment?.creator?.login ?? '');
  const environment = String(deployment?.environment ?? '');
  return /^vercel(\[bot\])?$/i.test(creator) && /^production\b/i.test(environment);
}

/** The newest Vercel production deployments first (GitHub already lists them newest first). */
export function pickVercelProductionDeployments(deployments, limit = 5) {
  if (!Array.isArray(deployments)) return [];
  return deployments.filter(isVercelProductionDeployment).slice(0, limit);
}

/**
 * One deployment and its latest status (`GET …/deployments/{id}/statuses`, newest first) as the panel
 * shows it. `state`: success, failure, error, in_progress, queued, pending or inactive. Only https URLs
 * are kept: the panel opens them.
 */
export function summarizeDeployment(deployment, statuses) {
  const latest = Array.isArray(statuses) ? statuses[0] : null;
  const https = (url) => (typeof url === 'string' && /^https:\/\/[^\s]+$/.test(url) ? url : null);
  const time = Date.parse(latest?.created_at ?? deployment?.created_at ?? '');
  return {
    state: String(latest?.state ?? 'pending'),
    url: https(latest?.environment_url),
    inspectUrl: https(latest?.log_url) ?? https(latest?.target_url),
    sha: typeof deployment?.sha === 'string' ? deployment.sha.slice(0, 7) : null,
    // Vercel's GitHub deployments name the commit, not the branch, as their ref.
    ref: typeof deployment?.ref === 'string' && !/^[0-9a-f]{40}$/i.test(deployment.ref) ? deployment.ref : null,
    time: Number.isFinite(time) ? time : null,
  };
}

/** The site's address: the newest deployment that has one and succeeded, else the newest that has one. */
export function liveUrlOf(summaries) {
  return summaries.find((s) => s.url && s.state === 'success')?.url ?? summaries.find((s) => s.url)?.url ?? null;
}

/**
 * The domains a deployment is served at, from `vercel inspect <url>` ("Aliases" section): custom
 * domains first, then `*.vercel.app` ones, shortest first; branch aliases (`-git-<branch>-`) are left
 * out. Only https hosts are returned.
 */
export function parseInspectDomains(output) {
  const text = stripAnsi(String(output ?? ''));
  const start = text.search(/^\s*Aliases\s*$/m);
  if (start < 0) return [];
  const section = text.slice(start).split('\n').slice(1);
  const hosts = [];
  for (const line of section) {
    const match = /^\s*[╶\-*]?\s*(https:\/\/[A-Za-z0-9.-]+)\/?\s*$/.exec(line);
    if (!match) {
      if (hosts.length > 0 && line.trim()) break; // the next section
      continue;
    }
    if (!/-git-/.test(match[1]) && !hosts.includes(match[1])) hosts.push(match[1]);
  }
  const custom = hosts.filter((h) => !h.endsWith('.vercel.app')).sort((a, b) => a.length - b.length);
  const vercel = hosts.filter((h) => h.endsWith('.vercel.app')).sort((a, b) => a.length - b.length);
  return [...custom, ...vercel];
}


// -- the dashboard: Vercel projects and how they hang together with their repositories ----------

/** Vercel's `readyState` in the words the panel uses. */
export function deployStateOf(readyState) {
  switch (String(readyState ?? '').toUpperCase()) {
    case 'READY':
      return 'ready';
    case 'ERROR':
      return 'error';
    case 'CANCELED':
    case 'DELETED':
      return 'canceled';
    case 'BUILDING':
    case 'INITIALIZING':
      return 'building';
    case 'QUEUED':
      return 'queued';
    default:
      return 'unknown';
  }
}

// Everything below builds an address out of what Vercel answers, and an address is opened in a
// browser: a name that is not a plain name, or a host that is not a plain host, is dropped rather
// than pasted into a URL.
const NAME_SEGMENT = /^[A-Za-z0-9._-]{1,100}$/;
const HOSTNAME = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)+$/;

/** `{ host, owner, name, slug, url, branch }` of the repository a Vercel project deploys from, or null. */
export function repoOfVercelLink(link) {
  if (!link || typeof link !== 'object') return null;
  const type = String(link.type ?? '').toLowerCase();
  const branch = typeof link.productionBranch === 'string' && link.productionBranch ? link.productionBranch : null;
  const pick = (host, owner, name) =>
    NAME_SEGMENT.test(String(owner ?? '')) && NAME_SEGMENT.test(String(name ?? ''))
      ? { host, owner, name, slug: `${owner}/${name}`, url: `https://${host}/${owner}/${name}`, branch }
      : null;
  if (type === 'github') return pick('github.com', link.org, link.repo);
  if (type === 'gitlab') return pick('gitlab.com', link.projectNamespace, link.projectName);
  if (type === 'bitbucket') return pick('bitbucket.org', link.owner, link.slug);
  return null;
}

/** Whether a string is a plain host name, safe to put in an `https://` address. */
export function isHostname(value) {
  return typeof value === 'string' && HOSTNAME.test(value);
}

/** Whether a string is a plain name, safe to put in a path segment of an address. */
export function isNameSegment(value) {
  return typeof value === 'string' && NAME_SEGMENT.test(value);
}

/** The address to show for a production deployment: a domain of its own before a Vercel one. */
export function pickPrimaryDomain(aliases, fallback = null) {
  const hosts = (Array.isArray(aliases) ? aliases : []).filter((a) => typeof a === 'string' && HOSTNAME.test(a));
  const custom = hosts.filter((h) => !h.endsWith('.vercel.app'));
  if (custom.length > 0) return custom[0];
  const plain = hosts.filter((h) => !h.includes('-git-')).sort((a, b) => a.length - b.length);
  return plain[0] ?? hosts[0] ?? fallback;
}

function firstLine(text) {
  const line = String(text ?? '').split('\n')[0].trim();
  return line.length > 0 ? line : null;
}

/** The commit metadata of a deployment, whichever git provider it came from. */
function commitOf(meta) {
  if (!meta || typeof meta !== 'object') return { ref: null, sha: null, message: null };
  const value = (name) => meta[`github${name}`] ?? meta[`gitlab${name}`] ?? meta[`bitbucket${name}`] ?? null;
  const sha = value('CommitSha');
  return {
    ref: typeof value('CommitRef') === 'string' ? value('CommitRef') : null,
    sha: typeof sha === 'string' && sha.length >= 7 ? sha.slice(0, 7) : null,
    message: firstLine(value('CommitMessage')),
  };
}

/**
 * One project from Vercel's `/v9/projects`, in the shape the dashboard draws:
 * `{ id, name, framework, repo, production, updatedAt }`. `null` when it isn't a project.
 */
export function normalizeVercelProject(raw, { scope = null } = {}) {
  if (!raw || typeof raw !== 'object' || typeof raw.name !== 'string' || raw.name.length === 0) return null;
  const target =
    (raw.targets && typeof raw.targets === 'object' ? raw.targets.production : null) ??
    (Array.isArray(raw.latestDeployments) ? raw.latestDeployments.find((d) => d?.target === 'production') : null) ??
    null;
  const commit = commitOf(target?.meta);
  const deploymentUrl = typeof target?.url === 'string' && HOSTNAME.test(target.url) ? target.url : null;
  const domains = Array.isArray(target?.alias) ? target.alias.filter((a) => typeof a === 'string') : [];
  const production = target
    ? {
        state: deployStateOf(target.readyState ?? target.state),
        domain: pickPrimaryDomain(domains, deploymentUrl),
        domains,
        url: deploymentUrl ? `https://${deploymentUrl}` : null,
        createdAt: Number.isFinite(target.createdAt) ? target.createdAt : null,
        ...commit,
      }
    : null;
  return {
    id: typeof raw.id === 'string' ? raw.id : raw.name,
    name: raw.name,
    framework: typeof raw.framework === 'string' && raw.framework ? raw.framework : null,
    repo: repoOfVercelLink(raw.link),
    production,
    updatedAt: Number.isFinite(raw.updatedAt) ? raw.updatedAt : null,
    // `https://vercel.com/<scope>/<project>` — the project's page, not a deployment's.
    inspectUrl: NAME_SEGMENT.test(String(scope ?? '')) && NAME_SEGMENT.test(raw.name) ? `https://vercel.com/${scope}/${raw.name}` : null,
  };
}

/** Every project in a `/v9/projects` answer, newest first. */
export function normalizeVercelProjects(payload, options = {}) {
  const list = Array.isArray(payload) ? payload : Array.isArray(payload?.projects) ? payload.projects : [];
  return list
    .map((raw) => normalizeVercelProject(raw, options))
    .filter(Boolean)
    .sort((a, b) => (b.production?.createdAt ?? b.updatedAt ?? 0) - (a.production?.createdAt ?? a.updatedAt ?? 0));
}

/**
 * The code → build → live diagram drawn under a project. Every stop is
 * `{ filled, title, lines }` and already carries formatted, translated text: this only
 * decides the shape, and keeps each line short enough for a narrow panel.
 */
export function flowDiagram(stops) {
  const list = (stops ?? []).filter(Boolean);
  const out = [];
  list.forEach((stop, index) => {
    const last = index === list.length - 1;
    out.push(`${stop.filled === false ? '○' : '●'} ${stop.title}`);
    for (const line of (stop.lines ?? []).filter(Boolean)) out.push(`${last ? ' ' : '│'} ${line}`);
    if (!last) out.push('│');
  });
  return out;
}

/** Roughly the columns a string takes in a monospace block: the wide scripts count double. */
export function displayWidth(text) {
  let width = 0;
  for (const char of String(text ?? '')) {
    const code = char.codePointAt(0);
    const wide =
      (code >= 0x1100 && code <= 0x115f) ||
      (code >= 0x2e80 && code <= 0xa4cf) ||
      (code >= 0xa960 && code <= 0xa97f) ||
      (code >= 0xac00 && code <= 0xd7a3) ||
      (code >= 0xf900 && code <= 0xfaff) ||
      (code >= 0xfe30 && code <= 0xfe6f) ||
      (code >= 0xff00 && code <= 0xff60) ||
      (code >= 0xffe0 && code <= 0xffe6) ||
      (code >= 0x1f300 && code <= 0x1f9ff) ||
      (code >= 0x20000 && code <= 0x3fffd);
    width += wide ? 2 : 1;
  }
  return width;
}

/** Cuts a diagram line to `columns`, counting Korean, Japanese and Chinese as the two columns they take. */
export function shortenToWidth(text, columns = 44) {
  const value = String(text ?? '').trim();
  if (displayWidth(value) <= columns) return value;
  let out = '';
  let width = 0;
  for (const char of value) {
    const next = width + displayWidth(char);
    if (next > columns - 1) break;
    out += char;
    width = next;
  }
  return `${out}…`;
}

/** The account an SSH key on this computer belongs to, from what `ssh -T git@github.com` answers. */
export function sshGithubLogin(output) {
  const match = /(?:^|\n)\s*Hi ([A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?)!\s+You've successfully authenticated/.exec(String(output ?? ''));
  return match ? match[1] : null;
}
