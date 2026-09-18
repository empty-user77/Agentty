// Tests for the Launch plugin: `node --test plugins/launch/test/plugin.test.mjs`
// Unit tests cover the pure parsing helpers; the end-to-end test drives the plugin as Agentty would,
// against a fake host and fake `gh`/`vercel` executables (shell scripts on a PATH we control) plus
// a real `git`, so the whole "already logged in → save to GitHub → deploy → launched" path runs for
// real against local repositories instead of GitHub/Vercel.

import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createInterface } from 'node:readline';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { stripAnsi, extractDeviceCode, extractDeviceUrl, parseDeployUrl, parseEnvFile, isLocalOnlyValue, sanitizeRepoName, mergeGitignore, envFilesAtRisk, detectFramework, parseInspectAlias } from '../lib/parse.mjs';
import {
  extractSupabaseLoginUrl,
  parseLooseJson,
  normalizeSupabaseProjects,
  normalizeSupabaseOrgs,
  pickSupabasePublicKey,
  supabaseEnvNames,
  detectSupabaseUse,
  mergeEnvFile,
  supabaseRegionForTimeZone,
  supabaseErrorKind,
} from '../lib/parse.mjs';
import { pickGhAsset, pickGhChecksums, checksumFor, findGhBinary } from '../lib/tools.mjs';
import { REQUIRED_VERCELIGNORE_LINES, IDEA_NOTES_GITIGNORE_LINES, secretFilesAtRisk, describeRemote } from '../lib/parse.mjs';
import { pathWithExtras } from '../lib/exec.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const pluginDir = path.resolve(here, '..');
const sdk = path.resolve(here, '../../../sdk/node/agentty-plugin.mjs');

// -- unit tests: pure helpers -----------------------------------------------------------------

test('extractDeviceCode / extractDeviceUrl parse realistic gh and vercel login output', () => {
  const ghOutput = [
    '! First copy your one-time code: \x1B[1m9ABC-2K7Q\x1B[0m',
    'Press Enter to open github.com in your browser...',
    '\x1B[90mOpening in your browser.\x1B[0m',
    'https://github.com/login/device',
  ].join('\n');
  assert.equal(extractDeviceCode(ghOutput), '9ABC-2K7Q');
  assert.equal(extractDeviceUrl(ghOutput), 'https://github.com/login/device');

  const vercelOutput = 'Please visit https://vercel.com/login/device?code=WXYZ-9988 and enter WXYZ-9988 to continue.';
  assert.equal(extractDeviceCode(vercelOutput), 'WXYZ-9988');
  assert.equal(extractDeviceUrl(vercelOutput), 'https://vercel.com/login/device?code=WXYZ-9988');
});

test('extractDeviceCode ignores plain text with no code, extractDeviceUrl picks the device-ish URL', () => {
  assert.equal(extractDeviceCode('no code here'), null);
  const text = 'See docs at https://vercel.com/docs then continue at https://vercel.com/login/device';
  assert.equal(extractDeviceUrl(text), 'https://vercel.com/login/device');
});

test('parseDeployUrl prefers the Production/Aliased line over other URLs', () => {
  const output = ['Vercel CLI 34.0.0', 'Inspect: https://vercel.com/acme/app/abc123', 'Production: https://my-app.vercel.app [copied to clipboard]'].join('\n');
  assert.equal(parseDeployUrl(output), 'https://my-app.vercel.app');

  const aliasedOutput = ['Deploying...', 'Aliased to https://my-app.vercel.app'].join('\n');
  assert.equal(parseDeployUrl(aliasedOutput), 'https://my-app.vercel.app');

  const fallback = 'Deploying...\nhttps://my-app-git-main-acme.vercel.app';
  assert.equal(parseDeployUrl(fallback), 'https://my-app-git-main-acme.vercel.app');

  assert.equal(parseDeployUrl('no urls in here'), null);
});

test('parseEnvFile handles quotes, comments and export prefixes', () => {
  const content = [
    '# a comment',
    'export API_URL=http://localhost:3000',
    'SECRET_KEY="quoted value with spaces"',
    "SINGLE='single quoted'",
    'WITH_COMMENT=value # trailing comment',
    '',
    'NOT_A_VAR this is not valid',
  ].join('\n');
  const vars = parseEnvFile(content);
  assert.equal(vars.API_URL, 'http://localhost:3000');
  assert.equal(vars.SECRET_KEY, 'quoted value with spaces');
  assert.equal(vars.SINGLE, 'single quoted');
  assert.equal(vars.WITH_COMMENT, 'value');
  assert.equal(vars.NOT_A_VAR, undefined);
});

test('isLocalOnlyValue flags loopback addresses only', () => {
  assert.equal(isLocalOnlyValue('http://localhost:3000'), true);
  assert.equal(isLocalOnlyValue('http://127.0.0.1:8080/api'), true);
  assert.equal(isLocalOnlyValue('https://api.example.com'), false);
  assert.equal(isLocalOnlyValue('sk-example-not-a-real-key'), false);
});

