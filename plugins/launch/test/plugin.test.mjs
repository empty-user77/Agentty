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
import { isVercelProductionDeployment, pickVercelProductionDeployments, summarizeDeployment, liveUrlOf, parseInspectDomains } from '../lib/parse.mjs';
import {
  deployStateOf,
  displayWidth,
  flowDiagram,
  normalizeVercelProject,
  normalizeVercelProjects,
  pickPrimaryDomain,
  repoOfVercelLink,
  shortenToWidth,
  sshGithubLogin,
} from '../lib/parse.mjs';
import { configFileCandidates, scopeList, sessionFileCandidates } from '../lib/vercel_api.mjs';
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
  // Nothing in a test may reach github.com: `githubIdentity` falls back to ssh when `gh` is not
  // signed in, so the sandbox brings its own.
  fs.writeFileSync(path.join(bin, 'ssh'), FAKE_SSH, { mode: 0o755 });
  const home = path.join(root, 'home');
  fs.mkdirSync(home, { recursive: true });
  const supabase = path.join(root, 'supabase-account');
  fs.mkdirSync(supabase, { recursive: true });

  const plugin = path.join(root, 'plugin');
  fs.mkdirSync(path.join(plugin, 'lib'), { recursive: true });
  for (const file of ['agentty-plugin.json', 'main.mjs']) fs.copyFileSync(path.join(pluginDir, file), path.join(plugin, file));
  for (const file of fs.readdirSync(path.join(pluginDir, 'lib'))) fs.copyFileSync(path.join(pluginDir, 'lib', file), path.join(plugin, 'lib', file));
  fs.copyFileSync(sdk, path.join(plugin, 'agentty-plugin.mjs'));

  return { root, project, remotes, bin, plugin, supabase, home, data: path.join(root, 'data') };
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

// `ssh -T git@github.com` on a computer whose key GitHub knows (FAKE_SSH_LOGIN), or one it doesn't.
const FAKE_SSH = `#!/bin/bash
if [ -n "$FAKE_SSH_LOGIN" ]; then
  echo "Hi $FAKE_SSH_LOGIN! You've successfully authenticated, but GitHub does not provide shell access." >&2
  exit 1
fi
echo "git@github.com: Permission denied (publickey)." >&2
exit 255
`;

const FAKE_GH = `#!/bin/bash
set -e
case "$1" in
  auth)
    case "$2" in
      status) if [ -n "$FAKE_GH_DELAY" ]; then sleep "$FAKE_GH_DELAY"; fi; if [ -n "$FAKE_GH_LOGGED_OUT" ]; then echo "You are not logged into any GitHub hosts." >&2; exit 1; fi; exit 0 ;;
      setup-git) exit 0 ;;
      login) exit 1 ;;
    esac
    ;;
  api)
    if [ "$3" = "--jq" ] && [ "$4" = ".login" ]; then echo "fakeuser"; exit 0; fi
    if [ "$3" = "--jq" ]; then echo '{"login":"fakeuser","id":424242}'; exit 0; fi
    # A repository Vercel already deploys from GitHub (FAKE_HOSTED=1): its production deployments.
    case "$2" in
      repos/fake-user/my-cool-app/deployments/7/statuses*)
        echo '[{"state":"success","environment_url":"https://my-cool-app-live.vercel.app","log_url":"https://vercel.com/fake-user/my-cool-app/dpl7","created_at":"2026-09-18T10:00:00Z"}]'; exit 0 ;;
      repos/fake-user/my-cool-app/deployments/6/statuses*)
        echo '[{"state":"failure","environment_url":"https://my-cool-app-old.vercel.app","created_at":"2026-09-17T10:00:00Z"}]'; exit 0 ;;
      repos/fake-user/my-cool-app/deployments*)
        if [ -n "$FAKE_HOSTED" ]; then
          echo '[{"id":7,"sha":"abcdef1234567","ref":"main","environment":"Production","creator":{"login":"vercel[bot]"},"created_at":"2026-09-18T09:59:00Z"},{"id":8,"sha":"fffffff","ref":"feature","environment":"Preview","creator":{"login":"vercel[bot]"}},{"id":6,"sha":"1234567aaaa","ref":"main","environment":"Production","creator":{"login":"vercel[bot]"}}]'
        else
          echo '[]'
        fi
        exit 0 ;;
    esac
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
        if [ "$4" = "nameWithOwner" ]; then echo "fake-user/my-cool-app";
        elif [ "$4" = "visibility" ]; then echo "\${FAKE_VISIBILITY:-PUBLIC}";
        else echo "https://github.com/fake-user/my-cool-app"; fi
        exit 0
        ;;
    esac
    ;;
esac
exit 1
`;

