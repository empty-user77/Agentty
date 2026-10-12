// Tests for the UI Gallery plugin: `node --test plugins/ui-gallery/test/plugin.test.mjs`
// The plugin runs as Agentty would start it, against a fake host.

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
const ALL = ['flow', 'popover', 'card', 'grid', 'tabs', 'table', 'keyValue', 'stat', 'progress', 'callout', 'select', 'checkbox', 'code'];
const BASE = ['column', 'row', 'section', 'text', 'button', 'input', 'list', 'choice', 'toggle', 'badge', 'spinner', 'divider'];

/** Starts the plugin; `host.next(method)` resolves with the next call of that method. */
function start() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ui-gallery-'));
  for (const file of ['main.mjs', 'agentty-plugin.json']) fs.copyFileSync(path.join(pluginDir, file), path.join(dir, file));
  fs.copyFileSync(sdk, path.join(dir, 'agentty-plugin.mjs'));
  const child = spawn(process.execPath, ['main.mjs'], { cwd: dir, stdio: ['pipe', 'pipe', 'pipe'] });
  const waiting = [];
  const seen = [];
  createInterface({ input: child.stdout }).on('line', (line) => {
    const message = JSON.parse(line);
    if (message.id !== undefined && message.method) {
      child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: message.id, result: null }) + '\n');
    }
    if (!message.method) return;
    const index = waiting.findIndex((w) => w.method === message.method);
    if (index === -1) seen.push(message);
    else waiting.splice(index, 1)[0].resolve(message);
  });
  const send = (method, params, id) => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method, params, ...(id ? { id } : {}) }) + '\n');
  send('initialize', { apiVersion: 4, plugin: { id: 'ui-gallery', name: 'UI Gallery', dir, dataDir: dir }, language: 'en', context: {} }, 1);
  return {
    send,
    event: (element, event, extra = {}) => send('ui/event', { element, event, context: {}, ...extra }),
    next(method) {
      const index = seen.findIndex((m) => m.method === method);
      if (index !== -1) return Promise.resolve(seen.splice(index, 1)[0]);
      return new Promise((resolve) => waiting.push({ method, resolve }));
    },
    stop: () => child.kill(),
  };
}

function walk(node, visit) {
  visit(node);
  for (const child of node.children ?? []) walk(child, visit);
}

test('every tab and recipe draws only elements Agentty knows, with unique ids', async () => {
  const host = start();
  try {
    host.send('panel/open', { context: {} });
    const trees = [(await host.next('ui/setPanel')).params.tree];
    host.event('gallery.code', 'change', { value: true });
    trees.push((await host.next('ui/setPanel')).params.tree);
    for (const tab of ['data', 'inputs', 'feedback', 'recipes']) {
      host.event('gallery.tabs', 'change', { value: tab });
      trees.push((await host.next('ui/setPanel')).params.tree);
    }
    for (const recipe of ['apiClient', 'settings', 'dashboard', 'job', 'states']) {
      host.event('gallery.recipe', 'change', { value: recipe });
      trees.push((await host.next('ui/setPanel')).params.tree);
    }
    const known = new Set([...BASE, ...ALL]);
    const drawn = new Set();
    for (const tree of trees) {
      const ids = [];
      walk(tree, (node) => {
        assert.ok(known.has(node.type), `unknown element ${node.type}`);
        drawn.add(node.type);
        if (node.id && node.type !== 'popover') ids.push(node.id);
      });
      assert.equal(new Set(ids).size, ids.length, `duplicate ids in ${JSON.stringify(ids)}`);
    }
    for (const type of [...BASE, ...ALL].filter((t) => t !== 'popover')) assert.ok(drawn.has(type), `the gallery never shows ${type}`);
    if (process.env.GALLERY_TREES) fs.writeFileSync(process.env.GALLERY_TREES, JSON.stringify(trees));
  } finally {
    host.stop();
  }
});

test('the code shown is the code that draws the example', async () => {
  const host = start();
  try {
    host.send('panel/open', { context: {} });
    await host.next('ui/setPanel');
    host.event('gallery.code', 'change', { value: true });
    const tree = (await host.next('ui/setPanel')).params.tree;
    const code = [];
    walk(tree, (node) => node.type === 'code' && code.push(node.text));
    assert.ok(code.some((text) => text.startsWith('ui.column([') && text.includes("ui.button('row.ok', 'OK'")), code[0]);
    assert.ok(code.every((text) => !text.startsWith(' ')), 'snippets are not indented');
  } finally {
    host.stop();
  }
});

test('controls change what the examples show', async () => {
  const host = start();
  try {
    host.send('panel/open', { context: {} });
    await host.next('ui/setPanel');
    host.event('gallery.tabs', 'change', { value: 'inputs' });
    await host.next('ui/setPanel');
    host.event('demo.region', 'change', { value: 'tokyo' });
    const tree = (await host.next('ui/setPanel')).params.tree;
    let select;
    walk(tree, (node) => node.id === 'demo.region' && (select = node));
    assert.equal(select.value, 'tokyo');
    host.event('btn.primary', 'click');
    const toast = await host.next('ui/notify');
    assert.match(toast.params.message, /btn\.primary/);
  } finally {
    host.stop();
  }
});