test('sanitizeRepoName produces a safe GitHub repo name', () => {
  assert.equal(sanitizeRepoName('My Cool App!'), 'my-cool-app');
  assert.equal(sanitizeRepoName('  ..weird--name..  '), 'weird--name');
  assert.equal(sanitizeRepoName(''), 'my-project');
  assert.equal(sanitizeRepoName('café_app'), 'caf-_app');
});

test('mergeGitignore appends only the missing required lines, leaving the rest untouched', () => {
  const existing = 'dist\ncustom-ignore\n';
  const merged = mergeGitignore(existing);
  assert.match(merged, /^dist\ncustom-ignore\n/);
  assert.match(merged, /node_modules/);
  assert.match(merged, /!\.env\.example/);
  // Re-merging is a no-op.
  assert.equal(mergeGitignore(merged), merged);
});

test('mergeGitignore starting from nothing produces a clean file', () => {
  const merged = mergeGitignore('');
  assert.match(merged, /^# Added by Agentty Launch\n/);
  assert.match(merged, /\.env\n/);
});

test('envFilesAtRisk refuses real .env files but allows examples', () => {
  const risky = envFilesAtRisk(['.env', '.env.production', '.env.example', 'src/.env.local', '.env.sample', 'README.md']);
  assert.deepEqual(risky.sort(), ['.env', '.env.production', 'src/.env.local'].sort());
});

test('detectFramework recognizes common frameworks and falls back to static', () => {
  assert.equal(detectFramework({ pkg: { dependencies: { next: '14.0.0' } } }), 'next');
  assert.equal(detectFramework({ pkg: { devDependencies: { vite: '5.0.0' } } }), 'vite');
  assert.equal(detectFramework({ pkg: { dependencies: { react: '18.0.0' } } }), 'react');
  assert.equal(detectFramework({ pkg: null, hasIndexHtml: true }), 'static');
  assert.equal(detectFramework({ pkg: null, hasIndexHtml: false }), null);
});

test('pickGhAsset finds the macOS universal zip in a release', () => {
  const release = {
    assets: [
      { name: 'gh_2.60.0_linux_amd64.tar.gz', browser_download_url: 'https://example.com/linux' },
      { name: 'gh_2.60.0_macOS_universal.zip', browser_download_url: 'https://example.com/mac' },
      { name: 'gh_2.60.0_windows_amd64.zip', browser_download_url: 'https://example.com/win' },
    ],
  };
  assert.equal(pickGhAsset(release).browser_download_url, 'https://example.com/mac');
  assert.equal(pickGhAsset({ assets: [] }), null);
  assert.equal(pickGhAsset({}), null);
});

test('findGhBinary locates bin/gh under a version-named extraction folder', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'gh-extract-'));
  try {
    const binDir = path.join(root, 'gh_2.60.0_macOS_universal', 'bin');
    fs.mkdirSync(binDir, { recursive: true });
    fs.writeFileSync(path.join(binDir, 'gh'), '#!/bin/sh\necho fake\n');
    assert.equal(await findGhBinary(root), path.join(binDir, 'gh'));
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('pathWithExtras puts the running Node.js first so `#!/usr/bin/env node` tools start', () => {
  // An app started from the Dock: nvm's Node.js runs the plugin but is not on PATH.
  const nodeDir = '/Users/me/.nvm/versions/node/v22.0.0/bin';
  assert.equal(pathWithExtras({ PATH: '/usr/bin:/bin' }, nodeDir), `${nodeDir}:/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin`);
  assert.equal(pathWithExtras({ PATH: `/usr/bin:${nodeDir}` }, nodeDir), `/usr/bin:${nodeDir}:/opt/homebrew/bin:/usr/local/bin`);
  assert.ok(pathWithExtras({ PATH: '/usr/bin' }).startsWith(path.dirname(process.execPath)));
});

test('the gh download is checked against the checksum file of its release', () => {
  const release = { assets: [{ name: 'gh_2.60.0_checksums.txt', browser_download_url: 'https://example.com/sums' }, { name: 'gh_2.60.0_macOS_universal.zip' }] };
  assert.equal(pickGhChecksums(release).browser_download_url, 'https://example.com/sums');
  assert.equal(pickGhChecksums({ assets: [] }), null);
  const sums = `${'a'.repeat(64)}  gh_2.60.0_linux_amd64.tar.gz\n${'b'.repeat(64)}  gh_2.60.0_macOS_universal.zip\n`;
  assert.equal(checksumFor(sums, 'gh_2.60.0_macOS_universal.zip'), 'b'.repeat(64));
  assert.equal(checksumFor(sums, 'gh_2.60.0_windows_amd64.zip'), null);
});

test('key files stop a save, and .envrc counts as an env file', () => {
  assert.deepEqual(secretFilesAtRisk(['src/app.ts', 'certs/server.pem', 'deploy/id_ed25519', '.aws/credentials', 'README.md']).sort(), ['.aws/credentials', 'certs/server.pem', 'deploy/id_ed25519']);
  assert.deepEqual(envFilesAtRisk(['.envrc', 'sub/.envrc', '.env.example']).sort(), ['.envrc', 'sub/.envrc']);
});

test('describeRemote names where a push goes, for every remote URL shape', () => {
  assert.equal(describeRemote('git@github.com:me/my-app.git').display, 'github.com/me/my-app');
  assert.equal(describeRemote('https://github.com/me/my-app').display, 'github.com/me/my-app');
  assert.equal(describeRemote('https://token_example@git.example.org/team/app.git').display, 'git.example.org/team/app');
  assert.equal(describeRemote('/tmp/remotes/app.git'), null);
});

test('stripAnsi removes color and cursor codes', () => {
  assert.equal(stripAnsi('\x1B[1mBold\x1B[0m plain'), 'Bold plain');
});

// -- unit tests: Supabase helpers ---------------------------------------------------------------

test('extractSupabaseLoginUrl finds the CLI login link and nothing else', () => {
  const output = 'Docs: https://supabase.com/docs\nHere is your login link, open it in the browser: \x1B[1mhttps://supabase.com/dashboard/cli/login?session_id=fake-session&token_name=cli_fake&public_key=fake\x1B[0m\n\nEnter your verification code: ';
  assert.equal(extractSupabaseLoginUrl(output), 'https://supabase.com/dashboard/cli/login?session_id=fake-session&token_name=cli_fake&public_key=fake');
  assert.equal(extractSupabaseLoginUrl('Access token not provided. See https://supabase.com/docs'), null);
});

test('Supabase project and organization lists are read from either JSON shape', () => {
  const current = '[{"id":"abcdefghijklmnopqrst","organization_id":"fake-org-slug","name":"shop","region":"ap-northeast-2","status":"ACTIVE_HEALTHY"}]';
  assert.deepEqual(normalizeSupabaseProjects(parseLooseJson(current)), [{ ref: 'abcdefghijklmnopqrst', name: 'shop', region: 'ap-northeast-2', status: 'ACTIVE_HEALTHY', orgId: 'fake-org-slug' }]);
  // A newer API carries a separate `ref`, and a CLI may wrap the list or print a log line first.
  const wrapped = 'Fetching projects...\n{"projects":[{"id":"6f1c0c2e","ref":"tsrqponmlkjihgfedcba","name":"blog"}]}';
  assert.deepEqual(normalizeSupabaseProjects(parseLooseJson(wrapped)).map((p) => p.ref), ['tsrqponmlkjihgfedcba']);
  assert.deepEqual(normalizeSupabaseProjects(parseLooseJson('not json')), []);
  assert.deepEqual(normalizeSupabaseOrgs(parseLooseJson('[{"id":"fake-org-slug","name":"Fake Org"}]')), [{ id: 'fake-org-slug', name: 'Fake Org' }]);
});

test('pickSupabasePublicKey only ever returns a key a browser may hold', () => {
  const legacy = [
    { name: 'service_role', api_key: 'service_example_not_a_real_key' },
    { name: 'anon', api_key: 'anon_example_not_a_real_key' },
  ];
  assert.equal(pickSupabasePublicKey(legacy), 'anon_example_not_a_real_key');
  const modern = [
    { name: 'default', type: 'secret', api_key: 'sb_secret_example_not_a_real_key' },
    { name: 'default', type: 'publishable', api_key: 'sb_publishable_example_not_a_real_key' },
  ];
  assert.equal(pickSupabasePublicKey(modern), 'sb_publishable_example_not_a_real_key');
  assert.equal(pickSupabasePublicKey([{ name: 'service_role', api_key: 'service_example_not_a_real_key' }]), null);
  assert.equal(pickSupabasePublicKey(null), null);
});

test('supabaseEnvNames follows the framework unless the project already names its variables', () => {
  assert.deepEqual(supabaseEnvNames('next', []), { url: 'NEXT_PUBLIC_SUPABASE_URL', key: 'NEXT_PUBLIC_SUPABASE_ANON_KEY' });
  assert.deepEqual(supabaseEnvNames('vite', []), { url: 'VITE_SUPABASE_URL', key: 'VITE_SUPABASE_ANON_KEY' });
  assert.deepEqual(supabaseEnvNames('node', []), { url: 'SUPABASE_URL', key: 'SUPABASE_ANON_KEY' });
  // The app's code already reads these names; a service-role name is never picked as "the key".
  const existing = ['NEXT_PUBLIC_SUPABASE_URL', 'SUPABASE_SERVICE_ROLE_KEY', 'NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY'];
  assert.deepEqual(supabaseEnvNames('next', existing), { url: 'NEXT_PUBLIC_SUPABASE_URL', key: 'NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY' });
});

test('detectSupabaseUse notices the client library, the supabase folder or env names', () => {
  assert.equal(detectSupabaseUse({ pkg: { dependencies: { '@supabase/supabase-js': '2.0.0' } } }), true);
  assert.equal(detectSupabaseUse({ pkg: { dependencies: { next: '15.0.0' } }, hasSupabaseDir: true }), true);
  assert.equal(detectSupabaseUse({ pkg: null, envNames: ['NEXT_PUBLIC_SUPABASE_URL'] }), true);
  assert.equal(detectSupabaseUse({ pkg: { dependencies: { next: '15.0.0' } }, envNames: ['API_URL'] }), false);
});

test('mergeEnvFile replaces keys in place and appends new ones without touching the rest', () => {
  const existing = '# local settings\nAPI_URL=http://localhost:3000\nexport VITE_SUPABASE_URL=http://127.0.0.1:54321\n';
  const merged = mergeEnvFile(existing, { VITE_SUPABASE_URL: 'https://abcdefghijklmnopqrst.supabase.co', VITE_SUPABASE_ANON_KEY: 'anon_example_not_a_real_key' });
  assert.equal(merged, '# local settings\nAPI_URL=http://localhost:3000\nVITE_SUPABASE_URL=https://abcdefghijklmnopqrst.supabase.co\nVITE_SUPABASE_ANON_KEY=anon_example_not_a_real_key\n');
  assert.equal(mergeEnvFile('', { A: '1' }), 'A=1\n');
  assert.equal(mergeEnvFile(merged, {}), merged);
  // `null` removes a key; nothing left means no file content at all.
  assert.equal(mergeEnvFile('A=1\nB=2\n', { A: null }), 'B=2\n');
  assert.equal(mergeEnvFile('A=1\n', { A: null, C: null }), '');
});

test('supabaseRegionForTimeZone picks a nearby region and falls back to us-east-1', () => {
  assert.equal(supabaseRegionForTimeZone('Asia/Seoul'), 'ap-northeast-2');
  assert.equal(supabaseRegionForTimeZone('Asia/Tokyo'), 'ap-northeast-1');
  assert.equal(supabaseRegionForTimeZone('Europe/Berlin'), 'eu-central-1');
  assert.equal(supabaseRegionForTimeZone('America/Los_Angeles'), 'us-west-1');
  assert.equal(supabaseRegionForTimeZone('America/New_York'), 'us-east-1');
  assert.equal(supabaseRegionForTimeZone(undefined), 'us-east-1');
});

test('supabaseErrorKind recognizes account problems an agent cannot fix', () => {
  assert.equal(supabaseErrorKind('Access token not provided. Supply an access token by running supabase login'), 'login');
  assert.equal(supabaseErrorKind('The following organization members have reached their maximum limits for the number of active free projects'), 'free-limit');
  assert.equal(supabaseErrorKind('failed to connect: password authentication failed for user "postgres"'), 'db-password');
  assert.equal(supabaseErrorKind('ERROR: relation "todos" already exists (SQLSTATE 42P07)'), null);
});

// -- end-to-end: fake gh + vercel, real git ----------------------------------------------------

function sandbox() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'launch-plugin-'));
  const project = path.join(root, 'my-cool-app');
  fs.mkdirSync(project, { recursive: true });
  fs.writeFileSync(path.join(project, 'package.json'), JSON.stringify({ name: 'my-cool-app', dependencies: { vite: '5.0.0' } }));
  const remotes = path.join(root, 'remotes');
  fs.mkdirSync(remotes, { recursive: true });

  const bin = path.join(root, 'bin');
  fs.mkdirSync(bin, { recursive: true });
  fs.writeFileSync(path.join(bin, 'gh'), FAKE_GH, { mode: 0o755 });
  fs.writeFileSync(path.join(bin, 'vercel'), FAKE_VERCEL, { mode: 0o755 });
  fs.writeFileSync(path.join(bin, 'supabase'), FAKE_SUPABASE, { mode: 0o755 });
  const supabase = path.join(root, 'supabase-account');
  fs.mkdirSync(supabase, { recursive: true });

  const plugin = path.join(root, 'plugin');
  fs.mkdirSync(path.join(plugin, 'lib'), { recursive: true });
  for (const file of ['agentty-plugin.json', 'main.mjs']) fs.copyFileSync(path.join(pluginDir, file), path.join(plugin, file));
  for (const file of fs.readdirSync(path.join(pluginDir, 'lib'))) fs.copyFileSync(path.join(pluginDir, 'lib', file), path.join(plugin, 'lib', file));
  fs.copyFileSync(sdk, path.join(plugin, 'agentty-plugin.mjs'));

  return { root, project, remotes, bin, plugin, supabase, data: path.join(root, 'data') };
}

