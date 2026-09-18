// Agentty plugin SDK for Node.js (18+). Single file, no dependencies: copy it next to your plugin.
// SPDX-License-Identifier: MIT
//
//   import { createPlugin, ui } from './agentty-plugin.mjs';
//   const plugin = createPlugin();
//   plugin.command('hello.say', async ({ context }) => plugin.notify(`Hello from ${context.pane?.cwd}`));
//   plugin.onPanelOpen(() => plugin.setPanel(ui.column([ui.text('Hello', 'title')])));
//   plugin.start();
//
// Protocol: JSON-RPC 2.0, one JSON object per line on stdin/stdout. Never write anything else to
// stdout — use `plugin.log()` (stderr), which shows up in Agentty's plugin log.

import { createInterface } from 'node:readline';

export const SDK_VERSION = '1.0.0';
export const API_VERSION = 1;

/** Builders for the panel UI tree. Every interactive element needs an `id` unique in the panel. */
export const ui = {
  column: (children, { gap = 'medium' } = {}) => ({ type: 'column', gap, children: compact(children) }),
  row: (children, { gap = 'medium', wrap = false } = {}) => ({ type: 'row', gap, wrap, children: compact(children) }),
  section: (title, children) => ({ type: 'section', title, children: compact(children) }),
  /** style: body | title | muted | small | code | error | success */
  text: (text, style = 'body') => ({ type: 'text', text: String(text ?? ''), style }),
  /** variant: primary | secondary | ghost | danger */
  button: (id, label, { icon, variant = 'secondary', disabled = false } = {}) => ({ type: 'button', id, label, icon, variant, disabled }),
  input: (id, { placeholder = '', value = '' } = {}) => ({ type: 'input', id, placeholder, value }),
  /** items: [{ id, title, subtitle?, detail?, icon?, actions?: [{ id, label?, icon?, tooltip? }] }] */
  list: (id, items, { empty } = {}) => ({ type: 'list', id, items, empty }),
  /** options: [{ value, label }] */
  choice: (id, options, value = '') => ({ type: 'choice', id, options, value }),
  toggle: (id, label, value = false) => ({ type: 'toggle', id, label, value }),
  /** tone: neutral | info | success | warning | error */
  badge: (text, tone = 'neutral') => ({ type: 'badge', text, tone }),
  spinner: (text = '') => ({ type: 'spinner', text }),
  divider: () => ({ type: 'divider' }),
};

function compact(children) {
  return (children ?? []).flat().filter(Boolean);
}

export class AgenttyError extends Error {
  constructor(message, code) {
    super(message);
    this.code = code;
  }
}

/**
 * Creates the plugin connection. Register handlers, then call `start()`.
 * @param {{ input?: NodeJS.ReadableStream, output?: NodeJS.WritableStream }} [streams] for tests
 */
