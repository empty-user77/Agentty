// Tests for the Cosmica plugin: `node --test plugins/cosmica/test/plugin.test.mjs`
// The plugin runs as Agentty would start it, against a fake host and a temporary Cosmica setup.

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createInterface } from 'node:readline';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const pluginDir = path.resolve(here, '..');
const sdk = path.resolve(here, '../../../sdk/node/agentty-plugin.mjs');

function sandbox() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'cosmica-plugin-'));
  const home = path.join(root, 'cosmica-home');
  const notes = path.join(root, 'Notes');
  fs.mkdirSync(home, { recursive: true });
  fs.mkdirSync(path.join(notes, 'Work'), { recursive: true });
  // Port 9 (discard) is never a Cosmica server, so writes fall back to files.
  fs.writeFileSync(path.join(home, 'config.json'), JSON.stringify({ notes: { path: notes }, server: { port: 9 }, ai: { apiKey: 'not_a_real_key' } }));
  fs.writeFileSync(path.join(notes, 'Work', '20260101-000000-aaaa.md'), '---\ntags: [#plan]\n---\n# Release plan\n\nShip the plugin store.\n');
  fs.writeFileSync(path.join(notes, '20260102-000000-bbbb.md'), '# Grocery list\n\nmilk\n');
  const outside = path.join(root, 'secret.md');
  fs.writeFileSync(outside, '# Outside\n');
  const plugin = path.join(root, 'plugin');
  fs.mkdirSync(plugin);
  for (const file of ['main.mjs', 'cosmica.mjs', 'agentty-plugin.json']) fs.copyFileSync(path.join(pluginDir, file), path.join(plugin, file));
  fs.copyFileSync(sdk, path.join(plugin, 'agentty-plugin.mjs'));
  return { root, home, notes, outside, plugin, data: path.join(root, 'data') };
}

/** Starts the plugin; `host.next(method)` resolves with the next call of that method (answered with `result`). */
function start(box, results = {}) {
  const child = spawn(process.execPath, ['main.mjs'], { cwd: box.plugin, env: { ...process.env, COSMICA_HOME: box.home }, stdio: ['pipe', 'pipe', 'pipe'] });
  const waiting = [];
  const seen = [];
  let stderr = '';
  child.stderr.on('data', (d) => (stderr += d));
  createInterface({ input: child.stdout }).on('line', (line) => {
    const message = JSON.parse(line);
    if (message.id !== undefined && message.method) {
      const result = results[message.method] ?? null;
      child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: message.id, result }) + '\n');
    }
    if (!message.method) return;
    const index = waiting.findIndex((w) => w.method === message.method);
    if (index === -1) seen.push(message);
    else waiting.splice(index, 1)[0].resolve(message);
  });
  const send = (method, params, id) => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method, params, ...(id ? { id } : {}) }) + '\n');
  const context = { workspace: null, pane: { id: 3, kind: 'claude', tool: 'claude', title: 'Claude Code', cwd: box.root, status: 'idle', running: true }, language: 'en' };
  send('initialize', { apiVersion: 1, plugin: { id: 'cosmica', name: 'Cosmica', dir: box.plugin, dataDir: box.data }, language: 'en', context }, 1);
  return {
    child,
    context,
    send,
    stderr: () => stderr,
    next(method, timeout = 8000) {
      const index = seen.findIndex((m) => m.method === method);
      if (index !== -1) return Promise.resolve(seen.splice(index, 1)[0]);
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error(`no ${method} within ${timeout} ms; stderr: ${stderr}`)), timeout);
        waiting.push({ method, resolve: (m) => (clearTimeout(timer), resolve(m)) });
      });
    },
    stop() {
      child.kill();
      fs.rmSync(box.root, { recursive: true, force: true });
    },
  };
}

function find(tree, predicate) {
  if (predicate(tree)) return tree;
  for (const child of tree.children ?? []) {
    const found = find(child, predicate);
    if (found) return found;
  }
  return null;
}

test('panel lists notes from the Cosmica config and searches them', async () => {
  const box = sandbox();
  const host = start(box);
  try {
    host.send('panel/open', { context: host.context });
    let panel = (await host.next('ui/setPanel')).params.tree;
    const list = find(panel, (n) => n.type === 'list');
    assert.deepEqual(list.items.map((i) => i.title).sort(), ['Grocery list', 'Release plan']);
    assert.ok(JSON.stringify(panel).includes(box.notes));
    assert.ok(!JSON.stringify(panel).includes('not_a_real_key'), 'config secrets never reach the UI');

    host.send('ui/event', { element: 'query', event: 'submit', value: 'plugin store', context: host.context });
    panel = (await host.next('ui/setPanel')).params.tree;
    assert.deepEqual(find(panel, (n) => n.type === 'list').items.map((i) => i.title), ['Release plan']);
  } finally {
    host.stop();
  }
});