/** Turns the sandbox project into one that uses Supabase and has a migration waiting. */
function useSupabase(box) {
  fs.writeFileSync(path.join(box.project, 'package.json'), JSON.stringify({ name: 'my-cool-app', dependencies: { vite: '5.0.0', '@supabase/supabase-js': '2.0.0' } }));
  fs.mkdirSync(path.join(box.project, 'supabase', 'migrations'), { recursive: true });
  fs.writeFileSync(path.join(box.project, 'supabase', 'migrations', '20260918000000_todos.sql'), 'create table todos (id bigint primary key);\nalter table todos enable row level security;\n');
}

// The "account" is a folder: `logged-in` marks a session, `projects.json` the hosted projects, and
// `calls.log` records what ran — whether a database password was supplied, never the password.
const FAKE_SUPABASE = `#!/bin/bash
account="$FAKE_SB_DIR"
case "$1" in
  login)
    echo "Here is your login link, open it in the browser: https://supabase.com/dashboard/cli/login?session_id=fake-session&token_name=cli_fake&public_key=fake"
    printf "Enter your verification code: "
    read -r code
    if [ "$code" = "fakecode" ]; then touch "$account/logged-in"; echo "You are now logged in."; exit 0; fi
    echo "invalid verification code" >&2
    exit 1
    ;;
  projects)
    if [ ! -f "$account/logged-in" ]; then echo "Access token not provided. Supply an access token by running supabase login." >&2; exit 1; fi
    case "$2" in
      list)
        if [ -f "$account/projects.json" ]; then cat "$account/projects.json"; else echo "[]"; fi
        exit 0
        ;;
      create)
        if [ -f "$account/free-limit" ]; then echo "The following organization members have reached their maximum limits for the number of active free projects" >&2; exit 1; fi
        echo "create name=$3 password-saved-first=$(grep -c '^SUPABASE_DB_PASSWORD=.' .env.local 2>/dev/null)" >> "$account/calls.log"
        echo '[{"id":"abcdefghijklmnopqrst","name":"my-cool-app","region":"us-east-1","status":"ACTIVE_HEALTHY"}]' > "$account/projects.json"
        echo '{"id":"abcdefghijklmnopqrst","name":"my-cool-app"}'
        exit 0
        ;;
      api-keys)
        echo '[{"name":"anon","api_key":"anon_example_not_a_real_key"},{"name":"service_role","api_key":"service_example_not_a_real_key"}]'
        exit 0
        ;;
    esac
    ;;
  orgs)
    if [ ! -f "$account/logged-in" ]; then echo "Access token not provided." >&2; exit 1; fi
    echo '[{"id":"fake-org-slug","name":"Fake Org"}]'
    exit 0
    ;;
  link)
    echo "link ref=$3 password=\${SUPABASE_DB_PASSWORD:+given}" >> "$account/calls.log"
    exit 0
    ;;
  db)
    if [ "$2" = "push" ]; then echo "push password=\${SUPABASE_DB_PASSWORD:+given}" >> "$account/calls.log"; exit 0; fi
    ;;
esac
exit 1
`;