export function createPlugin(streams = {}) {
  const input = streams.input ?? process.stdin;
  const output = streams.output ?? process.stdout;
  const commands = new Map();
  const events = new Map();
  const urls = new Map();
  const listeners = { activate: [], panelOpen: [], panelClose: [], context: [], event: [], url: [], shutdown: [] };
  const pending = new Map();
  let nextId = 1;
  let started = false;
  // Everything but `initialize` waits until the activate handlers have finished.
  let markActivated;
  const activated = new Promise((resolve) => (markActivated = resolve));

  const plugin = {
    /** Filled by `initialize`: { apiVersion, agentty: { version }, plugin: { id, dir, dataDir }, language, context } */
    info: null,
    /** Last context Agentty reported (focused workspace and pane). */
    context: null,

    command(id, handler) {
      commands.set(id, handler);
      return plugin;
    },
    /** Handler for events of one element id: (event, context) => … */
    onEvent(elementId, handler) {
      events.set(elementId, handler);
      return plugin;
    },
    /** Handler for every UI event not claimed by `onEvent`. */
    onAnyEvent(handler) {
      listeners.event.push(handler);
      return plugin;
    },
    /** `agentty://plugin/<id>/<path>?…` → handler({ path, query, url }) */
    onUrl(path, handler) {
      urls.set(path, handler);
      return plugin;
    },
    onAnyUrl(handler) {
      listeners.url.push(handler);
      return plugin;
    },
    onActivate(handler) {
      listeners.activate.push(handler);
      return plugin;
    },
    onPanelOpen(handler) {
      listeners.panelOpen.push(handler);
      return plugin;
    },
    onPanelClose(handler) {
      listeners.panelClose.push(handler);
      return plugin;
    },
    onContextChange(handler) {
      listeners.context.push(handler);
      return plugin;
    },
    onShutdown(handler) {
      listeners.shutdown.push(handler);
      return plugin;
    },

    /** Replaces the panel's content. */
    setPanel(tree) {
      return call('ui/setPanel', { tree });
    },
    /** Opens (focuses) this plugin's panel. */
    showPanel() {
      return call('ui/showPanel', {});
    },
    /** kind: info | success | warning | error */
    notify(message, kind = 'info') {
      return call('ui/notify', { message: String(message), kind });
    },
    /** Small text on the plugin's header button (e.g. a count); '' clears it. */
    setBadge(text) {
      return call('ui/setBadge', { text: String(text ?? '') });
    },
    getContext() {
      return call('context/get', {});
    },
    /**
     * Sends a prompt to an agent. target: ask (dialog, default) | active | newWorkspace | newTab |
     * pane (paneId) | workspace (workspaceId). agent: claude | codex | shell. Needs `prompt.inject`.
     */
    injectPrompt(request) {
      return call('prompt/inject', request);
    },
    /** Types text into a pane (Enter when `submit`). Needs `terminal.write`. */
    sendToTerminal({ paneId, text, submit = false }) {
      return call('terminal/send', { paneId, text, submit });
    },
    /** Conversation of an agent pane: { agent, sessionId, title, cwd, status, turns: [{ role, text }] }. Needs `session.read`. */
    getSession({ paneId, maxTurns } = {}) {
      return call('session/get', { paneId, maxTurns });
    },
    /** Open workspaces with their panes. Needs `workspace.read`. */
    listWorkspaces() {
      return call('workspace/list', {});
    },
    openUrl(url) {
      return call('host/openUrl', { url });
    },
    revealPath(path) {
      return call('host/revealPath', { path });
    },
    /** Writes to Agentty's log for this plugin (stderr). */
    log(...parts) {
      process.stderr.write(parts.map((p) => (typeof p === 'string' ? p : safeJson(p))).join(' ') + '\n');
    },

    /** Starts reading messages from Agentty. */
    start() {
      if (started) return plugin;
      started = true;
      const lines = createInterface({ input, crlfDelay: Infinity });
      lines.on('line', (line) => {
        if (!line.trim()) return;
        let message;
        try {
          message = JSON.parse(line);
        } catch {
          plugin.log('agentty-plugin: ignoring a line that is not JSON');
          return;
        }
        handle(message).catch((err) => plugin.log('agentty-plugin:', err?.stack ?? String(err)));
      });
      lines.on('close', () => shutdown(0));
      process.on('uncaughtException', (err) => plugin.log('uncaught exception:', err?.stack ?? String(err)));
      process.on('unhandledRejection', (err) => plugin.log('unhandled rejection:', err?.stack ?? String(err)));
      return plugin;
    },
  };

  function send(message) {
    output.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n');
  }

  function call(method, params) {
    const id = nextId++;
    send({ id, method, params });
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
    });
  }

  async function run(listenersList, ...args) {
    for (const listener of listenersList) {
      await listener(...args);
    }
  }

  async function shutdown(code) {
    try {
      await run(listeners.shutdown);
    } finally {
      process.exit(code);
    }
  }

  async function handle(message) {
    if (message.method === undefined && message.id !== undefined) {
      const waiter = pending.get(message.id);
      if (!waiter) return;
      pending.delete(message.id);
      if (message.error) waiter.reject(new AgenttyError(message.error.message, message.error.code));
      else waiter.resolve(message.result);
      return;
    }
    const params = message.params ?? {};
    if (message.method === 'initialize') {
      plugin.info = params;
      plugin.context = params.context ?? null;
      send({ id: message.id, result: { sdkVersion: SDK_VERSION, apiVersion: API_VERSION } });
      try {
        await guarded(() => run(listeners.activate, params));
      } finally {
        markActivated();
      }
      return;
    }
    await activated;
    if (params.context !== undefined) plugin.context = params.context;
    switch (message.method) {
      case 'command/execute': {
        const handler = commands.get(params.command);
        if (!handler) {
          plugin.log(`no handler for command ${params.command}`);
          return;
        }
        await guarded(() => handler({ context: params.context, args: params.args ?? {} }));
        return;
      }
      case 'ui/event': {
        const handler = events.get(params.element);
        if (handler) await guarded(() => handler(params, params.context));
        else await guarded(() => run(listeners.event, params, params.context));
        return;
      }
      case 'panel/open':
        await guarded(() => run(listeners.panelOpen, params.context));
        return;
      case 'panel/close':
        await guarded(() => run(listeners.panelClose, params.context));
        return;
      case 'context/changed':
        await guarded(() => run(listeners.context, params.context));
        return;
      case 'url/open': {
        const handler = urls.get(params.path);
        if (handler) await guarded(() => handler(params));
        else await guarded(() => run(listeners.url, params));
        return;
      }
      case 'shutdown':
        await shutdown(0);
        return;
      default:
        if (message.id !== undefined) {
          send({ id: message.id, error: { code: -32601, message: `unknown method ${message.method}` } });
        }
    }
  }

  /** Errors in handlers become an error notification instead of killing the plugin. */
  async function guarded(fn) {
    try {
      await fn();
    } catch (err) {
      plugin.log(err?.stack ?? String(err));
      try {
        await plugin.notify(err?.message ?? String(err), 'error');
      } catch {
        // Agentty is gone.
      }
    }
  }

  return plugin;
}

function safeJson(value) {
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}
