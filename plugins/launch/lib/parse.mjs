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
export const REQUIRED_GITIGNORE_LINES = ['node_modules', '.env', '.env.local', '.env.*.local', '.env.*', '!.env.example', '!.env.sample', '.vercel', '.next', 'dist', '.DS_Store'];

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
    if (!/^\.env(?:\..+)?$/.test(base)) return false;
    return !/^\.env\.(?:example|sample)$/i.test(base);
  });
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

/** The shortest `*.vercel.app` alias in `vercel inspect` output (the project's public domain). */
export function parseInspectAlias(output) {
  const urls = (stripAnsi(output).match(URL_RE) || []).map((u) => u.replace(/[.,]+$/, '')).filter((u) => /^https:\/\/[^/]+\.vercel\.app\/?$/.test(u));
  return urls.sort((a, b) => a.length - b.length)[0] ?? null;
}