const FAKE_GH = `#!/bin/bash
set -e
case "$1" in
  auth)
    case "$2" in
      status) exit 0 ;;
      setup-git) exit 0 ;;
      login) exit 1 ;;
    esac
    ;;
  api)
    if [ "$3" = "--jq" ] && [ "$4" = ".login" ]; then echo "fakeuser"; exit 0; fi
    if [ "$3" = "--jq" ]; then echo '{"login":"fakeuser","id":424242}'; exit 0; fi
    ;;
  repo)
    case "$2" in
      create)
        name="$3"
        bare="$FAKE_REMOTES_DIR/$name.git"
        git init --quiet --bare "$bare"
        git remote add origin "$bare"
        git push --quiet -u origin HEAD
        exit 0
        ;;
      view)
        echo "https://github.com/fake-user/my-cool-app"
        exit 0
        ;;
    esac
    ;;
esac
exit 1
`;

const FAKE_VERCEL = `#!/bin/bash
case "$1" in
  whoami) echo "fakeuser"; exit 0 ;;
  login) exit 1 ;;
  env)
    if [ "$2" = "add" ]; then cat >/dev/null; exit 0; fi
    ;;
  deploy)
    echo "deploy" >> "$FAKE_REMOTES_DIR/../deploys.log"
    echo "Vercel CLI 34.0.0"
    echo "Deploying my-cool-app"
    echo "Production: https://my-cool-app.vercel.app"
    exit 0
    ;;
  git)
    if [ "$2" = "connect" ]; then exit 0; fi
    ;;
esac
exit 1
`;