test('continue link injects the note through the Send to dialog', async () => {
  const box = sandbox();
  const host = start(box, { 'prompt/inject': { status: 'asked' } });
  try {
    const note = path.join(box.notes, 'Work', '20260101-000000-aaaa.md');
    host.send('url/open', { path: 'continue', query: { path: note, title: 'Release plan' }, url: 'agentty://plugin/cosmica/continue', context: host.context });
    const inject = await host.next('prompt/inject');
    assert.equal(inject.params.target, 'ask');
    assert.equal(inject.params.title, 'Release plan');
    assert.match(inject.params.text, /Ship the plugin store\./);
    assert.ok(!inject.params.text.includes('tags:'), 'front matter is not part of the prompt');

    host.send('url/open', { path: 'continue', query: { path: box.outside }, url: 'agentty://plugin/cosmica/continue', context: host.context });
    const notify = await host.next('ui/notify');
    assert.equal(notify.params.kind, 'error');
    assert.match(notify.params.message, /outside the Cosmica notes folder/);
  } finally {
    host.stop();
  }
});

test('AI summary asks the focused agent to write into the Cosmica folder', async () => {
  const box = sandbox();
  const host = start(box, { 'terminal/send': { paneId: 3 } });
  try {
    host.send('command/execute', { command: 'cosmica.saveSummary', args: {}, context: host.context });
    const send = await host.next('terminal/send');
    assert.equal(send.params.paneId, 3);
    assert.equal(send.params.submit, true);
    const target = send.params.text.split('\n').find((line) => line.startsWith(box.notes));
    assert.ok(target && target.startsWith(path.join(box.notes, 'Agentty') + path.sep), send.params.text);

    // The agent writes the note; the plugin notices and reports it.
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, '---\nsource: agentty\n---\n# Plugin store work\n\n## Goal\n');
    const done = await host.next('ui/notify', 15000);
    assert.equal(done.params.kind, 'info');
    const saved = await host.next('ui/notify', 15000);
    assert.equal(saved.params.kind, 'success');
    assert.match(saved.params.message, /Plugin store work/);
  } finally {
    host.stop();
  }
});

test('busy agents are not interrupted and shells are refused', async () => {
  const box = sandbox();
  const host = start(box);
  try {
    host.send('command/execute', { command: 'cosmica.saveSummary', args: {}, context: { ...host.context, pane: { ...host.context.pane, status: 'working' } } });
    assert.equal((await host.next('ui/notify')).params.kind, 'warning');
    host.send('command/execute', { command: 'cosmica.saveSummary', args: {}, context: { ...host.context, pane: { ...host.context.pane, kind: 'shell' } } });
    assert.equal((await host.next('ui/notify')).params.kind, 'warning');
  } finally {
    host.stop();
  }
});

test('conversation log is saved as a Cosmica note file when Cosmica is closed', async () => {
  const box = sandbox();
  const session = {
    paneId: 3,
    agent: 'claude',
    sessionId: 'example-session',
    title: 'Plugin work',
    cwd: box.root,
    status: 'idle',
    turnCount: 2,
    turns: [
      { role: 'user', text: 'Build the plugin store' },
      { role: 'assistant', text: 'Done.' },
    ],
  };
  const host = start(box, { 'session/get': session });
  try {
    host.send('command/execute', { command: 'cosmica.saveTranscript', args: {}, context: host.context });
    const saved = await host.next('ui/notify');
    assert.equal(saved.params.kind, 'success', saved.params.message);
    const dir = path.join(box.notes, 'Agentty');
    const [file] = fs.readdirSync(dir);
    assert.match(file, /^\d{8}-\d{6}-[a-z0-9]{4}\.md$/);
    const text = fs.readFileSync(path.join(dir, file), 'utf8');
    assert.match(text, /^---\nsource: agentty\ntags: \[#agentty, #session-log\]/);
    assert.match(text, /\n# Plugin work — conversation log\n/);
    assert.match(text, /## 1\. User\n\nBuild the plugin store/);
  } finally {
    host.stop();
  }
});
