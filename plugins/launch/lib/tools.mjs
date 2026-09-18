// Finds or installs the `gh`, `vercel` and `supabase` CLIs. No Homebrew: `gh` is downloaded straight
// from its GitHub release, `vercel` and `supabase` are installed with npm into the plugin's own data
// folder, so this works even on a machine with none of them and no package manager set up.

import { existsSync } from 'node:fs';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { run, baseEnv } from './exec.mjs';

const GH_RELEASE_API = 'https://api.github.com/repos/cli/cli/releases/latest';

/** The release asset to download: the macOS universal zip. `null` if the release has none. */
export function pickGhAsset(release) {
  const assets = Array.isArray(release?.assets) ? release.assets : [];
  return assets.find((a) => /macOS_universal\.zip$/i.test(a?.name ?? '')) ?? null;
}

/** Finds `<root>/**\/bin/gh` after extracting the release zip (the folder name carries the version). */
export async function findGhBinary(root) {
  const stack = [root];
  while (stack.length) {
    const dir = stack.pop();
    let entries;
    try {
      entries = await fs.readdir(dir, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) stack.push(full);
      else if (entry.isFile() && entry.name === 'gh' && path.basename(dir) === 'bin') return full;
    }
  }
  return null;
}

function toolsDir(dataDir) {
  return path.join(dataDir, 'tools');
}

/** Looks for `name` on every directory of `PATH`. */
async function findOnPath(name, env) {
  for (const dir of String(env.PATH ?? '').split(':').filter(Boolean)) {
    const candidate = path.join(dir, name);
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

/** `gh` already on PATH, already installed by Launch, or `null`. Never installs. */
export async function findGh(dataDir) {
  const env = baseEnv();
  const onPath = await findOnPath('gh', env);
  if (onPath) return onPath;
  const installed = path.join(toolsDir(dataDir), 'gh', 'bin', 'gh');
  return existsSync(installed) ? installed : null;
}

/** `vercel` already on PATH, already installed by Launch, or `null`. Never installs. */
export async function findVercel(dataDir) {
  const env = baseEnv();
  const onPath = await findOnPath('vercel', env);
  if (onPath) return onPath;
  const installed = path.join(toolsDir(dataDir), 'node_modules', '.bin', 'vercel');
  return existsSync(installed) ? installed : null;
}

/** Downloads and extracts the `gh` CLI into `<dataDir>/tools/gh`. Returns its binary path. */
export async function installGh(dataDir, { log } = {}) {
  log?.('Downloading the GitHub CLI…');
  const response = await fetch(GH_RELEASE_API, { headers: { 'User-Agent': 'agentty-launch-plugin', Accept: 'application/vnd.github+json' } });
  if (!response.ok) throw new Error(`could not check the latest GitHub CLI release (HTTP ${response.status})`);
  const release = await response.json();
  const asset = pickGhAsset(release);
  if (!asset) throw new Error('no macOS build found in the latest GitHub CLI release');
  const dir = toolsDir(dataDir);
  await fs.mkdir(dir, { recursive: true });
  const zipPath = path.join(dir, 'gh.zip');
  const extractDir = path.join(dir, 'gh');
  await fs.rm(extractDir, { recursive: true, force: true });
  const assetResponse = await fetch(asset.browser_download_url);
  if (!assetResponse.ok) throw new Error(`could not download the GitHub CLI (HTTP ${assetResponse.status})`);
  await fs.writeFile(zipPath, Buffer.from(await assetResponse.arrayBuffer()));
  log?.('Installing the GitHub CLI…');
  await fs.mkdir(extractDir, { recursive: true });
  const result = await run('ditto', ['-x', '-k', zipPath, extractDir], { timeoutMs: 60_000 });
  await fs.rm(zipPath, { force: true });
  if (result.code !== 0) throw new Error(`could not extract the GitHub CLI: ${result.stderr || result.stdout}`);
  const binary = await findGhBinary(extractDir);
  if (!binary) throw new Error('the GitHub CLI download did not contain a gh binary');
  await fs.chmod(binary, 0o755).catch(() => {});
  return binary;
}

/** Path to `npm` next to the Node.js running this plugin, falling back to `npm` on PATH. */
function npmPath() {
  const nextToNode = path.join(path.dirname(process.execPath), 'npm');
  return existsSync(nextToNode) ? nextToNode : 'npm';
}

/** Installs an npm package that ships a CLI into `<dataDir>/tools`. Returns the CLI's binary path. */
async function installNpmCli(dataDir, { pkg, bin, label, log }) {
  const dir = toolsDir(dataDir);
  await fs.mkdir(dir, { recursive: true });
  log?.(`Installing the ${label}…`);
  const result = await run(npmPath(), ['install', '--prefix', dir, `${pkg}@latest`, '--no-audit', '--no-fund'], {
    cwd: dir,
    timeoutMs: 5 * 60_000,
    env: baseEnv({ npm_config_yes: 'true' }),
  });
  const binary = path.join(dir, 'node_modules', '.bin', bin);
  if (result.code !== 0 || !existsSync(binary)) throw new Error(`could not install the ${label}: ${result.stderr || result.stdout || 'unknown error'}`);
  return binary;
}

/** Installs `vercel@latest` into `<dataDir>/tools` with npm. Returns the CLI's binary path. */
export async function installVercel(dataDir, { log } = {}) {
  return installNpmCli(dataDir, { pkg: 'vercel', bin: 'vercel', label: 'Vercel CLI', log });
}

/** `supabase` already on PATH, already installed by Launch, or `null`. Never installs. */
export async function findSupabase(dataDir) {
  const onPath = await findOnPath('supabase', baseEnv());
  if (onPath) return onPath;
  const installed = path.join(toolsDir(dataDir), 'node_modules', '.bin', 'supabase');
  return existsSync(installed) ? installed : null;
}

/** Finds `supabase`, installing the npm package into the plugin's data folder if it isn't available anywhere. */
export async function ensureSupabase(dataDir, { log } = {}) {
  return (await findSupabase(dataDir)) ?? (await installNpmCli(dataDir, { pkg: 'supabase', bin: 'supabase', label: 'Supabase CLI', log }));
}

/** Finds `gh`, installing it into the plugin's data folder if it isn't available anywhere. */
export async function ensureGh(dataDir, opts) {
  return (await findGh(dataDir)) ?? (await installGh(dataDir, opts));
}

/** Finds `vercel`, installing it into the plugin's data folder if it isn't available anywhere. */
export async function ensureVercel(dataDir, opts) {
  return (await findVercel(dataDir)) ?? (await installVercel(dataDir, opts));
}

/** True on Apple Silicon and Intel Macs alike — `ditto` and the universal gh build need no arch check. */
export function isMac() {
  return os.platform() === 'darwin';
}