/** Starts the plugin; `host.next(method)` resolves with the next call of that method (answered with `result`). */
function start(box, results = {}) {
  const env = {
    ...process.env,
    PATH: `${box.bin}:${process.env.PATH}`,
    FAKE_REMOTES_DIR: box.remotes,
    FAKE_SB_DIR: box.supabase,
  };
  const child = spawn(process.execPath, ['main.mjs'], { cwd: box.plugin, env, stdio: ['pipe', 'pipe', 'pipe'] });
  child.stdin.on('error', () => {}); // the plugin process may already be gone by the time a late reply is sent
  const waiting = [];
  const seen = [];
  let stderr = '';
  let stopped = false;
  child.stderr.on('data', (d) => (stderr += d));
  createInterface({ input: child.stdout }).on('line', (line) => {
    if (stopped) return;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      return;
    }
    if (message.id !== undefined && message.method) {
      const result = results[message.method] ?? null;
      write({ jsonrpc: '2.0', id: message.id, result });
    }
    if (!message.method) return;
    const index = waiting.findIndex((w) => w.method === message.method);
    if (index === -1) seen.push(message);
    else waiting.splice(index, 1)[0].resolve(message);
  });
  function write(message) {
    try {
      child.stdin.write(JSON.stringify(message) + '\n');
    } catch {
      // The plugin process already exited.
    }
  }
  const send = (method, params, id) => write({ jsonrpc: '2.0', method, params, ...(id ? { id } : {}) });
  const context = { workspace: null, pane: { id: 1, kind: 'shell', tool: 'zsh', title: 'Terminal', cwd: box.project, status: 'idle', running: true }, language: 'en' };
  send('initialize', { apiVersion: 1, plugin: { id: 'launch', name: 'Launch', dir: box.plugin, dataDir: box.data }, language: 'en', context }, 1);
  return {
    child,
    context,
    send,
    stderr: () => stderr,
    next(method, timeout = 15000) {
      const index = seen.findIndex((m) => m.method === method);
      if (index !== -1) return Promise.resolve(seen.splice(index, 1)[0]);
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error(`no ${method} within ${timeout} ms; stderr: ${stderr}`)), timeout);
        waiting.push({ method, resolve: (m) => (clearTimeout(timer), resolve(m)) });
      });
    },
    stop() {
      stopped = true;
      child.kill();
      fs.rmSync(box.root, { recursive: true, force: true });
    },
  };
}

