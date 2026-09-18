// Projects that already run on Vercel without Launch: Vercel's GitHub integration deploys on every
// push and records each deploy as a GitHub deployment, so `gh` (already signed in) tells us whether
// the repository is live, where, and how the last deploys went — no Vercel token is read and nothing
// is written into the project.

import { existsSync } from 'node:fs';
import path from 'node:path';
import { baseEnv, run } from './exec.mjs';
import { liveUrlOf, parseInspectDomains, pickVercelProductionDeployments, summarizeDeployment } from './parse.mjs';

function ghEnv() {
  return baseEnv({ GH_PROMPT_DISABLED: '1' });
}

async function ghJson(ghBin, cwd, apiPath) {
  const result = await run(ghBin, ['api', apiPath], { cwd, env: ghEnv(), timeoutMs: 20_000 });
  if (result.code !== 0) return null;
  try {
    return JSON.parse(result.stdout);
  } catch {
    return null;
  }
}

/** `owner/name` of the repository `cwd` pushes to, or null. */
export async function repoSlug(ghBin, cwd) {
  const result = await run(ghBin, ['repo', 'view', '--json', 'nameWithOwner', '-q', '.nameWithOwner'], { cwd, env: ghEnv(), timeoutMs: 15_000 });
  const slug = result.code === 0 ? result.stdout.trim() : '';
  return /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(slug) ? slug : null;
}

/** Whether `vercel link` connected this folder to a project (`.vercel/project.json`). */
export function isLinkedToVercel(root) {
  return existsSync(path.join(root, '.vercel', 'project.json'));
}

/** `{ branch, homepage }` of the repository on GitHub. */
async function repoInfo(ghBin, cwd) {
  const result = await run(ghBin, ['repo', 'view', '--json', 'defaultBranchRef,homepageUrl'], { cwd, env: ghEnv(), timeoutMs: 15_000 });
  try {
    const info = JSON.parse(result.stdout);
    const homepage = typeof info?.homepageUrl === 'string' && /^https:\/\/[^\s]+$/.test(info.homepageUrl) ? info.homepageUrl : null;
    return { branch: info?.defaultBranchRef?.name || null, homepage };
  } catch {
    return { branch: null, homepage: null };
  }
}

/**
 * `{ slug, url, domains, branch, latest, history }` when Vercel already deploys this repository to
 * production, else null. `url` is the site's main address: a custom domain when the deployment has
 * one (read with `vercel inspect` when `vercelBin` is given), else the repository's homepage, else
 * the deployment's own address. `latest` / `history` entries: `{ state, url, inspectUrl, sha, ref,
 * time }`. Never throws.
 */
export async function vercelHosting(ghBin, cwd, { limit = 5, vercelBin = null } = {}) {
  const slug = await repoSlug(ghBin, cwd);
  if (!slug) return null;
  const deployments = await ghJson(ghBin, cwd, `repos/${slug}/deployments?per_page=30`);
  const production = pickVercelProductionDeployments(deployments, limit);
  if (production.length === 0) return null;
  const history = [];
  for (const deployment of production) {
    const id = Number(deployment?.id);
    const statuses = Number.isSafeInteger(id) ? await ghJson(ghBin, cwd, `repos/${slug}/deployments/${id}/statuses?per_page=1`) : null;
    history.push(summarizeDeployment(deployment, statuses));
  }
  const deploymentUrl = liveUrlOf(history);
  let domains = [];
  if (vercelBin && deploymentUrl) {
    const inspect = await run(vercelBin, ['inspect', deploymentUrl], { cwd, env: baseEnv({ CI: '' }), timeoutMs: 30_000 });
    if (inspect.code === 0) domains = parseInspectDomains(inspect.stdout + inspect.stderr);
  }
  const { branch, homepage } = await repoInfo(ghBin, cwd);
  return { slug, url: domains[0] ?? homepage ?? deploymentUrl, domains, branch, latest: history[0], history };
}