const FAKE_VERCEL = `#!/bin/bash
case "$1" in
  whoami)
    if [ -n "$FAKE_VERCEL_LOGGED_OUT" ]; then echo "Error: not authenticated" >&2; exit 1; fi
    echo "fakeuser"; exit 0 ;;
  project)
    if [ "$2" = "ls" ]; then
      echo '{"projects":[{"id":"prj_example_one","name":"my-cool-app","latestProductionUrl":"https://my-cool-app.vercel.app","updatedAt":1789000000000},{"id":"prj_example_two","name":"another-site","updatedAt":1788000000000}],"pagination":{},"contextName":"fakeuser"}'
      exit 0
    fi
    ;;
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
    // The tests never read the session of a Vercel CLI the person running them is signed in to,
    // and never reach the network: the dashboard falls back to what the fake CLI lists.
    HOME: box.home,
    VERCEL_TOKEN: '',
    NOW_TOKEN: '',
    ...(box.env ?? {}),
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
        const waiter = { method, resolve: (m) => (clearTimeout(timer), resolve(m)) };
        // A wait that gave up leaves the queue, or the next message would go to nobody.
        const timer = setTimeout(() => {
          const index = waiting.indexOf(waiter);
          if (index !== -1) waiting.splice(index, 1);
          reject(new Error(`no ${method} within ${timeout} ms; stderr: ${stderr}`));
        }, timeout);
        waiting.push(waiter);
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
    assert.match(JSON.stringify(panel), /Vite project/); // framework detected, written the way Vite writes it

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
    // The deploy's own log carries the URL while it is still running, so wait for the state that
    // says it finished rather than for the address appearing anywhere on the panel.
    panel = await waitForText(host, /Launched|출시 완료/);
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

test('a folder that is not a web project opens on the dashboard, and the deploy tab still says why', async () => {
  const box = sandbox();
  // `box.root` holds the project folder but is not one itself.
  box.env = {};
  const host = start(box);
  host.context.pane.cwd = box.root;
  try {
    host.send('panel/open', { context: host.context });
    // The dashboard is what is worth showing here: the account, and every project on it.
    let panel = await waitForText(host, /Projects on Vercel/);
    const tree = JSON.stringify(panel);
    assert.match(tree, /"id":"tab"/, 'both tabs are offered');
    assert.match(tree, /"value":"dashboard"/);
    assert.match(tree, /my-cool-app/, 'the fake CLI listed the account\'s projects');
    assert.match(tree, /another-site/);
    assert.match(tree, /Only the project names could be read/, 'the CLI-only fallback says what is missing');
    assert.ok(buttonIds(panel).includes('dash-open-vercel-home'));

    // Nothing is missing, so no login button is offered.
    assert.ok(!buttonIds(panel).includes('install-tools'));
    assert.ok(!buttonIds(panel).includes('gh-login-start'));
    assert.ok(!buttonIds(panel).includes('vercel-login-start'));

    host.send('ui/event', { element: 'tab', event: 'change', value: 'project', context: host.context });
    panel = await waitForText(host, /package\.json|index\.html/);
    assert.ok(buttonIds(panel).includes('open-dashboard'), 'and a way back to the dashboard');
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
/** Texts of a panel tree, for failure messages. */
function panelTexts(tree) {
  const texts = [];
  (function walk(node) {
    if (!node) return;
    for (const key of ['text', 'label', 'title']) if (typeof node[key] === 'string') texts.push(node[key]);
    for (const child of node.children ?? []) walk(child);
  })(tree);
  return texts.join(' | ').slice(0, 600);
}

async function waitForPanel(host, matches, what, timeout) {
  const started = Date.now();
  let last = null;
  for (;;) {
    let message;
    try {
      message = await host.next('ui/setPanel', timeout - (Date.now() - started));
    } catch (err) {
      throw new Error(`${err.message}\nwaiting for ${what}; last panel: ${panelTexts(last)}`);
    }
    last = message.params.tree;
    if (matches(last)) return last;
    if (Date.now() - started > timeout) throw new Error(`no panel with ${what} within ${timeout}ms; last panel: ${panelTexts(last)}`);
  }
}

function waitForButton(host, id, timeout = 15000) {
  return waitForPanel(host, (tree) => buttonIds(tree).includes(id), `button ${id}`, timeout);
}

function waitForText(host, re, timeout = 15000) {
  return waitForPanel(host, (tree) => re.test(JSON.stringify(tree)), String(re), timeout);
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

test('Vercel production deployments are recognized from GitHub', () => {
  const vercel = (environment, login = 'vercel[bot]') => ({ environment, creator: { login } });
  assert.ok(isVercelProductionDeployment(vercel('Production')));
  assert.ok(isVercelProductionDeployment(vercel('Production – my-app')));
  assert.ok(!isVercelProductionDeployment(vercel('Preview')));
  assert.ok(!isVercelProductionDeployment(vercel('Production', 'someone')), 'deployments made by others are not Vercel\'s');
  assert.deepEqual(pickVercelProductionDeployments([vercel('Preview'), vercel('Production'), vercel('Production')], 1), [vercel('Production')]);
  assert.deepEqual(pickVercelProductionDeployments(null), []);

  const summary = summarizeDeployment(
    { sha: 'abcdef1234567', ref: 'main', created_at: '2026-09-18T09:59:00Z' },
    [{ state: 'success', environment_url: 'https://app.vercel.app', log_url: 'javascript:alert(1)', target_url: 'https://vercel.com/x/y', created_at: '2026-09-18T10:00:00Z' }],
  );
  assert.deepEqual(summary, { state: 'success', url: 'https://app.vercel.app', inspectUrl: 'https://vercel.com/x/y', sha: 'abcdef1', ref: 'main', time: Date.parse('2026-09-18T10:00:00Z') });
  // Only https links are kept: the panel opens them.
  assert.equal(summarizeDeployment({}, [{ environment_url: 'http://plain.example' }]).url, null);
  assert.equal(summarizeDeployment({}, null).state, 'pending');
  assert.equal(liveUrlOf([{ state: 'failure', url: 'https://new.vercel.app' }, { state: 'success', url: 'https://ok.vercel.app' }]), 'https://ok.vercel.app');
  assert.equal(liveUrlOf([{ state: 'in_progress', url: 'https://new.vercel.app' }]), 'https://new.vercel.app');
});

test('Launch: a repository Vercel already deploys shows the live site, and publishing only pushes', async () => {
  const box = sandbox();
  box.env = { FAKE_HOSTED: '1' };
  // An existing project with a remote that Launch never saved to (set up outside Launch).
  const git = (...args) => execFileSync('git', args, { cwd: box.project, stdio: 'pipe' });
  const bare = path.join(box.remotes, 'my-cool-app.git');
  execFileSync('git', ['init', '--quiet', '--bare', bare]);
  git('init', '--quiet');
  git('add', '-A');
  git('-c', 'user.name=Fake', '-c', 'user.email=fake@example.com', 'commit', '--quiet', '-m', 'first');
  git('remote', 'add', 'origin', bare);
  git('push', '--quiet', '-u', 'origin', 'HEAD');
  const host = start(box);
  try {
    host.send('panel/open', { context: host.context });
    let panel = await waitForButton(host, 'open-vercel');
    const text = JSON.stringify(panel);
    assert.match(text, /Live on Vercel/);
    assert.match(text, /https:\/\/my-cool-app-live\.vercel\.app/, 'the newest successful production URL');
    assert.match(text, /abcdef1/, 'the latest commit');
    assert.match(text, /fake-user\/my-cool-app/, 'the repository pushes go to is named before publishing');
    assert.ok(!buttonIds(panel).includes('deploy-start'), 'no first-launch "Publish" for a site that is already live');
    assert.ok(!buttonIds(panel).includes('gh-save'), 'no remote confirmation needed to look at it');
    // Every step is behind it, so the checklist is one line instead of six rows.
    assert.match(text, /All 6 steps done/);
    assert.ok(!text.includes('"id":"steps"'), 'no step list when nothing is left to do');

    // "Publish my changes" pushes to GitHub; Vercel builds it. No `vercel deploy` (it could create a second project).
    fs.writeFileSync(path.join(box.project, 'index.html'), '<h1>new</h1>');
    host.send('ui/event', { element: 'update-site', event: 'click', context: host.context });
    panel = await waitForText(host, /Pushed/);
    assert.ok(!fs.existsSync(path.join(box.root, 'deploys.log')), 'vercel deploy was not run');
    const remoteLog = execFileSync('git', ['--git-dir', bare, 'log', '--oneline'], { encoding: 'utf8' });
    assert.equal(remoteLog.trim().split('\n').length, 2, 'the change reached the remote');
    // A remote Launch did not create is treated as public unless GitHub says it is private.
    assert.ok(fs.readFileSync(path.join(box.project, '.gitignore'), 'utf8').split('\n').includes(IDEA_NOTES_GITIGNORE_LINES[0]), 'a public remote never gets the idea notes');
  } finally {
    host.stop();
  }
});

test('the live site is shown at its own domain, and commit refs are not branch names', () => {
  const inspect = [
    'Vercel CLI 50.0.0',
    '  General',
    '    name\tmy-site',
    '    url\t\thttps://my-site-abc123-team.vercel.app',
    '  Aliases',
    '    ╶ https://www.example.com',
    '    ╶ https://example.com',
    '    ╶ https://my-site.vercel.app',
    '    ╶ https://my-site-git-main-team.vercel.app',
    '  Builds',
    '    ╶ https://not-an-alias.example',
  ].join('\n');
  assert.deepEqual(parseInspectDomains(inspect), ['https://example.com', 'https://www.example.com', 'https://my-site.vercel.app']);
  assert.deepEqual(parseInspectDomains('no aliases here'), []);
  const sha = 'a'.repeat(40);
  assert.equal(summarizeDeployment({ ref: sha, sha }, []).ref, null);
  assert.equal(summarizeDeployment({ ref: 'main', sha }, []).ref, 'main');
});


// -- the dashboard ---------------------------------------------------------------------------------

test('Vercel ready states become the words the panel uses', () => {
  assert.equal(deployStateOf('READY'), 'ready');
  assert.equal(deployStateOf('ERROR'), 'error');
  assert.equal(deployStateOf('BUILDING'), 'building');
  assert.equal(deployStateOf('INITIALIZING'), 'building');
  assert.equal(deployStateOf('QUEUED'), 'queued');
  assert.equal(deployStateOf('CANCELED'), 'canceled');
  assert.equal(deployStateOf(undefined), 'unknown');
  assert.equal(deployStateOf('something else'), 'unknown');
});

test('a Vercel project link names the repository, whichever git host it is on', () => {
  assert.deepEqual(repoOfVercelLink({ type: 'github', org: 'acme', repo: 'shop', productionBranch: 'main' }), {
    host: 'github.com',
    owner: 'acme',
    name: 'shop',
    slug: 'acme/shop',
    url: 'https://github.com/acme/shop',
    branch: 'main',
  });
  assert.equal(repoOfVercelLink({ type: 'github', org: 'acme', repo: 'shop' }).branch, null);
  assert.equal(repoOfVercelLink({ type: 'gitlab', projectNamespace: 'acme', projectName: 'shop' }).url, 'https://gitlab.com/acme/shop');
  assert.equal(repoOfVercelLink({ type: 'bitbucket', owner: 'acme', slug: 'shop' }).slug, 'acme/shop');
  assert.equal(repoOfVercelLink({ type: 'github', org: 'acme' }), null, 'half a link is no link');
  assert.equal(repoOfVercelLink(null), null);
  assert.equal(repoOfVercelLink({ type: 'svn', org: 'acme', repo: 'shop' }), null);
});

test('the address shown for a site is its own domain before any Vercel one', () => {
  assert.equal(pickPrimaryDomain(['www.example.com', 'example.com', 'shop.vercel.app']), 'www.example.com');
  assert.equal(pickPrimaryDomain(['shop-git-main-acme.vercel.app', 'shop-acme.vercel.app', 'shop.vercel.app']), 'shop.vercel.app');
  assert.equal(pickPrimaryDomain([], 'shop-abc123.vercel.app'), 'shop-abc123.vercel.app');
  assert.equal(pickPrimaryDomain(null, null), null);
});

test('a project from Vercel becomes what the dashboard draws', () => {
  const raw = {
    id: 'prj_example_not_a_real_id',
    name: 'shop',
    framework: 'nextjs',
    updatedAt: 1789000000000,
    link: { type: 'github', org: 'acme', repo: 'shop-web', productionBranch: 'main' },
    targets: {
      production: {
        readyState: 'READY',
        url: 'shop-abc123-acme.vercel.app',
        alias: ['www.example.com', 'shop.vercel.app'],
        createdAt: 1789000000000,
        meta: { githubCommitRef: 'main', githubCommitSha: 'abcdef1234567890', githubCommitMessage: 'fix: the cart\nand more' },
      },
    },
  };
  const project = normalizeVercelProject(raw, { scope: 'acme' });
  assert.equal(project.name, 'shop');
  assert.equal(project.framework, 'nextjs');
  assert.equal(project.repo.slug, 'acme/shop-web');
  assert.equal(project.repo.branch, 'main');
  assert.equal(project.production.state, 'ready');
  assert.equal(project.production.domain, 'www.example.com');
  assert.equal(project.production.url, 'https://shop-abc123-acme.vercel.app');
  assert.equal(project.production.sha, 'abcdef1', 'the short commit, not the whole hash');
  assert.equal(project.production.message, 'fix: the cart', 'the first line only');
  assert.equal(project.inspectUrl, 'https://vercel.com/acme/shop');

  const bare = normalizeVercelProject({ name: 'blank' });
  assert.equal(bare.repo, null);
  assert.equal(bare.production, null);
  assert.equal(normalizeVercelProject({}), null);
  assert.equal(normalizeVercelProject('shop'), null);
});

test('projects are listed newest deploy first, and anything that is not a project is dropped', () => {
  const payload = {
    projects: [
      { name: 'old', updatedAt: 1000 },
      'not a project',
      { name: 'newest', targets: { production: { readyState: 'READY', createdAt: 9000 } } },
      { name: 'middle', updatedAt: 5000 },
    ],
  };
  assert.deepEqual(
    normalizeVercelProjects(payload).map((p) => p.name),
    ['newest', 'middle', 'old'],
  );
  assert.deepEqual(normalizeVercelProjects(null), []);
});

test('the flow diagram links its stops and leaves the last one open', () => {
  assert.deepEqual(
    flowDiagram([
      { filled: true, title: 'GitHub', lines: ['acme/shop-web', 'branch main'] },
      { filled: false, title: 'Live', lines: ['Not deployed yet'] },
    ]),
    ['● GitHub', '│ acme/shop-web', '│ branch main', '│', '○ Live', '  Not deployed yet'],
  );
  assert.deepEqual(flowDiagram([]), []);
});

test('a diagram line is cut by the columns it takes, not the characters it has', () => {
  assert.equal(displayWidth('abc'), 3);
  assert.equal(displayWidth('한글'), 4);
  assert.equal(shortenToWidth('abc', 10), 'abc');
  assert.equal(shortenToWidth('abcdefghij', 5), 'abcd…');
  // Eight Korean syllables are sixteen columns: six of them fit in a thirteen-column line.
  assert.equal(shortenToWidth('가나다라마바사아', 13), '가나다라마바…');
});

test("an SSH key's GitHub account is read from what the server answers, and nothing else is", () => {
  assert.equal(sshGithubLogin("Hi octocat! You've successfully authenticated, but GitHub does not provide shell access."), 'octocat');
  assert.equal(sshGithubLogin('git@github.com: Permission denied (publickey).'), null);
  assert.equal(sshGithubLogin('Hi there! Welcome.'), null);
  assert.equal(sshGithubLogin(''), null);
});

test('the Vercel session is looked for where that platform keeps it', () => {
  const mac = sessionFileCandidates({}, '/Users/me', 'darwin');
  assert.equal(mac[0], '/Users/me/Library/Application Support/com.vercel.cli/auth.json');
  assert.ok(mac.includes('/Users/me/.vercel/auth.json'), 'the older layout is still looked at');
  const linux = sessionFileCandidates({ XDG_DATA_HOME: '/home/me/.share' }, '/home/me', 'linux');
  assert.equal(linux[0], '/home/me/.share/com.vercel.cli/auth.json');
  const windows = sessionFileCandidates({ APPDATA: 'C:\\Users\\me\\AppData\\Roaming' }, 'C:\\Users\\me', 'win32');
  assert.ok(windows[0].includes('com.vercel.cli'));
  assert.deepEqual(
    configFileCandidates({}, '/Users/me', 'darwin').map((f) => f.split('/').pop()),
    mac.map(() => 'config.json'),
  );
});

test('an SSH key that can push is enough: Launch does not ask for a GitHub login it does not need', async () => {
  const box = sandbox();
  // No `gh` session, but this computer's SSH key belongs to an account GitHub knows.
  box.env = { FAKE_GH_LOGGED_OUT: '1', FAKE_SSH_LOGIN: 'fake-user' };
  const git = (...args) => execFileSync('git', args, { cwd: box.project, stdio: 'pipe' });
  const bare = path.join(box.remotes, 'my-cool-app.git');
  execFileSync('git', ['init', '--quiet', '--bare', bare]);
  git('init', '--quiet');
  git('add', '-A');
  git('-c', 'user.name=Fake', '-c', 'user.email=fake@example.com', 'commit', '--quiet', '-m', 'first');
  git('remote', 'add', 'origin', bare);
  git('push', '--quiet', '-u', 'origin', 'HEAD');
  const host = start(box);
  try {
    host.send('panel/open', { context: host.context });
    // Straight to confirming the remote — no "Log in to GitHub" in the way.
    const panel = await waitForButton(host, 'gh-save');
    assert.ok(!buttonIds(panel).includes('gh-login-start'), 'no login is asked for');
    assert.match(JSON.stringify(panel), /fake-user/, 'the account the SSH key belongs to is named');

    host.send('ui/event', { element: 'tab', event: 'change', value: 'dashboard', context: host.context });
    const dashboard = await waitForText(host, /Projects on Vercel/);
    // Everything is connected, so the connections shrink to one line that still says how.
    assert.match(JSON.stringify(dashboard), /GitHub · fake-user · SSH/);
    assert.ok(!JSON.stringify(dashboard).includes('"title":"Connections"'), 'no full list while nothing is missing');
  } finally {
    host.stop();
  }
});

test('the dashboard offers the logins that are missing, and only those', async () => {
  const box = sandbox();
  box.env = { FAKE_GH_LOGGED_OUT: '1', FAKE_VERCEL_LOGGED_OUT: '1' };
  const host = start(box);
  host.context.pane.cwd = box.root;
  try {
    host.send('panel/open', { context: host.context });
    const panel = await waitForText(host, /Sign in to Vercel/);
    // Something is missing, so the full list is there with the buttons that fix it.
    assert.match(JSON.stringify(panel), /"title":"Connections"/);
    const buttons = buttonIds(panel);
    assert.ok(buttons.includes('gh-login-start'), 'GitHub is offered: neither gh nor an SSH key is signed in');
    assert.ok(buttons.includes('vercel-login-start'));
    assert.ok(!buttons.includes('install-tools'), 'both tools are on the PATH');
    assert.match(JSON.stringify(panel), /Not signed in/);
  } finally {
    host.stop();
  }
});

test('a converted Vercel account is not offered twice, once as itself and once as its team', () => {
  const teams = [{ id: 'team_example_one', slug: 'acme', name: 'Acme' }];
  // "northstar": the personal account became a team, so it is not a scope of its own any more.
  assert.deepEqual(
    scopeList({ username: 'me', version: 'northstar', defaultTeamId: 'team_example_one' }, teams).map((s) => s.id),
    ['team_example_one'],
  );
  // An older account still has one, and it comes first.
  const older = scopeList({ username: 'me', name: 'Me' }, teams);
  assert.deepEqual(
    older.map((s) => s.id),
    [null, 'team_example_one'],
  );
  assert.equal(older[0].personal, true);
  // A converted account with no team left is still somewhere to look.
  assert.equal(scopeList({ username: 'me', version: 'northstar' }, []).length, 1);
  assert.deepEqual(scopeList(null, null), [{ id: null, slug: 'me', name: 'Personal', personal: true }]);
});

test('an address is only built from a plain host and a plain name', () => {
  // Everything here comes back from Vercel and ends up in a browser, so anything that is not a
  // name or a host is dropped instead of pasted into a URL.
  assert.equal(repoOfVercelLink({ type: 'github', org: '../../evil', repo: 'shop' }), null);
  assert.equal(repoOfVercelLink({ type: 'github', org: 'acme', repo: 'shop?x=1' }), null);
  assert.equal(pickPrimaryDomain(['javascript:alert(1)', 'example.com']), 'example.com');
  assert.equal(pickPrimaryDomain(['not a host at all']), null);
  assert.equal(normalizeVercelProject({ name: 'shop' }, { scope: 'acme/../x' }).inspectUrl, null);
  assert.equal(normalizeVercelProject({ name: 'shop' }, { scope: 'acme' }).inspectUrl, 'https://vercel.com/acme/shop');
  assert.equal(normalizeVercelProject({ name: 'shop', targets: { production: { url: 'shop.example/../x' } } }, {}).production.url, null);
});

test('a login started from the dashboard reports its failure on the dashboard', async () => {
  const box = sandbox();
  box.env = { FAKE_GH_LOGGED_OUT: '1' };
  const host = start(box);
  host.context.pane.cwd = box.root;
  try {
    host.send('panel/open', { context: host.context });
    await waitForButton(host, 'gh-login-start');
    // The fake `gh auth login` refuses; the walk-through is not the tab in front of the user, so
    // the message has to appear here rather than on a step nobody is looking at.
    host.send('ui/event', { element: 'gh-login-start', event: 'click', context: host.context });
    const panel = await waitForText(host, /GitHub login did not finish/);
    assert.ok(buttonIds(panel).includes('retry'), 'and can be tried again');
    assert.match(JSON.stringify(panel), /"id":"tab"/, 'still on the dashboard');
  } finally {
    host.stop();
  }
});

test('the folder decides the tab every time Launch is opened, whatever was looked at last', async () => {
  const box = sandbox();
  const host = start(box);
  try {
    host.send('panel/open', { context: host.context });
    // A folder that can be published opens on the publishing steps.
    await waitForButton(host, 'gh-save');

    // Looking at the dashboard does not change what the next open does.
    host.send('ui/event', { element: 'tab', event: 'change', value: 'dashboard', context: host.context });
    await waitForText(host, /Projects on Vercel/);
    host.send('panel/close', { context: host.context });
    host.send('panel/open', { context: host.context });
    await waitForButton(host, 'gh-save');

    // And a folder with nothing to publish brings the dashboard back on its own.
    const elsewhere = { ...host.context, pane: { ...host.context.pane, cwd: box.root } };
    host.send('context/changed', { context: elsewhere });
    await waitForText(host, /Projects on Vercel/);
  } finally {
    host.stop();
  }
});

test('Launch follows the terminal the user is looking at, not the folder it was last opened on', async () => {
  const box = sandbox();
  const host = start(box);
  const at = (cwd) => ({ ...host.context, pane: { ...host.context.pane, cwd } });
  try {
    // Opened by a link on a folder that is not a web project: that folder, for now.
    host.send('url/open', { path: 'open', query: { path: box.root }, url: 'agentty://plugin/launch/open', context: at(box.root) });
    await waitForText(host, /Projects on Vercel/);

    // The terminal moves into the web project while the panel is open: Launch moves with it.
    host.send('context/changed', { context: at(box.project) });
    await waitForButton(host, 'gh-save');

    // The terminal moves away while the panel is closed: the next opening looks again.
    host.send('panel/close', { context: at(box.project) });
    host.send('panel/open', { context: at(box.root) });
    await waitForText(host, /Projects on Vercel/);
    host.send('panel/close', { context: at(box.root) });
    host.send('panel/open', { context: at(box.project) });
    const panel = await waitForButton(host, 'gh-save');
    assert.match(JSON.stringify(panel), /"value":"project"/, 'on the Deploy tab');
  } finally {
    host.stop();
  }
});

test('moving into a web project shows its tab and a spinner at once, and a folder left behind never answers late', async () => {
  const box = sandbox();
  // GitHub answers slowly, as it does for real: a folder's checks are still running when the user moves on.
  box.env = { FAKE_GH_DELAY: '1' };
  const host = start(box);
  const at = (cwd) => ({ ...host.context, pane: { ...host.context.pane, cwd } });
  try {
    host.send('panel/open', { context: at(box.root) });
    await waitForText(host, /Projects on Vercel/);

    // The folder's own tab, with the folder and a spinner, before GitHub and Vercel have answered.
    host.send('context/changed', { context: at(box.project) });
    const loading = await waitForText(host, /Checking how this project stands/);
    assert.match(JSON.stringify(loading), /"value":"project"/);
    assert.ok(JSON.stringify(loading).includes(box.project), 'already naming the new folder');
    await waitForButton(host, 'gh-save');

    // Out, back in, and out again while the web project is still being checked (GitHub takes a
    // second here): its checks finish after the user has left, and must not win.
    host.send('context/changed', { context: at(box.root) });
    await waitForText(host, /Projects on Vercel/);
    host.send('context/changed', { context: at(box.project) });
    await waitForText(host, /Checking how this project stands/);
    host.send('context/changed', { context: at(box.root) });
    let last = await waitForText(host, /Projects on Vercel/);
    // Give the web project's checks every chance to finish late, and keep whatever comes after.
    const until = Date.now() + 3000;
    while (Date.now() < until) {
      const next = await host.next('ui/setPanel', Math.max(until - Date.now(), 1)).catch(() => null);
      if (next) last = next.params.tree;
    }
    assert.match(JSON.stringify(last), /"value":"dashboard"/, 'still on the folder the terminal is in');
    // The Deploy tab still describes the folder the terminal is in, not the one left behind.
    host.send('ui/event', { element: 'tab', event: 'change', value: 'project', context: at(box.root) });
    const deploy = await waitForText(host, /package\.json|index\.html/);
    assert.ok(!buttonIds(deploy).includes('gh-save'), 'the web project did not take over');
  } finally {
    host.stop();
  }
});