function buttonIds(tree) {
  const ids = [];
  (function walk(node) {
    if (!node) return;
    if (node.type === 'button') ids.push(node.id);
    for (const child of node.children ?? []) walk(child);
  })(tree);
  return ids;
}

test('Launch: already logged in to GitHub and Vercel, saves the project and deploys it', async () => {
  const box = sandbox();
  const host = start(box);
  try {
    host.send('panel/open', { context: host.context });
    // Tools and both logins are already available, so the wizard lands straight on "Save to GitHub"
    // once the initial loading spinner settles.
    let panel = await waitForButton(host, 'gh-save');
    assert.match(JSON.stringify(panel), /vite/); // framework detected

    host.send('ui/event', { element: 'gh-save', event: 'click', context: host.context });
    // Wait for the panel to settle on the next step (deploy) rather than an intermediate spinner frame.
    panel = await waitForButton(host, 'deploy-start');
    assert.ok(fs.existsSync(path.join(box.remotes, 'my-cool-app.git')), 'gh repo create pushed to a local bare repo');
    // Finished steps are green checks; the ones still ahead stay neutral.
    const steps = {};
    (function walk(node) {
      if (node?.type === 'list' && node.id === 'steps') for (const item of node.items) steps[item.id] = `${item.icon} ${item.tone}`;
      for (const child of node?.children ?? []) walk(child);
    })(panel);
    assert.equal(steps['gh-save'], 'circle-check success');
    assert.equal(steps.deploy, 'circle-pause neutral');
    assert.ok(!JSON.stringify(panel).includes('fakeuser@'), 'no raw credentials leak into the panel');

    // A double click on "Publish": the second event arrives while the first runs and is ignored.
    host.send('ui/event', { element: 'deploy-start', event: 'click', context: host.context });
    host.send('ui/event', { element: 'deploy-start', event: 'click', context: host.context });
    panel = await waitForText(host, /my-cool-app\.vercel\.app/);
    assert.equal(fs.readFileSync(path.join(box.root, 'deploys.log'), 'utf8').trim().split('\n').length, 1, 'one deploy for a double click');
    // The owner's idea notes, agent settings and env files never go up with a deploy.
    const vercelignore = fs.readFileSync(path.join(box.project, '.vercelignore'), 'utf8');
    for (const line of REQUIRED_VERCELIGNORE_LINES) assert.ok(vercelignore.split('\n').includes(line), `.vercelignore has ${line}`);
    assert.ok(!fs.readFileSync(path.join(box.project, '.gitignore'), 'utf8').includes(IDEA_NOTES_GITIGNORE_LINES[0]), 'a private repository keeps the idea notes');
    assert.match(JSON.stringify(panel), /Launched|출시 완료/);
    assert.match(JSON.stringify(panel), /https:\/\/my-cool-app\.vercel\.app/);

    const saved = JSON.parse(fs.readFileSync(path.join(box.data, 'projects.json'), 'utf8'));
    assert.equal(saved[box.project].lastDeployUrl, 'https://my-cool-app.vercel.app');
    assert.equal(saved[box.project].repoUrl, 'https://github.com/fake-user/my-cool-app');
  } finally {
    host.stop();
  }
});

test('Launch refuses to save when a real .env file is already staged', async () => {
  const box = sandbox();
  // Simulate an agent having already `git add`ed a real .env before Launch ever ran — adding
  // ".env" to .gitignore afterwards would not by itself untrack it, so the guard must catch this.
  fs.writeFileSync(path.join(box.project, '.env'), 'SECRET=not_a_real_value\n');
  const git = (...args) => execFileSync('git', args, { cwd: box.project, stdio: ['ignore', 'pipe', 'ignore'] });
  git('init', '--quiet', '-b', 'main');
  git('config', 'user.email', 'test@example.com');
  git('config', 'user.name', 'Test');
  git('add', '.env');

  const host = start(box);
  try {
    host.send('panel/open', { context: host.context });
    await host.next('ui/setPanel');
    host.send('ui/event', { element: 'gh-save', event: 'click', context: host.context });
    const panel = await waitForText(host, /\.env/);
    assert.match(JSON.stringify(panel), /"style":"error"/);
    // Nothing was committed: no HEAD exists yet.
    assert.throws(() => git('rev-parse', 'HEAD'));
  } finally {
    host.stop();
  }
});

test('no project detected when the folder has neither package.json nor index.html', async () => {
  const box = sandbox();
  const empty = path.join(box.root, 'empty-folder');
  fs.mkdirSync(empty);
  const host = start(box);
  try {
    host.send('panel/open', { context: { ...host.context, pane: { ...host.context.pane, cwd: empty } } });
    const panel = await waitForText(host, /package\.json|index\.html|아직 웹 프로젝트/);
    assert.match(JSON.stringify(panel), /package\.json|index\.html|아직 웹 프로젝트/);
  } finally {
    host.stop();
  }
});

test('Launch connects Supabase: login code, new project, public key only, migrations, then the env step', async () => {
  const box = sandbox();
  useSupabase(box);
  const host = start(box);
  const click = (element) => host.send('ui/event', { element, event: 'click', context: host.context });
  try {
    host.send('panel/open', { context: host.context });
    await waitForButton(host, 'gh-save');
    click('gh-save');
    // The project uses Supabase and has no keys yet, so the database step comes before env/deploy.
    await waitForButton(host, 'sb-connect');
    click('sb-connect');

    // Not logged in: the login link opens in the browser and the panel asks for the verification code.
    const opened = await host.next('host/openUrl');
    assert.match(opened.params.url, /^https:\/\/supabase\.com\/dashboard\/cli\/login\?/);
    await waitForText(host, /"id":"sb-code"/);
    host.send('ui/event', { element: 'sb-code', event: 'submit', value: 'fakecode', context: host.context });

    // Logged in with no projects yet: create one.
    await waitForButton(host, 'sb-create');
    click('sb-create');
    let panel = await waitForButton(host, 'sb-continue');
    const envLocal = fs.readFileSync(path.join(box.project, '.env.local'), 'utf8');
    assert.match(envLocal, /^VITE_SUPABASE_URL=https:\/\/abcdefghijklmnopqrst\.supabase\.co$/m);
    assert.match(envLocal, /^VITE_SUPABASE_ANON_KEY=anon_example_not_a_real_key$/m);
    assert.match(envLocal, /^SUPABASE_DB_PASSWORD=.{20,}$/m);
    assert.ok(!envLocal.includes('service_example'), 'the service_role key never reaches the project');
    assert.equal(fs.statSync(path.join(box.project, '.env.local')).mode & 0o777, 0o600);
    assert.match(fs.readFileSync(path.join(box.project, '.env.example'), 'utf8'), /^VITE_SUPABASE_URL=$/m);
    assert.match(fs.readFileSync(path.join(box.project, '.gitignore'), 'utf8'), /^\.env\.local$/m);
    const dbPassword = /^SUPABASE_DB_PASSWORD=(.+)$/m.exec(envLocal)[1];
    assert.ok(!JSON.stringify(panel).includes(dbPassword) && !JSON.stringify(panel).includes('anon_example'), 'no key or password is shown in the panel');

    // The migration written by the agent is applied, with the password passed in the environment.
    click('sb-continue');
    await waitForButton(host, 'sb-migrate');
    click('sb-migrate');
    panel = await waitForButton(host, 'env-add');
    const calls = fs.readFileSync(path.join(box.supabase, 'calls.log'), 'utf8');
    // The generated password was in .env.local before the project existed, so it cannot be lost.
    assert.match(calls, /^create name=my-cool-app password-saved-first=1$/m);
    assert.match(calls, /^link ref=abcdefghijklmnopqrst password=given$/m);
    assert.match(calls, /^push password=given$/m);
    assert.ok(!calls.includes(dbPassword));

    // The new variables are offered to the live site; the database password starts unselected.
    const toggles = {};
    (function walk(node) {
      if (node?.type === 'toggle') toggles[node.id] = node.value;
      for (const child of node?.children ?? []) walk(child);
    })(panel);
    assert.deepEqual(toggles, { 'env:VITE_SUPABASE_URL': true, 'env:VITE_SUPABASE_ANON_KEY': true, 'env:SUPABASE_DB_PASSWORD': false });

    click('env-add');
    await waitForButton(host, 'deploy-start');
    const saved = JSON.parse(fs.readFileSync(path.join(box.data, 'projects.json'), 'utf8'))[box.project];
    assert.equal(saved.supabaseRef, 'abcdefghijklmnopqrst');
    assert.deepEqual(saved.supabaseMigrations, ['20260918000000_todos.sql']);
    assert.ok(!JSON.stringify(saved).includes(dbPassword), 'Launch state holds no password');
  } finally {
    host.stop();
  }
});

test('a refused project creation is explained in plain words and leaves no password behind', async () => {
  const box = sandbox();
  useSupabase(box);
  fs.writeFileSync(path.join(box.supabase, 'logged-in'), '');
  fs.writeFileSync(path.join(box.supabase, 'free-limit'), '');
  const host = start(box);
  const click = (element) => host.send('ui/event', { element, event: 'click', context: host.context });
  try {
    host.send('panel/open', { context: host.context });
    await waitForButton(host, 'gh-save');
    click('gh-save');
    await waitForButton(host, 'sb-connect');
    click('sb-connect');
    await waitForButton(host, 'sb-create');
    // A double click: the second event arrives while the first is still running and is ignored.
    click('sb-create');
    click('sb-create');
    const panel = await waitForText(host, /maximum number of free projects/);
    assert.match(JSON.stringify(panel), /"style":"error"/);
    assert.ok(!fs.existsSync(path.join(box.project, '.env.local')), 'the unused password is removed again');
    assert.ok(!fs.existsSync(path.join(box.supabase, 'calls.log')), 'no project was created');
  } finally {
    host.stop();
  }
});

test('Launch uses an existing Supabase project and asks for its database password before migrating', async () => {
  const box = sandbox();
  useSupabase(box);
  fs.writeFileSync(path.join(box.supabase, 'logged-in'), '');
  fs.writeFileSync(path.join(box.supabase, 'projects.json'), '[{"id":"abcdefghijklmnopqrst","name":"existing-shop","region":"ap-northeast-2","status":"ACTIVE_HEALTHY"}]');
  const host = start(box);
  const click = (element) => host.send('ui/event', { element, event: 'click', context: host.context });
  try {
    host.send('panel/open', { context: host.context });
    await waitForButton(host, 'gh-save');
    click('gh-save');
    await waitForButton(host, 'sb-connect');
    click('sb-connect');
    const panel = await waitForButton(host, 'sb-create');
    assert.match(JSON.stringify(panel), /existing-shop/);
    host.send('ui/event', { element: 'sb-projects', event: 'select', item: 'abcdefghijklmnopqrst', context: host.context });
    await waitForButton(host, 'sb-continue');
    assert.ok(!fs.readFileSync(path.join(box.project, '.env.local'), 'utf8').includes('SUPABASE_DB_PASSWORD'), 'no password is invented for a project Launch did not create');

    click('sb-continue');
    await waitForButton(host, 'sb-migrate');
    click('sb-migrate');
    await waitForButton(host, 'sb-dbpass-save');
    host.send('ui/event', { element: 'sb-dbpass', event: 'submit', value: 'typed_example_not_a_real_password', context: host.context });
    await waitForButton(host, 'env-add');
    assert.match(fs.readFileSync(path.join(box.supabase, 'calls.log'), 'utf8'), /^push password=given$/m);
  } finally {
    host.stop();
  }
});

/** Waits for a `ui/setPanel` whose tree contains a button with this id (skips intermediate spinner frames). */
async function waitForButton(host, id, timeout = 15000) {
  const started = Date.now();
  for (;;) {
    const message = await host.next('ui/setPanel', timeout - (Date.now() - started));
    if (buttonIds(message.params.tree).includes(id)) return message.params.tree;
    if (Date.now() - started > timeout) throw new Error(`no panel with button ${id} within ${timeout}ms`);
  }
}

/** Waits for a `ui/setPanel` whose serialized tree matches `re`. */
async function waitForText(host, re, timeout = 15000) {
  const started = Date.now();
  for (;;) {
    const message = await host.next('ui/setPanel', timeout - (Date.now() - started));
    if (re.test(JSON.stringify(message.params.tree))) return message.params.tree;
    if (Date.now() - started > timeout) throw new Error(`no panel matching ${re} within ${timeout}ms`);
  }
}

test('parseDeployUrl prefers the public alias over the protected deployment URL', () => {
  const output = [
    '🔍  Inspect: https://vercel.com/acme/my-app/AbC123 [2s]',
    '✅  Production: https://my-app-k2j3h4-acme.vercel.app [20s]',
    '🔗  Aliased: https://my-app.vercel.app [21s]',
  ].join('\n');
  assert.equal(parseDeployUrl(output), 'https://my-app.vercel.app');
});

test('parseInspectAlias picks the shortest vercel.app alias', () => {
  const output = 'Aliases\n  ╶ https://my-app-git-main-acme.vercel.app\n  ╶ https://my-app.vercel.app\n  ╶ https://my-app-acme.vercel.app\n';
  assert.equal(parseInspectAlias(output), 'https://my-app.vercel.app');
  assert.equal(parseInspectAlias('nothing'), null);
});
